#!/usr/bin/env python3
"""Capture protected UI, keyboard IPC reply, and display restart in local QEMU.

Use native PNG: the installed QEMU PPM dump mishandles RGB row padding at1206px.
Review generated screenshots in target/process-validation.
"""
import subprocess,time,pathlib,socket,json,tempfile,shutil,re,argparse
root=pathlib.Path(__file__).resolve().parents[1]
parser=argparse.ArgumentParser(description=__doc__)
parser.add_argument('--kernel',type=pathlib.Path,default=root/'target/aarch64-unknown-none/release/alloyos-kernel')
parser.add_argument('--disk',type=pathlib.Path,default=root/'target/aarch64-unknown-none/release/cobaltfs.img')
parser.add_argument('--output',type=pathlib.Path,default=root/'target/process-validation')
args=parser.parse_args();out=args.output;out.mkdir(parents=True,exist_ok=True)
with tempfile.TemporaryDirectory(dir="/tmp") as tmp:
 sock=pathlib.Path(tmp)/'qmp.sock';log=out/'normal-ui.log'
 disk=pathlib.Path(tmp)/'cobaltfs.img';shutil.copy2(args.disk,disk)
 p=subprocess.Popen(['qemu-system-aarch64','-M','virt,gic-version=3','-accel','tcg','-cpu','cortex-a72','-kernel',str(args.kernel),'-drive',f'file={disk},if=none,format=raw,id=hd0','-device','virtio-blk-device,drive=hd0','-device','virtio-gpu-device,xres=1206,yres=2622','-device','virtio-keyboard-device','-device','virtio-tablet-device','-serial',f'file:{log}','-qmp',f'unix:{sock},server=on,wait=off','-display','none','-m','256M','-snapshot','-no-reboot'])
 try:
  until=time.monotonic()+30
  while not sock.exists():time.sleep(.1);assert p.poll() is None;assert time.monotonic()<until
  ch=socket.socket(socket.AF_UNIX,socket.SOCK_STREAM);ch.connect(str(sock));stream=ch.makefile('rwb');stream.readline()
  def qmp(cmd,args=None):
   msg={'execute':cmd}
   if args:msg['arguments']=args
   stream.write((json.dumps(msg)+'\n').encode());stream.flush()
   while True:
    reply=json.loads(stream.readline())
    if 'event' in reply:continue
    assert 'error' not in reply,reply
    return reply
  qmp('qmp_capabilities')
  while '[ferrofluidd] EL0 frame presented' not in log.read_text():time.sleep(.2);assert time.monotonic()<until,log.read_text()
  assert '[mm] Frame allocator and 160 MiB kernel heap online.' in log.read_text(),log.read_text()
  time.sleep(1)
  qmp('screendump',{'filename':str(out/'normal-ui-native.png'),'format':'png'})
  qmp('input-send-event',{'events':[{'type':'abs','data':{'axis':'x','value':int(300/1206*32767)}},{'type':'abs','data':{'axis':'y','value':int(2500/2622*32767)}},{'type':'btn','data':{'down':True,'button':'left'}},{'type':'btn','data':{'down':False,'button':'left'}}]});time.sleep(.4)
  for key in ('slash','a','p','p','s'):
   qmp('input-send-event',{'events':[{'type':'key','data':{'down':True,'key':{'type':'qcode','data':key}}},{'type':'key','data':{'down':False,'key':{'type':'qcode','data':key}}}]});time.sleep(.1)
  qmp('screendump',{'filename':str(out/'keyboard-ui-native.png'),'format':'png'})
  qmp('input-send-event',{'events':[{'type':'key','data':{'down':True,'key':{'type':'qcode','data':'ret'}}},{'type':'key','data':{'down':False,'key':{'type':'qcode','data':'ret'}}}]});time.sleep(2)
  qmp('screendump',{'filename':str(out/'apps-reply-ui-native.png'),'format':'png'})
  for cycle in range(3):
   if cycle:
    qmp('input-send-event',{'events':[{'type':'abs','data':{'axis':'x','value':int(300/1206*32767)}},{'type':'abs','data':{'axis':'y','value':int(2500/2622*32767)}},{'type':'btn','data':{'down':True,'button':'left'}},{'type':'btn','data':{'down':False,'button':'left'}}]});time.sleep(.4)
    for key in ('slash','a','p','p','s','ret'):
     qmp('input-send-event',{'events':[{'type':'key','data':{'down':True,'key':{'type':'qcode','data':key}}},{'type':'key','data':{'down':False,'key':{'type':'qcode','data':key}}}]});time.sleep(.1)
    time.sleep(2)
   for key in ('slash','d','i','s','p','l','a','y','spc','r','e','s','t','a','r','t','ret'):
    qmp('input-send-event',{'events':[{'type':'key','data':{'down':True,'key':{'type':'qcode','data':key}}},{'type':'key','data':{'down':False,'key':{'type':'qcode','data':key}}}]});time.sleep(.1)
   until=time.monotonic()+30
   while log.read_text().count('EL0 frame presented')<cycle+2:time.sleep(.2);assert time.monotonic()<until,log.read_text()
   time.sleep(.3);qmp('screendump',{'filename':str(out/f'restarted-ui-{cycle+1}-native.png'),'format':'png'})
  pids=re.findall(r'protected EL0 executable loaded pid=(\d+)',log.read_text());assert len(set(pids))>=4,pids
  shutil.copy2(out/'restarted-ui-3-native.png',out/'restarted-ui-native.png')
  print(log.read_text());print('Screenshots:',out)
 finally:
  p.terminate();p.wait(timeout=5)
