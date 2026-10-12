use alloc::string::String;
use alloc::vec::Vec;
use crate::fs::vfs::{FileSystem, FsError, Inode, InodeKind};

pub const INO_DEV_NULL: u64 = 10;
pub const INO_DEV_ZERO: u64 = 11;
pub const INO_DEV_SERIAL: u64 = 12;

pub struct DevFS;

impl DevFS {
    pub const fn new() -> Self {
        Self
    }
}

impl FileSystem for DevFS {
    fn lookup(&self, path: &str) -> Result<Inode, FsError> {
        match path {
            "/dev/null" => Ok(Inode {
                ino: INO_DEV_NULL,
                kind: InodeKind::CharDevice,
                size: 0,
            }),
            "/dev/zero" => Ok(Inode {
                ino: INO_DEV_ZERO,
                kind: InodeKind::CharDevice,
                size: 0,
            }),
            "/dev/serial" => Ok(Inode {
                ino: INO_DEV_SERIAL,
                kind: InodeKind::CharDevice,
                size: 0,
            }),
            _ => Err(FsError::NotFound),
        }
    }

    fn create(&self, _path: &str, _kind: InodeKind) -> Result<Inode, FsError> {
        Err(FsError::PermissionDenied)
    }

    fn read(&self, ino: u64, _offset: usize, buf: &mut [u8]) -> Result<usize, FsError> {
        match ino {
            INO_DEV_NULL => Ok(0), // EOF immediately
            INO_DEV_ZERO => {
                buf.fill(0);
                Ok(buf.len())
            }
            INO_DEV_SERIAL => Ok(0),
            _ => Err(FsError::NotFound),
        }
    }

    fn write(&self, ino: u64, _offset: usize, buf: &[u8]) -> Result<usize, FsError> {
        match ino {
            INO_DEV_NULL | INO_DEV_ZERO => Ok(buf.len()),
            INO_DEV_SERIAL => {
                if let Ok(s) = core::str::from_utf8(buf) {
                    crate::serial_print!("{}", s);
                }
                Ok(buf.len())
            }
            _ => Err(FsError::NotFound),
        }
    }

    fn readdir(&self, _ino: u64) -> Result<Vec<String>, FsError> {
        let mut list = Vec::new();
        list.push(String::from("null"));
        list.push(String::from("zero"));
        list.push(String::from("serial"));
        Ok(list)
    }

    fn unlink(&self, _path: &str) -> Result<(), FsError> {
        Err(FsError::PermissionDenied)
    }
}
