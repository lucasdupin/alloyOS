#!/usr/bin/env python3
"""Capture real guest UI through native QMP PNGs; JSON commands on stdin.
Uses production cloud relay and private credential references. No fixtures.
"""
import json, pathlib, shutil, socket, subprocess, sys, threading, time
ROOT=pathlib.Path(__file__).resolve().parents[1]
OUT=pathlib.Path(sys.argv[1]).resolve();OUT.mkdir(parents=True,exist_ok=True)
shutil.copy2(pathlib.Path(sys.argv[2]) if len(sys.argv)>2 else ROOT/'target/aarch64-unknown-none/release/cobaltfs.img',OUT/'disk.img')
log=open(OUT/'guest.log','wb')
p=subprocess.Popen([str(pathlib.Path(__import__('os').environ.get('ALLOYOS_README_LAUNCHER', str(ROOT/'target/debug/examples/readme_qemu_capture')))),str(ROOT/'target/aarch64-unknown-none/release/alloyos-kernel'),str(OUT/'disk.img'),str(OUT/'qmp.sock'),str(ROOT/'target/alloyos-voice-helper')],stdout=log,stderr=log,cwd=ROOT)
try:
 deadline=time.monotonic()+60
 while not (OUT/'qmp.sock').exists():
  assert p.poll() is None,'guest exited';assert time.monotonic()<deadline,'QMP timeout';time.sleep(.1)
 sock=socket.socket(socket.AF_UNIX);sock.settimeout(15);sock.connect(str(OUT/'qmp.sock'));stream=sock.makefile('rwb');stream.readline();lock=threading.Lock()
 def qmp(cmd,args=None):
  with lock:
   msg={'execute':cmd}
   if args:msg['arguments']=args
   stream.write((json.dumps(msg)+'\n').encode());stream.flush()
   while True:
    reply=json.loads(stream.readline())
    if 'event' in reply:continue
    if 'error' in reply:raise RuntimeError(reply)
    return reply
 qmp('qmp_capabilities')
 while b'EL0 frame presented' not in (OUT/'guest.log').read_bytes():
  assert p.poll() is None,'guest exited';assert time.monotonic()<deadline,'UI timeout';time.sleep(.1)
 time.sleep(2)
 def shot(name):qmp('screendump',{'filename':str(OUT/name),'format':'png'})
 def key(k):qmp('input-send-event',{'events':[{'type':'key','data':{'down':d,'key':{'type':'qcode','data':k}}} for d in (True,False)]})
 def text(s):
  names={' ':'spc','/':'slash','.':'dot',':':'semicolon','=':'equal','|':'backslash','-':'minus','?':'slash',',':'comma'}
  for c in s:
   shift=c.isupper() or c in ':|?'
   if shift:qmp('input-send-event',{'events':[{'type':'key','data':{'down':True,'key':{'type':'qcode','data':'shift'}}}]})
   key(names.get(c,c.lower()))
   if shift:qmp('input-send-event',{'events':[{'type':'key','data':{'down':False,'key':{'type':'qcode','data':'shift'}}}]})
   time.sleep(.15)
 def click(x,y):qmp('input-send-event',{'events':[{'type':'abs','data':{'axis':'x','value':int(x/1206*32767)}},{'type':'abs','data':{'axis':'y','value':int(y/2622*32767)}},*({'type':'btn','data':{'down':d,'button':'left'}} for d in (True,False))]})
 shot('current.png');print('READY',flush=True)
 for line in sys.stdin:
  try:
   command=json.loads(line);op=command['op']
   if op=='quit':break
   if op=='shot':shot(command.get('name','current.png'))
   elif op=='text':text(command['text'])
   elif op=='key':key(command['key'])
   elif op=='clear':
    qmp('input-send-event',{'events':[{'type':'key','data':{'down':True,'key':{'type':'qcode','data':'ctrl'}}}]});key('a');qmp('input-send-event',{'events':[{'type':'key','data':{'down':False,'key':{'type':'qcode','data':'ctrl'}}}]});key('backspace')
   elif op=='click':click(command['x'],command['y'])
   elif op=='wait':time.sleep(command['seconds'])
   elif op=='scene':
    dest=OUT/command['name'];dest.mkdir(exist_ok=True);stop=threading.Event()
    def capture():
     start=time.monotonic();i=0
     while not stop.is_set():
      qmp('screendump',{'filename':str(dest/f'{i:05}.png'),'format':'png'});i+=1;stop.wait(max(0,i/8-(time.monotonic()-start)))
    worker=threading.Thread(target=capture);worker.start()
    try:
     for action in command['actions']:
      if 'wait' in action:time.sleep(action['wait'])
      if 'click' in action:click(*action['click'])
      if 'text' in action:text(action['text'])
      if 'key' in action:key(action['key'])
    finally:stop.set();worker.join()
   shot('current.png');print('DONE '+op,flush=True)
  except Exception as e:print(type(e).__name__+': '+str(e),flush=True)
finally:
 p.terminate()
 try:p.wait(timeout=8)
 except subprocess.TimeoutExpired:p.kill()
 log.close()
