#!/usr/bin/env python3
"""Build and run real protected-process acceptance probes in QEMU."""
import argparse
import pathlib
import select
import subprocess
import time

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('--accel', choices=['tcg', 'hvf'], default='tcg')
parser.add_argument('--gic-version', choices=['2', '3'], default='2')
args = parser.parse_args()
root = pathlib.Path(__file__).resolve().parents[1]
subprocess.run(['cargo', 'build', '-p', 'alloyos-kernel', '--target',
                'aarch64-unknown-none', '--features', 'debug-shell,process-selftest'],
               cwd=root, check=True)
command = ['qemu-system-aarch64', '-M', f'virt,gic-version={args.gic_version}',
           '-accel', args.accel, '-cpu', 'host' if args.accel == 'hvf' else 'cortex-a72',
           '-kernel', str(root / 'target/aarch64-unknown-none/debug/alloyos-kernel'),
           '-serial', 'stdio', '-display', 'none', '-m', '256M', '-no-reboot']
process = subprocess.Popen(command, stdout=subprocess.PIPE, stderr=subprocess.STDOUT)
output = b''
deadline = time.monotonic() + 30
try:
    while time.monotonic() < deadline:
        if select.select([process.stdout], [], [], 1)[0]:
            chunk = process.stdout.read1(65536)
            if not chunk:
                break
            output += chunk
            if b'kernel survival\n' in output or b'panicked at' in output:
                break
    print(output.decode(errors='replace'))
    if b'intentional EL0 panic' not in output:
        raise SystemExit('Protected-process panic message missing')
    if b'[crash] exception domain=user' not in output or b'[crash] frame=2' not in output:
        raise SystemExit('Protected-process fault diagnostics missing')
    if b'kernel survival\n' not in output:
        raise SystemExit('Protected-process guest acceptance failed')
finally:
    process.terminate()
    try:
        process.wait(timeout=5)
    except subprocess.TimeoutExpired:
        process.kill()
        process.wait()
