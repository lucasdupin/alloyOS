#!/usr/bin/env python3
"""Cross-compiles and installs all WebCore C/C++ rendering and parsing dependencies for alloyOS AArch64.

Dependencies:
- zlib 1.3.1 (compression)
- libpng 1.6.43 (raster images)
- libjpeg-turbo 3.0.1 (JPEG decoding with ARM64 Neon SIMD)
- sqlite3 3.45.0 (local storage, database backend)
- libxml2 2.12.5 (XML / SVG / HTML parsing)
- freetype 2.13.2 (font glyph rasterization)
- harfbuzz 8.3.0 (complex text shaping and layout)
"""
import argparse
import os
from pathlib import Path
import shutil
import subprocess
import sys

REPO_ROOT = Path(__file__).resolve().parents[2]
SYSROOT = REPO_ROOT / "build/sysroot"
TOOLCHAIN_FILE = REPO_ROOT / "scripts/web-engine/alloyos-toolchain.cmake"
BUILD_DIR = REPO_ROOT / "build/deps"


def run(cmd, cwd=None, env=None):
    print(f"[webcore-deps] Running: {' '.join(str(c) for c in cmd)}")
    res = subprocess.run(cmd, cwd=cwd, env=env)
    if res.returncode != 0:
        raise RuntimeError(f"Command failed with code {res.returncode}: {cmd}")


def build_zlib(src_dir: Path, build_dir: Path):
    zlib_a = SYSROOT / "lib/libz.a"
    if zlib_a.is_file():
        print("[webcore-deps] zlib already built, skipping.")
        return
    print("[webcore-deps] Building zlib 1.3.1...")
    src_dir.mkdir(parents=True, exist_ok=True)
    tarball = src_dir / "zlib-1.3.1.tar.gz"
    if not (src_dir / "CMakeLists.txt").is_file():
        run(["curl", "-sL", "https://github.com/madler/zlib/archive/refs/tags/v1.3.1.tar.gz", "-o", str(tarball)])
        run(["tar", "-xz", "-f", str(tarball), "--strip-components=1"], cwd=src_dir)
    
    b_dir = build_dir / "zlib"
    run([
        "cmake", "-B", str(b_dir), "-S", str(src_dir), "-G", "Ninja",
        f"-DCMAKE_TOOLCHAIN_FILE={TOOLCHAIN_FILE}",
        f"-DALLOYOS_SYSROOT={SYSROOT}",
        f"-DCMAKE_INSTALL_PREFIX={SYSROOT}",
        "-DBUILD_SHARED_LIBS=OFF",
    ])
    run(["cmake", "--build", str(b_dir)])
    run(["cmake", "--install", str(b_dir)])
    shutil.copy2(SYSROOT / "lib/libzlibstatic.a", zlib_a)


def build_libpng(src_dir: Path, build_dir: Path):
    png_a = SYSROOT / "lib/libpng.a"
    if png_a.is_file() or (SYSROOT / "lib/liblibpng16_static.a").is_file():
        print("[webcore-deps] libpng already built, skipping.")
        return
    print("[webcore-deps] Building libpng 1.6.43...")
    src_dir.mkdir(parents=True, exist_ok=True)
    tarball = src_dir / "libpng-1.6.43.tar.gz"
    if not (src_dir / "CMakeLists.txt").is_file():
        run(["curl", "-sL", "https://github.com/pnggroup/libpng/archive/refs/tags/v1.6.43.tar.gz", "-o", str(tarball)])
        run(["tar", "-xz", "-f", str(tarball), "--strip-components=1"], cwd=src_dir)
    
    b_dir = build_dir / "libpng"
    run([
        "cmake", "-B", str(b_dir), "-S", str(src_dir), "-G", "Ninja",
        f"-DCMAKE_TOOLCHAIN_FILE={TOOLCHAIN_FILE}",
        f"-DALLOYOS_SYSROOT={SYSROOT}",
        f"-DCMAKE_INSTALL_PREFIX={SYSROOT}",
        f"-DZLIB_ROOT={SYSROOT}",
        f"-DZLIB_LIBRARY={SYSROOT}/lib/libz.a",
        f"-DZLIB_INCLUDE_DIR={SYSROOT}/include",
        "-DPNG_SHARED=OFF", "-DPNG_STATIC=ON", "-DPNG_TESTS=OFF", "-DPNG_TOOLS=OFF",
    ])
    run(["cmake", "--build", str(b_dir)])
    run(["cmake", "--install", str(b_dir)])


