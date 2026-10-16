//! CobaltFS on-disk structures.
//!
//! All structs mirror the ext2 binary layout exactly.  Fields are read and
//! written through explicit little-endian byte helpers to avoid packed-struct
//! alignment UB and to make every offset deliberate and auditable.

use alloc::string::String;

// ── Magic ────────────────────────────────────────────────────────────────────

/// ext2 superblock magic number — must be present at byte offset 56 of the
/// superblock to confirm this is a valid CobaltFS / ext2 volume.
pub const COBALTFS_MAGIC: u16 = 0xEF53;

/// Inode number of the root directory (fixed by the ext2 spec).
pub const ROOT_INO: u64 = 2;

// ── i_mode type bits ─────────────────────────────────────────────────────────

pub const S_IFMT: u16   = 0xF000;
pub const S_IFLNK: u16  = 0xA000;
pub const S_IFREG: u16  = 0x8000;
pub const S_IFDIR: u16  = 0x4000;
pub const S_IFCHR: u16  = 0x2000;

// ── Directory entry file_type field ──────────────────────────────────────────

pub const FT_UNKNOWN:  u8 = 0;
pub const FT_REG_FILE: u8 = 1;
pub const FT_DIR:      u8 = 2;
pub const FT_CHRDEV:   u8 = 3;

// ── Little-endian byte helpers ────────────────────────────────────────────────

#[inline] pub fn r16(b: &[u8], o: usize) -> u16 { u16::from_le_bytes([b[o], b[o+1]]) }
#[inline] pub fn r32(b: &[u8], o: usize) -> u32 { u32::from_le_bytes([b[o], b[o+1], b[o+2], b[o+3]]) }
#[inline] pub fn w16(b: &mut [u8], o: usize, v: u16) { let x = v.to_le_bytes(); b[o]=x[0]; b[o+1]=x[1]; }
#[inline] pub fn w32(b: &mut [u8], o: usize, v: u32) { let x = v.to_le_bytes(); b[o]=x[0]; b[o+1]=x[1]; b[o+2]=x[2]; b[o+3]=x[3]; }

// ── Superblock ────────────────────────────────────────────────────────────────

/// Parsed representation of the ext2 superblock.
///
/// The raw superblock lives at byte offset 1024 on disk and is 1024 bytes.
/// Only fields used by CobaltFS are decoded here.
#[derive(Debug, Clone)]
pub struct Superblock {
    pub inodes_count:      u32,
    pub blocks_count:      u32,
    pub free_blocks_count: u32,
    pub free_inodes_count: u32,
    /// First block that contains data (0 for block_size > 1 KiB, 1 otherwise).
    pub first_data_block:  u32,
    /// `block_size = 1024 << log_block_size`
    pub log_block_size:    u32,
    pub blocks_per_group:  u32,
    pub inodes_per_group:  u32,
    pub rev_level:         u32,
    /// First non-reserved inode (11 for rev 0, variable for rev ≥ 1).
    pub first_ino:         u32,
    /// Size of a single on-disk inode structure in bytes (128 for rev 0).
    pub inode_size:        u16,
}

impl Superblock {
    /// Byte offset of the superblock from the start of the device.
    pub const DISK_OFFSET: u64  = 1024;
    /// Total size of the superblock on disk.
    pub const DISK_SIZE:   usize = 1024;

    /// Parse a superblock from a 1024-byte buffer.
    /// Returns `None` if the magic number is wrong.
    pub fn from_bytes(b: &[u8]) -> Option<Self> {
        if b.len() < 90 { return None; }
        if r16(b, 56) != COBALTFS_MAGIC { return None; }

        let rev_level = r32(b, 76);
        let (first_ino, inode_size) = if rev_level >= 1 {
            (r32(b, 84), r16(b, 88))
        } else {
            (11, 128)
        };

        Some(Self {
            inodes_count:      r32(b,  0),
            blocks_count:      r32(b,  4),
            free_blocks_count: r32(b, 12),
            free_inodes_count: r32(b, 16),
            first_data_block:  r32(b, 20),
            log_block_size:    r32(b, 24),
            blocks_per_group:  r32(b, 32),
            inodes_per_group:  r32(b, 40),
            rev_level,
            first_ino,
            inode_size,
        })
    }

    /// Filesystem block size in bytes.
    #[inline] pub fn block_size(&self) -> usize { 1024 << self.log_block_size }

    /// Number of block groups on the volume.
    pub fn num_block_groups(&self) -> usize {
        (self.blocks_count as usize).div_ceil(self.blocks_per_group as usize)
    }
}

// ── Block Group Descriptor ────────────────────────────────────────────────────

/// One 32-byte entry in the Block Group Descriptor table.
#[derive(Debug, Clone)]
pub struct BlockGroupDescriptor {
    /// Block number of this group's block-usage bitmap.
    pub block_bitmap:       u32,
    /// Block number of this group's inode-usage bitmap.
    pub inode_bitmap:       u32,
    /// Block number of the first block of this group's inode table.
    pub inode_table:        u32,
    pub free_blocks_count:  u16,
    pub free_inodes_count:  u16,
    pub used_dirs_count:    u16,
}

impl BlockGroupDescriptor {
    pub const DISK_SIZE: usize = 32;

    pub fn from_bytes(b: &[u8]) -> Self {
        Self {
            block_bitmap:      r32(b,  0),
            inode_bitmap:      r32(b,  4),
            inode_table:       r32(b,  8),
            free_blocks_count: r16(b, 12),
            free_inodes_count: r16(b, 14),
            used_dirs_count:   r16(b, 16),
        }
    }

