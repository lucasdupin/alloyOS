#!/usr/bin/env python3
"""Run native protected-memory and process-isolation acceptance in QEMU.

Uses a diskless guest; no user disk, network or credentials are attached.
The kernel must be built with process-selftest. This is a prerequisite probe,
not evidence that JavaScriptCore or WPE can execute yet.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path
import selectors
import subprocess
import time

from process_acceptance import acceptance_errors, has_fatal_failure

CXX_MARKER = b'[web-runtime] PASS: native C++ static ELF virtual dispatch and protected diagnostic report'

MEMORY_MARKER = b'[web-runtime] PASS: native EL0 reserve, zero commit, preserved commit, decommit zeroing, release, quota denial, executable denial, guard fault and teardown'


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--kernel', type=Path)
    parser.add_argument('--build', action='store_true')
    parser.add_argument('--require-cxx', action='store_true')
    parser.add_argument('--output', type=Path, required=True)
    parser.add_argument('--accel', choices=('hvf', 'tcg'), default='hvf')
    parser.add_argument('--timeout', type=int, default=60)
    args = parser.parse_args()
    args.output.mkdir(parents=True, exist_ok=True)
    root = Path(__file__).resolve().parents[1]
    target = Path(os.environ.get('CARGO_TARGET_DIR', str(root / 'target'))).resolve()
    if args.build:
        env = os.environ.copy()
        if args.require_cxx:
            cxx_output = args.output.resolve() / 'cxx'
            subprocess.run(['python3', str(root / 'scripts/web-engine/build-native-probe.py'),
                            '--output', str(cxx_output)], cwd=root, check=True)
            env['ALLOYOS_CXX_PROBE_ELF'] = str(cxx_output / 'cxx-probe.elf')
            env['RUSTFLAGS'] = env.get('RUSTFLAGS', '-C force-frame-pointers=yes') + ' --cfg alloyos_cxx_probe --check-cfg=cfg(alloyos_cxx_probe)'
        subprocess.run(['cargo', 'build', '-p', 'alloyos-kernel', '--target',
                        'aarch64-unknown-none', '--release', '--features', 'process-selftest'],
                       cwd=root, env=env, check=True)
    kernel = args.kernel.resolve() if args.kernel else target / 'aarch64-unknown-none/release/alloyos-kernel'
    required = [MEMORY_MARKER] + ([CXX_MARKER] if args.require_cxx else [])
    command = ['qemu-system-aarch64', '-M', 'virt,gic-version=' + ('3' if args.accel == 'hvf' else '2'),
               '-accel', args.accel, '-cpu', 'host' if args.accel == 'hvf' else 'cortex-a72',
               '-kernel', str(kernel), '-m', '512M', '-display', 'none',
               '-serial', 'stdio', '-no-reboot']
    manifest = {'kernel': str(kernel), 'sha256': hashlib.sha256(kernel.read_bytes()).hexdigest(),
                'command': command, 'acceptance': 'not run'}
    child = subprocess.Popen(command, stdout=subprocess.PIPE, stderr=subprocess.STDOUT)
    poll = selectors.DefaultSelector()
    poll.register(child.stdout, selectors.EVENT_READ)
    data = bytearray()
    try:
        deadline = time.monotonic() + args.timeout
        while time.monotonic() < deadline:
            if poll.select(.2):
                chunk = os.read(child.stdout.fileno(), 65536)
                if not chunk:
                    break
                data.extend(chunk)
                if len(data) > 2 * 1024 * 1024:
                    raise RuntimeError('guest transcript exceeds bound')
                lines = data.split(b'\n')[:-1]
                if has_fatal_failure(b'\n'.join(lines) + b'\n') or (all(marker in [line.rstrip(b'\r') for line in lines] for marker in required)
                                              and not acceptance_errors(data, isolation=True)):
                    break
            if child.poll() is not None:
                break
        errors = acceptance_errors(data, isolation=True)
        for marker in required:
            if marker not in [line.rstrip(b'\r') for line in data.split(b'\n')[:-1]]:
                errors.append('missing: ' + marker.decode())
        manifest['acceptance'] = 'failed' if errors else 'passed'
        if errors:
            raise RuntimeError('; '.join(errors))
        print('PASS: native EL0 web runtime memory and process isolation')
    finally:
        (args.output / 'guest.log').write_bytes(data)
        (args.output / 'artifact.json').write_text(json.dumps(manifest, indent=2) + '\n')
        poll.close()
        if child.poll() is None:
            child.terminate()
        try:
            child.wait(timeout=5)
        except subprocess.TimeoutExpired:
            child.kill()
            child.wait()


if __name__ == '__main__':
    main()
