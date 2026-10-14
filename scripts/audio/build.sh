#!/bin/sh
set -eu
repo=$(CDPATH= cd -- "$(dirname -- "$0")/../.." && pwd)
mkdir -p "$repo/target"
clang -O2 -fobjc-arc -fblocks -framework Foundation -framework AVFoundation "$repo/scripts/audio/VoiceHelper.m" -Wl,-sectcreate,__TEXT,__info_plist,"$repo/scripts/audio/Info.plist" -o "$repo/target/alloyos-voice-helper"
codesign --force --sign - --identifier dev.alloyos.voice-helper "$repo/target/alloyos-voice-helper" 2>/dev/null
