# alloyOS Filesystem Structure

alloyOS exposes a unified POSIX-inspired hierarchy managed by the kernel's Virtual Filesystem Switch ([`crates/alloyos-kernel/src/fs/vfs.rs`](crates/alloyos-kernel/src/fs/vfs.rs)). At boot, the VFS mounts **CobaltFS** (backed by VirtIO-Blk MMIO or ATA PIO) as the root filesystem `/`, falling back to **RamFS** when no persistent block volume is attached, and overlays virtual device nodes under `/dev`.

---

## Directory Tree Overview

```text
/                           # Root directory (CobaltFS persistent volume or RamFS fallback)
├── bin/                    # System binaries & bring-up utilities
│   └── sh                  # Debug / bring-up shell binary stub (#!/bin/sh)
├── dev/                    # Virtual character devices (DevFS)
│   ├── tty                 # Controlling terminal alias (PL011 UART0, Inode 12)
│   ├── null                # Bit-bucket device; reads return EOF, writes succeed (Inode 10)
│   ├── zero                # Zero-byte generator; reads fill buffer with 0x00 (Inode 11)
│   ├── serial              # Primary PL011 UART0 serial console (stdin/stdout/stderr, Inode 12)
│   └── urandom             # Hardware RNG stream (/dev/random alias, Inode 13)
├── proc/                   # Kernel & hardware telemetry files
│   ├── meminfo             # Physical frame allocator & memory usage statistics (kB)
│   ├── uptime              # System monotonic uptime and idle seconds
│   ├── cpuinfo             # AArch64 processor revision and ISA feature flags
│   └── version             # Kernel version and compiler target string
├── etc/                    # System & AI runtime configuration
│   ├── motd                # Message of the Day boot banner
│   ├── ai.conf             # Active AI provider, model, and shell-free security policy
│   └── schedules.conf      # Persistent registry of scheduled local JavaScript automations
├── var/                    # Per-app allow-listed writable state (/var/<app_id>/)
├── tmp/                    # Scratch directory for temporary files
├── scripts/                # Local JavaScript ES modules executed by scriptd (alloyos-js)
│   └── heartbeat.js        # Default system telemetry heartbeat script
├── apps/                   # Headless MCP apps (MCP entry points, SKILL.md files & declarative Tiles)
│   ├── clock/              # System uptime & time awareness (clock.now)
│   ├── tasks/              # Scheduled automation CRUD (tasks.board, exclusive ScriptScheduler access)
│   ├── package-manager/    # System package inventory & validation diagnostics (package_manager.status)
│   ├── calc/               # Deterministic math & unit conversions (calc.scratchpad)
│   ├── memory/             # User facts, preferences & bookmarks (memory.profile)
│   └── weather/            # Ambient meteorological observations & forecasts (weather.card)
└── knowledge/              # Local user context managed by Welder
    ├── profile.md          # Explicit user facts
    ├── preferences.md      # Scoped communication, visual, and workflow preferences
    ├── interests.md        # Interests and ongoing goals
    ├── observations.md     # Tentative inferred patterns
    └── projects/           # Project-specific context
```

---

## Top-Level Directories & Files

### `/bin` — System Binaries
Contains executable entry points for userland tasks:
- `/bin/sh`: Bring-up console shell (`alloy-sh`) used for kernel diagnostics and serial console testing.

### `/dev` — Device Filesystem ([`devfs.rs`](crates/alloyos-kernel/src/fs/devfs.rs))
Routed directly by [`Vfs`](crates/alloyos-kernel/src/fs/vfs.rs) to [`DevFS`](crates/alloyos-kernel/src/fs/devfs.rs) without touching disk storage. Creation and unlinking inside `/dev` return `PermissionDenied`.

