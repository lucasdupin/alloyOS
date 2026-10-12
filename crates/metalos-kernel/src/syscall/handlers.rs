use alloc::string::String;
use core::slice;
use crate::fs::vfs::{FileHandle, FsError, InodeKind, GLOBAL_FD_TABLE, VFS};
use crate::task::scheduler;

pub const EPERM: i64 = -1;
pub const ENOENT: i64 = -2;
pub const ESRCH: i64 = -3;
pub const EINTR: i64 = -4;
pub const EIO: i64 = -5;
pub const EBADF: i64 = -9;
pub const EFAULT: i64 = -14;
pub const EINVAL: i64 = -22;
pub const ENOSYS: i64 = -38;

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct UtsName {
    pub sysname: [u8; 65],
    pub nodename: [u8; 65],
    pub release: [u8; 65],
    pub version: [u8; 65],
    pub machine: [u8; 65],
    pub domainname: [u8; 65],
}

impl UtsName {
    pub fn new() -> Self {
        let mut uts = Self {
            sysname: [0; 65],
            nodename: [0; 65],
            release: [0; 65],
            version: [0; 65],
            machine: [0; 65],
            domainname: [0; 65],
        };

        Self::set_field(&mut uts.sysname, b"metalOS");
        Self::set_field(&mut uts.nodename, b"metalos");
        Self::set_field(&mut uts.release, b"0.1.0");
        Self::set_field(&mut uts.version, b"#1 nightly");
        Self::set_field(&mut uts.machine, b"x86_64");

        uts
    }

    fn set_field(dest: &mut [u8; 65], src: &[u8]) {
        let len = core::cmp::min(src.len(), 64);
        dest[..len].copy_from_slice(&src[..len]);
        dest[len] = 0;
    }
}

pub fn sys_read(fd: u64, buf_ptr: u64, len: u64) -> i64 {
    if buf_ptr == 0 {
        return EFAULT;
    }

    let mut fd_table_guard = GLOBAL_FD_TABLE.lock();
    let fd_table = match fd_table_guard.as_mut() {
        Some(t) => t,
        None => return EBADF,
    };

    let handle = match fd_table.get_mut(fd as usize) {
        Some(h) => h,
        None => return EBADF,
    };

    let buffer = unsafe { slice::from_raw_parts_mut(buf_ptr as *mut u8, len as usize) };

    let vfs_guard = VFS.lock();
    let vfs = match vfs_guard.as_ref() {
        Some(v) => v,
        None => return EIO,
    };

    match vfs.read(handle, buffer) {
        Ok(bytes) => bytes as i64,
        Err(FsError::BadFileDescriptor) => EBADF,
        Err(_) => EIO,
    }
}

pub fn sys_write(fd: u64, buf_ptr: u64, len: u64) -> i64 {
    if buf_ptr == 0 {
        return EFAULT;
    }

    let mut fd_table_guard = GLOBAL_FD_TABLE.lock();
    let fd_table = match fd_table_guard.as_mut() {
        Some(t) => t,
        None => return EBADF,
    };

    let handle = match fd_table.get_mut(fd as usize) {
        Some(h) => h,
        None => return EBADF,
    };

    let buffer = unsafe { slice::from_raw_parts(buf_ptr as *const u8, len as usize) };

    let vfs_guard = VFS.lock();
    let vfs = match vfs_guard.as_ref() {
        Some(v) => v,
        None => return EIO,
    };

    match vfs.write(handle, buffer) {
        Ok(bytes) => bytes as i64,
        Err(FsError::BadFileDescriptor) => EBADF,
        Err(_) => EIO,
    }
}

pub fn sys_open(path_ptr: u64, _flags: u64, _mode: u64) -> i64 {
    if path_ptr == 0 {
        return EFAULT;
    }

    // Read null-terminated path
    let mut path_len = 0;
    unsafe {
        let ptr = path_ptr as *const u8;
        while *ptr.add(path_len) != 0 && path_len < 4096 {
            path_len += 1;
        }
    }

    let path_str = match core::str::from_utf8(unsafe {
        slice::from_raw_parts(path_ptr as *const u8, path_len)
    }) {
        Ok(s) => s,
        Err(_) => return EINVAL,
    };

    let vfs_guard = VFS.lock();
    let vfs = match vfs_guard.as_ref() {
        Some(v) => v,
        None => return EIO,
    };

    let inode = match vfs.lookup(path_str) {
        Ok(i) => i,
        Err(FsError::NotFound) => match vfs.create(path_str, InodeKind::File) {
            Ok(i) => i,
            Err(_) => return ENOENT,
        },
        Err(_) => return ENOENT,
    };

    let mut fd_table_guard = GLOBAL_FD_TABLE.lock();
    let fd_table = match fd_table_guard.as_mut() {
        Some(t) => t,
        None => return EBADF,
    };

    let handle = FileHandle {
        ino: inode.ino,
        offset: 0,
        kind: inode.kind,
        path: String::from(path_str),
    };

    match fd_table.open(handle) {
        Ok(fd) => fd as i64,
        Err(_) => EBADF,
    }
}

pub fn sys_close(fd: u64) -> i64 {
    let mut fd_table_guard = GLOBAL_FD_TABLE.lock();
    let fd_table = match fd_table_guard.as_mut() {
        Some(t) => t,
        None => return EBADF,
    };

    match fd_table.close(fd as usize) {
        Ok(()) => 0,
        Err(_) => EBADF,
    }
}

pub fn sys_exit(code: i32) -> ! {
    if let Some(task) = scheduler::current_task() {
        task.set_exit_code(code);
    }
    loop {
        #[cfg(not(test))]
        x86_64::instructions::hlt();
        #[cfg(test)]
        panic!("sys_exit called with code {}", code);
    }
}

pub fn sys_getpid() -> i64 {
    if let Some(task) = scheduler::current_task() {
        task.pid as i64
    } else {
        1 // Default init PID
    }
}

pub fn sys_uname(buf_ptr: u64) -> i64 {
    if buf_ptr == 0 {
        return EFAULT;
    }

    let uts = UtsName::new();
    let dest_ptr = buf_ptr as *mut UtsName;
    unsafe {
        dest_ptr.write(uts);
    }
    0
}

pub fn sys_yield() -> i64 {
    scheduler::tick();
    0
}
