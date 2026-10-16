//! CobaltFS — the alloyOS filesystem, based on ext2.
//!
//! See [`README.md`](README.md) for an overview.
//!
//! # Architecture
//!
//! [`CobaltFS`] implements the [`FileSystem`] trait and is plugged into the
//! kernel VFS at mount time.  It delegates all I/O to a [`BlockDevice`]
//! implementation (ATA PIO, virtio-blk, …).
//!
//! Block-group descriptors are cached in memory after mount; inode data is
//! always read from / written to disk on demand.

pub mod disk;

use alloc::string::{String, ToString};
use alloc::sync::Arc;
use alloc::vec;
use alloc::vec::Vec;
use spin::Mutex;

use crate::block::BlockDevice;
use crate::vfs::{FileSystem, FsError, Inode, InodeKind};
use disk::{
    BlockGroupDescriptor, DiskInode, FT_CHRDEV, FT_DIR, FT_REG_FILE,
    ROOT_INO, S_IFCHR, S_IFDIR, S_IFREG, Superblock, r32, w32, w16,
    DirEntry,
};

// ── CobaltFS ──────────────────────────────────────────────────────────────────

/// CobaltFS filesystem instance.
///
/// Created by [`CobaltFS::mount`] and registered with the kernel VFS.  All
/// methods take `&self` (the [`FileSystem`] trait contract); interior
/// mutability via [`Mutex`] guards the cached BGD table.
pub struct CobaltFS {
    device:           Arc<dyn BlockDevice>,
    /// Filesystem block size derived from the superblock (1024/2048/4096 …).
    block_size:       usize,
    blocks_per_group: u32,
    inodes_per_group: u32,
    /// On-disk inode record size (128 bytes for rev 0, larger for rev ≥ 1).
    inode_size:       usize,
    /// First filesystem block (0 for block_size > 1 KiB, 1 for 1 KiB).
    first_data_block: u32,
    num_block_groups: usize,
    /// In-memory cache of block group descriptors; kept in sync with disk.
    bgds:             Mutex<Vec<BlockGroupDescriptor>>,
}

impl CobaltFS {
    /// Mount a CobaltFS volume from `device`.
    ///
    /// Reads and validates the superblock, then loads all block group
    /// descriptors into memory.  Returns [`FsError::InvalidInput`] if the
    /// magic number is missing or the geometry looks wrong.
    pub fn mount(device: Arc<dyn BlockDevice>) -> Result<Self, FsError> {
        // Read superblock (always at byte 1024).
        let mut sb_buf = vec![0u8; Superblock::DISK_SIZE];
        Self::dev_read_bytes(&*device, Superblock::DISK_OFFSET, &mut sb_buf)?;

        let sb = Superblock::from_bytes(&sb_buf).ok_or(FsError::InvalidInput)?;
        let block_size    = sb.block_size();
        let num_groups    = sb.num_block_groups();
        let inode_size    = core::cmp::max(sb.inode_size as usize, DiskInode::MIN_SIZE);

        // BGD table starts at the block immediately after the superblock block.
        let bgd_block      = sb.first_data_block + 1;
        let bgd_byte_start = bgd_block as u64 * block_size as u64;
        let bgd_total      = num_groups * BlockGroupDescriptor::DISK_SIZE;

        let mut bgd_buf = vec![0u8; bgd_total];
        Self::dev_read_bytes(&*device, bgd_byte_start, &mut bgd_buf)?;

        let mut bgds = Vec::with_capacity(num_groups);
        for i in 0..num_groups {
            let off = i * BlockGroupDescriptor::DISK_SIZE;
            bgds.push(BlockGroupDescriptor::from_bytes(
                &bgd_buf[off..off + BlockGroupDescriptor::DISK_SIZE],
            ));
        }

        Ok(Self {
            device,
            block_size,
            blocks_per_group: sb.blocks_per_group,
            inodes_per_group: sb.inodes_per_group,
            inode_size,
            first_data_block: sb.first_data_block,
            num_block_groups: num_groups,
            bgds: Mutex::new(bgds),
        })
    }

    // ── Low-level device I/O ─────────────────────────────────────────────────

