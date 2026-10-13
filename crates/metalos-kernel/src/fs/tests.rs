#[cfg(test)]
pub mod tests {
    use alloc::string::String;
    use crate::fs::devfs::{DevFS, INO_DEV_NULL, INO_DEV_ZERO};
    use crate::fs::ramfs::RamFS;
    use crate::fs::vfs::{
        FileDescriptorTable, FileHandle, FileSystem, FsError, InodeKind, Vfs,
    };

    #[test]
    fn ramfs_create_write_read() {
        let ramfs = RamFS::new();
        let inode = ramfs
            .create("/test.txt", InodeKind::File)
            .expect("Create file should succeed");

        let data = b"Hello, metalOS!";
        let written = ramfs.write(inode.ino, 0, data).expect("Write to file");
        assert_eq!(written, data.len());

        let mut buf = [0u8; 32];
        let read = ramfs.read(inode.ino, 0, &mut buf).expect("Read from file");
        assert_eq!(read, data.len());
        assert_eq!(&buf[..read], data);
    }

    #[test]
    fn ramfs_create_dir_lookup() {
        let ramfs = RamFS::new();
        ramfs
            .create("/mydir", InodeKind::Directory)
            .expect("Create directory");
        let inode = ramfs.lookup("/mydir").expect("Lookup directory");
        assert_eq!(inode.kind, InodeKind::Directory);
    }

    #[test]
    fn ramfs_unlink_removes() {
        let ramfs = RamFS::new();
        ramfs.create("/remove_me.txt", InodeKind::File).expect("Create");
        assert!(ramfs.lookup("/remove_me.txt").is_ok());

        ramfs.unlink("/remove_me.txt").expect("Unlink file");
        assert_eq!(ramfs.lookup("/remove_me.txt"), Err(FsError::NotFound));
    }

    #[test]
    fn fd_table_open_returns_3() {
        let mut table = FileDescriptorTable::new();
        let handle = FileHandle {
            ino: 50,
            offset: 0,
            kind: InodeKind::File,
            path: String::from("/test.txt"),
        };
        let fd = table.open(handle).expect("Open should succeed");
        assert_eq!(fd, 3, "First user opened FD must be 3");
    }

    #[test]
    fn fd_table_close_bad_fd_returns_ebadf() {
        let mut table = FileDescriptorTable::new();
        let res = table.close(99);
        assert_eq!(res, Err(FsError::BadFileDescriptor));
    }

    #[test]
    fn fd_table_double_close_returns_ebadf() {
        let mut table = FileDescriptorTable::new();
        let handle = FileHandle {
            ino: 51,
            offset: 0,
            kind: InodeKind::File,
            path: String::from("/temp.txt"),
        };
        let fd = table.open(handle).expect("Open");
        assert!(table.close(fd).is_ok());
        assert_eq!(table.close(fd), Err(FsError::BadFileDescriptor));
    }

    #[test]
    fn devfs_null_read_returns_empty() {
        let devfs = DevFS::new();
        let mut buf = [0u8; 32];
        let bytes = devfs.read(INO_DEV_NULL, 0, &mut buf).expect("Read null");
        assert_eq!(bytes, 0);
    }

    #[test]
    fn devfs_zero_read_all_zeros() {
        let devfs = DevFS::new();
        let mut buf = [0xFFu8; 16];
        let bytes = devfs.read(INO_DEV_ZERO, 0, &mut buf).expect("Read zero");
        assert_eq!(bytes, 16);
        for &byte in buf.iter() {
            assert_eq!(byte, 0x00);
        }
    }

    #[test]
    fn vfs_path_resolution_root() {
        let vfs = Vfs::new();
        let root = vfs.lookup("/").expect("Root lookup should succeed");
        assert_eq!(root.kind, InodeKind::Directory);
        assert_eq!(root.ino, 1);
    }

    #[test]
    fn vfs_path_resolution_nested() {
        let vfs = Vfs::new();
        vfs.create("/a/b/c.txt", InodeKind::File)
            .expect("Create nested path");
        let inode = vfs
            .lookup("/a/b/c.txt")
            .expect("Lookup nested path should succeed");
        assert_eq!(inode.kind, InodeKind::File);
    }