def build_libjpeg(src_dir: Path, build_dir: Path):
    jpeg_a = SYSROOT / "lib/libjpeg.a"
    if jpeg_a.is_file():
        print("[webcore-deps] libjpeg-turbo already built, skipping.")
        return
    print("[webcore-deps] Building libjpeg-turbo 3.0.1...")
    src_dir.mkdir(parents=True, exist_ok=True)
    tarball = src_dir / "libjpeg-turbo-3.0.1.tar.gz"
    if not (src_dir / "CMakeLists.txt").is_file():
        run(["curl", "-sL", "https://github.com/libjpeg-turbo/libjpeg-turbo/archive/refs/tags/3.0.1.tar.gz", "-o", str(tarball)])
        run(["tar", "-xz", "-f", str(tarball), "--strip-components=1"], cwd=src_dir)
    
    b_dir = build_dir / "libjpeg"
    run([
        "cmake", "-B", str(b_dir), "-S", str(src_dir), "-G", "Ninja",
        "-DCMAKE_POLICY_VERSION_MINIMUM=3.5",
        f"-DCMAKE_TOOLCHAIN_FILE={TOOLCHAIN_FILE}",
        f"-DALLOYOS_SYSROOT={SYSROOT}",
        f"-DCMAKE_INSTALL_PREFIX={SYSROOT}",
        "-DENABLE_SHARED=OFF", "-DENABLE_STATIC=ON", "-DWITH_TURBOJPEG=OFF", "-DENABLE_EXECUTABLES=OFF",
    ])
    run(["cmake", "--build", str(b_dir)])
    run(["cmake", "--install", str(b_dir)])


def build_sqlite3(src_dir: Path, build_dir: Path):
    sqlite_a = SYSROOT / "lib/libsqlite3.a"
    if sqlite_a.is_file():
        print("[webcore-deps] sqlite3 already built, skipping.")
        return
    print("[webcore-deps] Building sqlite3 3.45.0...")
    src_dir.mkdir(parents=True, exist_ok=True)
    zip_path = src_dir / "sqlite.zip"
    if not (src_dir / "sqlite3.c").is_file():
        run(["curl", "-sL", "https://www.sqlite.org/2024/sqlite-amalgamation-3450000.zip", "-o", str(zip_path)])
        run(["unzip", "-q", "-o", str(zip_path), "-d", str(src_dir)])
        amalgam_dir = src_dir / "sqlite-amalgamation-3450000"
        for item in amalgam_dir.iterdir():
            shutil.copy2(item, src_dir / item.name)

    b_dir = build_dir / "sqlite3"
    b_dir.mkdir(parents=True, exist_ok=True)
    obj = b_dir / "sqlite3.o"
    run([
        "clang", "--target=aarch64-none-elf", f"--sysroot={SYSROOT}",
        "-O2", "-ffreestanding", "-fno-stack-protector", "-fno-pic", "-fno-builtin",
        "-D__alloyos__=1", "-D_POSIX_C_SOURCE=200809L",
        "-DSQLITE_THREADSAFE=1", "-DSQLITE_OMIT_LOAD_EXTENSION=1", "-DSQLITE_ENABLE_MATH_FUNCTIONS=1",
        "-isystem", str(SYSROOT / "include"),
        "-c", str(src_dir / "sqlite3.c"), "-o", str(obj),
    ])
    ar = REPO_ROOT / "scripts/web-engine/bin/ar"
    if not ar.is_file():
        rust_sysroot = Path(subprocess.check_output(["rustc", "--print", "sysroot"], text=True).strip())
        host = next(line.split(": ", 1)[1] for line in subprocess.check_output(["rustc", "-vV"], text=True).splitlines() if line.startswith("host: "))
        ar = rust_sysroot / "lib/rustlib" / host / "bin/llvm-ar"
    run([str(ar), "-rcs", str(sqlite_a), str(obj)])
    shutil.copy2(src_dir / "sqlite3.h", SYSROOT / "include/sqlite3.h")
    shutil.copy2(src_dir / "sqlite3ext.h", SYSROOT / "include/sqlite3ext.h")
    pkgconfig_dir = SYSROOT / "lib/pkgconfig"
    pkgconfig_dir.mkdir(parents=True, exist_ok=True)
    (pkgconfig_dir / "sqlite3.pc").write_text(f"""prefix={SYSROOT}
exec_prefix=${{prefix}}
libdir=${{exec_prefix}}/lib
includedir=${{prefix}}/include

Name: SQLite
Description: SQL database engine
Version: 3.45.0
Libs: -L${{libdir}} -lsqlite3
Cflags: -I${{includedir}}
""")