| Path | Inode | Kind | Behavior |
|---|---|---|---|
| `/dev/tty` | `12` | `CharDevice` | Alias for `/dev/serial` (controlling console) |
| `/dev/null` | `10` | `CharDevice` | Reads return `0` bytes (`EOF`); writes discard all bytes |
| `/dev/zero` | `11` | `CharDevice` | Reads fill destination buffer with `0x00`; writes discard all bytes |
| `/dev/serial` | `12` | `CharDevice` | Reads/writes directly to ARM PL011 UART0 (`0x0900_0000`). Pre-bound to FDs `0`, `1`, `2` |
| `/dev/urandom` (`/dev/random`) | `13` | `CharDevice` | Reads fill buffer from the kernel hardware CSPRNG ([`drivers/rng.rs`](crates/alloyos-kernel/src/drivers/rng.rs)) |

### `/proc` — Kernel Telemetry
Populated during [`Vfs::with_root`](crates/alloyos-kernel/src/fs/vfs.rs) initialization with system state:
- `/proc/meminfo`: Reports `MemTotal`, `MemFree`, and `MemUsed` in KiB from the physical bitmap frame allocator ([`mm/frame_allocator.rs`](crates/alloyos-kernel/src/mm/frame_allocator.rs)).
- `/proc/uptime`: Monotonic system uptime and idle time in seconds.
- `/proc/cpuinfo`: Processor architecture (`AArch64`) and feature flags (`fp asimd evtstrm aes pmull sha1 sha2 crc32`).
- `/proc/version`: Kernel release string (`alloyOS version 0.1.0 (rustc nightly aarch64)`).

### `/etc` — System & AI Configuration
Holds system-wide configuration files shared between the kernel, `alloy-sh`, and the AI runtime ([`alloyos-welder`](crates/alloyos-welder)):
- `/etc/motd`: Welcome banner (`Welcome to alloyOS v0.1.0 (aarch64 POSIX-compliant Rust kernel)`).
- `/etc/ai.conf`: Default AI operator settings (`provider=forge`, `model=gemma-2-2b-it`, `shell_access=disabled`, `knowledge_learning_paused=false`).
- `/etc/schedules.conf`: Registry of time- and event-triggered JavaScript jobs managed exclusively through the `Tasks` app (`tasks.create`, `tasks.edit`, `tasks.delete`).

### `/var` — Scoped MCP App Writable Storage
Holds persistent per-application state under `/var/<app_id>/` (for example `/var/clock/`). MCP app write permissions are strictly allow-list based: an app `app_id` can only write within `/var/<app_id>/` (with `tasks` additionally allowed its scheduler paths `/scripts` and `/etc/schedules.conf`).

### `/scripts` — Local JavaScript Automation ([`alloyos-js`](crates/alloyos-js))
Stores sandboxed JavaScript scripts executed by `scriptd` via the `AlloyOS.*` host bindings (`AlloyOS.fs`, `AlloyOS.net`, `AlloyOS.ui`, `AlloyOS.sys`):
- `/scripts/heartbeat.js`: Built-in health-check script (`export function main() { return AlloyOS.sys.status(); }`).

### `/apps` — Headless MCP Applications, SKILLs & Tiles ([`alloyos-mcp`](crates/alloyos-mcp))
Package directory for headless Model Context Protocol (MCP) applications. Each app under `/apps/<app_id>/` can expose:
- **MCP entry points**: Tools, resources, and prompts (`<app_id>.mcp` manifest).
- **SKILLs**: Plain-text Markdown instruction files (`/apps/<app_id>/SKILL.md`) read by Welder.
- **Tiles**: Declarative, UI-agnostic `AppTile` descriptors that Ferrofluid can embed in the chat stream or pin to the Dashboard.

Isolation invariants enforced by `alloyos-mcp` and `alloyos-welder`:
- Write permissions for MCP apps are allow-list based and restricted to `/var/<app_id>/`.
- `/apps/tasks/` (**Tasks**) is the first canonical app and is the **only** app allowed to access `ScriptScheduler` (`/etc/schedules.conf` and `/scripts/*.js`). Welder routes all schedule management through `Tasks`.
- No app can access or invoke another app; **only Welder centralizes app access and orchestration**.


### `/knowledge` — Local User Context ([`knowledge.rs`](crates/alloyos-welder/src/knowledge.rs))

