//! File system primitives for metalOS userland.

use metalos_proto::SyscallNo;

pub const O_RDONLY: u32 = 0;
pub const O_WRONLY: u32 = 1;
pub const O_RDWR: u32 = 2;

/// Open a file at `path` with `flags`.
pub fn open(path: &str, flags: u32) -> i64 {
    unsafe {
        crate::syscall::syscall3(
            SyscallNo::Open.as_u64(),
            path.as_ptr() as u64,
            flags as u64,
            path.len() as u64,
        )
    }
}

/// Close a previously opened file descriptor.
pub fn close(fd: u32) -> i64 {
    unsafe {
        crate::syscall::syscall1(SyscallNo::Close.as_u64(), fd as u64)
    }
}

/// Open, read up to 4096 bytes from, and close a file at `path`.
pub fn read_file(path: &str) -> Result<[u8; 4096], i64> {
    let fd = open(path, O_RDONLY);
    if fd < 0 {
        return Err(fd);
    }

    let mut buf = [0u8; 4096];
    let n = crate::io::read(fd as u32, &mut buf);
    let _ = close(fd as u32);

    if n < 0 {
        return Err(n);
    }

    Ok(buf)
}

/// Open (or create), write `data` to, and close a file at `path`.
pub fn write_file(path: &str, data: &[u8]) -> Result<usize, i64> {
    let fd = open(path, O_WRONLY);
    if fd < 0 {
        return Err(fd);
    }

    let n = crate::io::write(fd as u32, data);
    let _ = close(fd as u32);

    if n < 0 {
        return Err(n);
    }

    Ok(n as usize)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::syscall::get_last_syscall;

    #[test]
    fn open_flags_passed_through() {
        assert_eq!(O_RDONLY, 0);
        assert_eq!(O_WRONLY, 1);
        assert_eq!(O_RDWR, 2);

        let path = "/test/file";
        let fd = open(path, O_RDWR);
        assert_eq!(fd, 3);

        let rec = get_last_syscall().expect("open syscall recorded");
        assert_eq!(rec.num, SyscallNo::Open.as_u64());
        assert_eq!(rec.a1, path.as_ptr() as u64);
        assert_eq!(rec.a2, O_RDWR as u64);
        assert_eq!(rec.a3, path.len() as u64);
    }

    #[test]
    fn read_file_result_size() {
        let res = read_file("/proc/uptime");
        assert!(res.is_ok());
        let buf = res.unwrap();
        assert_eq!(buf.len(), 4096);
    }

    #[test]
    fn close_constructs_correct_args() {
        let res = close(7);
        assert_eq!(res, 0);

        let rec = get_last_syscall().expect("close syscall recorded");
        assert_eq!(rec.num, SyscallNo::Close.as_u64());
        assert_eq!(rec.a1, 7);
    }
}
