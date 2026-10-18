#!/usr/bin/env python3
"""Fetch/check pinned WPE sources and print source-derived port requirements."""
import argparse
import hashlib
import json
from pathlib import Path
import re
import shutil
import sys
import urllib.request

VERSION = "2.54.1"
SHA256 = "0de8bdfba011f9cd63ba9a9171fa6038dd2c08237894f4082abbb821f29d3fe9"
URL = f"https://wpewebkit.org/releases/wpewebkit-{VERSION}.tar.xz"


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--source", type=Path, help="extracted pinned source tree")
    parser.add_argument("--fetch", type=Path, help="download archive here; does not extract or execute it")
    args = parser.parse_args()
    if args.fetch:
        args.fetch.parent.mkdir(parents=True, exist_ok=True)
        temporary = args.fetch.with_suffix(args.fetch.suffix + ".part")
        try:
            with urllib.request.urlopen(URL, timeout=60) as response, temporary.open("wb") as output:
                shutil.copyfileobj(response, output)
            with temporary.open("rb") as archive:
                if hashlib.file_digest(archive, "sha256").hexdigest() != SHA256:
                    raise ValueError("upstream archive checksum mismatch")
            temporary.replace(args.fetch)
        finally:
            temporary.unlink(missing_ok=True)
    report = {"version": VERSION, "url": URL, "sha256": SHA256,
              "host_tools": {tool: shutil.which(tool) for tool in
                             ("cmake", "ninja", "clang", "clang++", "pkg-config", "python3", "perl", "ruby")}}
    if args.source:
        options = (args.source / "Source/cmake/OptionsWPE.cmake").read_text()
        if "SET_PROJECT_VERSION(2 54 1)" not in options:
            raise ValueError("source is not the pinned WPE version")
        # Includes conditional dependencies: expose the exact expression, rather than
        # claiming optional libraries are mandatory or reporting host libraries as target ones.
        report["wpe_find_package_expressions"] = re.findall(r"find_package\(([^)]+)\)", options)
        report["native_port_files"] = [
            "Source/WTF/wtf/posix/OSAllocatorPOSIX.cpp",
            "Source/WTF/wtf/posix/ThreadingPOSIX.cpp",
            "Source/WTF/wtf/generic/RunLoopGeneric.cpp",
            "Source/WebCore/platform/unix/SharedMemoryUnix.cpp",
            "Source/WebKit/Platform/IPC/unix/ConnectionUnix.cpp",
            "Source/WebKit/UIProcess/Launcher/glib/ProcessLauncherGLib.cpp",
            "Source/WebKit/WPEPlatform/wpe/WPEBufferSHM.cpp",
            "Source/WebKit/WPEPlatform/wpe/headless/WPEViewHeadless.cpp",
        ]
        for name in report["native_port_files"]:
            if not (args.source / name).is_file():
                raise ValueError(f"missing audited source: {name}")
    print(json.dumps(report, indent=2))


if __name__ == "__main__":
    try:
        main()
    except (OSError, ValueError) as error:
        print(f"source audit failed: {error}", file=sys.stderr)
        sys.exit(1)