def build_libxml2(src_dir: Path, build_dir: Path):
    xml_a = SYSROOT / "lib/libxml2.a"
    if xml_a.is_file():
        print("[webcore-deps] libxml2 already built, skipping.")
        return
    print("[webcore-deps] Building libxml2 2.12.5...")
    src_dir.mkdir(parents=True, exist_ok=True)
    tarball = src_dir / "libxml2-2.12.5.tar.xz"
    if not (src_dir / "CMakeLists.txt").is_file():
        run(["curl", "-sL", "https://download.gnome.org/sources/libxml2/2.12/libxml2-2.12.5.tar.xz", "-o", str(tarball)])
        run(["tar", "-xJ", "-f", str(tarball), "--strip-components=1"], cwd=src_dir)
    
    b_dir = build_dir / "libxml2"
    run([
        "cmake", "-B", str(b_dir), "-S", str(src_dir), "-G", "Ninja",
        f"-DCMAKE_TOOLCHAIN_FILE={TOOLCHAIN_FILE}",
        f"-DALLOYOS_SYSROOT={SYSROOT}",
        f"-DCMAKE_INSTALL_PREFIX={SYSROOT}",
        "-DBUILD_SHARED_LIBS=OFF",
        "-DLIBXML2_WITH_PYTHON=OFF",
        "-DLIBXML2_WITH_TESTS=OFF",
        "-DLIBXML2_WITH_PROGRAMS=OFF",
        "-DLIBXML2_WITH_ICONV=OFF",
        "-DLIBXML2_WITH_LZMA=OFF",
        "-DLIBXML2_WITH_HTTP=OFF",
        "-DLIBXML2_WITH_FTP=OFF",
        "-DLIBXML2_WITH_ZLIB=ON",
        f"-DZLIB_ROOT={SYSROOT}",
        f"-DZLIB_LIBRARY={SYSROOT}/lib/libz.a",
        f"-DZLIB_INCLUDE_DIR={SYSROOT}/include",
    ])
    run(["cmake", "--build", str(b_dir)])
    run(["cmake", "--install", str(b_dir)])