    /// Read `buf.len()` bytes from the device starting at `byte_offset`.
    /// Handles crossing device-block boundaries.
    fn dev_read_bytes(dev: &dyn BlockDevice, byte_offset: u64, buf: &mut [u8]) -> Result<(), FsError> {
        let dev_bs    = dev.block_size();
        let mut lba   = byte_offset / dev_bs as u64;
        let mut inner = (byte_offset % dev_bs as u64) as usize;
        let mut done  = 0usize;
        let mut sector = vec![0u8; dev_bs];

        while done < buf.len() {
            dev.read_block(lba, &mut sector).map_err(|_| FsError::IoError)?;
            let chunk = core::cmp::min(buf.len() - done, dev_bs - inner);
            buf[done..done + chunk].copy_from_slice(&sector[inner..inner + chunk]);
            done  += chunk;
            inner  = 0;
            lba   += 1;
        }
        Ok(())
    }

    /// Write `buf.len()` bytes to the device at `byte_offset`.
    /// Uses read-modify-write for unaligned or partial sector writes.
    fn dev_write_bytes(dev: &dyn BlockDevice, byte_offset: u64, buf: &[u8]) -> Result<(), FsError> {
        let dev_bs    = dev.block_size();
        let mut lba   = byte_offset / dev_bs as u64;
        let mut inner = (byte_offset % dev_bs as u64) as usize;
        let mut done  = 0usize;
        let mut sector = vec![0u8; dev_bs];

        while done < buf.len() {
            let chunk = core::cmp::min(buf.len() - done, dev_bs - inner);
            // Only read-modify-write if we are not writing a full sector.
            if inner != 0 || chunk < dev_bs {
                dev.read_block(lba, &mut sector).map_err(|_| FsError::IoError)?;
            }
            sector[inner..inner + chunk].copy_from_slice(&buf[done..done + chunk]);
            dev.write_block(lba, &sector).map_err(|_| FsError::IoError)?;
            done  += chunk;
            inner  = 0;
            lba   += 1;
        }
        Ok(())
    }

    fn read_bytes(&self, byte_offset: u64, buf: &mut [u8]) -> Result<(), FsError> {
        Self::dev_read_bytes(&*self.device, byte_offset, buf)
    }

    fn write_bytes(&self, byte_offset: u64, buf: &[u8]) -> Result<(), FsError> {
        Self::dev_write_bytes(&*self.device, byte_offset, buf)
    }

    // ── Block helpers ─────────────────────────────────────────────────────────

    #[inline]
    fn block_byte_offset(&self, block: u32) -> u64 {
        block as u64 * self.block_size as u64
    }

    fn read_fs_block(&self, block: u32, buf: &mut [u8]) -> Result<(), FsError> {
        debug_assert_eq!(buf.len(), self.block_size);
        self.read_bytes(self.block_byte_offset(block), buf)
    }

    fn write_fs_block(&self, block: u32, buf: &[u8]) -> Result<(), FsError> {
        debug_assert_eq!(buf.len(), self.block_size);
        self.write_bytes(self.block_byte_offset(block), buf)
    }

    // ── BGD helpers ───────────────────────────────────────────────────────────

    fn bgd_disk_offset(&self, group: usize) -> u64 {
        let bgd_block = self.first_data_block + 1;
        bgd_block as u64 * self.block_size as u64
            + group as u64 * BlockGroupDescriptor::DISK_SIZE as u64
    }

    fn write_bgd(&self, group: usize) -> Result<(), FsError> {
        let bgds = self.bgds.lock();
        let mut buf = vec![0u8; BlockGroupDescriptor::DISK_SIZE];
        bgds[group].to_bytes(&mut buf);
        drop(bgds);
        self.write_bytes(self.bgd_disk_offset(group), &buf)
    }

    // ── Inode helpers ─────────────────────────────────────────────────────────

    fn inode_disk_offset(&self, ino: u64) -> Result<u64, FsError> {
        if ino == 0 { return Err(FsError::NotFound); }
        let group     = ((ino - 1) / self.inodes_per_group as u64) as usize;
        let local_idx = (ino - 1) % self.inodes_per_group as u64;
        if group >= self.num_block_groups { return Err(FsError::NotFound); }
        let inode_table = self.bgds.lock()[group].inode_table;
        Ok(inode_table as u64 * self.block_size as u64 + local_idx * self.inode_size as u64)
    }

