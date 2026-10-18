#!/usr/bin/env python3
"""Build and inspect a freestanding native alloyOS C++ compiler probe.

This proves cross compilation and ELF linkage only. Guest execution is a separate
gate. It deliberately does not borrow a host sysroot or pretend to provide std.
"""
import argparse
import json
from pathlib import Path
import shutil
import struct
import subprocess


def run(args):
    result = subprocess.run([str(arg) for arg in args], text=True,
                            stdout=subprocess.PIPE, stderr=subprocess.PIPE)
    if result.returncode:
        raise RuntimeError(f"command failed: {args}\n{result.stdout}{result.stderr}")
    return result.stdout.strip()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--cxx", default="clang++")
    parser.add_argument("--linker", type=Path)
    args = parser.parse_args()
    root = Path(__file__).resolve().parents[2]
    output = args.output.resolve()
    output.mkdir(parents=True, exist_ok=True)
    linker = args.linker
    if linker is None:
        sysroot = Path(run(["rustc", "--print", "sysroot"]))
        host = next(line.split(": ", 1)[1] for line in run(["rustc", "-vV"]).splitlines()
                    if line.startswith("host: "))
        linker = sysroot / "lib/rustlib" / host / "bin/rust-lld"
    compiler = shutil.which(args.cxx)
    if compiler is None:
        raise ValueError(f"C++ compiler not found: {args.cxx}")
    common = [compiler, "--target=aarch64-unknown-none", "-std=c++23", "-O2",
              "-ffreestanding", "-fno-exceptions", "-fno-rtti", "-fno-stack-protector",
              "-fno-unwind-tables", "-fno-asynchronous-unwind-tables", "-fno-pic",
              "-nostdinc++", "-Wall", "-Wextra", "-Werror"]
    objects = []
    for name in ("main", "evaluate"):
        obj = output / f"{name}.o"
        run(common + ["-c", root / f"scripts/web-engine/probe/{name}.cpp", "-o", obj])
        objects.append(obj)
    elf = output / "cxx-probe.elf"
    run([linker, "-flavor", "gnu", "-m", "aarch64elf", "--no-undefined",
         "--fatal-warnings", "--build-id=none", "-T",
         root / "crates/alloyos-ferrofluid/process.ld", *objects, "-o", elf])
    data = elf.read_bytes()
    if data[:6] != b"\x7fELF\x02\x01":
        raise ValueError("expected a little-endian 64-bit ELF")
    kind, machine = struct.unpack_from("<HH", data, 16)
    entry, phoff = struct.unpack_from("<QQ", data, 24)
    phsize, phcount = struct.unpack_from("<HH", data, 54)
    segments = []
    for index in range(phcount):
        segment = struct.unpack_from("<IIQQQQQQ", data, phoff + index * phsize)
        typ, flags, offset, address, _, filesz, memsz, alignment = segment
        if typ in (2, 3, 7):
            raise ValueError("dynamic linkage/interpreter/ELF TLS is unsupported")
        if typ == 1:
            if flags & 3 == 3:
                raise ValueError("writable executable segment")
            segments.append(dict(address=address, filesz=filesz, memsz=memsz,
                                 flags=flags, alignment=alignment))
    if kind != 2 or machine != 183 or not any(
            s["flags"] & 1 and s["address"] <= entry < s["address"] + s["memsz"]
            for s in segments):
        raise ValueError("invalid AArch64 executable or entry point")
    report = dict(compiler=run([compiler, "--version"]).splitlines()[0],
                  linker=run([linker, "-flavor", "gnu", "--version"]),
                  target="aarch64-unknown-none", elf=str(elf), entry=entry,
                  size=len(data), segments=segments, guest_execution="not run",
                  cxx_standard_library="not provided", javascriptcore="not built")
    (output / "report.json").write_text(json.dumps(report, indent=2) + "\n")
    print(json.dumps(report, indent=2))


if __name__ == "__main__":
    main()
