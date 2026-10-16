#!/usr/bin/env python3
"""Opt-in ROADMAP M9 build evidence gate; see test-build-independence.md."""
import argparse
import hashlib
import json
from pathlib import Path
import subprocess
import sys

ROOT = Path(__file__).resolve().parents[1]


def fingerprint(path):
    path = Path(path).resolve(strict=True)
    return {"path": str(path), "sha256": hashlib.sha256(path.read_bytes()).hexdigest(),
            "mtime_ns": path.stat().st_mtime_ns}


def architecture_errors(metadata):
    packages = {p["name"]: p for p in metadata["packages"]}
    paths = {str(Path(p["manifest_path"]).parent.resolve()): p["name"]
             for p in packages.values()}
    edges = {}
    for name, package in packages.items():
        edges[name] = [paths[str(Path(dep["path"]).resolve())]
                       for dep in package["dependencies"]
                       if dep.get("kind") != "dev" and dep.get("path") in paths]

    def reaches(start, forbidden, seen=None):
        seen = set() if seen is None else seen
        if start in forbidden:
            return True
        if start in seen:
            return False
        seen.add(start)
        return any(reaches(child, forbidden, seen) for child in edges.get(start, []))

    errors = []
    for start, forbidden in [("xtask", {"alloyos-kernel"}),
                             ("alloyos-kernel", {"alloyos-welder", "alloyos-ferrofluid"})]:
        if start not in packages:
            errors.append(f"missing package {start}")
        elif reaches(start, forbidden):
            errors.append(f"{start} still depends on {' / '.join(sorted(forbidden))}")
    welder = packages.get("alloyos-welder", {})
    if not any("bin" in t["kind"] for t in welder.get("targets", [])):
        errors.append("Welder has no separately packaged executable target")
    return errors


def verify(baseline, log, component):
    errors = []
    for before in baseline:
        if fingerprint(before["path"]) != before:
            errors.append(f"kernel artifact changed: {before['path']}")
    built = False
    successful = False
    for line in log.splitlines():
        if not line.strip():
            continue
        try:
            message = json.loads(line)
        except json.JSONDecodeError:
            errors.append("Cargo evidence must be JSON lines, with stderr captured separately")
            continue
        if not isinstance(message, dict):
            errors.append("Cargo evidence messages must be JSON objects")
            continue
        reason = message.get("reason")
        if reason == "build-finished":
            successful = message.get("success") is True
            if not successful:
                errors.append("Cargo build failed")
        if reason == "compiler-artifact" and message.get("fresh") is False:
            # Current Cargo IDs are path+file:///...#package@version; older IDs
            # start with the package name. Inspect the target as well.
            package = message.get("package_id", "")
            target = message.get("target", {})
            if "alloyos-kernel" in package:
                errors.append("Cargo compiled a kernel target (host or guest)")
            if component in package and "bin" in target.get("kind", []):
                built = True
    if not successful:
        errors.append("missing successful Cargo build-finished evidence")
    if not built:
        errors.append(f"missing newly built {component} executable evidence")
    return errors


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest="command", required=True)
    commands.add_parser("architecture")
    record = commands.add_parser("record")
    record.add_argument("--kernel", type=Path, action="append", required=True)
    record.add_argument("--baseline", type=Path, required=True)
    check = commands.add_parser("verify")
    check.add_argument("--baseline", type=Path, required=True)
    check.add_argument("--cargo-log", type=Path, required=True)
    check.add_argument("--component", choices=["alloyos-welder", "alloyos-ferrofluid"], required=True)
    args = parser.parse_args()
    if args.command == "architecture":
        result = subprocess.run(["cargo", "metadata", "--offline", "--locked", "--no-deps", "--format-version", "1"],
                                cwd=ROOT, check=True, capture_output=True, text=True)
        errors = architecture_errors(json.loads(result.stdout))
    elif args.command == "record":
        with args.baseline.open("x") as out:
            json.dump([fingerprint(p) for p in args.kernel], out, indent=2)
        errors = []
    else:
        baseline = json.loads(args.baseline.read_text())
        if not isinstance(baseline, list) or not baseline:
            raise ValueError("baseline must contain at least one kernel artifact")
        errors = verify(baseline, args.cargo_log.read_text(), args.component)
    for error in errors:
        print(f"FAIL: {error}", file=sys.stderr)
    if not errors:
        print("PASS: build evidence gate (runtime acceptance remains separate)")
    return bool(errors)


if __name__ == "__main__":
    try:
        sys.exit(main())
    except (OSError, ValueError, subprocess.CalledProcessError) as error:
        print(f"FAIL: {error}", file=sys.stderr)
        sys.exit(1)