    #[test]
    fn tasks_package_contains_its_declared_entrypoint_and_preserves_updates() {
        let vfs = Vfs::new();
        for (path, expected) in [
            (metalos_mcp::TASKS_MANIFEST_PATH, metalos_mcp::TASKS_MANIFEST_MCP),
            (metalos_mcp::TASKS_SKILL_PATH, metalos_mcp::TASKS_SKILL_MD),
            (metalos_mcp::TASKS_MAIN_PATH, metalos_mcp::TASKS_MAIN_JS),
        ] {
            let inode = vfs.lookup(path).unwrap();
            assert_eq!(inode.kind, InodeKind::File);
            let mut bytes = vec![0; expected.len()];
            assert_eq!(vfs.root_fs.read(inode.ino, 0, &mut bytes).unwrap(), bytes.len());
            assert_eq!(bytes, expected.as_bytes());
        }
        let inode = vfs.lookup(metalos_mcp::TASKS_MAIN_PATH).unwrap();
        vfs.root_fs.write(inode.ino, 0, b"// updated").unwrap();
        let rebooted = Vfs::with_root(vfs.root_fs.clone(), "RamFS");
        let mut bytes = [0; 10];
        rebooted.root_fs.read(inode.ino, 0, &mut bytes).unwrap();
        assert_eq!(&bytes, b"// updated");
    }

    // ── CobaltFS tests ────────────────────────────────────────────────────────

    use alloc::sync::Arc;
    use alloc::vec;
    use alloc::vec::Vec;
    use spin::Mutex;
    use crate::fs::block::{BlockDevice, BlockError};
    use crate::fs::cobaltfs::CobaltFS;
    use crate::fs::cobaltfs::disk::{
        BlockGroupDescriptor, COBALTFS_MAGIC, DiskInode, FT_DIR, ROOT_INO,
        S_IFDIR, w16, w32,
    };

    struct MemBlockDevice {
        data: Mutex<Vec<u8>>,
        block_size: usize,
    }

    impl MemBlockDevice {
        fn new_formatted() -> Arc<Self> {
            // Create a minimal 1 KiB-block ext2 / CobaltFS image with 64 blocks:
            // Block 0: boot block
            // Block 1: Superblock
            // Block 2: BGD table
            // Block 3: Block bitmap
            // Block 4: Inode bitmap
            // Block 5..9: Inode table (32 inodes * 128B = 4096B = 4 blocks)
            // Block 9: Root directory data block
            let bs = 1024usize;
            let total_blocks = 64u32;
            let inodes_count = 32u32;
            let mut img = vec![0u8; bs * total_blocks as usize];

            // Superblock at offset 1024 (Block 1)
            let sb = &mut img[1024..2048];
            w32(sb, 0, inodes_count);
            w32(sb, 4, total_blocks);
            w32(sb, 12, total_blocks - 10); // free blocks
            w32(sb, 16, inodes_count - 11); // free inodes
            w32(sb, 20, 1);                 // first_data_block = 1 (for 1KiB blocks)
            w32(sb, 24, 0);                 // log_block_size = 0 (1024 B)
            w32(sb, 32, total_blocks);      // blocks_per_group
            w32(sb, 40, inodes_count);      // inodes_per_group
            w16(sb, 56, COBALTFS_MAGIC);
            w32(sb, 76, 0);                 // rev 0

            // Block Group Descriptor at Block 2 (offset 2048)
            let bgd = BlockGroupDescriptor {
                block_bitmap: 3,
                inode_bitmap: 4,
                inode_table: 5,
                free_blocks_count: (total_blocks - 10) as u16,
                free_inodes_count: (inodes_count - 11) as u16,
                used_dirs_count: 1,
            };
            bgd.to_bytes(&mut img[2048..2048 + 32]);

            // Mark blocks 0..=9 used in block bitmap (Block 3, offset 3072)
            img[3072] = 0xFF; // blocks 0..7
            img[3073] = 0b0000_0011; // blocks 8, 9

            // Mark reserved inodes 1..=11 used in inode bitmap (Block 4, offset 4096)
            img[4096] = 0xFF; // inodes 1..8
            img[4097] = 0b0000_0111; // inodes 9, 10, 11

            // Root inode (ino 2 -> index 1 in inode table at Block 5, offset 5120 + 128)
            let mut root_ino = DiskInode {
                mode: S_IFDIR | 0o755,
                uid: 0,
                size_lo: bs as u32,
                atime: 0,
                ctime: 0,
                mtime: 0,
                dtime: 0,
                gid: 0,
                links_count: 2,
                blocks_512: 2,
                flags: 0,
                block: [0; 15],
                file_acl: 0,
                dir_acl: 0,
            };
            root_ino.block[0] = 9;
            let root_off = 5120 + (ROOT_INO as usize - 1) * 128;
            root_ino.to_bytes(&mut img[root_off..root_off + 128]);

            // Root directory data block at Block 9 (offset 9216): "." and ".."
            let dir = &mut img[9 * bs..10 * bs];
            w32(dir, 0, ROOT_INO as u32);
            w16(dir, 4, 12);
            dir[6] = 1;
            dir[7] = FT_DIR;
            dir[8] = b'.';

            w32(dir, 12, ROOT_INO as u32);
            w16(dir, 16, (bs - 12) as u16);
            dir[18] = 2;
            dir[19] = FT_DIR;
            dir[20] = b'.';
            dir[21] = b'.';

            Arc::new(Self {
                data: Mutex::new(img),
                block_size: 512,
            })
        }
    }

