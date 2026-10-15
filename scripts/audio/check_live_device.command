#!/bin/sh
# Explicitly authorized real microphone + billable Gemini acceptance; no mocks.
set -eu
repo=$(CDPATH= cd -- "$(dirname -- "$0")/../.." && pwd)
cd "$repo"
printf '%s\n' 'Building current kernel, host runner and native helper first (cached builds may take time).'
cargo build -p alloyos-kernel --features forge --target aarch64-unknown-none --release
cargo build -p alloyos-dev-harness --example voice_qemu_probe
sh scripts/audio/build.sh
printf '%s\n' 'Builds complete. Starting isolated live acceptance, bounded to 44 seconds.'
exec /usr/bin/python3 scripts/audio/check_qemu_media.py --live-device