    fn read_disk_inode(&self, ino: u64) -> Result<DiskInode, FsError> {
        let off = self.inode_disk_offset(ino)?;
        let mut buf = vec![0u8; self.inode_size];
        self.read_bytes(off, &mut buf)?;
        Ok(DiskInode::from_bytes(&buf))
    }

    fn write_disk_inode(&self, ino: u64, inode: &DiskInode) -> Result<(), FsError> {
        let off = self.inode_disk_offset(ino)?;
        let mut buf = vec![0u8; self.inode_size];
        inode.to_bytes(&mut buf);
        self.write_bytes(off, &buf)
    }

    // ── Block pointer resolution ──────────────────────────────────────────────

    /// Return the absolute block number for the `idx`-th data block of `inode`.
    ///
    /// Handles direct (0–11), single-indirect (12), and double-indirect (13)
    /// block pointers.  Triple-indirect (14) is not yet implemented.
    fn data_block_num(&self, inode: &DiskInode, idx: u32) -> Result<u32, FsError> {
        let ptrs = (self.block_size / 4) as u32;

        if idx < 12 {
            // Direct
            let b = inode.block[idx as usize];
            if b == 0 { return Err(FsError::NotFound); }
            Ok(b)
        } else if idx < 12 + ptrs {
            // Single-indirect
            let si = inode.block[12];
            if si == 0 { return Err(FsError::NotFound); }
            let mut buf = [0u8; 4];
            self.read_bytes(self.block_byte_offset(si) + (idx - 12) as u64 * 4, &mut buf)?;
            let b = r32(&buf, 0);
            if b == 0 { Err(FsError::NotFound) } else { Ok(b) }
        } else if idx < 12 + ptrs + ptrs * ptrs {
            // Double-indirect
            let di = inode.block[13];
            if di == 0 { return Err(FsError::NotFound); }
            let li    = idx - 12 - ptrs;
            let l1    = li / ptrs;
            let l2    = li % ptrs;
            let mut buf = [0u8; 4];
            self.read_bytes(self.block_byte_offset(di) + l1 as u64 * 4, &mut buf)?;
            let l1b = r32(&buf, 0);
            if l1b == 0 { return Err(FsError::NotFound); }
            self.read_bytes(self.block_byte_offset(l1b) + l2 as u64 * 4, &mut buf)?;
            let b = r32(&buf, 0);
            if b == 0 { Err(FsError::NotFound) } else { Ok(b) }
        } else {
            // Triple-indirect: planned, not yet implemented.
            Err(FsError::InvalidInput)
        }
    }

    /// Ensure the `idx`-th data block of the inode exists, allocating it (and
    /// any required indirect blocks) if necessary.  Updates `inode` in place
    /// and writes it to disk before returning.
    fn ensure_data_block(&self, ino: u64, inode: &mut DiskInode, idx: u32) -> Result<u32, FsError> {
        let ptrs     = (self.block_size / 4) as u32;
        let blks_512 = (self.block_size / 512) as u32;

        macro_rules! new_zeroed_block {
            () => {{
                let b = self.alloc_block()?;
                self.write_bytes(self.block_byte_offset(b), &vec![0u8; self.block_size])?;
                inode.blocks_512 += blks_512;
                b
            }};
        }

        if idx < 12 {
            if inode.block[idx as usize] == 0 {
                inode.block[idx as usize] = new_zeroed_block!();
                self.write_disk_inode(ino, inode)?;
            }
            Ok(inode.block[idx as usize])
        } else if idx < 12 + ptrs {
            if inode.block[12] == 0 {
                inode.block[12] = new_zeroed_block!();
                self.write_disk_inode(ino, inode)?;
            }
            let local  = idx - 12;
            let ptr_off = self.block_byte_offset(inode.block[12]) + local as u64 * 4;
            let mut buf = [0u8; 4];
            self.read_bytes(ptr_off, &mut buf)?;
            let existing = r32(&buf, 0);
            if existing != 0 { return Ok(existing); }
            let b = new_zeroed_block!();
            self.write_bytes(ptr_off, &b.to_le_bytes())?;
            self.write_disk_inode(ino, inode)?;
            Ok(b)
        } else if idx < 12 + ptrs + ptrs * ptrs {
            if inode.block[13] == 0 {
                inode.block[13] = new_zeroed_block!();
                self.write_disk_inode(ino, inode)?;
            }
            let li  = idx - 12 - ptrs;
            let l1  = li / ptrs;
            let l2  = li % ptrs;
            let l1_off = self.block_byte_offset(inode.block[13]) + l1 as u64 * 4;
            let mut buf = [0u8; 4];
            self.read_bytes(l1_off, &mut buf)?;
            let mut l1b = r32(&buf, 0);
            if l1b == 0 {
                l1b = new_zeroed_block!();
                self.write_bytes(l1_off, &l1b.to_le_bytes())?;
                self.write_disk_inode(ino, inode)?;
            }
            let l2_off = self.block_byte_offset(l1b) + l2 as u64 * 4;
            self.read_bytes(l2_off, &mut buf)?;
            let existing = r32(&buf, 0);
            if existing != 0 { return Ok(existing); }
            let b = new_zeroed_block!();
            self.write_bytes(l2_off, &b.to_le_bytes())?;
            self.write_disk_inode(ino, inode)?;
            Ok(b)
        } else {
            Err(FsError::InvalidInput)
        }
    }

