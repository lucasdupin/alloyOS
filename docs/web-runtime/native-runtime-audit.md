# Native browser runtime: initial platform audit

Status: planning and prerequisite validation, 2026-10-18. WPE WebKit and
JavaScriptCore are the preferred engines. Neither currently runs on alloyOS.
Removing `no_std` from selected services is a direction, not an implemented
hosted runtime.

## Existing foundation

| Facility | Current implementation | Consequence for a native browser |
| --- | --- | --- |
| Executables | `task/app_process.rs::AddressSpace::load_elf` loads static ELF64 AArch64 `ET_EXEC`, with a bounded address window beginning at `0x10_0000_0000`; rejects dynamic/interpreter/TLS segments and writable executable pages | A normal Linux WPE binary cannot be loaded; produce alloyOS binaries or implement further compatibility |
| Startup | `task/pcb.rs::Task::user_executable` supplies private heap start/length in `x0`/`x1`; `alloyos-user/src/runtime.rs` initializes `ProcessAllocator` | This is an alloyOS entry contract, not libc process startup with argument/environment/auxiliary vectors |
| Memory | Private page backing, inaccessible gaps, validated copies, fixed process heap/stack and bounded anonymous private reservations | Reservations can commit zero-filled RO/RW non-executable pages, decommit backing and release address space; shared mappings and executable transitions remain unsupported |
| Processes | Scheduler, private address spaces, supervisor-owned lifecycle and fault containment | Useful isolation foundation; no general browser subprocess-launch interface exists |
| IPC | Granted endpoint handles; 4096-byte payload limit and 16-message queue limit | Suitable for bounded control messages; full viewport frames need chunking or separately owned shared buffers |
| Time | Protected `ClockGettime` accepts clock ID 1 and writes a monotonic timespec through private-memory validation | Preserve this existing primitive; wall time and blocking timer waits need separate work |
| Display/input | Protected display-service calls 130 through 134 | The browser needs a subordinate surface capability, not the system renderer's authority |
| Files/network | Generic handlers exist but use shared kernel descriptor/network state; protected dispatch denies those calls | Do not route a browser to these handlers without per-process ownership and private-pointer validation |
| Randomness | Generic `GetRandom` handler exists; protected dispatcher does not grant it | A browser-compatible secure-entropy interface remains required |

Source paths above are relative to `crates/alloyos-kernel/src/` unless a crate is
named. `syscall/protected.rs`, rather than `syscall/dispatch.rs`, is the relevant
syscall surface for protected EL0 executables. Kernel-hosted functionality and
host unit-test mocks do not establish protected-process availability.

`alloyos-user/src/tls.rs` implements network TLS. It is unrelated to the
thread-local storage also abbreviated TLS by toolchains and native libraries.

## Runtime gaps to resolve before an engine build

1. **Choose a supported platform contract.** Retain the kernel `no_std`. Define
   an alloyOS hosted-userland target and a narrow compatibility runtime; do not
   point Linux binaries at syscall numbers that happen to resemble Linux's.
   Rust `std` requires a target/platform implementation. WPE is native C/C++
   software, so Rust `std` alone is insufficient.
2. **Define memory and thread semantics.** Add bounded page allocation and
   mappings, stack ownership, synchronization with atomic wait/wake, and real
   thread-local storage. Define process teardown while threads are blocked.
   Start JavaScriptCore feasibility with JIT disabled; this avoids making
   executable-memory transitions the first milestone. An interpreter still
   needs the remaining runtime facilities.
3. **Bring up C/C++ dependencies.** Supply compiler runtime, libc/libm, required
   C++ runtime behavior, startup constructors, and thread-local initialization.
   Link a representative dependency with the same target and entry contract.
   Decide static linking first; a dynamic loader is a separate project.
4. **Implement owned I/O and event waiting.** Per-process descriptors or
   capability-backed equivalents need bounded private-buffer operations,
   seek/stat semantics as required, readiness polling, monotonic deadlines,
   cancellation and closed-handle behavior. Scope browser profile files and
   network access to the browser service.
5. **Pin and audit upstream dependencies.** Before writing stubs, select exact
   WPE/WebKit revisions and build options, then inventory required symbols,
   libraries, system calls and rendering backend. Expected categories include
   event loops, Unicode, fonts, rasterization, image decoding, TLS/networking
   and process IPC. The exact dependency closure is not verified by this
   repository audit.
6. **Connect rendering and input.** Use a software-rendered local page first.
   Ferrofluid owns placement, trusted origin UI, focus and soft-keyboard routing.
   Web execution receives neither app/MCP handles nor direct system display
   authority. Preserve the browser engine's necessary internal process roles.

### First engine target and next syscall increment

Use a JavaScriptCore-only build with WTF's generic run loop for the first native
engine experiment. This reduces the initial dependency surface; it does not
provide WPE rendering or eliminate the runtime requirements. Inspection of the
WPE WebKit 2.54.1 source shows:

- `Source/WTF/wtf/posix/OSAllocatorPOSIX.cpp` uses `mmap`, `munmap` and
  `mprotect`, including inaccessible reservations and read/write commitment.
