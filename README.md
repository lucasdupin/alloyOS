<p align="center">
  <img src="assets/logo.svg" alt="metalOS Logo" width="720" />
</p>

# metalOS

**metalOS** is an AI-first, bare-metal `x86_64` operating system written in Rust (`#![no_std]`). It boots through a liquid-metal drip animation into **Mercury (`mercuryd`)**, a display server that initializes with a single terminal-style AI chat window powered by pluggable models (**Gemini**, **Claude**, **ChatGPT**).

See **[ROADMAP.md](ROADMAP.md)** for the full architecture and milestone plan.

## Workspace Structure

| Crate | Description |
|---|---|
| [`crates/metalos-kernel`](crates/metalos-kernel) | Core `x86_64` kernel: GDT/IDT, paging, heap allocator, round-robin scheduler, syscalls, VFS, and **CobaltFS** |
| [`crates/metalos-boot`](crates/metalos-boot) | BIOS/UEFI boot configuration and memory map layout |
| [`crates/metalos-proto`](crates/metalos-proto) | Shared ABI/syscall definitions between kernel and userland |
| [`crates/metalos-user`](crates/metalos-user) | Userland standard library (`libmetal`) and interactive shell (`sh`) |
| [`xtask`](xtask) | Build, QEMU runner, and headless E2E test harness |

## Filesystem

metalOS uses a Virtual Filesystem Switch ([`vfs.rs`](crates/metalos-kernel/src/fs/vfs.rs)) supporting:
- **[CobaltFS](crates/metalos-kernel/src/fs/cobaltfs/README.md)** — Persistent on-disk filesystem based on the ext2 specification, implemented from scratch in Rust.
- **RamFS** — In-memory filesystem for transient storage.
- **DevFS** — Device nodes (`/dev/null`, `/dev/zero`, `/dev/serial`).

## Building & Running

Build the bootable BIOS disk image and launch in QEMU:

```bash
cargo run --package xtask -- run
```

Run the headless QEMU E2E test suite:

```bash
cargo run --package xtask -- test
```