    // ── Allocation ────────────────────────────────────────────────────────────

    /// Allocate one free data block from the first group that has space.
    /// Returns the absolute block number.
    fn alloc_block(&self) -> Result<u32, FsError> {
        for g in 0..self.num_block_groups {
            if self.bgds.lock()[g].free_blocks_count == 0 { continue; }

            let bm_block = self.bgds.lock()[g].block_bitmap;
            let mut bitmap = vec![0u8; self.block_size];
            self.read_bytes(self.block_byte_offset(bm_block), &mut bitmap)?;

            for byte in 0..self.block_size {
                if bitmap[byte] == 0xFF { continue; }
                for bit in 0..8u32 {
                    if bitmap[byte] & (1 << bit) == 0 {
                        bitmap[byte] |= 1 << bit;
                        self.write_bytes(self.block_byte_offset(bm_block), &bitmap)?;
                        {
                            let mut bgds = self.bgds.lock();
                            bgds[g].free_blocks_count -= 1;
                        }
                        self.write_bgd(g)?;
                        let abs = g as u32 * self.blocks_per_group + (byte as u32 * 8 + bit);
                        return Ok(abs);
                    }
                }
            }
        }
        Err(FsError::NoSpace)
    }

    /// Release a data block back to its group's free pool.
    fn free_block(&self, block: u32) -> Result<(), FsError> {
        let g         = (block / self.blocks_per_group) as usize;
        let local     = (block % self.blocks_per_group) as usize;
        let bm_block  = self.bgds.lock()[g].block_bitmap;
        let mut bitmap = vec![0u8; self.block_size];
        self.read_bytes(self.block_byte_offset(bm_block), &mut bitmap)?;
        bitmap[local / 8] &= !(1u8 << (local % 8));
        self.write_bytes(self.block_byte_offset(bm_block), &bitmap)?;
        {
            let mut bgds = self.bgds.lock();
            bgds[g].free_blocks_count += 1;
        }
        self.write_bgd(g)
    }

    /// Allocate one free inode from the first group that has space.
    /// Returns the absolute inode number (1-based).
    fn alloc_inode(&self, kind: InodeKind) -> Result<u64, FsError> {
        for g in 0..self.num_block_groups {
            if self.bgds.lock()[g].free_inodes_count == 0 { continue; }

            let bm_block  = self.bgds.lock()[g].inode_bitmap;
            let mut bitmap = vec![0u8; self.block_size];
            self.read_bytes(self.block_byte_offset(bm_block), &mut bitmap)?;

            let limit = self.inodes_per_group as usize;
            for byte in 0..(limit + 7) / 8 {
                if bitmap[byte] == 0xFF { continue; }
                for bit in 0..8usize {
                    let local = byte * 8 + bit;
                    if local >= limit { break; }
                    if bitmap[byte] & (1u8 << bit) == 0 {
                        bitmap[byte] |= 1u8 << bit;
                        self.write_bytes(self.block_byte_offset(bm_block), &bitmap)?;
                        {
                            let mut bgds = self.bgds.lock();
                            bgds[g].free_inodes_count -= 1;
                            if kind == InodeKind::Directory {
                                bgds[g].used_dirs_count += 1;
                            }
                        }
                        self.write_bgd(g)?;
                        let ino = g as u64 * self.inodes_per_group as u64 + local as u64 + 1;
                        return Ok(ino);
                    }
                }
            }
        }
        Err(FsError::NoSpace)
    }

