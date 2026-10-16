#!/usr/bin/env python3
"""Verify real ELF app execution and display crash/recovery in headless QEMU."""
import argparse
import pathlib
import select
import subprocess
import time
from process_acceptance import acceptance_errors, has_fatal_failure

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('--skip-build', action='store_true')
parser.add_argument('--kernel', type=pathlib.Path)
parser.add_argument('--disk', type=pathlib.Path)
parser.add_argument('--accel', choices=['tcg', 'hvf'], default='tcg')
parser.add_argument('--gic-version', choices=['2', '3'])
parser.add_argument('--timeout', type=int, default=90)
args = parser.parse_args()
args.gic_version = args.gic_version or ('3' if args.accel == 'hvf' else '2')
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
           '-serial', 'stdio', '-display', 'none', '-m', '512M', '-no-reboot',
           '-snapshot']

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
            if not acceptance_errors(output) or has_fatal_failure(output):
                break
    print(output.decode(errors='replace'))
    errors = acceptance_errors(output)
    if errors:
        raise SystemExit('Protected-runtime acceptance failed; ' + '; '.join(errors))
finally:
    process.terminate()
    try:
        process.wait(timeout=5)
    except subprocess.TimeoutExpired:
        process.kill()
        process.wait()