- `Source/WTF/wtf/ThreadSpecific.h` uses pthread key creation/get/set for its
  pthread path. This is distinct from compiler-generated ELF TLS.
- `Source/WTF/wtf/posix/ThreadingPOSIX.cpp` uses thread creation/join,
  mutexes and condition-variable waits; several other facilities depend on
  platform build flags.

The first memory increment now represents anonymous private reservations in
`AddressSpace`, separately from the initial ELF, heap and stack. The protected
ABI uses calls 404 through 407 for reserve, commit, decommit and release:

- Reserve requires a nonzero page-multiple length and chooses a free interval
  above the initial address-space extent. It allocates metadata and empty page
  tables, without allocating physical page backing. Initial pages plus all
  reservation pages must remain within the loader's process `max_pages` quota.
- Commit accepts only read-only or read/write, non-executable pages. New backing
  is zero-filled; recommitting existing pages preserves bytes and changes access
  permissions. Allocation failure does not partially commit an interval.
- Decommit invalidates page-table translations before releasing physical backing.
  The interval remains reserved and inaccessible; recommit supplies zeroed pages.
- Release accepts an exact whole reservation and makes its address and quota
  available for reuse. Process teardown owns and drops every reservation.
- All mutations require aligned intervals within one owned reservation; static
  executable/heap/stack ranges and cross-reservation intervals are rejected.
  Kernel copies validate permissions and access kernel-owned backing rather than
  dereferencing user pointers.

Host tests cover these state transitions, quota restoration, sparse first-fit
reuse, page-table boundaries, invalid ranges and distinct process backing. These
checks alone do not establish native protected-syscall acceptance. Shared mappings,
executable/JIT transitions and libc wrappers remain separate work. This deliberately
does not expose the global kernel allocator or accept arbitrary caller pointers.

## Native proof sequence

Each step requires an actual protected process running in QEMU. Host compilation
and host-side rendering are useful checks but do not satisfy these milestones.

- [x] Bounded private-memory prerequisite: real protected EL0 calls reserve,
  commit, preserve existing bytes, decommit/recommit with zeroing and release.
  Native QEMU acceptance rejects malformed lengths, quota overflow, executable
  permission requests and static-code mutation; uncommitted access faults only
  the caller, and teardown discards the process address space. This is not a
  hosted `std` runtime. Reproduce with the command below.
- [ ] Hosted runtime probe allocates/frees, reads monotonic time, reports failure
  through a granted endpoint, and exits; invalid pointers cannot affect peers.
- [ ] A representative C/C++ dependency runs with constructors, thread-local
  state and the supported error model. Two threads prove TLS separation and
  bounded wait/wake behavior if the dependency uses threads.
- [ ] JavaScriptCore evaluates a fixed script in the guest and reports the
  expected result without acquiring app, display or knowledge capabilities.
- [ ] WPE renders a bundled local page with text, an image and a JavaScript
  form into a Ferrofluid-owned surface. Guest touch and keyboard input update
  the page; a deliberate browser crash preserves Ferrofluid and allows restart.
- [ ] HTTPS navigation works with secure entropy, certificate validation, origin
  display and scoped profile persistence; denial paths remain testable.

Existing regression commands for the foundation:

```sh
cargo test -p alloyos-kernel --lib task::app_process::tests
python3 scripts/test-process-isolation.py
python3 scripts/test-process-runtime.py
```

The first command tests host-side loader/memory logic. The Python commands build
and run the existing protected-process QEMU acceptance suites; neither suite
currently validates `std`, JavaScriptCore or WPE.

The initial hardening change explicitly rejects `PT_TLS` instead of silently
accepting an executable whose thread-local storage cannot be initialized. Its
regression test preserves valid load segments and compares rejection against an
otherwise identical executable with an inert metadata header. No new guest
browser execution is claimed.

### Native memory and freestanding C++ acceptance

```sh
CARGO_TARGET_DIR=/private/tmp/alloyos-web-guest-target \
  python3 scripts/check_web_runtime_qemu.py --build --require-cxx \
  --output /private/tmp/alloyos-web-runtime-acceptance
```

Verified on 2026-10-18 using QEMU HVF, a diskless/networkless 512 MiB guest and
an actual protected C++ static ELF. The script also requires the original EL0
isolation acceptance, including IPC, invalid pointers and timer preemption.
`--accel tcg` selects portable emulation. Existing kernels can be selected with
`--kernel`; they must include the `process-selftest` feature and, for
`--require-cxx`, the embedded compiler probe. The optional compile-time
`alloyos_cxx_probe` cfg and `ALLOYOS_CXX_PROBE_ELF` path are supplied by `--build
--require-cxx`; no generated ELF is committed.

The artifact directory contains `guest.log`, the generated C++ compiler report,
and `artifact.json` with command, acceptance outcome and kernel SHA-256. The
verified kernel hash was
`b0ff17c699a161a2d9a6a732b48ce022ba43680063a6ac3222b853a5cba3ec53`.
Neither this probe nor its passing markers establish libc, Rust `std`, C++
standard-library, thread-local-storage, JavaScriptCore or WPE support.