Welder reads and maintains Markdown records of user facts (`profile.md`),
scoped preferences (`preferences.md`), interests (`interests.md`), tentative
observations (`observations.md`), and project context (`projects/<slug>.md`).
This is runtime user data, not a folder of personal information in the source
checkout. Apps may contribute scoped project context through Welder rather than
rewriting the user profile (MCP apps are allow-listed to write only to
`/var/<app_id>/`). See [KNOWLEDGE.md](KNOWLEDGE.md) for provenance, retrieval,
and user controls. Persistence requires CobaltFS (`DurableCobaltFs`); records on
the RamFS fallback are explicitly marked volatile (`VolatileRamFs`).

### `/tmp` — Temporary Storage
General-purpose scratch directory for transient files created during runtime or E2E testing.

---

## Filesystem Backends

| Backend | Implementation | Persistence | Description |
|---|---|---|---|
| **CobaltFS** | [`crates/alloyos-kernel/src/fs/cobaltfs/`](crates/alloyos-kernel/src/fs/cobaltfs/README.md) | Persistent (Disk) | ext2-compatible filesystem supporting 1 KiB–4 KiB blocks, direct/single-indirect/double-indirect block pointers, inode/block bitmaps, and directory entry management over VirtIO-Blk (`virtio-blk-device`) or ATA PIO. |
| **RamFS** | [`crates/alloyos-kernel/src/fs/ramfs.rs`](crates/alloyos-kernel/src/fs/ramfs.rs) | Volatile (RAM) | In-memory `BTreeMap`-backed filesystem with automatic parent directory creation and path normalization (`..`, `.`). Used when no block device is present. |
| **DevFS** | [`crates/alloyos-kernel/src/fs/devfs.rs`](crates/alloyos-kernel/src/fs/devfs.rs) | Virtual | Kernel device node multiplexer mounted at `/dev/*`. |

---

## Accessing the Filesystem

### 1. System Calls (`svc #0` ABI)
Userland processes interact with the VFS through [`liballoy::fs`](crates/alloyos-user/src/fs/mod.rs) and [`crates/alloyos-kernel/src/syscall/handlers.rs`](crates/alloyos-kernel/src/syscall/handlers.rs):
- `SyscallNo::Open (2)`: Opens a file or directory handle (allocating FDs `3..255`; FDs `0`, `1`, `2` are pre-allocated to `/dev/serial`).
- `SyscallNo::Read (0)`: Reads file bytes or, when invoked on an open directory handle, returns a newline-delimited directory listing (`Vfs::readdir`) with `/` suffixes on subdirectories.
- `SyscallNo::Write (1)`: Writes bytes to an open file or character device.
- `SyscallNo::Close (3)`: Closes an open file descriptor.
- `SyscallNo::Stat (4)`: Resolves a path in the VFS and returns `1` for `InodeKind::Directory`, `0` for `InodeKind::File` / `InodeKind::CharDevice`, or `ENOENT (-2)` if not found.

### 2. Structured AI Tools ([`alloyos-welder`](crates/alloyos-welder/src/tools.rs))
Because the AI agent operates under a strict shell-free policy, it accesses the filesystem exclusively through native structured tools backed by `ToolVfsBackend` (`KernelVfsBackend` on CobaltFS/VFS):

- `fs_read(path)` — Read UTF-8 file contents from CobaltFS/VFS.
- `fs_write(path, content)` — Persist UTF-8 file contents to CobaltFS/VFS (direct raw writes to `/knowledge/` are rejected; use the structured knowledge tools).
- `fs_list(path)` — List entries under a directory prefix.
- `script_create(path, source)` — Write a JavaScript automation module under `/scripts/*.js`.
- `knowledge_search(query, category?, scope?)` — Search `/knowledge/` records and synthesize the `knowledge.profile` `AppTile`.
- `knowledge_upsert(category, title, value, basis?, scope?, source?)` — Validate, filter secrets, and merge a structured entry into `/knowledge/*.md` with interrupted-write protection.
- `knowledge_forget(query, category?)` — Remove matching records from `/knowledge/*.md`, summaries, and retrieval.
