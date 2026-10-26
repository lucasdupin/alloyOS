use alloc::string::String;
use core::slice;
use alloyos_proto::SocketAddrV4;
use crate::fs::vfs::{FileHandle, FsError, InodeKind, GLOBAL_FD_TABLE, VFS};
use crate::net::{with_net_stack, NetworkStack};
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
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TimeSpec {
    pub tv_sec: i64,
    pub tv_nsec: i64,
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct UtsName {
    pub sysname: [u8; 65],
    pub nodename: [u8; 65],
    pub release: [u8; 65],
    pub version: [u8; 65],
    pub machine: [u8; 65],
}

impl UtsName {
    pub fn new() -> Self {
        let mut uts = Self {
            sysname: [0; 65],
            nodename: [0; 65],
            release: [0; 65],
            version: [0; 65],
            machine: [0; 65],
        };

        Self::set_field(&mut uts.sysname, b"alloyOS");
        Self::set_field(&mut uts.nodename, b"alloyos");
        Self::set_field(&mut uts.release, b"0.1.0");
        Self::set_field(&mut uts.version, b"#1 nightly");
        Self::set_field(&mut uts.machine, b"aarch64");

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

    if (fd as usize) >= NetworkStack::SOCKET_FD_BASE {
        return sys_recv(fd, buf_ptr, len);
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

    if (fd as usize) >= NetworkStack::SOCKET_FD_BASE {
        return sys_send(fd, buf_ptr, len);
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

pub fn sys_open(path_ptr: u64, _flags: u64, mode: u64) -> i64 {
    if path_ptr == 0 {
        return EFAULT;
    }

    let path_len = if mode > 0 && mode <= 4096 {
        let max_len = mode as usize;
        let s = unsafe { slice::from_raw_parts(path_ptr as *const u8, max_len) };
        s.iter().position(|&b| b == 0).unwrap_or(max_len)
    } else {
        let mut len = 0;
        unsafe {
            let ptr = path_ptr as *const u8;
            while *ptr.add(len) != 0 && len < 4096 {
                len += 1;
            }
        }
        len
    };

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
    if (fd as usize) >= NetworkStack::SOCKET_FD_BASE {
        let closed = with_net_stack(|net| net.close_socket(fd as usize));
        return if closed { 0 } else { EBADF };
    }

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

pub fn sys_stat(path_ptr: u64, path_len_arg: u64) -> i64 {
    if path_ptr == 0 {
        return EFAULT;
    }

    let path_len = if path_len_arg > 0 && path_len_arg <= 4096 {
        let max_len = path_len_arg as usize;
        let s = unsafe { slice::from_raw_parts(path_ptr as *const u8, max_len) };
        s.iter().position(|&b| b == 0).unwrap_or(max_len)
    } else {
        let mut len = 0;
        unsafe {
            let ptr = path_ptr as *const u8;
            while *ptr.add(len) != 0 && len < 4096 {
                len += 1;
            }
        }
        len
    };

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

    match vfs.lookup(path_str) {
        Ok(inode) => match inode.kind {
            InodeKind::Directory => 1,
            InodeKind::File | InodeKind::CharDevice => 0,
        },
        Err(_) => ENOENT,
    }
}

pub fn sys_socket(domain: u64, sock_type: u64, _protocol: u64) -> i64 {
    with_net_stack(|net| match net.create_socket(domain, sock_type) {
        Ok(fd) => fd as i64,
        Err(err) => err,
    })
}

pub fn sys_connect(fd: u64, addr_ptr: u64, addr_len: u64) -> i64 {
    if addr_ptr == 0 {
        return EFAULT;
    }
    if (addr_len as usize) < core::mem::size_of::<SocketAddrV4>() {
        return EINVAL;
    }
    let addr = unsafe { *(addr_ptr as *const SocketAddrV4) };
    with_net_stack(|net| match net.connect_socket(fd as usize, addr) {
        Ok(()) => 0,
        Err(err) => err,
    })
}

pub fn sys_send(fd: u64, buf_ptr: u64, len: u64) -> i64 {
    if buf_ptr == 0 {
        return EFAULT;
    }
    let data = unsafe { slice::from_raw_parts(buf_ptr as *const u8, len as usize) };
    with_net_stack(|net| match net.send_socket(fd as usize, data) {
        Ok(n) => n as i64,
        Err(err) => err,
    })
}

pub fn sys_recv(fd: u64, buf_ptr: u64, len: u64) -> i64 {
    if buf_ptr == 0 {
        return EFAULT;
    }
    let out = unsafe { slice::from_raw_parts_mut(buf_ptr as *mut u8, len as usize) };
    with_net_stack(|net| match net.recv_socket(fd as usize, out) {
        Ok(n) => n as i64,
        Err(err) => err,
    })
}

/// `ResolveHost(name_ptr, name_len, out_ptr)`: write the 4-byte IPv4 address for `name`.
pub fn sys_resolve_host(name_ptr: u64, name_len: u64, out_ptr: u64) -> i64 {
    if name_ptr == 0 || out_ptr == 0 {
        return EFAULT;
    }
    if name_len == 0 || name_len > 253 {
        return EINVAL;
    }
    let bytes = unsafe { slice::from_raw_parts(name_ptr as *const u8, name_len as usize) };
    let Ok(name) = core::str::from_utf8(bytes) else {
        return EINVAL;
    };
    with_net_stack(|net| match net.resolve_host(name) {
        Ok(ip) => {
            unsafe { core::ptr::copy_nonoverlapping(ip.as_ptr(), out_ptr as *mut u8, 4) };
            0
        }
        Err(err) => err,
    })
}

pub fn sys_clock_gettime(_clk_id: u64, tp_ptr: u64) -> i64 {
    if tp_ptr == 0 {
        return EFAULT;
    }
    let ns = crate::drivers::rng::monotonic_time_ns();
    let ts = TimeSpec {
        tv_sec: (ns / 1_000_000_000) as i64,
        tv_nsec: (ns % 1_000_000_000) as i64,
    };
    unsafe {
        (tp_ptr as *mut TimeSpec).write(ts);
    }
    0
}

pub fn sys_getrandom(buf_ptr: u64, len: u64, _flags: u64) -> i64 {
    if buf_ptr == 0 {
        return EFAULT;
    }
    let buf = unsafe { slice::from_raw_parts_mut(buf_ptr as *mut u8, len as usize) };
    if crate::drivers::rng::try_fill_secure_random(buf).is_err() { return -11; }
    len as i64
}

pub fn sys_exit(code: i32) -> ! {
    if let Some(task) = scheduler::current_task() {
        task.set_exit_code(code);
    }
    loop {
        #[cfg(not(test))]
        crate::arch::wfi();
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
