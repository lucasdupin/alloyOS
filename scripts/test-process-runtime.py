#!/usr/bin/env python3
"""Verify real ELF app execution and display crash/recovery in headless QEMU."""
import argparse
import pathlib
import select
import subprocess
import time

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('--skip-build', action='store_true')
parser.add_argument('--kernel', type=pathlib.Path)
parser.add_argument('--disk', type=pathlib.Path)
parser.add_argument('--accel', choices=['tcg', 'hvf'], default='tcg')
parser.add_argument('--gic-version', choices=['2', '3'], default='2')
parser.add_argument('--timeout', type=int, default=90)
args = parser.parse_args()
root = pathlib.Path(__file__).resolve().parents[1]
if not args.skip_build:
    subprocess.run(['cargo', 'run', '-p', 'xtask', '--', 'build', '--release'], cwd=root, check=True)
    subprocess.run(['cargo', 'build', '-p', 'alloyos-kernel', '--target',
                    'aarch64-unknown-none', '--release', '--features',
                    'process-runtime-selftest'], cwd=root, check=True)
profile = root / 'target/aarch64-unknown-none/release'
kernel = args.kernel.resolve() if args.kernel else profile / 'alloyos-kernel'
disk = args.disk.resolve() if args.disk else profile / 'cobaltfs.img'
command = ['qemu-system-aarch64', '-M', f'virt,gic-version={args.gic_version}',
           '-accel', args.accel, '-cpu', 'host' if args.accel == 'hvf' else 'cortex-a72',
           '-kernel', str(kernel),
           '-drive', f'file={disk},if=none,format=raw,id=hd0',
           '-device', 'virtio-blk-device,drive=hd0',
           '-device', 'virtio-gpu-device,xres=1206,yres=2622',
           '-device', 'virtio-keyboard-device', '-device', 'virtio-tablet-device',
           '-serial', 'stdio', '-display', 'none', '-m', '256M', '-no-reboot',
           '-snapshot']
markers = [b'APP-PROCESS: hostile package parser limits and host recovery PASS',
           b'APP-PROCESS: persistent EL0 handler and filesystem IPC PASS',
           b'APP-PROCESS: denied peer filesystem capability PASS',
           b'APP-PROCESS: bounded infinite loop and independent peer PASS',
           b'APP-PROCESS: process restart PASS',
           b'APP-PROCESS: Tasks scheduler IPC persistence PASS',
           b'APP-PROCESS: bounded host-call flood and budget reset PASS',
           b'APP-PROCESS: private heap exhaustion and restart PASS',
           b'APP-PROCESS: display input and shared kernel operation denial PASS',
           b'APP-PROCESS: timer cancels nonyielding process and peer continues PASS',
           b'DISPLAY-PROCESS: real EL0 fault, restart, conversation/dashboard restoration PASS']
process = subprocess.Popen(command, stdout=subprocess.PIPE, stderr=subprocess.STDOUT)
output = b''
deadline = time.monotonic() + args.timeout
try:
    while time.monotonic() < deadline:
        if select.select([process.stdout], [], [], 1)[0]:
            chunk = process.stdout.read1(65536)
            if not chunk:
                break
            output += chunk
            if all(marker in output for marker in markers) or b'panicked at' in output:
                break
    print(output.decode(errors='replace'))
    missing = [marker.decode() for marker in markers if marker not in output]
    if missing:
        raise SystemExit('Protected-runtime acceptance failed; missing: ' + '; '.join(missing))
finally:
    process.terminate()
    try:
        process.wait(timeout=5)
    except subprocess.TimeoutExpired:
        process.kill()
        process.wait()
