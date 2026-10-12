//! Raw system call interface for metalOS (x86_64 ABI).

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SyscallRecord {
    pub num: u64,
    pub a1: u64,
    pub a2: u64,
    pub a3: u64,
}

#[cfg(test)]
std::thread_local! {
    static LAST_SYSCALL: std::cell::RefCell<Option<SyscallRecord>> = const { std::cell::RefCell::new(None) };
    #[allow(clippy::type_complexity)]
    static MOCK_HANDLER: std::cell::RefCell<Option<std::boxed::Box<dyn Fn(u64, u64, u64, u64) -> i64 + 'static>>> = const { std::cell::RefCell::new(None) };
}

#[cfg(test)]
pub fn get_last_syscall() -> Option<SyscallRecord> {
    LAST_SYSCALL.with(|s| *s.borrow())
}

#[cfg(test)]
pub fn set_mock_handler<F>(f: F)
where
    F: Fn(u64, u64, u64, u64) -> i64 + 'static,
{
    MOCK_HANDLER.with(|h| {
        *h.borrow_mut() = Some(std::boxed::Box::new(f));
    });
}

#[cfg(test)]
pub fn clear_mock_handler() {
    MOCK_HANDLER.with(|h| {
        *h.borrow_mut() = None;
    });
}

#[cfg(any(
    test,
    feature = "host-tests",
    not(all(target_arch = "x86_64", target_os = "none"))
))]
#[allow(unused_variables)]
fn dispatch_mock(num: u64, a1: u64, a2: u64, a3: u64) -> i64 {
    #[cfg(test)]
    {
        LAST_SYSCALL.with(|s| {
            *s.borrow_mut() = Some(SyscallRecord { num, a1, a2, a3 });
        });

        let handler_res = MOCK_HANDLER.with(|h| {
            h.borrow().as_ref().map(|f| f(num, a1, a2, a3))
        });
        if let Some(res) = handler_res {
            return res;
        }
    }

    match num {
        0 => 0,          // Read
        1 => a3 as i64,  // Write: return requested length
        2 => 3,          // Open: return fd 3
        3 => 0,          // Close
        24 => 0,         // Yield
        39 => 1,         // Getpid: return PID 1
        60 => 0,         // Exit
        63 => {
            // Uname
            if a1 != 0 {
                let uts = unsafe { &mut *(a1 as *mut crate::process::UtsName) };
                let sysname = b"metalOS\0";
                uts.sysname[..sysname.len()].copy_from_slice(sysname);
                let nodename = b"metalos\0";
                uts.nodename[..nodename.len()].copy_from_slice(nodename);
                let release = b"0.1.0\0";
                uts.release[..release.len()].copy_from_slice(release);
                let version = b"metalOS 0.1.0 x86_64\0";
                uts.version[..version.len()].copy_from_slice(version);
                let machine = b"x86_64\0";
                uts.machine[..machine.len()].copy_from_slice(machine);
            }
            0
        }
        _ => 0,
    }
}

/// Perform a 0-argument system call.
///
/// # Safety
/// The caller must ensure that the given syscall number is valid and adheres to the kernel ABI.
pub unsafe fn syscall0(num: u64) -> i64 {
    #[cfg(all(not(test), target_arch = "x86_64", target_os = "none"))]
    {
        let ret: i64;
        core::arch::asm!(
            "syscall",
            inlateout("rax") num as i64 => ret,
            out("rcx") _,
            out("r11") _,
            options(nostack, preserves_flags)
        );
        ret
    }
    #[cfg(any(
        test,
        feature = "host-tests",
        not(all(target_arch = "x86_64", target_os = "none"))
    ))]
    {
        dispatch_mock(num, 0, 0, 0)
    }
}

/// Perform a 1-argument system call.
///
/// # Safety
/// The caller must ensure that the given syscall number and argument are valid and memory pointers are dereferenceable.
pub unsafe fn syscall1(num: u64, a1: u64) -> i64 {
    #[cfg(all(not(test), target_arch = "x86_64", target_os = "none"))]
    {
        let ret: i64;
        core::arch::asm!(
            "syscall",
            inlateout("rax") num as i64 => ret,
            in("rdi") a1,
            out("rcx") _,
            out("r11") _,
            options(nostack, preserves_flags)
        );
        ret
    }
    #[cfg(any(
        test,
        feature = "host-tests",
        not(all(target_arch = "x86_64", target_os = "none"))
    ))]
    {
        dispatch_mock(num, a1, 0, 0)
    }
}

/// Perform a 2-argument system call.
///
/// # Safety
/// The caller must ensure that the arguments conform to the requested system call specification.
pub unsafe fn syscall2(num: u64, a1: u64, a2: u64) -> i64 {
    #[cfg(all(not(test), target_arch = "x86_64", target_os = "none"))]
    {
        let ret: i64;
        core::arch::asm!(
            "syscall",
            inlateout("rax") num as i64 => ret,
            in("rdi") a1,
            in("rsi") a2,
            out("rcx") _,
            out("r11") _,
            options(nostack, preserves_flags)
        );
        ret
    }
    #[cfg(any(
        test,
        feature = "host-tests",
        not(all(target_arch = "x86_64", target_os = "none"))
    ))]
    {
        dispatch_mock(num, a1, a2, 0)
    }
}

/// Perform a 3-argument system call.
///
/// # Safety
/// The caller must ensure that pointers, sizes, and file descriptors are valid.
pub unsafe fn syscall3(num: u64, a1: u64, a2: u64, a3: u64) -> i64 {
    #[cfg(all(not(test), target_arch = "x86_64", target_os = "none"))]
    {
        let ret: i64;
        core::arch::asm!(
            "syscall",
            inlateout("rax") num as i64 => ret,
            in("rdi") a1,
            in("rsi") a2,
            in("rdx") a3,
            out("rcx") _,
            out("r11") _,
            options(nostack, preserves_flags)
        );
        ret
    }
    #[cfg(any(
        test,
        feature = "host-tests",
        not(all(target_arch = "x86_64", target_os = "none"))
    ))]
    {
        dispatch_mock(num, a1, a2, a3)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_syscall_mock_dispatch() {
        let ret0 = unsafe { syscall0(39) };
        assert_eq!(ret0, 1);
        let rec0 = get_last_syscall().unwrap();
        assert_eq!(rec0.num, 39);

        let ret1 = unsafe { syscall1(60, 42) };
        assert_eq!(ret1, 0);
        let rec1 = get_last_syscall().unwrap();
        assert_eq!(rec1.num, 60);
        assert_eq!(rec1.a1, 42);

        let ret2 = unsafe { syscall2(2, 10, 20) };
        assert_eq!(ret2, 3);
        let rec2 = get_last_syscall().unwrap();
        assert_eq!(rec2.num, 2);
        assert_eq!(rec2.a1, 10);
        assert_eq!(rec2.a2, 20);

        let ret3 = unsafe { syscall3(1, 1, 100, 50) };
        assert_eq!(ret3, 50);
        let rec3 = get_last_syscall().unwrap();
        assert_eq!(rec3.num, 1);
        assert_eq!(rec3.a1, 1);
        assert_eq!(rec3.a2, 100);
        assert_eq!(rec3.a3, 50);
    }

    #[test]
    fn test_custom_mock_handler() {
        set_mock_handler(|num, a1, _, _| {
            if num == 999 {
                (a1 * 2) as i64
            } else {
                -1
            }
        });

        let res = unsafe { syscall1(999, 21) };
        assert_eq!(res, 42);
        clear_mock_handler();

        let default_res = unsafe { syscall0(39) };
        assert_eq!(default_res, 1);
    }
}