    /// Release an inode slot back to its group's free pool.
    fn free_inode(&self, ino: u64, kind: InodeKind) -> Result<(), FsError> {
        let group    = ((ino - 1) / self.inodes_per_group as u64) as usize;
        let local    = ((ino - 1) % self.inodes_per_group as u64) as usize;
        let bm_block = self.bgds.lock()[group].inode_bitmap;
        let mut bitmap = vec![0u8; self.block_size];
        self.read_bytes(self.block_byte_offset(bm_block), &mut bitmap)?;
        bitmap[local / 8] &= !(1u8 << (local % 8));
        self.write_bytes(self.block_byte_offset(bm_block), &bitmap)?;
        {
            let mut bgds = self.bgds.lock();
            bgds[group].free_inodes_count += 1;
            if kind == InodeKind::Directory {
                bgds[group].used_dirs_count = bgds[group].used_dirs_count.saturating_sub(1);
            }
        }
        self.write_bgd(group)
    }

    // ── Directory operations ──────────────────────────────────────────────────

    /// Scan a directory inode and return the inode number of the entry named
    /// `name`, or [`FsError::NotFound`].
    fn dir_lookup(&self, dir_ino: u64, name: &str) -> Result<u64, FsError> {
        let inode = self.read_disk_inode(dir_ino)?;
        if !inode.is_dir() { return Err(FsError::NotADirectory); }

        let dir_size   = inode.size_lo as usize;
        let name_bytes = name.as_bytes();
        let mut offset = 0usize;

        while offset < dir_size {
            let bidx = (offset / self.block_size) as u32;
            let bnum = self.data_block_num(&inode, bidx)?;
            let mut block = vec![0u8; self.block_size];
            self.read_fs_block(bnum, &mut block)?;

            let mut pos = 0usize;
            while pos + DirEntry::HEADER <= self.block_size {
                let e_ino     = r32(&block, pos);
                let e_rec_len = r32(&block, pos + 4) as u16;
                let e_nam_len = block[pos + 6] as usize;
                let rec_len   = e_rec_len as usize;
                if rec_len == 0 { break; }

                if e_ino != 0
                    && e_nam_len == name_bytes.len()
                    && pos + DirEntry::HEADER + e_nam_len <= self.block_size
                    && &block[pos + DirEntry::HEADER..pos + DirEntry::HEADER + e_nam_len] == name_bytes
                {
                    return Ok(e_ino as u64);
                }
                pos += rec_len;
            }
            offset += self.block_size;
        }
        Err(FsError::NotFound)
    }

