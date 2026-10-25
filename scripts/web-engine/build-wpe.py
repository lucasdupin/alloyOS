#!/usr/bin/env python3
"""Cross-compiles and packages native WPE WebKit 2.54.1 for alloyOS AArch64.

Orchestrates the Phase 3 build pipeline:
1. Audits all required POSIX, C++, GLib, Soup, and graphics dependencies in build/sysroot.
2. Configures WPE WebKit with CMake targeting aarch64-none-elf using alloyos-toolchain.cmake.
3. Compiles WebCore, Skia, PAL, and all WebKit subtargets (UIProcess, WebProcess, NetworkProcess, Shared, WPEPlatform).
4. Bundles all compiled object files into build/sysroot/lib/libWPEWebKit-2.0.a and installs headers and pkg-config files.
"""

import argparse
import glob
import os
from pathlib import Path
import shutil
import subprocess
import sys

REPO_ROOT = Path(__file__).resolve().parents[2]
SYSROOT = REPO_ROOT / "build/sysroot"
TOOLCHAIN_FILE = REPO_ROOT / "scripts/web-engine/alloyos-toolchain.cmake"
WPE_SRC = REPO_ROOT / "build/wpewebkit"
WPE_BUILD = REPO_ROOT / "build/wpe-test"

REQUIRED_DEPS = [
    "lib/liballoyos_posix.a",
    "lib/liballoyos_cxxrt.a",
    "lib/libc.a",
    "lib/libc++.a",
    "lib/libicudata.a",
    "lib/libicui18n.a",
    "lib/libicuuc.a",
    "lib/libglib-2.0.a",
    "lib/libgio-2.0.a",
    "lib/libgobject-2.0.a",
    "lib/libsoup-3.0.a",
    "lib/libwpe.a",
    "lib/libepoxy.a",
    "lib/libxkbcommon.a",
    "lib/libwebp.a",
    "lib/libz.a",
    "lib/libpng.a",
    "lib/libjpeg.a",
    "lib/libsqlite3.a",
    "lib/libxml2.a",
    "lib/libfreetype.a",
    "lib/libharfbuzz.a",
]

def find_llvm_ar() -> Path:
    candidates = [
        Path(os.path.expanduser("~/.rustup/toolchains/nightly-aarch64-apple-darwin/lib/rustlib/aarch64-apple-darwin/bin/llvm-ar")),
        Path("/opt/homebrew/opt/llvm/bin/llvm-ar"),
        Path("/usr/local/opt/llvm/bin/llvm-ar"),
    ]
    for c in candidates:
        if c.is_file() and os.access(c, os.X_OK):
            return c
    p = shutil.which("llvm-ar")
    if p:
        return Path(p)
    return Path("llvm-ar")


def run(cmd, cwd=None, env=None):
    print(f"[wpe-build] Running: {' '.join(str(c) for c in cmd)}")
    res = subprocess.run(cmd, cwd=cwd, env=env)
    if res.returncode != 0:
        raise RuntimeError(f"Command failed with code {res.returncode}: {cmd}")


def check_prerequisites():
    print("[wpe-build] Verifying sysroot prerequisites...")
    missing = []
    for dep in REQUIRED_DEPS:
        p = SYSROOT / dep
        if not p.is_file():
            missing.append(dep)
    if missing:
        raise RuntimeError(f"Missing required sysroot dependencies: {missing}\nPlease run 'cargo xtask sysroot' and 'cargo xtask webcore-deps'.")
    print(f"[wpe-build] All {len(REQUIRED_DEPS)} prerequisite libraries verified in sysroot.")


