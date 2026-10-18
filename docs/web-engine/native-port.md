# Native WPE WebKit port

Status: source feasibility audit, 2026-10-18. No native engine binary has been built.
The target is WPE WebKit **2.54.1**, with its matching JavaScriptCore. Apps
remain headless; Ferrofluid owns navigation, presentation, and input. An engine
process gets no app/MCP capabilities merely by rendering a page.

## Reproduce the source audit

```sh
python3 scripts/web-engine/source-audit.py --fetch /tmp/wpewebkit-2.54.1.tar.xz
tar -xf /tmp/wpewebkit-2.54.1.tar.xz -C /tmp
python3 scripts/web-engine/source-audit.py --source /tmp/wpewebkit-2.54.1
```

The fetch checks SHA-256 before publishing the archive. The audit is read-only,
uses the Python standard library, and reports CMake dependency expressions and
platform implementation files. It does not claim the host's tools or libraries
are a guest sysroot. Extract only the checksum-verified archive.

Pin: [upstream release and checksum](https://wpewebkit.org/release/wpewebkit-2.54.1.html).
The archive was downloaded, checksum verified, extracted, and audited during this
investigation. Its SHA-256 is
`0de8bdfba011f9cd63ba9a9171fa6038dd2c08237894f4082abbb821f29d3fe9`.

## First engine milestone: JavaScriptCore without WebCore

Use the upstream JSCOnly port first, its C API, Generic event loop, static library,
and C-loop interpreter. This runs the real engine while deferring GLib, browser
process IPC, networking, graphics, executable-memory allocation, and JIT policy.
It still needs ICU, a C/C++ runtime, thread services, and virtual memory. A Rust
`std` port alone cannot satisfy these C++ requirements.

Once an actual alloyOS C/C++ sysroot and CMake toolchain file exist, the initial
configuration to test is:

```sh
cmake -S /tmp/wpewebkit-2.54.1 -B /tmp/alloyos-jsc -G Ninja \
  -DCMAKE_TOOLCHAIN_FILE=/absolute/path/to/alloyos-toolchain.cmake \
  -DPORT=JSCOnly -DCMAKE_BUILD_TYPE=Release \
  -DENABLE_STATIC_JSC=ON -DEVENT_LOOP_TYPE=Generic \
  -DENABLE_JSC_GLIB_API=OFF -DENABLE_REMOTE_INSPECTOR=OFF \
  -DENABLE_JIT=OFF -DENABLE_DFG_JIT=OFF -DENABLE_FTL_JIT=OFF \
  -DENABLE_C_LOOP=ON -DENABLE_WEBASSEMBLY=OFF \
  -DENABLE_SAMPLING_PROFILER=OFF -DENABLE_API_TESTS=OFF
cmake --build /tmp/alloyos-jsc --target JavaScriptCore
```

This is a proposed cross configuration, **not a passing build recipe**. There is
no full alloyOS toolchain file or guest C++ sysroot yet. Do not use a Darwin or Linux
sysroot and label its output native alloyOS. Private feature switches require
rechecking on engine upgrades. The first acceptance binary should create a JS
context through `JavaScriptCore/JavaScript.h`, evaluate `6 * 7`, validate `42`,
exercise object allocation/GC and thrown exceptions, then exit from protected
EL0 under QEMU. Do not replace the app interpreter before this gate passes.

Audited upstream files:

- `Source/cmake/OptionsJSCOnly.cmake`: Threads; ICU >=70.1; optional GLib API;
  Generic event loop; static-library option.
- `Source/cmake/WebKitFeatures.cmake`: C-loop conflicts with JIT, WebAssembly,
  and sampling profiler.
- `Source/WTF/wtf/posix/OSAllocatorPOSIX.cpp`: reserve/commit/decommit and guard
  pages through `mmap`/memory protections.
- `Source/WTF/wtf/posix/ThreadingPOSIX.cpp`: pthread implementation and stack bounds.
- `Source/WTF/wtf/generic/RunLoopGeneric.cpp`: generic scheduling implementation.

Observed first blockers on the development host: `cmake`, `ninja`, and
`pkg-config` are absent from PATH (also absent from checked Homebrew and bundled
fallback locations). Apple Clang 17 is present. A direct target-header probe
fails before compiling engine code:

```sh
printf '#include <thread>\n' | clang++ --target=aarch64-unknown-none \
  -std=c++23 -x c++ -fsyntax-only -
# fatal error: 'thread' file not found
```

Installing CMake would remove a host-tool blocker, not the missing target ABI.
Next implement the target runtime and sysroot, cross-build ICU into that sysroot,
and introduce explicit alloyOS platform selection in WTF rather than defining
`__linux__` to take unsupported Linux code paths.

### Verified freestanding C++ compiler foundation

```sh
python3 scripts/web-engine/build-native-probe.py --output /tmp/alloyos-cxx-probe
```

This script cross-compiles two C++23 translation units with Apple Clang 17 for
`aarch64-unknown-none`, without host C++ headers, exceptions, or RTTI. It uses the
active Rust toolchain's bundled `rust-lld` and the existing protected-process
linker layout to produce `cxx-probe.elf`. `--cxx` and `--linker` allow explicit
tool selection. It rejects dynamic linkage, a non-AArch64 executable, a bad entry
point, and writable executable segments, and records the actual tools and ELF
segments in `report.json`.

The build passed on 2026-10-18 with Clang 17 and Rust's LLD 23.1.3. The executable
contains a cross-translation-unit virtual method call computing `6 * 7`, checks
process ABI version 1, reports `CXX_NATIVE_PASS virtual-dispatch=42`, and exits with
status zero on success. Failure exit statuses are 72 (ABI mismatch), 73 (wrong
expression result), and 74 (diagnostic report failed). The protected ABI denies
stdout, so this uses bounded `PANIC_REPORT` syscall 403, which returns zero and
currently prefixes all messages with `[crash] user-panic`; the probe itself exits
normally. The loader provides the initial stack;
the probe takes the existing heap/length entry arguments but does not allocate.
The compiler script itself does **not execute the ELF**. Native execution was
separately verified on 2026-10-18 through the real protected ELF launcher in QEMU:

```sh
CARGO_TARGET_DIR=/private/tmp/alloyos-web-guest-target \
  python3 scripts/check_web_runtime_qemu.py --build --require-cxx \
  --output /private/tmp/alloyos-web-runtime-acceptance
```

The guest reported `CXX_NATIVE_PASS virtual-dispatch=42`, exited zero, and its
private address space was released. The same acceptance run passed bounded
private-memory and original process-isolation probes. Reproduction artifacts
are `guest.log`, `artifact.json` and `cxx/report.json` in the output directory;
the manifest records the exact kernel hash and QEMU command. This remains a
compiler/linker and native-execution foundation, not a C++ standard library,
a libc sysroot, constructors, thread-local storage or a JavaScriptCore build.
An additional LLVM download is unnecessary for these first native C++ objects.

## Full WPE milestone

WPE 2.54 bundles WPEPlatform. New integrations do not require legacy libwpe,
WPEBackend-fdo, Cog, Wayland, or a desktop toolkit. Use an alloyOS WPEPlatform
implementation based on the display/view/toplevel classes.
[Upstream architecture](https://wpewebkit.org/about/architecture.html).

The built-in headless implementation is a lifecycle reference, not the surface
transport: `Source/WebKit/WPEPlatform/wpe/headless/WPEViewHeadless.cpp` acknowledges
buffers on a timer and deliberately produces no visible output. Implement
`WPEViewClass::render_buffer`, SHM buffer handling, buffer-rendered/released
notifications, resize, focus, touch, keyboard, and input method events for
Ferrofluid. Validate stride, pixel format, dimensions, damage bounds, lifetime,
and a memory budget before importing any engine frame.

Use software rendering and shared-memory buffers as the first investigation.
That does **not** mean upstream WPE is graphics-dependency-free: the pinned
`Source/cmake/OptionsWPE.cmake` unconditionally finds Epoxy >=1.5.4 and enables
Skia/coordinated graphics. Establish the exact nonaccelerated path and needed
platform changes before claiming a GL-free build. Disable DRM, Wayland, GBM,
legacy API, multimedia, and other optional features only where supported.

Mandatory WPE dependencies in that file include GLib >=2.70 with GioUnix/thread/
module, HarfBuzz >=2.7.4, ICU >=70.1, JPEG, Epoxy, LibGcrypt >=1.7, Soup3 >=3.0,
Tasn1, XkbCommon >=0.4, LibXml2 >=2.9.13, PNG, SQLite, threads, WebP, zlib, and
FreeType >=2.9. Optional dependency expressions are emitted by the audit tool.

After JSC, the next guest foundations are a GLib event loop with descriptor
polling and clocks, filesystem/profile services, shared mappings and transferable
handles, child process launch/termination, and TLS/socket services. Inspect
`Source/WebKit/Platform/IPC/unix/ConnectionUnix.cpp` (`sendmsg`/`recvmsg`),
`Source/WebCore/platform/unix/SharedMemoryUnix.cpp`, and
`Source/WebKit/UIProcess/Launcher/glib/ProcessLauncherGLib.cpp` (GLib spawn) before
choosing POSIX adapters versus native implementations. Linux bubblewrap cannot
provide alloyOS isolation; enforce capabilities and process isolation in the
native launcher instead.

Full acceptance: local HTML/CSS/JS form visible in QEMU, keyboard and touch input,
bounded frame transport, independent engine crash/restart, then HTTPS navigation
and separate profile storage. Host rendering does not satisfy this milestone.
