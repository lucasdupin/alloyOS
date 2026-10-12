use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec::Vec;
use spin::Mutex;
use crate::fs::devfs::DevFS;
use crate::fs::ramfs::RamFS;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InodeKind {
    File,
    Directory,
    CharDevice,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Inode {
    pub ino: u64,
    pub kind: InodeKind,
    pub size: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FsError {
    NotFound,
    AlreadyExists,
    IsADirectory,
    NotADirectory,
    InvalidInput,
    BadFileDescriptor,
    PermissionDenied,
    IoError,
    NoSpace,
}

pub trait FileSystem: Send + Sync {
    fn lookup(&self, path: &str) -> Result<Inode, FsError>;
    fn create(&self, path: &str, kind: InodeKind) -> Result<Inode, FsError>;
    fn read(&self, ino: u64, offset: usize, buf: &mut [u8]) -> Result<usize, FsError>;
    fn write(&self, ino: u64, offset: usize, buf: &[u8]) -> Result<usize, FsError>;
    fn readdir(&self, ino: u64) -> Result<Vec<String>, FsError>;
    fn unlink(&self, path: &str) -> Result<(), FsError>;
}

#[derive(Debug, Clone)]
pub struct FileHandle {
    pub ino: u64,
    pub offset: usize,
    pub kind: InodeKind,
    pub path: String,
}

pub const MAX_FDS: usize = 256;

pub struct FileDescriptorTable {
    pub open_file_table: Vec<Option<FileHandle>>,
}

impl FileDescriptorTable {
    pub fn new() -> Self {
        let mut open_file_table = Vec::with_capacity(MAX_FDS);
        open_file_table.resize(MAX_FDS, None);
        let mut table = Self { open_file_table };

        // Pre-populate stdin (0), stdout (1), stderr (2) as serial CharDevice
        table.open_file_table[0] = Some(FileHandle {
            ino: crate::fs::devfs::INO_DEV_SERIAL,
            offset: 0,
            kind: InodeKind::CharDevice,
            path: String::from("/dev/serial"),
        });
        table.open_file_table[1] = Some(FileHandle {
            ino: crate::fs::devfs::INO_DEV_SERIAL,
            offset: 0,
            kind: InodeKind::CharDevice,
            path: String::from("/dev/serial"),
        });
        table.open_file_table[2] = Some(FileHandle {
            ino: crate::fs::devfs::INO_DEV_SERIAL,
            offset: 0,
            kind: InodeKind::CharDevice,
            path: String::from("/dev/serial"),
        });

        table
    }

    pub fn open(&mut self, handle: FileHandle) -> Result<usize, FsError> {
        for fd in 3..MAX_FDS {
            if self.open_file_table[fd].is_none() {
                self.open_file_table[fd] = Some(handle);
                return Ok(fd);
            }
        }
        Err(FsError::PermissionDenied)
    }

    pub fn close(&mut self, fd: usize) -> Result<(), FsError> {
        if fd >= MAX_FDS || self.open_file_table[fd].is_none() {
            return Err(FsError::BadFileDescriptor);
        }
        self.open_file_table[fd] = None;
        Ok(())
    }

    pub fn get(&self, fd: usize) -> Option<&FileHandle> {
        if fd < MAX_FDS {
            self.open_file_table[fd].as_ref()
        } else {
            None
        }
    }

    pub fn get_mut(&mut self, fd: usize) -> Option<&mut FileHandle> {
        if fd < MAX_FDS {
            self.open_file_table[fd].as_mut()
        } else {
            None
        }
    }
}

pub struct Vfs {
    pub root_fs: Arc<dyn FileSystem>,
    pub dev_fs: Arc<DevFS>,
    pub backend_name: &'static str,
}

impl Vfs {
    pub fn new() -> Self {
        Self::with_root(Arc::new(RamFS::new()), "RamFS")
    }

    pub fn with_root(root_fs: Arc<dyn FileSystem>, backend_name: &'static str) -> Self {
        let _ = root_fs.create("/bin", InodeKind::Directory);
        let _ = root_fs.create("/dev", InodeKind::Directory);
        let _ = root_fs.create("/proc", InodeKind::Directory);
        let _ = root_fs.create("/etc", InodeKind::Directory);
        let _ = root_fs.create("/tmp", InodeKind::Directory);
        let _ = root_fs.create("/scripts", InodeKind::Directory);
        let _ = root_fs.create("/apps", InodeKind::Directory);

        if root_fs.lookup("/etc/motd").is_err() {
            if let Ok(ino) = root_fs.create("/etc/motd", InodeKind::File) {
                let _ = root_fs.write(
                    ino.ino,
                    0,
                    b"Welcome to metalOS v0.1.0 (aarch64 POSIX-compliant Rust kernel)\n",
                );
            }
        }
        if root_fs.lookup("/etc/ai.conf").is_err() {
            if let Ok(ino) = root_fs.create("/etc/ai.conf", InodeKind::File) {
                let _ = root_fs.write(
                    ino.ino,
                    0,
                    b"provider=gemini\nmodel=gemini-2.5-pro\n",
                );
            }
        }
        if root_fs.lookup("/etc/schedules.conf").is_err() {
            if let Ok(ino) = root_fs.create("/etc/schedules.conf", InodeKind::File) {
                let _ = root_fs.write(
                    ino.ino,
                    0,
                    b"# metalOS local JavaScript schedule registry\n",
                );
            }
        }
        if root_fs.lookup("/bin/sh").is_err() {
            if let Ok(ino) = root_fs.create("/bin/sh", InodeKind::File) {
                let _ = root_fs.write(ino.ino, 0, b"#!/bin/sh\n");
            }
        }
        if root_fs.lookup("/scripts/heartbeat.js").is_err() {
            if let Ok(ino) = root_fs.create("/scripts/heartbeat.js", InodeKind::File) {
                let _ = root_fs.write(
                    ino.ino,
                    0,
                    b"export function main() { return MetalOS.sys.status(); }\n",
                );
            }
        }

        let (total_f, used_f, free_f) = crate::mm::frame_allocator::memory_stats();
        let total_kb = if total_f > 0 { total_f * 4 } else { 262144 };
        let free_kb = if total_f > 0 { free_f * 4 } else { 258048 };
        let used_kb = used_f * 4;
        let info = alloc::format!(
            "MemTotal:       {:>8} kB\nMemFree:        {:>8} kB\nMemUsed:        {:>8} kB\n",
            total_kb,
            free_kb,
            used_kb
        );
        if let Ok(ino) = root_fs.lookup("/proc/meminfo").or_else(|_| root_fs.create("/proc/meminfo", InodeKind::File)) {
            let _ = root_fs.write(ino.ino, 0, info.as_bytes());
        }
        if let Ok(ino) = root_fs.lookup("/proc/uptime").or_else(|_| root_fs.create("/proc/uptime", InodeKind::File)) {
            let _ = root_fs.write(ino.ino, 0, b"1.25 1.10\n");
        }
        if let Ok(ino) = root_fs.lookup("/proc/cpuinfo").or_else(|_| root_fs.create("/proc/cpuinfo", InodeKind::File)) {
            let _ = root_fs.write(
                ino.ino,
                0,
                b"Processor\t: AArch64 Processor rev 0 (aarch64)\nFeatures\t: fp asimd evtstrm aes pmull sha1 sha2 crc32\n",
            );
        }
        if let Ok(ino) = root_fs.lookup("/proc/version").or_else(|_| root_fs.create("/proc/version", InodeKind::File)) {
            let _ = root_fs.write(ino.ino, 0, b"metalOS version 0.1.0 (rustc nightly aarch64)\n");
        }

        Self {
            root_fs,
            dev_fs: Arc::new(DevFS::new()),
            backend_name,
        }
    }

    pub fn lookup(&self, path: &str) -> Result<Inode, FsError> {
        let clean_path = path.trim();
        if clean_path.starts_with("/dev/") {
            self.dev_fs.lookup(clean_path)
        } else {
            self.root_fs.lookup(clean_path)
        }
    }

    pub fn create(&self, path: &str, kind: InodeKind) -> Result<Inode, FsError> {
        let clean_path = path.trim();
        if clean_path.starts_with("/dev/") {
            self.dev_fs.create(clean_path, kind)
        } else {
            self.root_fs.create(clean_path, kind)
        }
    }

    pub fn readdir(&self, path: &str) -> Result<Vec<String>, FsError> {
        let clean_path = path.trim();
        if clean_path == "/dev" || clean_path == "/dev/" {
            return self.dev_fs.readdir(0);
        }
        let inode = self.lookup(clean_path)?;
        if inode.kind != InodeKind::Directory {
            return Err(FsError::NotADirectory);
        }
        let raw_entries = self.root_fs.readdir(inode.ino)?;
        let prefix = if clean_path == "/" {
            String::from("")
        } else {
            String::from(clean_path.trim_end_matches('/'))
        };

        let mut formatted = Vec::with_capacity(raw_entries.len());
        for name in raw_entries {
            let child_path = alloc::format!("{}/{}", prefix, name);
            if let Ok(child_ino) = self.lookup(&child_path) {
                if child_ino.kind == InodeKind::Directory {
                    formatted.push(alloc::format!("{}/", name));
                    continue;
                }
            }
            formatted.push(name);
        }

        if clean_path == "/" {
            // Keep canonical root directories in standard order first
            let canonical = ["bin/", "dev/", "proc/", "etc/", "tmp/", "scripts/", "apps/"];
            let mut ordered = Vec::with_capacity(formatted.len());
            for &c in &canonical {
                if formatted.iter().any(|e| e == c) {
                    ordered.push(String::from(c));
                }
            }
            for e in formatted {
                if !canonical.contains(&e.as_str()) {
                    ordered.push(e);
                }
            }
            return Ok(ordered);
        }

        Ok(formatted)
    }

    pub fn read(&self, handle: &mut FileHandle, buf: &mut [u8]) -> Result<usize, FsError> {
        if handle.kind == InodeKind::Directory {
            let entries = self.readdir(&handle.path)?;
            let mut listing = String::new();
            for entry in entries {
                listing.push_str(&entry);
                listing.push('\n');
            }
            let bytes = listing.as_bytes();
            if handle.offset >= bytes.len() {
                return Ok(0);
            }
            let available = bytes.len() - handle.offset;
            let to_read = core::cmp::min(buf.len(), available);
            buf[..to_read].copy_from_slice(&bytes[handle.offset..handle.offset + to_read]);
            handle.offset += to_read;
            return Ok(to_read);
        }

        if handle.path.starts_with("/dev/") {
            let bytes = self.dev_fs.read(handle.ino, handle.offset, buf)?;
            handle.offset += bytes;
            Ok(bytes)
        } else {
            let bytes = self.root_fs.read(handle.ino, handle.offset, buf)?;
            handle.offset += bytes;
            Ok(bytes)
        }
    }

    pub fn write(&self, handle: &mut FileHandle, buf: &[u8]) -> Result<usize, FsError> {
        if handle.path.starts_with("/dev/") {
            let bytes = self.dev_fs.write(handle.ino, handle.offset, buf)?;
            handle.offset += bytes;
            Ok(bytes)
        } else {
            let bytes = self.root_fs.write(handle.ino, handle.offset, buf)?;
            handle.offset += bytes;
            Ok(bytes)
        }
    }

    pub fn unlink(&self, path: &str) -> Result<(), FsError> {
        let clean_path = path.trim();
        if clean_path.starts_with("/dev/") {
            self.dev_fs.unlink(clean_path)
        } else {
            self.root_fs.unlink(clean_path)
        }
    }
}

pub static VFS: Mutex<Option<Vfs>> = Mutex::new(None);
pub static GLOBAL_FD_TABLE: Mutex<Option<FileDescriptorTable>> = Mutex::new(None);

pub fn init() -> &'static str {
    let mounted_vfs = try_mount_cobaltfs().unwrap_or_else(Vfs::new);
    let backend = mounted_vfs.backend_name;

    let mut vfs = Mutex::lock(&VFS);
    *vfs = Some(mounted_vfs);

    let mut fd_table = Mutex::lock(&GLOBAL_FD_TABLE);
    *fd_table = Some(FileDescriptorTable::new());

    backend
}

fn try_mount_cobaltfs() -> Option<Vfs> {
    #[cfg(target_os = "none")]
    {
        use crate::drivers::ata::AtaPio;
        use crate::fs::cobaltfs::CobaltFS;

        // Probe ATA Primary Slave (drive 1, attached data disk) first, then Primary Master (drive 0)
        for drive in [1u8, 0u8] {
            if let Some(ata) = AtaPio::probe(drive) {
                let dev: Arc<dyn crate::fs::block::BlockDevice> = Arc::new(ata);
                if let Ok(cobalt) = CobaltFS::mount(dev) {
                    return Some(Vfs::with_root(Arc::new(cobalt), "CobaltFS"));
                }
            }
        }
    }
    None
}