    /// Add a new directory entry `name` → `child_ino` into `dir_ino`.
    fn dir_add_entry(&self, dir_ino: u64, name: &str, child_ino: u64, file_type: u8) -> Result<(), FsError> {
        let mut inode  = self.read_disk_inode(dir_ino)?;
        let needed     = DirEntry::rec_len_for(name.len()) as usize;
        let name_bytes = name.as_bytes();
        let dir_size   = inode.size_lo as usize;
        let mut offset = 0usize;

        while offset < dir_size {
            let bidx = (offset / self.block_size) as u32;
            let bnum = self.data_block_num(&inode, bidx)?;
            let boff = self.block_byte_offset(bnum);
            let mut block = vec![0u8; self.block_size];
            self.read_fs_block(bnum, &mut block)?;

            let mut pos = 0usize;
            while pos + DirEntry::HEADER <= self.block_size {
                let e_ino     = r32(&block, pos);
                let rec_len   = r32(&block, pos + 4) as u16 as usize;
                let e_nam_len = block[pos + 6] as usize;
                if rec_len == 0 { break; }

                let actual = DirEntry::rec_len_for(e_nam_len) as usize;
                let spare  = if e_ino == 0 { rec_len } else { rec_len - actual };

                if spare >= needed {
                    if e_ino != 0 {
                        // Shrink the existing entry to its real size so we can
                        // place the new one immediately after.
                        w16(&mut block, pos + 4, actual as u16);
                        pos += actual;
                    }
                    // Write new entry spanning the available space.
                    w32(&mut block, pos,     child_ino as u32);
                    w16(&mut block, pos + 4, spare as u16);
                    block[pos + 6] = name_bytes.len() as u8;
                    block[pos + 7] = file_type;
                    block[pos + DirEntry::HEADER..pos + DirEntry::HEADER + name_bytes.len()]
                        .copy_from_slice(name_bytes);
                    self.write_bytes(boff, &block)?;
                    return Ok(());
                }
                pos += rec_len;
            }
            offset += self.block_size;
        }

        // No space in existing blocks — allocate a new directory block.
        let new_bidx  = (inode.size_lo / self.block_size as u32) as u32;
        let new_bnum  = self.ensure_data_block(dir_ino, &mut inode, new_bidx)?;
        let mut block = vec![0u8; self.block_size];
        w32(&mut block, 0, child_ino as u32);
        w16(&mut block, 4, self.block_size as u16);
        block[6] = name_bytes.len() as u8;
        block[7] = file_type;
        block[DirEntry::HEADER..DirEntry::HEADER + name_bytes.len()].copy_from_slice(name_bytes);
        self.write_fs_block(new_bnum, &block)?;
        inode.size_lo += self.block_size as u32;
        self.write_disk_inode(dir_ino, &inode)
    }

    /// Remove the directory entry `name` from `dir_ino`.
    fn dir_remove_entry(&self, dir_ino: u64, name: &str) -> Result<(), FsError> {
        let inode      = self.read_disk_inode(dir_ino)?;
        let name_bytes = name.as_bytes();
        let dir_size   = inode.size_lo as usize;
        let mut offset = 0usize;

        while offset < dir_size {
            let bidx = (offset / self.block_size) as u32;
            let bnum = self.data_block_num(&inode, bidx)?;
            let boff = self.block_byte_offset(bnum);
            let mut block = vec![0u8; self.block_size];
            self.read_fs_block(bnum, &mut block)?;

            let mut pos      = 0usize;
            let mut prev_pos: Option<usize> = None;

            while pos + DirEntry::HEADER <= self.block_size {
                let e_ino     = r32(&block, pos);
                let rec_len   = r32(&block, pos + 4) as u16 as usize;
                let e_nam_len = block[pos + 6] as usize;
                if rec_len == 0 { break; }

                if e_ino != 0
                    && e_nam_len == name_bytes.len()
                    && pos + DirEntry::HEADER + e_nam_len <= self.block_size
                    && &block[pos + DirEntry::HEADER..pos + DirEntry::HEADER + e_nam_len] == name_bytes
                {
                    match prev_pos {
                        Some(p) => {
                            // Merge this record's space into the previous entry.
                            let prev_rec = r32(&block, p + 4) as u16 as usize;
                            w16(&mut block, p + 4, (prev_rec + rec_len) as u16);
                        }
                        None => {
                            // First entry in block — zero the inode number only.
                            w32(&mut block, pos, 0);
                        }
                    }
                    self.write_bytes(boff, &block)?;
                    return Ok(());
                }

                prev_pos = Some(pos);
                pos += rec_len;
            }
            offset += self.block_size;
        }
        Err(FsError::NotFound)
    }

    // ── Path resolution ───────────────────────────────────────────────────────

    fn normalize(path: &str) -> String {
        let trimmed = path.trim();
        if trimmed.is_empty() || trimmed == "/" { return String::from("/"); }
        let mut parts: Vec<&str> = Vec::new();
        for seg in trimmed.split('/') {
            match seg {
                "" | "." => {}
                ".."     => { parts.pop(); }
                s        => parts.push(s),
            }
        }
        let mut out = String::new();
        for p in &parts { out.push('/'); out.push_str(p); }
        if out.is_empty() { String::from("/") } else { out }
    }

