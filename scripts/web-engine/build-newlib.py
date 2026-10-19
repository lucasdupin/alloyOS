#!/usr/bin/env python3
"""Build pinned freestanding AArch64 newlib archives; no alloyOS syscall shim.

Only downloads source when --fetch is supplied. The SHA256 pin is the observed
official release archive digest, not a separately authenticated signature.
"""
import argparse
import hashlib
import json
from pathlib import Path
import shutil
import subprocess
import tarfile
import urllib.request

VERSION = "4.6.0.20260123"
SHA256 = "6ff27e3bf022666f43f7802255be680eeff722ac181b1725d21e2e8318604ee3"
URL = f"https://sourceware.org/pub/newlib/newlib-{VERSION}.tar.gz"


def run(command, cwd=None, logfile=None):
    result = subprocess.run([str(value) for value in command], cwd=cwd,
                            text=True, stdout=subprocess.PIPE, stderr=subprocess.STDOUT)
    if logfile:
        logfile.write_text(result.stdout)
    if result.returncode:
        raise RuntimeError(f"command failed: {command}\n{result.stdout[-6000:]}")
    return result.stdout.strip()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--archive", type=Path, required=True)
    parser.add_argument("--fetch", action="store_true")
    parser.add_argument("--jobs", type=int, default=4)
    args = parser.parse_args()
    if args.jobs < 1:
        parser.error("--jobs must be positive")
    output = args.output.resolve()
    archive = args.archive.resolve()
    output.mkdir(parents=True, exist_ok=True)
    if args.fetch and not archive.exists():
        archive.parent.mkdir(parents=True, exist_ok=True)
        temporary = archive.with_suffix(archive.suffix + ".part")
        try:
            with urllib.request.urlopen(URL, timeout=60) as response, temporary.open("wb") as target:
                shutil.copyfileobj(response, target)
            if hashlib.sha256(temporary.read_bytes()).hexdigest() != SHA256:
                raise ValueError("newlib archive checksum mismatch")
            temporary.replace(archive)
        finally:
            temporary.unlink(missing_ok=True)
    if hashlib.sha256(archive.read_bytes()).hexdigest() != SHA256:
        raise ValueError("newlib archive checksum mismatch")
    source = output / f"newlib-{VERSION}"
    if not source.exists():
        with tarfile.open(archive) as contents:
            contents.extractall(output, filter="data")
    rustroot = Path(run(["rustc", "--print", "sysroot"]))
    host = next(line.split(": ", 1)[1] for line in run(["rustc", "-vV"]).splitlines()
                if line.startswith("host: "))
    tools = rustroot / "lib/rustlib" / host / "bin"
    build = output / "build"
    prefix = output / "sysroot"
    build.mkdir(exist_ok=True)
    command = [source / "newlib/configure", "--host=aarch64-none-elf",
               "--target=aarch64-none-elf", f"--prefix={prefix}", "--disable-multilib",
               "--disable-newlib-supplied-syscalls", "CC=clang --target=aarch64-none-elf",
               "CFLAGS=-O2 -ffreestanding -fno-stack-protector",
               f"AR={tools / 'llvm-ar'}", f"RANLIB={tools / 'llvm-ar'} s",
               f"READELF={tools / 'llvm-readobj'}"]
    run(command, build, output / "configure.log")
    run(["make", f"-j{args.jobs}"], build, output / "build.log")
    run(["make", "install"], build, output / "install.log")
    libraries = {}
    for name in ("libc.a", "libm.a"):
        matches = list(prefix.rglob(name))
        if len(matches) != 1:
            raise ValueError(f"expected one installed {name}, got {matches}")
        libraries[name] = dict(path=str(matches[0]), size=matches[0].stat().st_size,
                               sha256=hashlib.sha256(matches[0].read_bytes()).hexdigest())
    report = dict(version=VERSION, source_url=URL, source_sha256=SHA256,
                  compiler=run(["clang", "--version"]).splitlines()[0],
                  target="aarch64-none-elf", configure=[str(value) for value in command],
                  libraries=libraries, guest_execution="not run",
                  alloyos_syscall_adapters="not supplied", pthreads="not supplied",
                  cxx_standard_library="not supplied", javascriptcore="not built")
    (output / "report.json").write_text(json.dumps(report, indent=2) + "\n")
    print(json.dumps(report, indent=2))


if __name__ == "__main__":
    main()
