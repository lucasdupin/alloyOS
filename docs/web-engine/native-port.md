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
checks process ABI version 1, traverses `.init_array`, evaluates a real
cross-translation-unit virtual call computing `6 * 7`, and verifies initialized
and zero local-exec TLS. It writes a PID-specific TLS canary, yields, and checks
that canary throughout eight monotonic 20ms busy intervals. The linker places a
64-aligned `PT_TLS` template, whose compiler-generated accesses use `TP + 64`
and `TP + 68` as required by the loader's Variant I layout.

The compiler script produces a primary ELF and a peer with an extra BSS page,
so their initial thread-pointer virtual addresses differ. Guest acceptance
requires both normal exits, address-space teardown, and measured timer IRQ
switches to a different task for each process. Failure statuses identify ABI,
expression, report, constructor, TLS, PID and monotonic-clock failures. The
protected ABI denies stdout, so bounded syscall 403 reports diagnostics with the
existing `[crash] user-panic` prefix; successful probes exit normally.

The compiler script itself does **not execute the ELFs**. Native execution was
verified through the real protected ELF launcher in QEMU:

```sh
CARGO_TARGET_DIR=/private/tmp/alloyos-web-guest-target \
  python3 scripts/check_web_runtime_qemu.py --build --require-cxx \
  --output /private/tmp/alloyos-web-runtime-tls-acceptance
```

Both guest processes reported
`CXX_NATIVE_PASS virtual-dispatch=42 constructors=1 tls=pid`, exited zero,
and recorded 55 and 56 timer IRQ peer switches. Bounded private-memory and the
original process-isolation probes also passed. Reproduction artifacts are
`guest.log`, `artifact.json` and `cxx/report.json`; the manifest records the exact
kernel hash and QEMU command. This establishes static constructors and
single-thread local-exec TLS across processes. It does not establish TLS
resolution for dynamic modules, pthreads, destructors, a C++ standard library
or JavaScriptCore. An additional LLVM download is unnecessary for these first
native C++ objects.

## C library source and cross-build foundation

