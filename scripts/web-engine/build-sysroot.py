#!/usr/bin/env python3
"""Build the native alloyOS AArch64 C/C++/POSIX compatibility sysroot and static archives.

Layers alloyOS protected-syscall adapters (liballoyos_posix.a) and a minimal
freestanding C++ ABI runtime (liballoyos_cxxrt.a) onto either a cross-built
aarch64-none-elf newlib sysroot (--newlib) or the built-in freestanding base
headers, then compiles and links a multi-file C++23 verification ELF with
--no-undefined. Host compilation/linkage only; guest execution is separate.
"""
import argparse
import hashlib
import json
from pathlib import Path
import shutil
import struct
import subprocess
import sys

NEWLIB_TIME_SUPPLEMENT = """
#ifndef _ALLOYOS_NEWLIB_TIME_SUPPLEMENT
#define _ALLOYOS_NEWLIB_TIME_SUPPLEMENT
#ifdef __cplusplus
extern "C" {
#endif
#ifndef CLOCK_REALTIME
#define CLOCK_REALTIME 0
#endif
#ifndef CLOCK_MONOTONIC
#define CLOCK_MONOTONIC 1
#endif
int clock_gettime(clockid_t clock_id, struct timespec* tp);
int clock_getres(clockid_t clock_id, struct timespec* res);
int nanosleep(const struct timespec* req, struct timespec* rem);
#ifdef __cplusplus
}
#endif
#endif
"""

NEWLIB_UNISTD_SUPPLEMENT = """
#ifndef _ALLOYOS_NEWLIB_UNISTD_SUPPLEMENT
#define _ALLOYOS_NEWLIB_UNISTD_SUPPLEMENT
#ifdef __cplusplus
extern "C" {
#endif
int getentropy(void* buf, size_t buflen);
#ifdef __cplusplus
}
#endif
#endif
"""


def run(args, cwd=None):
    result = subprocess.run(
        [str(arg) for arg in args],
        cwd=cwd,
        text=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
    )
    if result.returncode != 0:
        raise RuntimeError(f"command failed: {args}\n{result.stdout}{result.stderr}")
    return result.stdout.strip()


