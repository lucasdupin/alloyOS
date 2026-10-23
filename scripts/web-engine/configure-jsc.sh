#!/bin/sh
# Configure the JSCOnly build for alloyOS into build/jsc (tracked, reproducible).
set -e
ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
cmake -S "$ROOT/build/wpewebkit" -B "$ROOT/build/jsc" -G Ninja \
  -DCMAKE_TOOLCHAIN_FILE="$ROOT/scripts/web-engine/alloyos-toolchain.cmake" \
  -DALLOYOS_SYSROOT="$ROOT/build/sysroot" \
  -DPORT=JSCOnly -DCMAKE_BUILD_TYPE=Release \
  -DENABLE_STATIC_JSC=ON -DEVENT_LOOP_TYPE=Generic \
  -DENABLE_JSC_GLIB_API=OFF -DENABLE_REMOTE_INSPECTOR=OFF \
  -DENABLE_JIT=OFF -DENABLE_DFG_JIT=OFF -DENABLE_FTL_JIT=OFF \
  -DENABLE_C_LOOP=ON -DENABLE_WEBASSEMBLY=OFF \
  -DENABLE_SAMPLING_PROFILER=OFF -DENABLE_API_TESTS=OFF \
  -DUSE_SYSTEM_MALLOC=ON -DUSE_HEADER_MAPS=OFF
