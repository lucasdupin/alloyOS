#!/usr/bin/env python3
"""Resolve alloyOS [crash] frames with the exact ELF that produced the log.

Use a separate invocation/ELF for each user executable (user VAs overlap).
Return addresses are adjusted by four bytes to resolve the AArch64 call site.
"""
import argparse
import bisect
from pathlib import Path
import re
import shutil
import subprocess


def find_nm():
    tool = shutil.which('llvm-nm')
    if tool:
        return tool
    sysroot = subprocess.check_output(['rustc', '--print', 'sysroot'], text=True).strip()
    matches = list(Path(sysroot).glob('lib/rustlib/*/bin/llvm-nm'))
    if not matches:
        raise RuntimeError('llvm-nm unavailable; install rustup component llvm-tools-preview')
    return str(matches[0])


def symbols(elf):
    output = subprocess.check_output([find_nm(), '--defined-only', '--numeric-sort', '--print-size', '--demangle', str(elf)], text=True)
    found = []
    for line in output.splitlines():
        match = re.match(r'^([0-9a-fA-F]+)\s+([0-9a-fA-F]+)\s+[tT]\s+(.+)$', line)
        if match:
            address, size, name = match.groups()
            found.append((int(address, 16), int(size, 16), name))
    return sorted(found)


def symbolicate(text, table):
    addresses = [item[0] for item in table]
    lines = []
    for line in text.splitlines():
        match = re.search(r'\[crash\] frame=\d+ pc=(0x[0-9a-fA-F]+) kind=(fault|return)', line)
        if match:
            pc = int(match[1], 16) - (4 if match[2] == 'return' else 0)
            index = bisect.bisect_right(addresses, pc) - 1
            if index >= 0 and table[index][0] <= pc < table[index][0] + table[index][1]:
                start, _, name = table[index]
                line += f' => {name}+0x{pc-start:x}'
            else:
                line += ' => unresolved (check ELF/build/domain)'
        lines.append(line)
    return '\n'.join(lines) + '\n'


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--elf', type=Path, required=True)
    parser.add_argument('--log', type=Path, required=True)
    args = parser.parse_args()
    print(symbolicate(args.log.read_text(), symbols(args.elf)), end='')


if __name__ == '__main__':
    main()