    impl BlockDevice for MemBlockDevice {
        fn block_size(&self) -> usize {
            self.block_size
        }

        fn block_count(&self) -> u64 {
            (self.data.lock().len() / self.block_size) as u64
        }

        fn read_block(&self, lba: u64, buf: &mut [u8]) -> Result<(), BlockError> {
            if buf.len() != self.block_size {
                return Err(BlockError::InvalidBufferSize);
            }
            let data = self.data.lock();
            let start = lba as usize * self.block_size;
            if start + self.block_size > data.len() {
                return Err(BlockError::OutOfBounds);
            }
            buf.copy_from_slice(&data[start..start + self.block_size]);
            Ok(())
        }

        fn write_block(&self, lba: u64, buf: &[u8]) -> Result<(), BlockError> {
            if buf.len() != self.block_size {
                return Err(BlockError::InvalidBufferSize);
            }
            let mut data = self.data.lock();
            let start = lba as usize * self.block_size;
            if start + self.block_size > data.len() {
                return Err(BlockError::OutOfBounds);
            }
            data[start..start + self.block_size].copy_from_slice(buf);
            Ok(())
        }
    }

    #[test]
    fn cobaltfs_mount_create_write_read_unlink() {
        let dev = MemBlockDevice::new_formatted();
        let fs = CobaltFS::mount(dev).expect("Mount CobaltFS should succeed");

        let root = fs.lookup("/").expect("Lookup root");
        assert_eq!(root.kind, InodeKind::Directory);
        assert_eq!(root.ino, ROOT_INO);

        let file = fs
            .create("/hello.txt", InodeKind::File)
            .expect("Create file on CobaltFS");
        assert_eq!(file.kind, InodeKind::File);

        let payload = b"CobaltFS magnetic platter test!";
        let written = fs.write(file.ino, 0, payload).expect("Write to CobaltFS");
        assert_eq!(written, payload.len());

        let mut buf = [0u8; 64];
        let n = fs.read(file.ino, 0, &mut buf).expect("Read from CobaltFS");
        assert_eq!(&buf[..n], payload);

        let entries = fs.readdir(ROOT_INO).expect("Readdir root");
        assert_eq!(entries, vec![String::from("hello.txt")]);

        fs.unlink("/hello.txt").expect("Unlink file");
        assert_eq!(fs.lookup("/hello.txt"), Err(FsError::NotFound));
    }

    impl MemBlockDevice {
        fn new_formatted_large() -> Arc<Self> {
            // 1 KiB-block CobaltFS image with 128 blocks and 64 inodes:
            // Block 0: boot block
            // Block 1: Superblock
            // Block 2: BGD table
            // Block 3: Block bitmap
            // Block 4: Inode bitmap
            // Block 5..13: Inode table (64 inodes * 128B = 8192B = 8 blocks)
            // Block 13: Root directory data block
            let bs = 1024usize;
            let total_blocks = 128u32;
            let inodes_count = 64u32;
            let mut img = vec![0u8; bs * total_blocks as usize];

            let sb = &mut img[1024..2048];
            w32(sb, 0, inodes_count);
            w32(sb, 4, total_blocks);
            w32(sb, 12, total_blocks - 14);
            w32(sb, 16, inodes_count - 11);
            w32(sb, 20, 1);
            w32(sb, 24, 0);
            w32(sb, 32, total_blocks);
            w32(sb, 40, inodes_count);
            w16(sb, 56, COBALTFS_MAGIC);
            w32(sb, 76, 0);

            let bgd = BlockGroupDescriptor {
                block_bitmap: 3,
                inode_bitmap: 4,
                inode_table: 5,
                free_blocks_count: (total_blocks - 14) as u16,
                free_inodes_count: (inodes_count - 11) as u16,
                used_dirs_count: 1,
            };
            bgd.to_bytes(&mut img[2048..2048 + 32]);

            // Mark blocks 0..=13 used in block bitmap (Block 3, offset 3072)
            img[3072] = 0xFF; // blocks 0..7
            img[3073] = 0b0011_1111; // blocks 8..=13

            // Mark reserved inodes 1..=11 used in inode bitmap (Block 4, offset 4096)
            img[4096] = 0xFF;
            img[4097] = 0b0000_0111;

            let mut root_ino = DiskInode {
                mode: S_IFDIR | 0o755,
                uid: 0,
                size_lo: bs as u32,
                atime: 0,
                ctime: 0,
                mtime: 0,
                dtime: 0,
                gid: 0,
                links_count: 2,
                blocks_512: 2,
                flags: 0,
                block: [0; 15],
                file_acl: 0,
                dir_acl: 0,
            };
            root_ino.block[0] = 13;
            let root_off = 5120 + (ROOT_INO as usize - 1) * 128;
            root_ino.to_bytes(&mut img[root_off..root_off + 128]);

            let dir = &mut img[13 * bs..14 * bs];
            w32(dir, 0, ROOT_INO as u32);
            w16(dir, 4, 12);
            dir[6] = 1;
            dir[7] = FT_DIR;
            dir[8] = b'.';

            w32(dir, 12, ROOT_INO as u32);
            w16(dir, 16, (bs - 12) as u16);
            dir[18] = 2;
            dir[19] = FT_DIR;
            dir[20] = b'.';
            dir[21] = b'.';

            Arc::new(Self {
                data: Mutex::new(img),
                block_size: 512,
            })
        }
    }