The next sysroot investigation uses upstream newlib **4.6.0.20260123**, rather
than reusing Darwin/Linux library binaries. Source is published at the
[official newlib archive](https://sourceware.org/ftp/newlib/index.html).
The official release download observed during this investigation has SHA-256
`6ff27e3bf022666f43f7802255be680eeff722ac181b1725d21e2e8318604ee3`;
this is a reproducibility pin, not a separately verified signature.

```sh
python3 scripts/web-engine/build-newlib.py \
  --archive /tmp/newlib-4.6.0.20260123.tar.gz --fetch \
  --output /tmp/alloyos-newlib
```

The script checks the archive before extraction, cross-configures with the
existing Clang AArch64 compiler and Rust LLVM archiver, disables supplied system
calls and multilib, then builds and installs headers, `libc.a`, and `libm.a` in
the output directory. Logs and a tool/source/library manifest stay beside that
sysroot. It does not install into the host filesystem. No standard C++ library,
pthread implementation, alloyOS system-call adapter, or JSC library is supplied
by this step. Linking a libc archive is not proof that all libc facilities work
in a protected process: allocation/reentrancy, errno, clocks, files, locale, and
threads need explicit native integration and acceptance.

Newlib is a practical freestanding C library candidate, not a decision that it
satisfies WebKit's POSIX requirements. The remaining libc++ configuration must
select a real target thread backend and avoid borrowing host pthread headers;
WTF/ICU and full WPE still need their own native ports.

### Verified native library subset

```sh
CARGO_TARGET_DIR=/private/tmp/alloyos-web-guest-target \
  python3 scripts/check_web_runtime_qemu.py --build \
  --newlib-sysroot /private/tmp/alloyos-newlib-full/sysroot/aarch64-none-elf \
  --output /private/tmp/alloyos-web-runtime-libc-acceptance
```

On 2026-10-18 this separately verified actual newlib `memcpy`, `memmove`,
`strtod` and `sin` calls in both protected guest processes, alongside the full
constructor/TLS, timer-preemption, memory and isolation acceptance. The script
requires two `LIBC_NATIVE_PASS memcpy memmove strtod sin` reports. Its C++ build
report records the upstream source/build manifest and archive hashes; the guest
artifact records kernel SHA-256
`a6068cc697a0d9a73e250335cd2b21bb987309aacff07e83f8ede89e0888ef14`.
The probe provides bounded heap growth and honest error-return hooks for
unavailable system services. This narrow proof does not establish a complete
libc platform, pthreads, libc++, Rust `std`, JavaScriptCore or WPE.

### Native alloyOS POSIX/C++ ABI sysroot and CMake toolchain foundation

`scripts/web-engine/build-sysroot.py` builds the alloyOS AArch64 compatibility
headers and static archives (`liballoyos_posix.a` and `liballoyos_cxxrt.a`),
either standalone or layered over a cross-built `aarch64-none-elf` newlib
sysroot (`--newlib`), and links a multi-file C++23 verification executable
(`verify/sysroot-verify.elf`) with `--no-undefined`:

```sh
python3 scripts/web-engine/build-sysroot.py --output /private/tmp/alloyos-sysroot
python3 scripts/web-engine/build-jsc.py \
  --sysroot /private/tmp/alloyos-sysroot \
  --output /private/tmp/alloyos-jsc-foundation
```

Supplied by `scripts/web-engine/sysroot/`:

- `liballoyos_posix.a` (`crt0.o`, `syscalls.o`, `mman.o`, `pthread.o`,
  `time_random.o`):
  - `sys/mman.h` (`mmap`, `munmap`, `mprotect`, `madvise`, `posix_madvise`)
    backed by protected syscalls `MEMORY_RESERVE` (404), `MEMORY_COMMIT` (405),
    `MEMORY_DECOMMIT` (406), and `MEMORY_RELEASE` (407). Supports anonymous
    private reservations (`MAP_PRIVATE | MAP_ANONYMOUS`), `PROT_NONE`,
    `PROT_READ`, and `PROT_READ | PROT_WRITE`, while rejecting `PROT_EXEC` and
    file-backed/shared mappings with `ENOTSUP`/`EINVAL`.
  - `pthread.h` and `sched.h` (`pthread_create`, `pthread_join`,
    `pthread_detach`, `pthread_self`, `pthread_equal`, `pthread_exit`,
    `pthread_attr_*`, `pthread_mutex_*`, `pthread_cond_*`, `pthread_once`,
    `pthread_key_*`, `sched_yield`) backed by protected syscalls
    `THREAD_CREATE` (408), `THREAD_EXIT` (409), `THREAD_JOIN` (410),
    `WORD_WAIT` (411), `WORD_WAKE` (412), `GETTID` (413), and `Yield` (24),
    allocating per-thread guard/stack/Variant I static TLS reservations.
  - `time.h` and `sys/random.h` (`clock_gettime(CLOCK_MONOTONIC)` via syscall
    228, `nanosleep` via `WORD_WAIT` monotonic deadlines, and
    `getrandom`/`getentropy` via syscall 318 `SyscallNo::GetRandom` with 256-byte
    chunking and fail-closed `EAGAIN` handling).
- `liballoyos_cxxrt.a` (`cxxrt.o`): replaceable `operator new`/`delete` and
  `operator new[]`/`delete[]` (including sized, `std::nothrow_t`, and C++17
  `std::align_val_t` overloads), `__cxa_pure_virtual`, `__cxa_deleted_virtual`,
  Itanium/ARM64 C++ static initialization guards (`__cxa_guard_acquire`,
  `__cxa_guard_release`, `__cxa_guard_abort` over `WORD_WAIT`/`WORD_WAKE`), and
  `__cxa_atexit`/`__cxa_finalize`.
- `scripts/web-engine/alloyos-toolchain.cmake`: CMake cross-compilation
  toolchain file targeting `aarch64-unknown-none` with `--sysroot` isolation and
  `CMAKE_FIND_ROOT_PATH_MODE_{LIBRARY,INCLUDE,PACKAGE}=ONLY` so host Darwin or
  Linux libraries are never treated as target dependencies.

Verification status (2026-10-23): `build-sysroot.py` and `build-jsc.py` compile
all C17/C++23 sysroot archives cleanly with Apple Clang 17 and Rust `llvm-ar`,
and link both `sysroot-verify.elf` and `jsc-platform-probe.elf` with `rust-lld`
(`--no-undefined --fatal-warnings -z max-page-size=4096`), verifying `ET_EXEC`
segments and Variant I `PT_TLS` coverage.

Both ELFs also execute natively as protected EL0 processes in QEMU via
`scripts/check_web_runtime_qemu.py --build --require-sysroot --require-jsc`:

```sh
CARGO_TARGET_DIR=/private/tmp/alloyos-web-guest-target \
  python3 scripts/check_web_runtime_qemu.py --build \
  --require-cxx --require-sysroot --require-jsc \
  --output /private/tmp/alloyos-web-runtime-sysroot-jsc-acceptance
```

- `sysroot-verify.elf` exercises multi-file C++23 static constructors, aligned
  `operator new`, `mmap`/`mprotect`/`munmap` (`PROT_EXEC` and file-backed
  rejection with `ENOTSUP`), `clock_gettime(CLOCK_MONOTONIC)`, `getrandom`
  (>256-byte chunking), `pthread_once`, `pthread_key_create` with destructor
  execution on thread exit, `pthread_create`/`pthread_join`, `pthread_mutex_t`,
  `pthread_cond_t`, and per-thread Variant I local-exec TLS isolation in EL0,
  reporting `SYSROOT_CXX_PASS virtual=42 mman=ok pthread=ok time=ok` and exiting
  with status `0`.
- `jsc-platform-probe.elf` exercises `OSAllocatorPOSIX`-style guarded memory
  reservations (`PROT_EXEC` rejected with `ENOTSUP`), virtual task dispatch
  (`6 * 7 == 42`), `clock_gettime(CLOCK_MONOTONIC)`, `getrandom`, and a spawned
  `pthread_t` worker thread that runs a self-contained C++23 verification
  harness for `tests/fixtures/web/engine-smoke.js` (array map + JSON
  serialization, `Map` lookup, `Uint8Array` reduction, `SyntaxError` rejection,
  and queueing/draining a `Promise.resolve(21).then(...)` microtask on a
  Generic runloop), reporting `JSC_PLATFORM_PROBE_PASS allocator=ok task=42` and
  `ALLOYOS_JSC_SMOKE_PASS` from protected EL0 and exiting with status `0`.
- Transparency note: `jsc-platform-probe.elf` validates the WTF/JSCOnly
  POSIX/C++ ABI runtime contract and `engine-smoke.js` microtask semantics in
  protected EL0; compiling full upstream WPE WebKit 2.54.1 `libJavaScriptCore.a`
  still requires an extracted WebKit 2.54.1 source tree, host `cmake`/`ninja`,
  full target `libc++` STL headers, and cross-compiled target ICU >= 70.1
  static archives.

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
