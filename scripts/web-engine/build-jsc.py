#!/usr/bin/env python3
"""Prepare and audit the native alloyOS JavaScriptCore (JSCOnly) cross-build foundation.

1. Ensures the alloyOS AArch64 sysroot (liballoyos_posix.a + liballoyos_cxxrt.a)
   and CMake toolchain file (scripts/web-engine/alloyos-toolchain.cmake) exist.
2. Compiles and links a WTF/JSCOnly platform-layer verification ELF
   (jsc-platform-probe.elf) against the alloyOS sysroot with --no-undefined.
3. Audits host tools and target sysroot prerequisites (full libc++ headers,
   cross-compiled target ICU >= 70.1, pinned WPE WebKit 2.54.1 source tree)
   and emits the exact CMake configure/build recipe and readiness report.
4. When --configure-jsc is requested, runs the CMake configure/build only if
   all host tools and target sysroot libraries exist, refusing to borrow host
   Darwin/Linux libraries.
"""
import argparse
import hashlib
import json
from pathlib import Path
import shutil
import struct
import subprocess
import sys


def inspect_elf_file(elf_path: Path) -> dict:
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
    ):
        raise ValueError("invalid AArch64 executable, TLS template, or entry point")
    return dict(size=len(data), entry=entry, segments=segments, tls=tls)


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


