use metalos_proto::SyscallNo;
use crate::syscall::handlers::*;

pub fn dispatch_syscall(
    num: u64,
    arg1: u64,
    arg2: u64,
    arg3: u64,
    _arg4: u64,
    _arg5: u64,
    _arg6: u64,
) -> i64 {
    let syscall = match SyscallNo::try_from_u64(num) {
        Ok(sc) => sc,
        Err(_) => return ENOSYS,
    };

    match syscall {
        SyscallNo::Read => sys_read(arg1, arg2, arg3),
        SyscallNo::Write => sys_write(arg1, arg2, arg3),
        SyscallNo::Open => sys_open(arg1, arg2, arg3),
        SyscallNo::Close => sys_close(arg1),
        SyscallNo::Yield => sys_yield(),
        SyscallNo::Getpid => sys_getpid(),
        SyscallNo::Uname => sys_uname(arg1),
        SyscallNo::Exit => {
            sys_exit(arg1 as i32);
        }
        _ => ENOSYS,
    }
}
