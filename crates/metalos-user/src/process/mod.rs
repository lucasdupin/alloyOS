//! Process management primitives for metalOS userland.

use metalos_proto::SyscallNo;

/// Information about the current operating system and architecture.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(C)]
pub struct UtsName {
    pub sysname: [u8; 65],
    pub nodename: [u8; 65],
    pub release: [u8; 65],
    pub version: [u8; 65],
    pub machine: [u8; 65],
}

impl Default for UtsName {
    fn default() -> Self {
        Self {
            sysname: [0; 65],
            nodename: [0; 65],
            release: [0; 65],
            version: [0; 65],
            machine: [0; 65],
        }
    }
}

fn nul_terminated_str(bytes: &[u8; 65]) -> &str {
    let len = bytes.iter().position(|&b| b == 0).unwrap_or(bytes.len());
    core::str::from_utf8(&bytes[..len]).unwrap_or("")
}

impl UtsName {
    pub fn sysname_str(&self) -> &str {
        nul_terminated_str(&self.sysname)
    }

    pub fn nodename_str(&self) -> &str {
        nul_terminated_str(&self.nodename)
    }

    pub fn release_str(&self) -> &str {
        nul_terminated_str(&self.release)
    }

    pub fn version_str(&self) -> &str {
        nul_terminated_str(&self.version)
    }

    pub fn machine_str(&self) -> &str {
        nul_terminated_str(&self.machine)
    }
}

/// Get the process ID of the calling process.
pub fn getpid() -> u64 {
    let ret = unsafe { crate::syscall::syscall0(SyscallNo::Getpid.as_u64()) };
    ret as u64
}

/// Yield execution to another schedulable process or thread.
pub fn yield_now() {
    unsafe {
        crate::syscall::syscall0(SyscallNo::Yield.as_u64());
    }
}

/// Retrieve system information.
pub fn uname() -> UtsName {
    let mut uts = UtsName::default();
    unsafe {
        crate::syscall::syscall1(SyscallNo::Uname.as_u64(), &mut uts as *mut _ as u64);
    }
    uts
}

/// Terminate the current process with an exit status code.
pub fn exit(code: i32) -> ! {
    unsafe {
        crate::syscall::syscall1(SyscallNo::Exit.as_u64(), code as u64);
    }
    loop {
        #[cfg(target_arch = "x86_64")]
        core::hint::spin_loop();

        #[cfg(not(target_arch = "x86_64"))]
        {
            #[cfg(test)]
            panic!("process::exit called with code {}", code);

            #[cfg(not(test))]
            core::hint::spin_loop();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn utsname_sysname_length() {
        let uts = UtsName::default();
        assert_eq!(uts.sysname.len(), 65);
    }

    #[test]
    fn utsname_default_zeroed() {
        let uts = UtsName::default();
        assert!(uts.sysname.iter().all(|&b| b == 0));
        assert!(uts.nodename.iter().all(|&b| b == 0));
        assert!(uts.release.iter().all(|&b| b == 0));
        assert!(uts.version.iter().all(|&b| b == 0));
        assert!(uts.machine.iter().all(|&b| b == 0));
    }

    #[test]
    fn test_uname_call_and_helpers() {
        let uts = uname();
        assert_eq!(uts.sysname_str(), "metalOS");
        assert_eq!(uts.nodename_str(), "metalos");
        assert_eq!(uts.release_str(), "0.1.0");
        assert!(uts.version_str().contains("metalOS"));
        assert_eq!(uts.machine_str(), "aarch64");
    }

    #[test]
    fn test_getpid_and_yield() {
        let pid = getpid();
        assert_eq!(pid, 1);
        yield_now();
    }
}
