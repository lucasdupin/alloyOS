#!/usr/bin/env python3
"""Boot diskless guests with/without VirtIO-RNG and assert crypto availability.

No entropy bytes, accounts, model keys, disks or network interfaces are exposed.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path
import selectors
import subprocess
import time


def run(kernel, present, output, modern=False):
    command=['qemu-system-aarch64','-M','virt,gic-version=3','-accel','hvf',
             '-cpu','host','-kernel',str(kernel.resolve()),'-m','512M',
             '-display','none','-serial','stdio','-no-reboot']
    if modern:
        command += ['-global','virtio-mmio.force-legacy=false']
    if present:
        command += ['-device','virtio-rng-device']
    child=subprocess.Popen(command,stdout=subprocess.PIPE,stderr=subprocess.PIPE)
    poll=selectors.DefaultSelector()
    poll.register(child.stdout,selectors.EVENT_READ)
    data=bytearray()
    try:
        deadline=time.monotonic()+30
        while time.monotonic()<deadline:
            if child.poll() is not None:
                raise RuntimeError('guest exited: '+child.stderr.read().decode(errors='replace'))
            if poll.select(.2):
                data.extend(os.read(child.stdout.fileno(),65536))
                if len(data)>1024*1024:
                    raise RuntimeError('excess guest output')
                if b'[entropy]' in data and b'crypto_api=' in data and data.endswith(b'\n'):
                    break
        text=data.decode(errors='replace')
        expected=('[entropy] secure source ready crypto_api=true' if present else
                  '[entropy] secure source unavailable; crypto requests fail closed crypto_api=false')
        if expected not in text:
            raise RuntimeError('missing exact entropy assertion: '+expected)
        print('PASS: trusted entropy device enables crypto' if present else
              'PASS: missing entropy device fails crypto closed',flush=True)
    finally:
        output.write_bytes(data)
        poll.close()
        child.kill()
        child.wait()


def main():
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--kernel',type=Path,required=True)
    parser.add_argument('--output',type=Path,required=True)
    args=parser.parse_args()
    args.output.mkdir(parents=True,exist_ok=True)
    digest=hashlib.sha256(args.kernel.read_bytes()).hexdigest()
    (args.output/'artifact.json').write_text(json.dumps({'kernel':str(args.kernel.resolve()),'sha256':digest},indent=2)+'\n')
    run(args.kernel,False,args.output/'absent.log')
    run(args.kernel,True,args.output/'present.log')
    run(args.kernel,True,args.output/'present-modern.log',modern=True)


if __name__=='__main__':
    main()