def configure_wpe(force=False):
    if not force and (WPE_BUILD / "build.ninja").is_file():
        print("[wpe-build] WPE build already configured.")
        return

    print("[wpe-build] Configuring WPE WebKit with CMake...")
    WPE_BUILD.mkdir(parents=True, exist_ok=True)
    
    cmake_args = [
        "cmake",
        "-S", str(WPE_SRC),
        "-B", str(WPE_BUILD),
        "-G", "Ninja",
        f"-DCMAKE_TOOLCHAIN_FILE={TOOLCHAIN_FILE}",
        f"-DALLOYOS_SYSROOT={SYSROOT}",
        "-DPORT=WPE",
        "-DCMAKE_BUILD_TYPE=Release",
        "-DENABLE_STATIC_JSC=ON",
        "-DEVENT_LOOP_TYPE=Generic",
        "-DENABLE_JIT=OFF",
        "-DENABLE_C_LOOP=ON",
        "-DENABLE_SAMPLING_PROFILER=OFF",
        "-DENABLE_WEBASSEMBLY=OFF",
        "-DUSE_SYSTEM_MALLOC=ON",
        "-DENABLE_WEBGL=OFF",
        "-DENABLE_WEB_AUDIO=OFF",
        "-DENABLE_VIDEO=OFF",
        "-DUSE_GSTREAMER=OFF",
        "-DENABLE_GAMEPAD=OFF",
        "-DENABLE_GEOLOCATION=OFF",
        "-DENABLE_INTROSPECTION=OFF",
        "-DENABLE_DOCUMENTATION=OFF",
        "-DENABLE_WPE_PLATFORM_DRM=OFF",
        "-DENABLE_WPE_PLATFORM_WAYLAND=OFF",
        "-DENABLE_WPE_PLATFORM_HEADLESS=ON",
        "-DUSE_ATK=OFF",
        "-DUSE_GBM=OFF",
        "-DUSE_LIBDRM=OFF",
        "-DUSE_LIBBACKTRACE=OFF",
        "-DUSE_AVIF=OFF",
        "-DUSE_JPEGXL=OFF",
        "-DUSE_LIBHYPHEN=OFF",
        "-DUSE_WOFF2=OFF",
        "-DENABLE_XSLT=OFF",
        "-DENABLE_WEBDRIVER=OFF",
        "-DENABLE_SPELLCHECK=OFF",
        "-DUSE_LCMS=OFF",
        "-DENABLE_WEBXR=OFF",
        "-DENABLE_JOURNALD_LOG=OFF",
        "-DUSE_SYSTEM_SYSPROF_CAPTURE=NO",
    ]
    run(cmake_args)


def build_wpe():
    targets = [
        "WebCore",
        "PAL",
        "Skia",
        "lib/libSysProfCapture.a",
        "lib/libxdgmime.a",
        "WebKitUIProcess",
        "WebKitWebProcess",
        "WebKitNetworkProcess",
        "WebKitShared",
        "lib/libWPEWebKit-2.0.a",
    ]
    print(f"[wpe-build] Building WPE WebKit targets: {', '.join(targets)}...")
    run(["ninja", "-C", str(WPE_BUILD)] + targets)


