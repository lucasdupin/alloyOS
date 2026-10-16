//! Shared filesystem contracts used by the kernel and image tools.
use alloc::{string::String, vec::Vec};

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
