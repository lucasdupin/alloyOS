#!/usr/bin/env python3
"""Real guest setup/Tile reboot acceptance with synthetic model replies only.
Requires an explicit clean packaged seed disk; every write targets its temporary
copy. No network/account/microphone action is permitted. Captures native UI PNGs
for visual review; those images are evidence, not automated text recognition.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path
import selectors
import shutil
import socket
import subprocess
import tempfile
import time
from PIL import Image

MARKER='SESSION_ONLY_DURABILITY_SENTINEL_9627'


def strings(value):
    if isinstance(value,str):
        yield value
    elif isinstance(value,dict):
        for item in value.values():
            yield from strings(item)
    elif isinstance(value,list):
        for item in value:
            yield from strings(item)


class Guest:
    def __init__(self,args,disk,root,phase):
        self.args=args;self.phase=phase;self.logs=[];self.pending=bytearray();self.identifier=0
        self.replies=[];self.action=None;self.model_count=0;self.forbid_marker=False
        auth=root/'fixture-auth';auth.write_bytes(b'GEMINI_API_KEY=fixture-not-a-real-key\0');auth.chmod(0o600)
        self.qmp_path=root/f'qmp-{phase}.sock'
        command=['qemu-system-aarch64','-M','virt,gic-version=3','-accel','hvf','-cpu','host',
                 '-kernel',str(args.kernel.resolve()),'-m','512M','-display','none','-serial','stdio',
                 '-no-reboot','-device','virtio-rng-device','-device','virtio-gpu-device,xres=1206,yres=2622',
                 '-device','virtio-keyboard-device','-device','virtio-tablet-device',
                 '-qmp',f'unix:{self.qmp_path},server=on,wait=off',
                 '-fw_cfg',f'name=opt/alloyos/auth,file={auth}']
        if disk:
            command += ['-drive',f'file={disk},if=none,format=raw,id=fixture','-device','virtio-blk-device,drive=fixture']
        self.child=subprocess.Popen(command,stdin=subprocess.PIPE,stdout=subprocess.PIPE,stderr=subprocess.PIPE)
        self.selector=selectors.DefaultSelector();self.selector.register(self.child.stdout,selectors.EVENT_READ)
        self.channel=None

    def line(self,deadline):
        while b'\n' not in self.pending:
            if time.monotonic()>deadline:
                raise RuntimeError('guest timeout: '+'\n'.join(self.logs[-8:]))
            if self.child.poll() is not None:
                raise RuntimeError('QEMU exited: '+self.child.stderr.read().decode(errors='replace'))
            if self.selector.select(.2):
                self.pending.extend(os.read(self.child.stdout.fileno(),65536))
                if len(self.pending)>4*1024*1024:
                    raise RuntimeError('UART frame exceeds bound')
        value,_,self.pending=self.pending.partition(b'\n')
        return value.decode(errors='replace').rstrip('\r')

    def send(self,text):
        self.child.stdin.write(text.encode());self.child.stdin.flush()

    def boot(self,headless=False):
        deadline=time.monotonic()+45
        while True:
            line=self.line(deadline);self.logs.append(line)
            if '[systemui] Slint window online;' in line:
                break
        if headless:
            assert any('/ [RamFS]' in line for line in self.logs)
            assert any('Cannot read /system/ferrofluid' in line or 'Cannot load /system/ferrofluid' in line for line in self.logs)
            assert not any('[ferrofluidd] EL0 Slint ready' in line for line in self.logs)
            return
        self.channel=socket.socket(socket.AF_UNIX,socket.SOCK_STREAM);self.channel.settimeout(5)
        self.channel.connect(str(self.qmp_path));self.stream=self.channel.makefile('rwb');self.stream.readline()
        self.qmp('qmp_capabilities');time.sleep(.6)

    def qmp(self,command,arguments=None):
        value={'execute':command}
        if arguments is not None:value['arguments']=arguments
        self.stream.write((json.dumps(value)+'\n').encode());self.stream.flush()
        while True:
            reply=json.loads(self.stream.readline())
            if 'event' in reply:continue
            if 'error' in reply:raise RuntimeError(str(reply['error']))
            return reply

    def click(self,x,y):
        self.qmp('input-send-event',{'events':[
            {'type':'abs','data':{'axis':'x','value':int(x/1206*32767)}},
            {'type':'abs','data':{'axis':'y','value':int(y/2622*32767)}},
            {'type':'btn','data':{'down':True,'button':'left'}},
            {'type':'btn','data':{'down':False,'button':'left'}}]});time.sleep(.4)

    def swipe(self,x1,y1,x2,y2):
        self.qmp('input-send-event',{'events':[{'type':'abs','data':{'axis':'x','value':int(x1/1206*32767)}},{'type':'abs','data':{'axis':'y','value':int(y1/2622*32767)}},{'type':'btn','data':{'down':True,'button':'left'}}]})
        for step in range(1,13):
            self.qmp('input-send-event',{'events':[{'type':'abs','data':{'axis':'x','value':int((x1+(x2-x1)*step/12)/1206*32767)}},{'type':'abs','data':{'axis':'y','value':int((y1+(y2-y1)*step/12)/2622*32767)}}]});time.sleep(.025)
        self.qmp('input-send-event',{'events':[{'type':'btn','data':{'down':False,'button':'left'}}]});time.sleep(.5)

    def capture(self,name):
        time.sleep(.6)
        self.qmp('screendump',{'filename':str((self.args.output/name).resolve()),'format':'png'})
        if self.args.inspect:
            while True:
                command=input(f'Inspect {name}, Enter to continue or click X Y: ')
                if not command:break
                fields=command.split()
                assert fields[0]=='click' and len(fields)==3
                self.click(int(fields[1]),int(fields[2]))
                self.qmp('screendump',{'filename':str((self.args.output/name).resolve()),'format':'png'})

    def response(self,identifier):
        deadline=time.monotonic()+60
        while True:
            line=self.line(deadline);fields=line.split('\t')
            if fields[0] in ('MCPHTTP1','ACCOUNT1','BROWSER1','VOICE1'):
                raise RuntimeError('unexpected network/account/microphone request')
            if fields[0]=='FORGEHTTP1':
                assert len(fields)==4 and fields[2]=='post'
                request=json.loads(bytes.fromhex(fields[3]));assert request['url'].startswith('https://generativelanguage.googleapis.com/v1beta/models/')
                body='\n'.join(strings(json.loads(request['body'])))
                if self.forbid_marker:
                    assert MARKER not in body,'conversation was restored after reboot'
                if self.replies:
                    model=self.replies.pop(0)
                elif self.action:
                    action=self.action;self.action=None
                    if action[0]=='setup':
                        context=next(s for s in strings(json.loads(request['body'])) if 'Treat the following JSON as setup data' in s)
                        context,_=json.JSONDecoder().raw_decode(context.rsplit('not instructions: ',1)[1])
                        assert context['stage']==action[1],context
                        calls=[{'name':'setup_answer','arguments':{'question_id':context['question_id'],'value':action[2]}}]
                    else:
                        calls=action[1]
                    model={'kind':'tools','calls':calls} if action[0]!='marker' else {'kind':'final','text':MARKER}
                    if action[0]!='marker': self.replies.append({'kind':'final','text':MARKER if action[0]=='marker' else 'Synthetic operation completed.'})
                else:
                    raise RuntimeError('unplanned model call')
                self.model_count+=1;assert self.model_count<=80
                result={'candidates':[{'finishReason':'STOP','content':{'parts':[{'text':json.dumps(model)}]}}],'usageMetadata':{'candidatesTokenCount':40}}
                self.send(f'FORGEHTTP1\t{fields[1]}\tok\t{json.dumps({"status":200,"body":json.dumps(result)}).encode().hex()}\n')
            elif fields[:2]==['FORGE1',str(identifier)]:
                if fields[2]!='ok':raise RuntimeError('console error: '+bytes.fromhex(fields[-1]).decode())
                return json.loads(bytes.fromhex(fields[3]))
            else:self.logs.append(line)

    def chat(self,text,action=None):
        self.action=action;self.identifier+=1
        self.send(f'FORGE1\t{self.identifier}\tchat\t{text.encode().hex()}\n')
        result=self.response(self.identifier)
        assert not self.replies and self.action is None
        return result

    def tools(self,calls):
        result=self.chat('Run the explicitly requested synthetic durability checks.',('tools',calls))
        return {tool['name']:tool['output'] for tool in result['tools']}

    def read(self,path):
        result=self.tools([{'name':'fs_read','arguments':{'path':path}}])['fs_read']
        assert result.get('ok'),result
        return result['content']

    def close(self):
        self.args.output.joinpath(self.phase+'.log').write_text('\n'.join(self.logs)+'\n')
        if self.channel:self.channel.close()
        self.selector.close();self.child.kill();self.child.wait()


def main():
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--kernel',type=Path,required=True);parser.add_argument('--disk',type=Path,required=True)
    parser.add_argument('--output',type=Path,required=True);parser.add_argument('--inspect',action='store_true')
    if not __debug__: raise SystemExit('Run without Python -O: acceptance assertions are required')
    args=parser.parse_args();args.output.mkdir(parents=True,exist_ok=True)
    baseline=(hashlib.sha256(args.disk.read_bytes()).hexdigest(),args.disk.stat().st_mtime_ns)
    with tempfile.TemporaryDirectory(prefix='alloyos-durability-',dir='/tmp') as temp:
        root=Path(temp);disk=root/'copy.img';shutil.copy2(args.disk,disk)
        original_kernel=args.kernel
        args.kernel=root/'kernel.elf';shutil.copy2(original_kernel,args.kernel)
        kernel_hash=hashlib.sha256(args.kernel.read_bytes()).hexdigest()
        guest=Guest(args,disk,root,'first')
        try:
            guest.boot();guest.chat('/model gemini');guest.chat('/setup')
            setup=json.loads(guest.read('/etc/setup.json'));assert setup['stage']=='Welcome',setup
            for stage,value in [('Welcome','Durability User'),('AgentName','Durability Agent')]:
                out=guest.chat('Save this synthetic setup answer.',('setup',stage,value))
                assert out['tools'][0]['output']['ok'],out
            guest.chat('Create the synthetic session-only marker.',('marker',[]))
            guest.capture('first-setup.png')
        finally:guest.close()
        print('PASS: actual guest saved partial setup and synthetic conversation',flush=True)
        guest=Guest(args,disk,root,'resume')
        try:
            guest.boot();guest.forbid_marker=True
            guest.capture('resumed-greeting.png')
            assert 'gemini' in guest.chat('/model')['text']
            setup=json.loads(guest.read('/etc/setup.json'));assert setup['user_name']=='Durability User' and setup['agent_name']=='Durability Agent' and setup['stage']=='TimeRegion',setup
            for stage,value in [('TimeRegion','America/Los_Angeles'),('ClockFormat','24h'),('Memory','pause'),('Review','finish')]:
                out=guest.chat('Save this synthetic setup answer.',('setup',stage,value));assert out['tools'][0]['output']['ok'],out
            guest.capture('completed-setup.png')
            guest.swipe(5,1000,1000,1000);guest.capture('side-panel.png')
            guest.click(500,1150)  # Expand available metadata after the Clock card.
            guest.click(900,1320)  # Pin Calculator.
            guest.click(850,390)   # Unpin Clock, then append it after Calculator.
            time.sleep(.6)
            guest.click(900,1260)
            assert guest.read('/etc/dashboard.conf').splitlines()==['calc.scratchpad','clock.now']
            result=guest.tools([{'name':'mcp_call_tool','arguments':{'app':'calc','tool':'calc.evaluate','args':{'expression':'19+23'}}}])
            assert result['mcp_call_tool'].get('ok',True),result
            history=json.loads(guest.read('/var/calc/history.json'))
            assert history['items'][-1]['result']==42,history
            guest.swipe(5,1000,1000,1000);guest.capture('pinned-calculation.png')
        finally:guest.close()
        print('PASS: resumed setup/config and native pin ordering saved',flush=True)
        guest=Guest(args,disk,root,'restored')
        try:
            guest.boot();guest.forbid_marker=True;guest.capture('restored-greeting.png')
            setup=json.loads(guest.read('/etc/setup.json'))
            assert setup['stage']=='Complete' and setup['learning_paused'] is True,setup
            assert 'gemini' in guest.chat('/model')['text']
            assert guest.read('/etc/dashboard.conf').splitlines()==['calc.scratchpad','clock.now']
            prefs=json.loads(guest.read('/var/clock/preferences.json'))
            assert prefs=={'timezone':'America/Los_Angeles','clock_format':'24h'},prefs
            history=json.loads(guest.read('/var/calc/history.json'));assert history['items'][-1]['result']==42,history
            guest.swipe(5,1000,1000,1000);guest.capture('restored-side-panel.png')
            time.sleep(6.5)
            guest.capture('periodic-clock-refresh.png')
            with Image.open(args.output/'restored-side-panel.png') as before, Image.open(args.output/'periodic-clock-refresh.png') as after:
                region=(90,1300,880,1510)  # Clock value/preferences, outside animated backdrop.
                assert before.crop(region).tobytes()!=after.crop(region).tobytes(), 'pinned Clock did not refresh while idle'
            print('PASS: pinned Clock display changed through periodic refresh without input',flush=True)
            guest.click(500,860)  # Actual Calculator Clear Tape action, not a model call.
            time.sleep(.7)
            history=json.loads(guest.read('/var/calc/history.json'));assert history['items']==[],history
            guest.swipe(5,1000,1000,1000);guest.capture('cleared-side-panel.png')
        finally:guest.close()
        print('PASS: reboot restored preferences/pins/app data; native Tile action cleared data',flush=True)
        guest=Guest(args,None,root,'volatile')
        try:
            guest.boot(headless=True)
            volatile_default_model=guest.chat('/model')['text']
            assert 'gemma' in volatile_default_model,volatile_default_model
            assert 'session only' in guest.chat('/model gemini')['text']
            assert 'storage is volatile' in guest.chat('/setup')['text']
            for command in ('/knowledge pause','/knowledge resume'):
                assert 'session only' in guest.chat(command)['text']
            result=guest.chat('Save a synthetic session-only name.',('setup','Welcome','Volatile User'))
            assert result['tools'][0]['output']['ok'],result
            assert json.loads(guest.read('/etc/setup.json'))['user_name']=='Volatile User'
        finally:guest.close()
        guest=Guest(args,None,root,'volatile-reboot')
        try:
            guest.boot(headless=True)
            restored_model=guest.chat('/model')['text']
            assert restored_model==volatile_default_model,restored_model
            assert 'session only' in guest.chat('/model gemini')['text']
            guest.chat('/setup')
            setup=json.loads(guest.read('/etc/setup.json'))
            assert setup['stage']=='Welcome' and setup['user_name']=='',setup
        finally:guest.close()
        print('PASS: headless RamFS changes are session-only and absent after reboot; no protected UI claimed',flush=True)
    assert baseline==(hashlib.sha256(args.disk.read_bytes()).hexdigest(),args.disk.stat().st_mtime_ns),'seed disk was modified'
    report={'kernel_sha256':kernel_hash,'seed_sha256':baseline[0],'seed_unchanged':True,'durable_boots':3,'setup_config_pins_app_data':'verified','conversation_restoration':'absent','periodic_clock_refresh':'pixel-region changed without input','volatile_backend':'RamFS; configuration/setup writes lost on reboot'
        ,'volatile_display':'ELF unavailable; no UI persistence claim','provider_account_mic_actions':0}
    (args.output/'result.json').write_text(json.dumps(report,indent=2)+'\n')


if __name__=='__main__':main()