def bundle_and_install():
    print("[wpe-build] Bundling complete libWebCore.a and libWPEWebKit-2.0.a archives...")
    ar = find_llvm_ar()

    # Bundle WebCore
    webcore_objs = glob.glob(str(WPE_BUILD / "Source/WebCore/CMakeFiles/WebCore.dir/**/*.obj"), recursive=True)
    print(f"[wpe-build] Aggregating {len(webcore_objs)} compiled WebCore object files...")
    target_webcore = SYSROOT / "lib/libWebCore.a"
    run([str(ar), "rcs", str(target_webcore)] + webcore_objs)
    print(f"[wpe-build] Installed {target_webcore.name} ({target_webcore.stat().st_size:,} bytes)")
    
    # Bundle WebKit
    obj_patterns = [
        "Source/WebKit/CMakeFiles/WebKitUIProcess.dir/**/*.obj",
        "Source/WebKit/CMakeFiles/WebKitWebProcess.dir/**/*.obj",
        "Source/WebKit/CMakeFiles/WebKitNetworkProcess.dir/**/*.obj",
        "Source/WebKit/CMakeFiles/WebKitShared.dir/**/*.obj",
        "Source/WebKit/CMakeFiles/WebKit.dir/**/*.obj",
        "Source/WebKit/WPEPlatform/CMakeFiles/WPEPlatform.dir/**/*.obj",
        "Source/WebKit/WPEPlatform/wpe/headless/CMakeFiles/WPEPlatformHeadless.dir/**/*.obj",
    ]
    
    objs = []
    for pattern in obj_patterns:
        found = glob.glob(str(WPE_BUILD / pattern), recursive=True)
        objs.extend(found)
        
    print(f"[wpe-build] Aggregating {len(objs)} compiled WebKit/WPEPlatform object files...")
    target_archive = SYSROOT / "lib/libWPEWebKit-2.0.a"
    run([str(ar), "rcs", str(target_archive)] + objs)
    print(f"[wpe-build] Installed {target_archive.name} ({target_archive.stat().st_size:,} bytes)")
    
    # Also install other libraries
    libs_to_copy = [
        ("lib/libSkia.a", "lib/libSkia.a"),
        ("lib/libPAL.a", "lib/libPAL.a"),
        ("lib/libSysProfCapture.a", "lib/libSysProfCapture.a"),
        ("lib/libxdgmime.a", "lib/libxdgmime.a"),
    ]
    for src_rel, dst_rel in libs_to_copy:
        src = WPE_BUILD / src_rel
        dst = SYSROOT / dst_rel
        if src.is_file():
            shutil.copy2(src, dst)
            print(f"[wpe-build] Installed {dst_rel} ({dst.stat().st_size:,} bytes)")

    # Install WebKit C API headers
    wk_inc = SYSROOT / "include/WebKit"
    wk_inc.mkdir(parents=True, exist_ok=True)
    for h in glob.glob(str(WPE_BUILD / "WebKit/Headers/WebKit/*.h")):
        shutil.copy2(h, wk_inc)
    for h in glob.glob(str(WPE_SRC / "Source/WebKit/UIProcess/API/C/*.h")):
        shutil.copy2(h, wk_inc)
    for h in glob.glob(str(WPE_SRC / "Source/WebKit/UIProcess/API/C/wpe/*.h")):
        shutil.copy2(h, wk_inc)
    for h in glob.glob(str(WPE_SRC / "Source/WebKit/Shared/API/c/*.h")):
        shutil.copy2(h, wk_inc)

    # Install WPEPlatform headers
    wpe_platform_inc = SYSROOT / "include/wpe-platform-2.0/wpe"
    wpe_platform_inc.mkdir(parents=True, exist_ok=True)
    for h in glob.glob(str(WPE_SRC / "Source/WebKit/WPEPlatform/wpe/*.h")):
        shutil.copy2(h, wpe_platform_inc)
    (wpe_platform_inc / "headless").mkdir(parents=True, exist_ok=True)
    for h in glob.glob(str(WPE_SRC / "Source/WebKit/WPEPlatform/wpe/headless/*.h")):
        shutil.copy2(h, wpe_platform_inc / "headless")
    wpe_enum = WPE_BUILD / "DerivedSources/WPEPlatform/wpe/WPEEnumTypes.h"
    if wpe_enum.is_file():
        shutil.copy2(wpe_enum, wpe_platform_inc)
        
    wpe_platform_symlink = SYSROOT / "include/wpe-platform"
    if not wpe_platform_symlink.exists():
        wpe_platform_symlink.symlink_to("wpe-platform-2.0/wpe")

    # Install pkgconfig files
    pc_dir = SYSROOT / "lib/pkgconfig"
    pc_dir.mkdir(parents=True, exist_ok=True)
    for pc in glob.glob(str(WPE_BUILD / "Source/WebKit/*.pc")):
        shutil.copy2(pc, pc_dir)
    for pc in glob.glob(str(WPE_BUILD / "Source/WebKit/WPEPlatform/*.pc")):
        shutil.copy2(pc, pc_dir)

    print(f"[wpe-build] Successfully bundled and installed WPE WebKit in {SYSROOT}!")
    print(f"[wpe-build] libWPEWebKit-2.0.a size: {target_archive.stat().st_size:,} bytes")


def main():
    parser = argparse.ArgumentParser(description="Build WPE WebKit for alloyOS")
    parser.add_argument("--force", action="store_true", help="Force re-configuration and re-build")
    parser.add_argument("--check-only", action="store_true", help="Verify prerequisites and build status without compiling")
    args = parser.parse_args()

    check_prerequisites()
    if args.check_only:
        wpe_lib = SYSROOT / "lib/libWPEWebKit-2.0.a"
        if wpe_lib.is_file():
            print(f"[wpe-build] WPE WebKit library is installed ({wpe_lib.stat().st_size:,} bytes).")
            return
        else:
            print("[wpe-build] WPE WebKit library is NOT installed.")
            sys.exit(1)

    configure_wpe(force=args.force)
    build_wpe()
    bundle_and_install()


if __name__ == "__main__":
    main()
