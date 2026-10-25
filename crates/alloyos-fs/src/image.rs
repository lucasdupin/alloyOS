//! Development-volume formatting shared by host packaging tools.
use alloc::{vec, vec::Vec};

/// Create the 512 MiB, 4 KiB-block development CobaltFS image (4 block groups).
/// This provides ample space for the full native WPE WebKit browser engine and Slint UI.
pub fn development_image() -> Vec<u8> {
    let bs = 4096usize;
    let blocks_per_group = 32768u32;
    let num_groups = 4u32;
    let total_blocks = blocks_per_group * num_groups; // 512 MiB
    let inodes_per_group = 256u32;
    let inodes_count = inodes_per_group * num_groups;
    let mut img = vec![0u8; bs * total_blocks as usize];

    let w16 = |b: &mut [u8], o: usize, v: u16| {
        let x = v.to_le_bytes();
        b[o] = x[0];
        b[o + 1] = x[1];
    };
    let w32 = |b: &mut [u8], o: usize, v: u32| {
        let x = v.to_le_bytes();
        b[o..o + 4].copy_from_slice(&x);
    };

    // Reserved blocks in group 0:
    // block 0: boot / superblock
    // block 1: block group descriptor table
    // block 2: block bitmap (group 0)
    // block 3: inode bitmap (group 0)
    // blocks 4..12: inode table (group 0) (8 blocks = 256 inodes * 128 bytes)
    // block 12: root directory data block
    let reserved_g0 = 13u32;

    // Reserved blocks in subsequent groups g > 0:
    // block g_base + 0: block bitmap
    // block g_base + 1: inode bitmap
    // blocks g_base + 2 .. g_base + 10: inode table (8 blocks)
    let reserved_other = 10u32;

    let free_blocks_total = (blocks_per_group - reserved_g0) + (num_groups - 1) * (blocks_per_group - reserved_other);
    let free_inodes_total = (inodes_per_group - 11) + (num_groups - 1) * inodes_per_group;

    // Superblock is always at byte offset 1024.
    {
        let sb = &mut img[1024..2048];
        w32(sb, 0, inodes_count);
        w32(sb, 4, total_blocks);
        w32(sb, 12, free_blocks_total);
        w32(sb, 16, free_inodes_total);
        w32(sb, 20, 0); // first_data_block for 4 KiB blocks
        w32(sb, 24, 2); // 4096-byte blocks
        w32(sb, 32, blocks_per_group);
        w32(sb, 40, inodes_per_group);
        w16(sb, 56, 0xEF53); // COBALTFS_MAGIC
        w32(sb, 76, 0); // rev 0
    }

    // Block group descriptor table at block 1.
    for g in 0..num_groups as usize {
        let bgd_off = bs + g * 32;
        let bgd = &mut img[bgd_off..bgd_off + 32];
        if g == 0 {
            w32(bgd, 0, 2); // block bitmap
            w32(bgd, 4, 3); // inode bitmap
            w32(bgd, 8, 4); // inode table
            w16(bgd, 12, (blocks_per_group - reserved_g0) as u16);
            w16(bgd, 14, (inodes_per_group - 11) as u16);
            w16(bgd, 16, 1); // used_dirs_count = 1 (root)
        } else {
            let base = g as u32 * blocks_per_group;
            w32(bgd, 0, base + 0); // block bitmap
            w32(bgd, 4, base + 1); // inode bitmap
            w32(bgd, 8, base + 2); // inode table
            w16(bgd, 12, (blocks_per_group - reserved_other) as u16);
            w16(bgd, 14, inodes_per_group as u16);
            w16(bgd, 16, 0);
        }
    }

    // Group 0 bitmaps:
    for b in 0..reserved_g0 as usize {
        img[2 * bs + b / 8] |= 1u8 << (b % 8);
    }
    img[3 * bs] = 0xFF;
    img[3 * bs + 1] = 0b0000_0111;

    // Subsequent groups' bitmaps:
    for g in 1..num_groups as usize {
        let base_byte = (g * blocks_per_group as usize) * bs;
        for b in 0..reserved_other as usize {
            img[base_byte + b / 8] |= 1u8 << (b % 8);
        }
    }

    // Root inode (ino 2 -> index 1 in inode table at block 4).
    {
        let root_off = 4 * bs + 128;
        let ino = &mut img[root_off..root_off + 128];
        w16(ino, 0, 0x4000 | 0o755); // S_IFDIR | 0755
        w32(ino, 4, bs as u32); // one directory block
        w16(ino, 26, 2); // links_count = 2
        w32(ino, 28, 8); // eight sectors
        w32(ino, 40, 12); // root directory block
    }

    // Root directory data block at block 12: "." and ".."
    {
        let dir = &mut img[12 * bs..13 * bs];
        w32(dir, 0, 2);
        w16(dir, 4, 12);
        dir[6] = 1;
        dir[7] = 2; // FT_DIR
        dir[8] = b'.';

        w32(dir, 12, 2);
        w16(dir, 16, (bs - 12) as u16);
        dir[18] = 2;
        dir[19] = 2; // FT_DIR
        dir[20] = b'.';
        dir[21] = b'.';
    }

    img
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{block::{BlockDevice, BlockError}, cobaltfs::CobaltFS,
                vfs::{FileSystem, InodeKind}};
    use alloc::sync::Arc;
    use spin::Mutex;

    struct MemoryImage(Mutex<Vec<u8>>);
    impl BlockDevice for MemoryImage {
        fn block_size(&self) -> usize { 512 }
        fn block_count(&self) -> u64 { (self.0.lock().len() / 512) as u64 }
        fn read_block(&self, lba: u64, out: &mut [u8]) -> Result<(), BlockError> {
            if out.len() != 512 { return Err(BlockError::InvalidBufferSize); }
            let data = self.0.lock();
            let block = data.get(lba as usize * 512..lba as usize * 512 + 512)
                .ok_or(BlockError::OutOfBounds)?;
            out.copy_from_slice(block);
            Ok(())
        }
        fn write_block(&self, lba: u64, input: &[u8]) -> Result<(), BlockError> {
            if input.len() != 512 { return Err(BlockError::InvalidBufferSize); }
            let mut data = self.0.lock();
            let block = data.get_mut(lba as usize * 512..lba as usize * 512 + 512)
                .ok_or(BlockError::OutOfBounds)?;
            block.copy_from_slice(input);
            Ok(())
        }
    }

    #[test]
    fn development_image_mounts_and_preserves_indirect_file_across_remount() {
        let device = Arc::new(MemoryImage(Mutex::new(development_image())));
        assert_eq!(device.block_count() * 512, 512 * 1024 * 1024);
        let filesystem = CobaltFS::mount(device.clone()).unwrap();
        filesystem.create("/packages", InodeKind::Directory).unwrap();
        let file = filesystem.create("/packages/service", InodeKind::File).unwrap();
        let payload = vec![0x93; 60_000]; // crosses twelve direct 4 KiB blocks
        assert_eq!(filesystem.write(file.ino, 0, &payload).unwrap(), payload.len());
        drop(filesystem);
        let filesystem = CobaltFS::mount(device).unwrap();
        let file = filesystem.lookup("/packages/service").unwrap();
        let mut actual = vec![0; file.size];
        assert_eq!(filesystem.read(file.ino, 0, &mut actual).unwrap(), payload.len());
        assert_eq!(actual, payload);
    }
}