    #[test]
    fn cobaltfs_durable_scripts_and_schedules_persist_across_remount_and_execute() {
        use crate::fs::automation::KernelVfsBackend;
        use metalos_welder::{AgentRuntime, AgentUiSynthesizer, ProviderToolCall, ToolVfsBackend};

        let dev = MemBlockDevice::new_formatted_large();

        // Boot 1: Mount CobaltFS on the block device, create & schedule JS automations via Welder
        {
            let fs1 = CobaltFS::mount(dev.clone()).expect("Mount CobaltFS boot 1");
            let vfs1 = Arc::new(Mutex::new(Vfs::with_root(Arc::new(fs1), "CobaltFS")));
            let backend1 = KernelVfsBackend::from_vfs_rc(vfs1);
            let mut runtime1 = AgentRuntime::with_vfs_backend(backend1);
            let mut ui = AgentUiSynthesizer::new();

            runtime1
                .tool_registry
                .execute_tool(
                    &ProviderToolCall::new(
                        "c1",
                        "script_create",
                        r#"{"path":"/scripts/boot_tag.js","source":"MetalOS.fs.write('/tmp/booted.txt', 'cobalt-boot-1'); 'ok';"}"#,
                    ),
                    &mut ui,
                )
                .expect("create boot_tag.js on CobaltFS");

            runtime1
                .tool_registry
                .execute_tool(
                    &ProviderToolCall::new(
                        "s1",
                        "script_schedule",
                        r#"{"path":"/scripts/boot_tag.js","trigger":"boot"}"#,
                    ),
                    &mut ui,
                )
                .expect("schedule boot_tag.js on CobaltFS");

            runtime1
                .tool_registry
                .execute_tool(
                    &ProviderToolCall::new(
                        "c2",
                        "script_create",
                        r#"{"path":"/scripts/tmp_watcher.js","source":"let v = MetalOS.fs.read(EVENT.path); MetalOS.fs.write('/etc/audit.txt', 'watched:' + v); true;"}"#,
                    ),
                    &mut ui,
                )
                .expect("create tmp_watcher.js on CobaltFS");

            runtime1
                .tool_registry
                .execute_tool(
                    &ProviderToolCall::new(
                        "s2",
                        "script_schedule",
                        r#"{"path":"/scripts/tmp_watcher.js","trigger":"fs:create:/tmp/*"}"#,
                    ),
                    &mut ui,
                )
                .expect("schedule tmp_watcher.js on CobaltFS");
        }

        // Boot 2: Remount CobaltFS from the raw block device into a brand-new Vfs and AgentRuntime
        let fs2 = CobaltFS::mount(dev).expect("Remount CobaltFS boot 2");
        let vfs2 = Arc::new(Mutex::new(Vfs::with_root(Arc::new(fs2), "CobaltFS")));
        let backend2 = KernelVfsBackend::from_vfs_rc(vfs2);
        let mut runtime2 = AgentRuntime::with_vfs_backend(backend2.clone());

        assert_eq!(
            runtime2.tool_registry.scheduler().schedules().len(),
            2,
            "Both schedules must be restored from /etc/schedules.conf on CobaltFS"
        );

        // Dispatch SystemBoot: boot_tag.js writes /tmp/booted.txt, which cascades to tmp_watcher.js
        // writing /etc/audit.txt on CobaltFS!
        let reports = runtime2.notify_system_boot();
        assert_eq!(
            reports.len(),
            2,
            "SystemBoot must run boot_tag.js and cascade FsCreated(/tmp/booted.txt) to tmp_watcher.js"
        );

        assert_eq!(
            backend2.borrow_mut().read_file("/tmp/booted.txt").as_deref(),
            Some("cobalt-boot-1")
        );
        assert_eq!(
            backend2.borrow_mut().read_file("/etc/audit.txt").as_deref(),
            Some("watched:cobalt-boot-1")
        );
    }
}
