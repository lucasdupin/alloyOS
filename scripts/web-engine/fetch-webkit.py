#!/usr/bin/env python3
"""Fetch WPE WebKit and apply the tracked alloyOS port patches.

Output: <build>/wpewebkit (pristine release tarball + patches/wpewebkit-<ver>/*.patch).
Never edit files under build/ by hand: change the patches in scripts/web-engine/patches/.
"""
import argparse, shutil, subprocess, sys, tarfile, urllib.request
from pathlib import Path

VERSION = "2.54.1"
URL = f"https://wpewebkit.org/releases/wpewebkit-{VERSION}.tar.xz"
ROOT = Path(__file__).resolve().parents[2]


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--build-dir", type=Path, default=ROOT / "build")
    ap.add_argument("--tarball", type=Path, help="use a local tarball instead of downloading")
    ap.add_argument("--force", action="store_true", help="discard and re-extract the source tree")
    args = ap.parse_args()

    build = args.build_dir.resolve()
    dest = build / "wpewebkit"
    stamp = dest / ".alloyos-patched"
    if stamp.exists() and not args.force:
        print(f"{dest} already patched (use --force to redo)")
        return

    tarball = args.tarball or build / "downloads" / f"wpewebkit-{VERSION}.tar.xz"
    if not tarball.is_file():
        tarball.parent.mkdir(parents=True, exist_ok=True)
        print(f"downloading {URL}")
        urllib.request.urlretrieve(URL, tarball)

    if dest.exists() or dest.is_symlink():
        shutil.rmtree(dest) if dest.is_dir() and not dest.is_symlink() else dest.unlink()
    staging = build / "scratch" / "extract"
    shutil.rmtree(staging, ignore_errors=True)
    staging.mkdir(parents=True)
    with tarfile.open(tarball) as tf:
        tf.extractall(staging)
    (staging / f"wpewebkit-{VERSION}").rename(dest)

    for patch in sorted((ROOT / "scripts/web-engine/patches" / f"wpewebkit-{VERSION}").glob("*.patch")):
        print(f"applying {patch.name}")
        subprocess.run(["patch", "-p1", "-d", str(dest), "-i", str(patch)], check=True)
    stamp.write_text("ok\n")


if __name__ == "__main__":
    sys.exit(main())
