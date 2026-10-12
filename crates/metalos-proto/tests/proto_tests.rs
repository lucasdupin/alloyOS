use metalos_proto::{BootInfo, MemoryRegion, MemoryRegionKind, SyscallNo};

#[test]
fn boot_info_layout() {
    static REGIONS: [MemoryRegion; 3] = [
        MemoryRegion::new(0x0, 0x1000, MemoryRegionKind::Reserved),
        MemoryRegion::new(0x1000, 0x8000, MemoryRegionKind::Usable),
        MemoryRegion::new(0x9000, 0x2000, MemoryRegionKind::AcpReclaimable),
    ];

    let boot_info = BootInfo {
        memory_regions: &REGIONS,
        physical_memory_offset: 0xFFFF_8000_0000_0000,
        kernel_image_offset: 0xFFFF_FFFF_8000_0000,
        rsdp_addr: Some(0x10000),
    };

    assert!(boot_info.memory_regions.len() > 0);
    assert_eq!(boot_info.memory_regions.len(), 3);
    assert_eq!(boot_info.physical_memory_offset, 0xFFFF_8000_0000_0000);
    assert_eq!(boot_info.kernel_image_offset, 0xFFFF_FFFF_8000_0000);
    assert_eq!(boot_info.rsdp_addr, Some(0x10000));
}

#[test]
fn syscall_numbers_stable() {
    assert_eq!(SyscallNo::Read as u64, 0);
    assert_eq!(SyscallNo::Write as u64, 1);
    assert_eq!(SyscallNo::Open as u64, 2);
    assert_eq!(SyscallNo::Close as u64, 3);
    assert_eq!(SyscallNo::Stat as u64, 4);
    assert_eq!(SyscallNo::Fstat as u64, 5);
    assert_eq!(SyscallNo::Lstat as u64, 6);
    assert_eq!(SyscallNo::Poll as u64, 7);
    assert_eq!(SyscallNo::Seek as u64, 8);
    assert_eq!(SyscallNo::Mmap as u64, 9);
    assert_eq!(SyscallNo::Munmap as u64, 11);
    assert_eq!(SyscallNo::Brk as u64, 12);
    assert_eq!(SyscallNo::Yield as u64, 24);
    assert_eq!(SyscallNo::Getpid as u64, 39);
    assert_eq!(SyscallNo::Fork as u64, 57);
    assert_eq!(SyscallNo::Execve as u64, 59);
    assert_eq!(SyscallNo::Exit as u64, 60);
    assert_eq!(SyscallNo::Waitpid as u64, 61);
    assert_eq!(SyscallNo::Kill as u64, 62);
    assert_eq!(SyscallNo::Uname as u64, 63);

    for syscall in SyscallNo::ALL {
        assert_eq!(SyscallNo::try_from_u64(syscall.as_u64()), Ok(syscall));
    }
    assert!(SyscallNo::try_from_u64(999).is_err());
}