def build_freetype(src_dir: Path, build_dir: Path):
    ft_a = SYSROOT / "lib/libfreetype.a"
    if ft_a.is_file():
        print("[webcore-deps] freetype already built, skipping.")
        return
    print("[webcore-deps] Building freetype 2.13.2...")
    src_dir.mkdir(parents=True, exist_ok=True)
    tarball = src_dir / "freetype-2.13.2.tar.xz"
    if not (src_dir / "CMakeLists.txt").is_file():
        run(["curl", "-sL", "https://download.savannah.gnu.org/releases/freetype/freetype-2.13.2.tar.xz", "-o", str(tarball)])
        run(["tar", "-xJ", "-f", str(tarball), "--strip-components=1"], cwd=src_dir)
    
    b_dir = build_dir / "freetype"
    run([
        "cmake", "-B", str(b_dir), "-S", str(src_dir), "-G", "Ninja",
        "-DCMAKE_POLICY_VERSION_MINIMUM=3.5",
        f"-DCMAKE_TOOLCHAIN_FILE={TOOLCHAIN_FILE}",
        f"-DALLOYOS_SYSROOT={SYSROOT}",
        f"-DCMAKE_INSTALL_PREFIX={SYSROOT}",
        "-DBUILD_SHARED_LIBS=OFF",
        "-DFT_DISABLE_ZLIB=TRUE",
        "-DFT_DISABLE_BZIP2=TRUE",
        "-DFT_DISABLE_PNG=TRUE",
        "-DFT_DISABLE_HARFBUZZ=TRUE",
        "-DFT_DISABLE_BROTLI=TRUE",
    ])
    run(["cmake", "--build", str(b_dir)])
    run(["cmake", "--install", str(b_dir)])


def build_harfbuzz(src_dir: Path, build_dir: Path):
    hb_a = SYSROOT / "lib/libharfbuzz.a"
    if hb_a.is_file():
        print("[webcore-deps] harfbuzz already built, skipping.")
        return
    print("[webcore-deps] Building harfbuzz 8.3.0...")
    src_dir.mkdir(parents=True, exist_ok=True)
    tarball = src_dir / "harfbuzz-8.3.0.tar.gz"
    if not (src_dir / "CMakeLists.txt").is_file():
        run(["curl", "-sL", "https://github.com/harfbuzz/harfbuzz/archive/refs/tags/8.3.0.tar.gz", "-o", str(tarball)])
        run(["tar", "-xz", "-f", str(tarball), "--strip-components=1"], cwd=src_dir)
    
    b_dir = build_dir / "harfbuzz"
    run([
        "cmake", "-B", str(b_dir), "-S", str(src_dir), "-G", "Ninja",
        f"-DCMAKE_TOOLCHAIN_FILE={TOOLCHAIN_FILE}",
        f"-DALLOYOS_SYSROOT={SYSROOT}",
        f"-DCMAKE_INSTALL_PREFIX={SYSROOT}",
        "-DCMAKE_CXX_STANDARD=20",
        "-DBUILD_SHARED_LIBS=OFF",
        "-DHB_HAVE_FREETYPE=ON",
        f"-DFREETYPE_INCLUDE_DIRS={SYSROOT}/include/freetype2",
        f"-DFREETYPE_LIBRARY={SYSROOT}/lib/libfreetype.a",
        "-DHB_HAVE_ICU=ON",
        f"-DICU_LIBRARY={SYSROOT}/lib/libicuuc.a",
        f"-DICU_INCLUDE_DIR={SYSROOT}/include",
        "-DHB_HAVE_GLIB=OFF",
        "-DHB_HAVE_GOBJECT=OFF",
        "-DHB_HAVE_INTROSPECTION=OFF",
        "-DHB_BUILD_UTILS=OFF",
        "-DHB_BUILD_TESTS=OFF",
    ])
    run(["cmake", "--build", str(b_dir)])
    run(["cmake", "--install", str(b_dir)])


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--force", action="store_true", help="force rebuild all dependencies")
    args = parser.parse_args()

    src_root = REPO_ROOT / "build/deps-src"
    b_root = REPO_ROOT / "build/deps-build"

    build_zlib(src_root / "zlib", b_root)
    build_libpng(src_root / "libpng", b_root)
    build_libjpeg(src_root / "libjpeg", b_root)
    build_sqlite3(src_root / "sqlite3", b_root)
    build_libxml2(src_root / "libxml2", b_root)
    build_freetype(src_root / "freetype", b_root)
    build_harfbuzz(src_root / "harfbuzz", b_root)

    print("\n[webcore-deps] ALL WebCore dependencies cross-compiled and installed successfully into sysroot!")


if __name__ == "__main__":
    main()