    fn parent_and_name(norm: &str) -> (String, String) {
        match norm.rfind('/') {
            Some(0) | None => (String::from("/"), String::from(&norm[1..])),
            Some(i)        => (String::from(&norm[..i]), String::from(&norm[i+1..])),
        }
    }

    fn resolve_path(&self, path: &str) -> Result<(u64, DiskInode), FsError> {
        let norm = Self::normalize(path);
        if norm == "/" {
            let inode = self.read_disk_inode(ROOT_INO)?;
            return Ok((ROOT_INO, inode));
        }

        let mut cur = ROOT_INO;
        for component in norm.trim_start_matches('/').split('/') {
            cur = self.dir_lookup(cur, component)?;
        }
        let inode = self.read_disk_inode(cur)?;
        Ok((cur, inode))
    }
}

// ── FileSystem trait ──────────────────────────────────────────────────────────

impl FileSystem for CobaltFS {
    fn lookup(&self, path: &str) -> Result<Inode, FsError> {
        let (ino, di) = self.resolve_path(path)?;
        let kind = if di.is_dir()     { InodeKind::Directory }
                   else if di.is_chardev() { InodeKind::CharDevice }
                   else               { InodeKind::File };
        Ok(Inode { ino, kind, size: di.file_size() as usize })
    }

    fn create(&self, path: &str, kind: InodeKind) -> Result<Inode, FsError> {
        let norm = Self::normalize(path);
        if self.resolve_path(&norm).is_ok() { return Err(FsError::AlreadyExists); }

        let (parent_path, name) = Self::parent_and_name(&norm);
        let (parent_ino, _)    = self.resolve_path(&parent_path)?;

        let mode = match kind {
            InodeKind::File      => S_IFREG | 0o644,
            InodeKind::Directory => S_IFDIR | 0o755,
            InodeKind::CharDevice => S_IFCHR | 0o666,
        };
        let links = if kind == InodeKind::Directory { 2 } else { 1 };

        let new_ino = self.alloc_inode(kind)?;
        let new_di  = DiskInode {
            mode,
            uid: 0, gid: 0,
            size_lo: 0,
            atime: 0, ctime: 0, mtime: 0, dtime: 0,
            links_count: links,
            blocks_512: 0,
            flags: 0,
            block: [0u32; 15],
            file_acl: 0,
            dir_acl: 0,
        };
        self.write_disk_inode(new_ino, &new_di)?;

        if kind == InodeKind::Directory {
            self.dir_add_entry(new_ino, ".",  new_ino,    FT_DIR)?;
            self.dir_add_entry(new_ino, "..", parent_ino, FT_DIR)?;
        }

        let ft = match kind {
            InodeKind::File       => FT_REG_FILE,
            InodeKind::Directory  => FT_DIR,
            InodeKind::CharDevice => FT_CHRDEV,
        };
        self.dir_add_entry(parent_ino, &name, new_ino, ft)?;

        Ok(Inode { ino: new_ino, kind, size: 0 })
    }

    fn read(&self, ino: u64, offset: usize, buf: &mut [u8]) -> Result<usize, FsError> {
        let inode = self.read_disk_inode(ino)?;
        if inode.is_dir() { return Err(FsError::IsADirectory); }

        let file_size = inode.file_size() as usize;
        if offset >= file_size { return Ok(0); }

        let to_read   = core::cmp::min(buf.len(), file_size - offset);
        let mut done  = 0usize;

        while done < to_read {
            let pos   = offset + done;
            let bidx  = (pos / self.block_size) as u32;
            let boff  = pos % self.block_size;
            let bnum  = self.data_block_num(&inode, bidx)?;
            let chunk = core::cmp::min(to_read - done, self.block_size - boff);

            self.read_bytes(
                self.block_byte_offset(bnum) + boff as u64,
                &mut buf[done..done + chunk],
            )?;
            done += chunk;
        }
        Ok(done)
    }

