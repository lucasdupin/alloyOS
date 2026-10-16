<p align="center">
  <img src="../../../../../assets/logo.svg" alt="alloyOS Logo" width="560" />
</p>

# CobaltFS

CobaltFS is the primary on-disk filesystem for **alloyOS** — named after the
cobalt magnetic recording layer on hard drive platters that physically holds
persisted bits.

It is based on the
[ext2 filesystem specification](https://www.kernel.org/doc/html/latest/filesystems/ext2.html),
sharing its exact on-disk binary format and therefore compatible with standard
host tooling (`mkfs.ext2`, `e2fsck`, `debugfs`, `mount -o loop`).

## Why ext2?

ext2 is the simplest production-grade filesystem that provides full Unix
semantics — inodes, hard links, permissions, ownership, and symbolic links —
without journaling overhead. Its straightforward block-group layout and
well-documented on-disk format make it ideal for a kernel written from scratch.

CobaltFS implements the ext2 structures directly in Rust with no external
crates, giving alloyOS complete control over its storage layer.

## On-Disk Layout

```
Byte 0       Byte 1024    Block 1 (or 2)    Per-group blocks …
┌────────┬───────────┬───────────────────┬──────────────────────────────────┐
│  Boot  │ Superblock│  BGD Table        │  [Bitmap | Bitmap | Inode Table  │
│  block │  (1 KiB)  │  (32 B × groups)  │   block     inode   | Data …]   │
└────────┴───────────┴───────────────────┴──────────────────────────────────┘
```

- **Superblock** – filesystem-wide metadata at byte offset 1024.
- **Block Group Descriptor (BGD) Table** – one 32-byte entry per block group,
  immediately after the superblock block.
- **Block bitmap** – one bit per block in the group (used/free).
- **Inode bitmap** – one bit per inode slot in the group (used/free).
- **Inode table** – array of 128-byte (or larger) inode structs.
- **Data blocks** – file and directory content.

## Block Pointer Structure

Each inode carries 15 block pointers (`i_block[0..15]`):

| Pointer     | Covers                         |
|-------------|-------------------------------|
| `[0..11]`   | 12 direct blocks               |
| `[12]`      | Single-indirect (256–1 024 blocks depending on block size) |
| `[13]`      | Double-indirect                |
| `[14]`      | Triple-indirect *(not yet implemented)* |

## Creating a Compatible Disk Image

```bash
# 64 MiB image with 4 KiB blocks
dd if=/dev/zero of=disk.img bs=1M count=64
mkfs.ext2 -b 4096 disk.img

# Populate from Linux host
sudo mount -o loop disk.img /mnt
sudo cp target/x86_64-unknown-none/release/init /mnt/
sudo umount /mnt
```

## Running under QEMU

Add a raw IDE drive to your QEMU invocation:

```
-drive file=disk.img,format=raw,if=ide
```

The ATA PIO driver reads this as a standard 512-byte-sector device.

## Future Work

- Triple-indirect block pointer support (files > ~4 GiB)
- `rename` / `link` / `symlink` VFS syscalls
- Directory entry htree indexing (ext3/4 feature, backward compatible)
