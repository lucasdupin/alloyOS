# metalOS Filesystem Structure

metalOS exposes a unified POSIX-inspired hierarchy managed by the kernel's Virtual Filesystem Switch ([`crates/metalos-kernel/src/fs/vfs.rs`](crates/metalos-kernel/src/fs/vfs.rs)). At boot, the VFS mounts **CobaltFS** (backed by VirtIO-Blk MMIO or ATA PIO) as the root filesystem `/`, falling back to **RamFS** when no persistent block volume is attached, and overlays virtual device nodes under `/dev`.

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
├── tmp/                    # Scratch directory for temporary files
├── scripts/                # Local JavaScript ES modules executed by scriptd (metalos-js)
│   └── heartbeat.js        # Default system telemetry heartbeat script
└── apps/                   # Headless Model Context Protocol (MCP) developer applications
```

---

## Top-Level Directories & Files

### `/bin` — System Binaries
Contains executable entry points for userland tasks:
- `/bin/sh`: Bring-up console shell (`metal-sh`) used for kernel diagnostics and serial console testing.

### `/dev` — Device Filesystem ([`devfs.rs`](crates/metalos-kernel/src/fs/devfs.rs))
Routed directly by [`Vfs`](crates/metalos-kernel/src/fs/vfs.rs) to [`DevFS`](crates/metalos-kernel/src/fs/devfs.rs) without touching disk storage. Creation and unlinking inside `/dev` return `PermissionDenied`.

| Path | Inode | Kind | Behavior |
|---|---|---|---|
| `/dev/tty` | `12` | `CharDevice` | Alias for `/dev/serial` (controlling console) |
| `/dev/null` | `10` | `CharDevice` | Reads return `0` bytes (`EOF`); writes discard all bytes |
| `/dev/zero` | `11` | `CharDevice` | Reads fill destination buffer with `0x00`; writes discard all bytes |
| `/dev/serial` | `12` | `CharDevice` | Reads/writes directly to ARM PL011 UART0 (`0x0900_0000`). Pre-bound to FDs `0`, `1`, `2` |
| `/dev/urandom` (`/dev/random`) | `13` | `CharDevice` | Reads fill buffer from the kernel hardware CSPRNG ([`drivers/rng.rs`](crates/metalos-kernel/src/drivers/rng.rs)) |

### `/proc` — Kernel Telemetry
Populated during [`Vfs::with_root`](crates/metalos-kernel/src/fs/vfs.rs) initialization with system state:
- `/proc/meminfo`: Reports `MemTotal`, `MemFree`, and `MemUsed` in KiB from the physical bitmap frame allocator ([`mm/frame_allocator.rs`](crates/metalos-kernel/src/mm/frame_allocator.rs)).
- `/proc/uptime`: Monotonic system uptime and idle time in seconds.
- `/proc/cpuinfo`: Processor architecture (`AArch64`) and feature flags (`fp asimd evtstrm aes pmull sha1 sha2 crc32`).
- `/proc/version`: Kernel release string (`metalOS version 0.1.0 (rustc nightly aarch64)`).

### `/etc` — System & AI Configuration
Holds system-wide configuration files shared between the kernel, `metal-sh`, and the AI runtime ([`metalos-ai`](crates/metalos-ai)):
- `/etc/motd`: Welcome banner (`Welcome to metalOS v0.1.0 (aarch64 POSIX-compliant Rust kernel)`).
- `/etc/ai.conf`: Default AI operator settings (`provider=gemini`, `model=gemini-2.5-pro`, `shell_access=disabled`).
- `/etc/schedules.conf`: Registry of time- and event-triggered JavaScript jobs managed by `script_schedule`.

### `/scripts` — Local JavaScript Automation ([`metalos-js`](crates/metalos-js))
Stores sandboxed JavaScript scripts executed by `scriptd` via the `MetalOS.*` host bindings (`MetalOS.fs`, `MetalOS.net`, `MetalOS.mcp`, `MetalOS.ui`, `MetalOS.sys`):
- `/scripts/heartbeat.js`: Built-in health-check script (`export function main() { return MetalOS.sys.status(); }`).

### `/apps` — Headless MCP Applications ([`metalos-mcp`](crates/metalos-mcp))
Mount point and package directory for headless Model Context Protocol (MCP) developer applications (`notes`, `sysmon`, `fetch`). MCP apps expose tools, resources, and prompt templates to the AI operator and are strictly forbidden from creating their own UI windows or invoking a shell.

### `/tmp` — Temporary Storage
General-purpose scratch directory for transient files created during runtime or E2E testing.

---

## Filesystem Backends

| Backend | Implementation | Persistence | Description |
|---|---|---|---|
| **CobaltFS** | [`crates/metalos-kernel/src/fs/cobaltfs/`](crates/metalos-kernel/src/fs/cobaltfs/README.md) | Persistent (Disk) | ext2-compatible filesystem supporting 1 KiB–4 KiB blocks, direct/single-indirect/double-indirect block pointers, inode/block bitmaps, and directory entry management over VirtIO-Blk (`virtio-blk-device`) or ATA PIO. |
| **RamFS** | [`crates/metalos-kernel/src/fs/ramfs.rs`](crates/metalos-kernel/src/fs/ramfs.rs) | Volatile (RAM) | In-memory `BTreeMap`-backed filesystem with automatic parent directory creation and path normalization (`..`, `.`). Used when no block device is present. |
| **DevFS** | [`crates/metalos-kernel/src/fs/devfs.rs`](crates/metalos-kernel/src/fs/devfs.rs) | Virtual | Kernel device node multiplexer mounted at `/dev/*`. |

---

## Accessing the Filesystem

### 1. System Calls (`svc #0` ABI)
Userland processes interact with the VFS through [`libmetal::fs`](crates/metalos-user/src/fs/mod.rs) and [`crates/metalos-kernel/src/syscall/handlers.rs`](crates/metalos-kernel/src/syscall/handlers.rs):
- `SyscallNo::Open (2)`: Opens a file or directory handle (allocating FDs `3..255`; FDs `0`, `1`, `2` are pre-allocated to `/dev/serial`).
- `SyscallNo::Read (0)`: Reads file bytes or, when invoked on an open directory handle, returns a newline-delimited directory listing (`Vfs::readdir`) with `/` suffixes on subdirectories.
- `SyscallNo::Write (1)`: Writes bytes to an open file or character device.
- `SyscallNo::Close (3)`: Closes an open file descriptor.
- `SyscallNo::Stat (4)`: Resolves a path in the VFS and returns `1` for `InodeKind::Directory`, `0` for `InodeKind::File` / `InodeKind::CharDevice`, or `ENOENT (-2)` if not found.

### 2. Structured AI Tools ([`metalos-ai`](crates/metalos-ai/src/tools.rs))
Because the AI agent operates under a strict shell-free policy, it accesses the filesystem exclusively through native structured tools:
- `fs_read(path)` — Read UTF-8 file contents from CobaltFS/VFS.
- `fs_write(path, content)` — Persist UTF-8 file contents to CobaltFS/VFS.
- `fs_list(path)` — List entries under a directory prefix.
- `script_create(path, source)` — Write a JavaScript automation module under `/scripts/*.js`.
