#[cfg(test)]
pub mod tests {
    use core::mem::MaybeUninit;
    use metalos_proto::SyscallNo;
    use crate::fs::vfs::{FileDescriptorTable, GLOBAL_FD_TABLE};
    use crate::syscall::dispatch::dispatch_syscall;
    use crate::syscall::handlers::{
        sys_getpid, sys_read, sys_uname, sys_write, UtsName, EBADF, EFAULT, ENOSYS,
    };

    #[test]
    fn dispatch_invalid_syscall_returns_enosys() {
        let res = dispatch_syscall(9999, 0, 0, 0, 0, 0, 0);
        assert_eq!(res, ENOSYS, "Syscall 9999 must return -38 (ENOSYS)");
    }

    #[test]
    fn dispatch_write_routes_to_handler() {
        // Init FD table
        {
            let mut guard = GLOBAL_FD_TABLE.lock();
            *guard = Some(FileDescriptorTable::new());
        }

        let buffer = b"Test Write";
        let res = dispatch_syscall(
            SyscallNo::Write as u64,
            99, // Unopened FD
            buffer.as_ptr() as u64,
            buffer.len() as u64,
            0,
            0,
            0,
        );
        // It routes to sys_write which returns EBADF for fd 99
        assert_eq!(res, EBADF);
    }

    #[test]
    fn sys_getpid_returns_positive() {
        let pid = sys_getpid();
        assert!(pid > 0, "getpid() must return a positive PID");
    }

    #[test]
    fn sys_uname_fills_sysname() {
        let mut uts = MaybeUninit::<UtsName>::uninit();
        let ret = sys_uname(uts.as_mut_ptr() as u64);
        assert_eq!(ret, 0);

        let uts = unsafe { uts.assume_init() };
        let sysname = core::ffi::CStr::from_bytes_until_nul(&uts.sysname)
            .expect("sysname should have null terminator")
            .to_str()
            .expect("valid utf8");
        assert_eq!(sysname, "metalOS");

        let release = core::ffi::CStr::from_bytes_until_nul(&uts.release)
            .expect("release should have null terminator")
            .to_str()
            .expect("valid utf8");
        assert_eq!(release, "0.1.0");
    }

    #[test]
    fn sys_write_bad_fd_returns_ebadf() {
        {
            let mut guard = GLOBAL_FD_TABLE.lock();
            *guard = Some(FileDescriptorTable::new());
        }

        let buf = [1u8; 4];
        let ret = sys_write(99, buf.as_ptr() as u64, 4);
        assert_eq!(ret, EBADF);
    }

    #[test]
    fn sys_read_null_ptr_returns_efault() {
        let ret = sys_read(0, 0, 16);
        assert_eq!(ret, EFAULT);
    }
}