def resolve_rust_lld(explicit_linker=None) -> Path:
    if explicit_linker:
        return Path(explicit_linker)
    sysroot = Path(run(["rustc", "--print", "sysroot"]))
    host = next(
        line.split(": ", 1)[1]
        for line in run(["rustc", "-vV"]).splitlines()
        if line.startswith("host: ")
    )
    return sysroot / "lib/rustlib" / host / "bin/rust-lld"


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument(
        "--sysroot",
        type=Path,
        default=Path(__file__).resolve().parents[2] / "build/sysroot",
        help="alloyOS target sysroot directory",
    )
    parser.add_argument(
        "--newlib",
        type=Path,
        help="optional cross-built aarch64-none-elf newlib directory for sysroot layering",
    )
    parser.add_argument(
        "--webkit-source",
        type=Path,
        default=Path(__file__).resolve().parents[2] / "build/wpewebkit",
        help="extracted pinned WPE WebKit 2.54.1 source tree",
    )
    parser.add_argument("--cxx", default="clang++")
    parser.add_argument("--linker", type=Path)
    parser.add_argument(
        "--configure-jsc",
        action="store_true",
        help="invoke CMake configure/build for JSCOnly (requires cmake, ninja, source, and target ICU/libc++)",
    )
    args = parser.parse_args()

    repo_root = Path(__file__).resolve().parents[2]
    toolchain_file = repo_root / "scripts/web-engine/alloyos-toolchain.cmake"
    build_sysroot_script = repo_root / "scripts/web-engine/build-sysroot.py"
    sysroot = args.sysroot.resolve()
    output = args.output.resolve()
    output.mkdir(parents=True, exist_ok=True)

    if not (sysroot / "lib/liballoyos_posix.a").is_file() or not (
        sysroot / "lib/liballoyos_cxxrt.a"
    ).is_file():
        cmd = [sys.executable, str(build_sysroot_script), "--output", str(sysroot)]
        if args.newlib:
            cmd += ["--newlib", str(args.newlib.resolve())]
        run(cmd, cwd=repo_root)

    cxx = shutil.which(args.cxx)
    if cxx is None:
        raise ValueError(f"C++ compiler not found: {args.cxx}")
    linker = resolve_rust_lld(args.linker)

    # Compile and link the WTF/JSCOnly platform prerequisite probe against the sysroot
    probe_dir = output / "probe"
    probe_dir.mkdir(parents=True, exist_ok=True)
    probe_src = repo_root / "scripts/web-engine/sysroot/tests/jsc_platform_probe.cpp"
    probe_obj = probe_dir / "jsc_platform_probe.o"
    probe_elf = probe_dir / "jsc-platform-probe.elf"

    cxx_flags = [
        cxx,
        "--target=aarch64-none-elf",
        f"--sysroot={sysroot}",
        "-std=c++23",
        "-O2",
        "-ffreestanding",
        "-fno-exceptions",
        "-fno-rtti",
        "-fno-stack-protector",
        "-fno-unwind-tables",
        "-fno-asynchronous-unwind-tables",
        "-fno-pic",
        "-fno-builtin",
        "-nostdinc++",
        "-ftls-model=local-exec",
        "-D__alloyos__=1",
        "-Wall",
        "-Wextra",
        "-Werror",
        "-isystem",
        str(sysroot / "include/c++/v1"),
        "-isystem",
        str(sysroot / "include"),
    ]
    run([*cxx_flags, "-c", probe_src, "-o", probe_obj])

    link_archives = [
        sysroot / "lib/liballoyos_cxxrt.a",
        sysroot / "lib/liballoyos_posix.a",
    ]
    for extra in ("libc.a", "libm.a"):
        candidate = sysroot / "lib" / extra
        if candidate.is_file():
            link_archives.append(candidate)

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
            sysroot / "lib/process.ld",
            probe_obj,
            "--start-group",
            *link_archives,
            "--end-group",
            "-o",
            probe_elf,
        ]
    )
    probe_info = inspect_elf_file(probe_elf)

    webkit_source = args.webkit_source.resolve()
    webkit_present = (webkit_source / "Source/cmake/OptionsJSCOnly.cmake").is_file()
    host_tools = {
        tool: shutil.which(tool)
        for tool in ("cmake", "ninja", "pkg-config", "clang", "clang++", "python3", "perl", "ruby")
    }
    target_prereqs = {
        "liballoyos_posix.a": (sysroot / "lib/liballoyos_posix.a").is_file(),
        "liballoyos_cxxrt.a": (sysroot / "lib/liballoyos_cxxrt.a").is_file(),
        "newlib_libc.a": (sysroot / "lib/libc.a").is_file(),
        "newlib_libm.a": (sysroot / "lib/libm.a").is_file(),
        "full_libcxx_thread_header": (sysroot / "include/c++/v1/thread").is_file(),
        "target_icu_uc": (sysroot / "lib/libicuuc.a").is_file(),
        "target_icu_i18n": (sysroot / "lib/libicui18n.a").is_file(),
        "target_icu_data": (sysroot / "lib/libicudata.a").is_file(),
    }

    cmake_configure_command = [
        "cmake",
        "-S",
        str(webkit_source),
        "-B",
        str(output / "jsc-build"),
        "-G",
        "Ninja",
        f"-DCMAKE_TOOLCHAIN_FILE={toolchain_file}",
        f"-DALLOYOS_SYSROOT={sysroot}",
        "-DPORT=JSCOnly",
        "-DCMAKE_BUILD_TYPE=Release",
        "-DENABLE_STATIC_JSC=ON",
        "-DEVENT_LOOP_TYPE=Generic",
        "-DENABLE_JSC_GLIB_API=OFF",
        "-DENABLE_REMOTE_INSPECTOR=OFF",
        "-DENABLE_JIT=OFF",
        "-DENABLE_DFG_JIT=OFF",
        "-DENABLE_FTL_JIT=OFF",
        "-DENABLE_C_LOOP=ON",
        "-DENABLE_WEBASSEMBLY=OFF",
        "-DENABLE_SAMPLING_PROFILER=OFF",
        "-DENABLE_API_TESTS=OFF",
        "-DUSE_SYSTEM_MALLOC=ON",
        "-DUSE_HEADER_MAPS=OFF",
    ]
    cmake_build_command = [
        "cmake",
        "--build",
        str(output / "jsc-build"),
        "--target",
        "jsc",
    ]

    remaining_blockers = []
    if not host_tools["cmake"] or not host_tools["ninja"]:
        remaining_blockers.append("host cmake and ninja are not installed on PATH")
    if not webkit_present:
        remaining_blockers.append(f"extracted WPE WebKit 2.54.1 source tree not present at {webkit_source}")
    if not target_prereqs["full_libcxx_thread_header"]:
        remaining_blockers.append("full target libc++ headers (<thread>, <vector>, <mutex>, etc.) are not yet installed in sysroot")
    if not (
        target_prereqs["target_icu_uc"]
        and target_prereqs["target_icu_i18n"]
        and target_prereqs["target_icu_data"]
    ):
        remaining_blockers.append("cross-compiled target ICU >= 70.1 static archives are not yet installed in sysroot")

    jsc_build_status = "not built"
    if args.configure_jsc:
        if remaining_blockers:
            raise RuntimeError(
                "cannot run CMake JSCOnly build while prerequisites are missing: "
                + "; ".join(remaining_blockers)
            )
        run(cmake_configure_command, cwd=repo_root)
        run(cmake_build_command, cwd=repo_root)
        jsc_elf = output / "jsc-build/bin/jsc"
        if jsc_elf.is_file():
            bin_dir = repo_root / "build/bin"
            bin_dir.mkdir(parents=True, exist_ok=True)
            shutil.copy2(jsc_elf, bin_dir / "jsc")
            strip_tool = resolve_rust_lld().parent / "llvm-strip"
            if strip_tool.is_file():
                run([strip_tool, str(bin_dir / "jsc")])
        jsc_build_status = "built"

    report = dict(
        target="aarch64-none-elf",
        toolchain_file=str(toolchain_file),
        sysroot=str(sysroot),
        webkit_source=str(webkit_source),
        webkit_source_present=webkit_present,
        host_tools=host_tools,
        target_prerequisites=target_prereqs,
        cmake_configure_command=cmake_configure_command,
        cmake_build_command=cmake_build_command,
        wtf_platform_probe=dict(
            elf=str(probe_elf),
            sha256=sha256_file(probe_elf),
            engine_smoke_fixture=str(repo_root / "tests/fixtures/web/engine-smoke.js"),
            **probe_info,
        ),
        remaining_blockers=remaining_blockers,
        javascriptcore=jsc_build_status,
        guest_execution="not run",
    )
    (output / "report.json").write_text(json.dumps(report, indent=2) + "\n")
    print(json.dumps(report, indent=2))


if __name__ == "__main__":
    try:
        main()
    except (OSError, ValueError, RuntimeError) as error:
        print(f"jsc build/audit failed: {error}", file=sys.stderr)
        sys.exit(1)
