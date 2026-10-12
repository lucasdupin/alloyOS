#![cfg_attr(not(test), no_std)]

#[cfg(test)]
extern crate std;

/// Memory region classification.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(u8)]
pub enum MemoryRegionKind {
    /// Usable RAM available for general allocation.
    Usable = 0,
    /// Reserved memory region (e.g. firmware, hardware-mapped).
    Reserved = 1,
    /// ACPI reclaimable memory.
    AcpReclaimable = 2,
    /// ACPI NVS memory.
    AcpiNvs = 3,
    /// Defective memory module or unusable RAM.
    BadMemory = 4,
    /// Memory used by the bootloader itself during boot.
    Bootloader = 5,
    /// Memory containing kernel text, data, and loaded modules.
    KernelAndModules = 6,
    /// Memory mapped to linear framebuffer.
    Framebuffer = 7,
}

/// A contiguous region of physical memory.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(C)]
pub struct MemoryRegion {
    pub start: u64,
    pub len: u64,
    pub kind: MemoryRegionKind,
}

impl MemoryRegion {
    pub const fn new(start: u64, len: u64, kind: MemoryRegionKind) -> Self {
        Self { start, len, kind }
    }

    pub const fn end(&self) -> u64 {
        self.start.saturating_add(self.len)
    }
}

/// Boot information passed to the kernel entry point.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BootInfo {
    pub memory_regions: &'static [MemoryRegion],
    pub physical_memory_offset: u64,
    pub kernel_image_offset: u64,
    pub rsdp_addr: Option<u64>,
}

/// metalOS system call numbers with explicit x86_64 ABI discriminants.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(u64)]
pub enum SyscallNo {
    Read = 0,
    Write = 1,
    Open = 2,
    Close = 3,
    Stat = 4,
    Fstat = 5,
    Lstat = 6,
    Poll = 7,
    Seek = 8,
    Mmap = 9,
    Munmap = 11,
    Brk = 12,
    Yield = 24,
    Getpid = 39,
    Fork = 57,
    Execve = 59,
    Exit = 60,
    Waitpid = 61,
    Kill = 62,
    Uname = 63,
}

impl SyscallNo {
    pub const ALL: [SyscallNo; 20] = [
        SyscallNo::Read,
        SyscallNo::Write,
        SyscallNo::Open,
        SyscallNo::Close,
        SyscallNo::Stat,
        SyscallNo::Fstat,
        SyscallNo::Lstat,
        SyscallNo::Poll,
        SyscallNo::Seek,
        SyscallNo::Mmap,
        SyscallNo::Munmap,
        SyscallNo::Brk,
        SyscallNo::Yield,
        SyscallNo::Getpid,
        SyscallNo::Fork,
        SyscallNo::Execve,
        SyscallNo::Exit,
        SyscallNo::Waitpid,
        SyscallNo::Kill,
        SyscallNo::Uname,
    ];

    pub const fn as_u64(self) -> u64 {
        self as u64
    }

    pub const fn try_from_u64(val: u64) -> Result<Self, u64> {
        match val {
            0 => Ok(Self::Read),
            1 => Ok(Self::Write),
            2 => Ok(Self::Open),
            3 => Ok(Self::Close),
            4 => Ok(Self::Stat),
            5 => Ok(Self::Fstat),
            6 => Ok(Self::Lstat),
            7 => Ok(Self::Poll),
            8 => Ok(Self::Seek),
            9 => Ok(Self::Mmap),
            11 => Ok(Self::Munmap),
            12 => Ok(Self::Brk),
            24 => Ok(Self::Yield),
            39 => Ok(Self::Getpid),
            57 => Ok(Self::Fork),
            59 => Ok(Self::Execve),
            60 => Ok(Self::Exit),
            61 => Ok(Self::Waitpid),
            62 => Ok(Self::Kill),
            63 => Ok(Self::Uname),
            other => Err(other),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    #[test]
    fn syscall_discriminants_are_unique() {
        let mut set = BTreeSet::new();
        for &syscall in &SyscallNo::ALL {
            let inserted = set.insert(syscall.as_u64());
            assert!(inserted, "Duplicate syscall discriminant: {:?}", syscall);
        }
        assert_eq!(set.len(), SyscallNo::ALL.len());
    }

    #[test]
    fn memory_region_size_expected() {
        // start (8) + len (8) + kind (1 + 7 padding) = 24 bytes
        assert_eq!(core::mem::size_of::<MemoryRegion>(), 24);
        assert_eq!(core::mem::align_of::<MemoryRegion>(), 8);
    }

    #[test]
    fn boot_info_static_construction() {
        static REGIONS: [MemoryRegion; 2] = [
            MemoryRegion::new(0x0, 0x100000, MemoryRegionKind::Bootloader),
            MemoryRegion::new(0x100000, 0x200000, MemoryRegionKind::Usable),
        ];

        let boot_info = BootInfo {
            memory_regions: &REGIONS,
            physical_memory_offset: 0xFFFF_8000_0000_0000,
            kernel_image_offset: 0xFFFF_FFFF_8000_0000,
            rsdp_addr: Some(0x7FFE_0000),
        };

        assert_eq!(boot_info.memory_regions.len(), 2);
        assert_eq!(boot_info.memory_regions[0].kind, MemoryRegionKind::Bootloader);
        assert_eq!(boot_info.memory_regions[1].end(), 0x300000);
        assert_eq!(boot_info.rsdp_addr, Some(0x7FFE_0000));
    }
}