def sha256_file(path: Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


def resolve_rust_lld_and_ar(explicit_linker=None, explicit_ar=None):
    sysroot = Path(run(["rustc", "--print", "sysroot"]))
    host = next(
        line.split(": ", 1)[1]
        for line in run(["rustc", "-vV"]).splitlines()
        if line.startswith("host: ")
    )
    tools_dir = sysroot / "lib/rustlib" / host / "bin"
    linker = explicit_linker or (tools_dir / "rust-lld")
    ar = explicit_ar or (tools_dir / "llvm-ar")
    if not Path(linker).is_file():
        raise ValueError(f"linker not found: {linker}")
    if not Path(ar).is_file():
        raise ValueError(f"archiver not found: {ar}")
    return Path(linker), Path(ar)


def resolve_newlib_dir(newlib_arg: Path) -> Path:
    resolved = newlib_arg.resolve()
    candidate = resolved / "sysroot/aarch64-none-elf"
    if (candidate / "include").is_dir() and (candidate / "lib/libc.a").is_file():
        return candidate
    if (resolved / "include").is_dir() and (resolved / "lib/libc.a").is_file():
        return resolved
    raise ValueError(
        f"newlib directory must contain include/ and lib/libc.a: {resolved}"
    )


def inspect_elf(elf_path: Path) -> dict:
    data = elf_path.read_bytes()
    if data[:6] != b"\x7fELF\x02\x01":
        raise ValueError("expected a little-endian 64-bit ELF")
    kind, machine = struct.unpack_from("<HH", data, 16)
    entry, phoff = struct.unpack_from("<QQ", data, 24)
    phsize, phcount = struct.unpack_from("<HH", data, 54)
    segments = []
    tls = None
    for index in range(phcount):
        segment = struct.unpack_from("<IIQQQQQQ", data, phoff + index * phsize)
        typ, flags, offset, address, _, filesz, memsz, alignment = segment
        if typ in (2, 3):
            raise ValueError("dynamic linkage/interpreter is unsupported")
        if typ == 1:
            if (flags & 3) == 3:
                raise ValueError("writable executable segment")
            segments.append(
                dict(
                    address=address,
                    offset=offset,
                    filesz=filesz,
                    memsz=memsz,
                    flags=flags,
                    alignment=alignment,
                )
            )
        if typ == 7:
            if tls is not None or filesz == 0 or filesz > memsz or alignment > 4096:
                raise ValueError("invalid static TLS template")
            tls = dict(
                address=address,
                offset=offset,
                filesz=filesz,
                memsz=memsz,
                alignment=alignment,
            )
    if (
        kind != 2
        or machine != 183
        or tls is None
        or not any(
            (s["flags"] & 1) and s["address"] <= entry < s["address"] + s["memsz"]
            for s in segments
        )
        or not any(
            (s["flags"] & 1) == 0
            and (s["flags"] & 4) != 0
            and tls["address"] >= s["address"]
            and (tls["address"] - s["address"]) == (tls["offset"] - s["offset"])
            and tls["offset"] + tls["filesz"] <= s["offset"] + s["filesz"]
            for s in segments
        )
    ):
        raise ValueError("invalid AArch64 executable, TLS template coverage, or entry point")
    return dict(size=len(data), entry=entry, segments=segments, tls=tls)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", type=Path, required=True, help="target sysroot directory")
    parser.add_argument(
        "--newlib",
        type=Path,
        help="optional cross-built aarch64-none-elf newlib directory to layer underneath",
    )
    parser.add_argument("--cc", default="clang")
    parser.add_argument("--cxx", default="clang++")
    parser.add_argument("--ar", type=Path)
    parser.add_argument("--linker", type=Path)
    args = parser.parse_args()

    cc = shutil.which(args.cc)
    cxx = shutil.which(args.cxx)
    if cc is None:
        raise ValueError(f"C compiler not found: {args.cc}")
    if cxx is None:
        raise ValueError(f"C++ compiler not found: {args.cxx}")
    linker, ar = resolve_rust_lld_and_ar(args.linker, args.ar)

    repo_root = Path(__file__).resolve().parents[2]
    sysroot_src = repo_root / "scripts/web-engine/sysroot"
    output = args.output.resolve()
    include_dir = output / "include"
    lib_dir = output / "lib"
    obj_dir = output / "build/obj"
    verify_dir = output / "verify"

    if include_dir.exists():
        shutil.rmtree(include_dir)
    if lib_dir.exists():
        shutil.rmtree(lib_dir)
    if obj_dir.exists():
        shutil.rmtree(obj_dir)
    if verify_dir.exists():
        shutil.rmtree(verify_dir)

    include_dir.mkdir(parents=True, exist_ok=True)
    lib_dir.mkdir(parents=True, exist_ok=True)
    obj_dir.mkdir(parents=True, exist_ok=True)
    verify_dir.mkdir(parents=True, exist_ok=True)

    newlib_info = None
    if args.newlib:
        newlib_dir = resolve_newlib_dir(args.newlib)
        shutil.copytree(newlib_dir / "include", include_dir, dirs_exist_ok=True)
        time_h = include_dir / "time.h"
        if time_h.is_file():
            time_h.write_text(time_h.read_text() + "\n" + NEWLIB_TIME_SUPPLEMENT)
        unistd_h = include_dir / "unistd.h"
        if unistd_h.is_file():
            unistd_h.write_text(unistd_h.read_text() + "\n" + NEWLIB_UNISTD_SUPPLEMENT)
        newlib_libs = {}
        for lib_name in ("libc.a", "libm.a"):
            src_lib = newlib_dir / "lib" / lib_name
            dst_lib = lib_dir / lib_name
            shutil.copy2(src_lib, dst_lib)
            newlib_libs[lib_name] = dict(
                path=str(dst_lib),
                size=dst_lib.stat().st_size,
                sha256=sha256_file(dst_lib),
            )
        newlib_info = dict(source_dir=str(newlib_dir), libraries=newlib_libs)
    else:
        shutil.copytree(sysroot_src / "include/base", include_dir, dirs_exist_ok=True)

    shutil.copytree(sysroot_src / "include/overlay", include_dir, dirs_exist_ok=True)
    linker_script = lib_dir / "process.ld"
    shutil.copy2(sysroot_src / "process.ld", linker_script)

    common_flags = [
        "--target=aarch64-unknown-none",
        f"--sysroot={output}",
        "-O2",
        "-ffreestanding",
        "-fno-stack-protector",
        "-fno-unwind-tables",
        "-fno-asynchronous-unwind-tables",
        "-fno-pic",
        "-fno-builtin",
        "-ftls-model=local-exec",
        "-D__alloyos__=1",
        "-Wall",
        "-Wextra",
        "-Werror",
    ]
    c_flags = [
        cc,
        *common_flags,
        "-std=c17",
        "-isystem",
        str(include_dir),
    ]
    cxx_flags = [
        cxx,
        *common_flags,
        "-std=c++23",
        "-fno-exceptions",
        "-fno-rtti",
        "-nostdinc++",
        "-isystem",
        str(include_dir / "c++/v1"),
        "-isystem",
        str(include_dir),
    ]

    posix_sources = ["crt0.c", "syscalls.c", "mman.c", "pthread.c", "time_random.c"]
    posix_objects = []
    for src_name in posix_sources:
        src_path = sysroot_src / "src" / src_name
        obj_path = obj_dir / f"{Path(src_name).stem}.o"
        run([*c_flags, "-c", src_path, "-o", obj_path])
        posix_objects.append(obj_path)

    lib_posix = lib_dir / "liballoyos_posix.a"
    run([ar, "rcsD", lib_posix, *posix_objects])

    cxxrt_obj = obj_dir / "cxxrt.o"
    run([*cxx_flags, "-c", sysroot_src / "src/cxxrt.cpp", "-o", cxxrt_obj])
    lib_cxxrt = lib_dir / "liballoyos_cxxrt.a"
    run([ar, "rcsD", lib_cxxrt, cxxrt_obj])

    # Multi-file C++23 verification compile & link with --no-undefined
    verify_objs = []
    for test_name in ("verify_main.cpp", "verify_worker.cpp"):
        src_path = sysroot_src / "tests" / test_name
        obj_path = verify_dir / f"{Path(test_name).stem}.o"
        run([*cxx_flags, "-c", src_path, "-o", obj_path])
        verify_objs.append(obj_path)

    link_archives = [lib_cxxrt, lib_posix]
    if args.newlib:
        link_archives.extend([lib_dir / "libc.a", lib_dir / "libm.a"])

    verify_elf = verify_dir / "sysroot-verify.elf"
    run(
        [
            linker,
            "-flavor",
            "gnu",
            "-m",
            "aarch64elf",
            "--no-undefined",
            "--fatal-warnings",
            "--build-id=none",
            "-z",
            "max-page-size=4096",
            "-T",
            linker_script,
            *verify_objs,
            "--start-group",
            *link_archives,
            "--end-group",
            "-o",
            verify_elf,
        ]
    )

    elf_info = inspect_elf(verify_elf)
    archives_report = {
        "liballoyos_posix.a": dict(
            path=str(lib_posix),
            size=lib_posix.stat().st_size,
            sha256=sha256_file(lib_posix),
            objects=[obj.name for obj in posix_objects],
        ),
        "liballoyos_cxxrt.a": dict(
            path=str(lib_cxxrt),
            size=lib_cxxrt.stat().st_size,
            sha256=sha256_file(lib_cxxrt),
            objects=[cxxrt_obj.name],
        ),
    }
    if newlib_info:
        archives_report.update(newlib_info["libraries"])

    report = dict(
        target="aarch64-unknown-none",
        sysroot=str(output),
        cc=run([cc, "--version"]).splitlines()[0],
        cxx=run([cxx, "--version"]).splitlines()[0],
        archiver=str(ar),
        linker=run([linker, "-flavor", "gnu", "--version"]),
        linker_script=str(linker_script),
        newlib=newlib_info,
        archives=archives_report,
        verification_binary=dict(
            elf=str(verify_elf),
            sha256=sha256_file(verify_elf),
            **elf_info,
        ),
        guest_execution="not run",
        full_libcxx="not supplied",
        icu="not built",
        javascriptcore="not built",
    )
    report_path = output / "report.json"
    report_path.write_text(json.dumps(report, indent=2) + "\n")
    print(json.dumps(report, indent=2))


if __name__ == "__main__":
    try:
        main()
    except (OSError, ValueError, RuntimeError) as error:
        print(f"sysroot build failed: {error}", file=sys.stderr)
        sys.exit(1)