    pub fn to_bytes(&self, b: &mut [u8]) {
        w32(b,  0, self.block_bitmap);
        w32(b,  4, self.inode_bitmap);
        w32(b,  8, self.inode_table);
        w16(b, 12, self.free_blocks_count);
        w16(b, 14, self.free_inodes_count);
        w16(b, 16, self.used_dirs_count);
    }
}

// ── Inode ─────────────────────────────────────────────────────────────────────

/// Parsed representation of an ext2 inode (128-byte standard layout).
///
/// `i_block[0..12]`  – direct block pointers  
/// `i_block[12]`     – single-indirect pointer  
/// `i_block[13]`     – double-indirect pointer  
/// `i_block[14]`     – triple-indirect pointer (reserved, not yet used)
#[derive(Debug, Clone)]
pub struct DiskInode {
    pub mode:        u16,
    pub uid:         u16,
    /// Lower 32 bits of file size.  Upper 32 bits are in `dir_acl` for
    /// regular files (large-file feature).
    pub size_lo:     u32,
    pub atime:       u32,
    pub ctime:       u32,
    pub mtime:       u32,
    pub dtime:       u32,
    pub gid:         u16,
    pub links_count: u16,
    /// Block count in 512-byte units (regardless of filesystem block size).
    pub blocks_512:  u32,
    pub flags:       u32,
    pub block:       [u32; 15],
    pub file_acl:    u32,
    /// For regular files: upper 32 bits of file size.
    /// For directories:   directory ACL (unused by CobaltFS).
    pub dir_acl:     u32,
}

impl DiskInode {
    /// Minimum on-disk inode size (ext2 rev 0).
    pub const MIN_SIZE: usize = 128;

    pub fn from_bytes(b: &[u8]) -> Self {
        let mut block = [0u32; 15];
        for i in 0..15 { block[i] = r32(b, 40 + i * 4); }
        Self {
            mode:        r16(b,   0),
            uid:         r16(b,   2),
            size_lo:     r32(b,   4),
            atime:       r32(b,   8),
            ctime:       r32(b,  12),
            mtime:       r32(b,  16),
            dtime:       r32(b,  20),
            gid:         r16(b,  24),
            links_count: r16(b,  26),
            blocks_512:  r32(b,  28),
            flags:       r32(b,  32),
            block,
            file_acl:    r32(b, 104),
            dir_acl:     r32(b, 108),
        }
    }

    pub fn to_bytes(&self, b: &mut [u8]) {
        w16(b,   0, self.mode);
        w16(b,   2, self.uid);
        w32(b,   4, self.size_lo);
        w32(b,   8, self.atime);
        w32(b,  12, self.ctime);
        w32(b,  16, self.mtime);
        w32(b,  20, self.dtime);
        w16(b,  24, self.gid);
        w16(b,  26, self.links_count);
        w32(b,  28, self.blocks_512);
        w32(b,  32, self.flags);
        for i in 0..15 { w32(b, 40 + i * 4, self.block[i]); }
        w32(b, 104, self.file_acl);
        w32(b, 108, self.dir_acl);
    }

    #[inline] pub fn is_dir(&self)     -> bool { self.mode & S_IFMT == S_IFDIR }
    #[inline] pub fn is_file(&self)    -> bool { self.mode & S_IFMT == S_IFREG }
    #[inline] pub fn is_chardev(&self) -> bool { self.mode & S_IFMT == S_IFCHR }
    #[inline] pub fn is_symlink(&self) -> bool { self.mode & S_IFMT == S_IFLNK }

    /// Full 64-bit file size (combines `size_lo` and `dir_acl` for regular files).
    pub fn file_size(&self) -> u64 {
        if self.is_file() {
            self.size_lo as u64 | ((self.dir_acl as u64) << 32)
        } else {
            self.size_lo as u64
        }
    }

    /// Set the 64-bit file size, updating `size_lo` and `dir_acl` appropriately.
    pub fn set_file_size(&mut self, size: u64) {
        self.size_lo = size as u32;
        if self.is_file() {
            self.dir_acl = (size >> 32) as u32;
        }
    }
}

// ── Directory entry ───────────────────────────────────────────────────────────

/// A single directory entry parsed from an on-disk block.
///
/// On disk, entries are variable-length and packed end-to-end within a block.
/// `rec_len` includes padding to the next 4-byte boundary; the last entry in
/// a block has `rec_len` extended to cover the rest of the block.
#[derive(Debug, Clone)]
pub struct DirEntry {
    pub inode:     u32,
    pub rec_len:   u16,
    pub name_len:  u8,
    pub file_type: u8,
    pub name:      String,
}

impl DirEntry {
    /// Fixed header bytes before the name field.
    pub const HEADER: usize = 8;

    /// Compute the on-disk `rec_len` for a name of `name_len` bytes,
    /// rounded up to the nearest 4-byte boundary.
    #[inline]
    pub fn rec_len_for(name_len: usize) -> u16 {
        ((Self::HEADER + name_len + 3) & !3) as u16
    }

    /// Parse one entry from the start of `b`.  Returns `None` if the buffer
    /// is too small or the record is structurally invalid.
    pub fn from_bytes(b: &[u8]) -> Option<Self> {
        if b.len() < Self::HEADER { return None; }
        let rec_len  = r16(b, 4);
        let name_len = b[6] as usize;
        if (rec_len as usize) < Self::HEADER + name_len { return None; }
        let name = core::str::from_utf8(&b[8..8 + name_len]).ok()?;
        Some(Self {
            inode:     r32(b, 0),
            rec_len,
            name_len:  b[6],
            file_type: b[7],
            name:      String::from(name),
        })
    }
}
