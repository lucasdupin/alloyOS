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
    pub open_file_table: [Option<FileHandle>; MAX_FDS],
}

impl FileDescriptorTable {
    pub fn new() -> Self {
        const INIT: Option<FileHandle> = None;
        let mut table = Self {
            open_file_table: [INIT; MAX_FDS],
        };

        // Pre-populate stdin (0), stdout (1), stderr (2) as serial CharDevice
        table.open_file_table[0] = Some(FileHandle {
            ino: 100,
            offset: 0,
            kind: InodeKind::CharDevice,
            path: String::from("/dev/serial"),
        });
        table.open_file_table[1] = Some(FileHandle {
            ino: 100,
            offset: 0,
            kind: InodeKind::CharDevice,
            path: String::from("/dev/serial"),
        });
        table.open_file_table[2] = Some(FileHandle {
            ino: 100,
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
    pub root_fs: Arc<RamFS>,
    pub dev_fs: Arc<DevFS>,
}

impl Vfs {
    pub fn new() -> Self {
        Self {
            root_fs: Arc::new(RamFS::new()),
            dev_fs: Arc::new(DevFS::new()),
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

    pub fn read(&self, handle: &mut FileHandle, buf: &mut [u8]) -> Result<usize, FsError> {
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

pub fn init() {
    let mut vfs = Mutex::lock(&VFS);
    *vfs = Some(Vfs::new());

    let mut fd_table = Mutex::lock(&GLOBAL_FD_TABLE);
    *fd_table = Some(FileDescriptorTable::new());
}
