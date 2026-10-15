#!/usr/bin/env python3
"""Crash a fresh diskless test guest and require a symbolicated stack trace."""
import argparse
from pathlib import Path
import selectors
import shutil
import subprocess
import time
from symbolicate_crash import symbolicate, symbols

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('--accel', choices=['hvf', 'tcg'], default='hvf')
parser.add_argument('--log', type=Path, default=Path('/tmp/alloyos-crash.log'))
args = parser.parse_args()
root = Path(__file__).resolve().parents[1]
subprocess.run(['cargo', 'build', '-p', 'alloyos-kernel', '--target', 'aarch64-unknown-none', '--features', 'crash-selftest', '--target-dir', 'target/crash-selftest'], cwd=root, check=True)
elf = root / 'target/crash-selftest/aarch64-unknown-none/debug/alloyos-kernel'
saved_elf = args.log.with_suffix(".elf")
shutil.copyfile(elf, saved_elf)
process = subprocess.Popen(['qemu-system-aarch64', '-M', 'virt', '-accel', args.accel,
                            '-cpu', 'host' if args.accel == 'hvf' else 'cortex-a72',
                            '-kernel', str(elf), '-m', '512M', '-serial', 'stdio',
                            '-display', 'none', '-monitor', 'none', '-no-reboot'],
                           stdin=subprocess.DEVNULL, stdout=subprocess.PIPE, stderr=subprocess.STDOUT)
selector = selectors.DefaultSelector()
selector.register(process.stdout, selectors.EVENT_READ)
output = bytearray()
try:
    deadline = time.monotonic() + 30
    while time.monotonic() < deadline:
        if selector.select(1):
            chunk = process.stdout.read1(65536)
            if not chunk:
                break
            output.extend(chunk)
            if b'[crash] stack-end reason=' in output and output.endswith(b'\n'):
                break
    text = output.decode(errors='replace')
    args.log.write_text(text)
    resolved = symbolicate(text, symbols(saved_elf))
    print(resolved, end='')
    assert 'intentional crash diagnostic selftest' in text
    assert '=> alloyos_kernel::diagnostics::crash_probe_leaf' in resolved
    assert '=> alloyos_kernel::diagnostics::crash_probe_parent' in resolved
    assert '[crash] stack-end reason=end' in text
    print('PASS: lock-independent panic output and symbolicated parent frames')
finally:
    selector.close()
    process.kill()
    process.wait()