    fn write(&self, ino: u64, offset: usize, buf: &[u8]) -> Result<usize, FsError> {
        let mut inode = self.read_disk_inode(ino)?;
        if inode.is_dir() { return Err(FsError::IsADirectory); }

        let mut done = 0usize;
        while done < buf.len() {
            let pos   = offset + done;
            let bidx  = (pos / self.block_size) as u32;
            let boff  = pos % self.block_size;
            let bnum  = self.ensure_data_block(ino, &mut inode, bidx)?;
            let chunk = core::cmp::min(buf.len() - done, self.block_size - boff);

            self.write_bytes(
                self.block_byte_offset(bnum) + boff as u64,
                &buf[done..done + chunk],
            )?;
            done += chunk;
        }

        let new_size = core::cmp::max(inode.file_size(), (offset + buf.len()) as u64);
        inode.set_file_size(new_size);
        self.write_disk_inode(ino, &inode)?;
        Ok(done)
    }

    fn readdir(&self, ino: u64) -> Result<Vec<String>, FsError> {
        let inode = self.read_disk_inode(ino)?;
        if !inode.is_dir() { return Err(FsError::NotADirectory); }

        let dir_size   = inode.size_lo as usize;
        let mut names  = Vec::new();
        let mut offset = 0usize;

        while offset < dir_size {
            let bidx = (offset / self.block_size) as u32;
            let bnum = match self.data_block_num(&inode, bidx) {
                Ok(b)  => b,
                Err(_) => break,
            };
            let mut block = vec![0u8; self.block_size];
            self.read_fs_block(bnum, &mut block)?;

            let mut pos = 0usize;
            while pos + DirEntry::HEADER <= self.block_size {
                let e_ino     = r32(&block, pos);
                let rec_len   = r32(&block, pos + 4) as u16 as usize;
                let e_nam_len = block[pos + 6] as usize;
                if rec_len == 0 { break; }

                if e_ino != 0 && pos + DirEntry::HEADER + e_nam_len <= self.block_size {
                    if let Ok(n) = core::str::from_utf8(
                        &block[pos + DirEntry::HEADER..pos + DirEntry::HEADER + e_nam_len],
                    ) {
                        if n != "." && n != ".." {
                            names.push(n.to_string());
                        }
                    }
                }
                pos += rec_len;
            }
            offset += self.block_size;
        }
        Ok(names)
    }

    fn unlink(&self, path: &str) -> Result<(), FsError> {
        let norm              = Self::normalize(path);
        let (ino, mut inode)  = self.resolve_path(&norm)?;
        let (parent_path, name) = Self::parent_and_name(&norm);
        let (parent_ino, _)   = self.resolve_path(&parent_path)?;

        self.dir_remove_entry(parent_ino, &name)?;

        inode.links_count = inode.links_count.saturating_sub(1);

        if inode.links_count == 0 {
            // Determine kind for BGD accounting.
            let kind = if inode.is_dir() { InodeKind::Directory } else { InodeKind::File };

            // Free direct data blocks.
            for i in 0..12 {
                if inode.block[i] != 0 { self.free_block(inode.block[i])?; }
            }
            // Free single-indirect pointer block and its targets.
            if inode.block[12] != 0 {
                let mut buf = vec![0u8; self.block_size];
                self.read_fs_block(inode.block[12], &mut buf)?;
                for i in 0..self.block_size / 4 {
                    let b = r32(&buf, i * 4);
                    if b != 0 { self.free_block(b)?; }
                }
                self.free_block(inode.block[12])?;
            }
            // Double-indirect: free L2 blocks then the L1 table then the index.
            if inode.block[13] != 0 {
                let mut l1 = vec![0u8; self.block_size];
                self.read_fs_block(inode.block[13], &mut l1)?;
                for i in 0..self.block_size / 4 {
                    let l1b = r32(&l1, i * 4);
                    if l1b == 0 { continue; }
                    let mut l2 = vec![0u8; self.block_size];
                    self.read_fs_block(l1b, &mut l2)?;
                    for j in 0..self.block_size / 4 {
                        let b = r32(&l2, j * 4);
                        if b != 0 { self.free_block(b)?; }
                    }
                    self.free_block(l1b)?;
                }
                self.free_block(inode.block[13])?;
            }

            inode.dtime = 0;   // Would hold deletion timestamp on real hardware.
            self.write_disk_inode(ino, &inode)?;
            self.free_inode(ino, kind)?;
        } else {
            self.write_disk_inode(ino, &inode)?;
        }
        Ok(())
    }
}
