//! Bounded boot-time VirtIO entropy acquisition. Device absence is not a seed.
//! DMA memory remains allocated even after timeout until the device is reset.
//! OASIS VirtIO 1.3 §5.4: consume only the device-reported used length.
#[cfg(target_os = "none")]
mod device {
    use crate::drivers::virtio_blk::{VIRTIO_MMIO_BASE, VIRTIO_MMIO_COUNT, VIRTIO_MMIO_STRIDE};
    use core::ptr::{read_volatile, write_volatile};
    use core::sync::atomic::{fence, Ordering};
    #[repr(C)]
    #[derive(Clone, Copy)]
    struct Desc {
        addr: u64,
        len: u32,
        flags: u16,
        next: u16,
    }
    #[repr(C)]
    struct Avail {
        flags: u16,
        idx: u16,
        ring: [u16; 8],
        event: u16,
    }
    #[repr(C)]
    #[derive(Clone, Copy)]
    struct UsedElem {
        id: u32,
        len: u32,
    }
    #[repr(C)]
    struct Used {
        flags: u16,
        idx: u16,
        ring: [UsedElem; 8],
        event: u16,
    }
    #[repr(C, align(4096))]
    struct Queue {
        desc: [Desc; 8],
        avail: Avail,
        pad: [u8; 4096 - 128 - 22],
        used: Used,
    }
    static mut QUEUE: Queue = Queue {
        desc: [Desc {
            addr: 0,
            len: 0,
            flags: 0,
            next: 0,
        }; 8],
        avail: Avail {
            flags: 0,
            idx: 0,
            ring: [0; 8],
            event: 0,
        },
        pad: [0; 3946],
        used: Used {
            flags: 0,
            idx: 0,
            ring: [UsedElem { id: 0, len: 0 }; 8],
            event: 0,
        },
    };
    #[repr(C, align(64))]
    struct Buffer([u8; 32]);
    static mut BUFFER: Buffer = Buffer([0; 32]);
    unsafe fn read(base: usize, offset: usize) -> u32 {
        read_volatile((base + offset) as *const u32)
    }
    unsafe fn write(base: usize, offset: usize, value: u32) {
        write_volatile((base + offset) as *mut u32, value)
    }
    pub fn seed() -> Option<[u8; 32]> {
        unsafe {
            for slot in 0..VIRTIO_MMIO_COUNT {
                let base = VIRTIO_MMIO_BASE + slot * VIRTIO_MMIO_STRIDE;
                if read(base, 0) != 0x74726976 || read(base, 8) != 4 {
                    continue;
                }
                let version = read(base, 4);
                if version != 1 && version != 2 {
                    continue;
                }
                write(base, 0x70, 0);
                write(base, 0x70, 3);
                write(base, 0x24, 0);
                write(base, 0x20, 0);
                if version == 2 {
                    write(base, 0x14, 1);
                    if read(base, 0x10) & 1 == 0 {
                        write(base, 0x70, 0);
                        continue;
                    }
                    write(base, 0x24, 1);
                    write(base, 0x20, 1);
                    write(base, 0x70, 11);
                    if read(base, 0x70) & 8 == 0 {
                        write(base, 0x70, 0);
                        continue;
                    }
                } else {
                    write(base, 0x28, 4096);
                }
                write(base, 0x30, 0);
                if read(base, 0x34) < 8 {
                    write(base, 0x70, 0);
                    continue;
                }
                core::ptr::write_bytes(&raw mut QUEUE as *mut u8, 0, core::mem::size_of::<Queue>());
                write(base, 0x38, 8);
                if version == 1 {
                    write(base, 0x3c, 4096);
                    write(base, 0x40, ((&raw const QUEUE as u64) >> 12) as u32);
                } else {
                    for (reg, address) in [
                        (0x80, &raw const QUEUE.desc as u64),
                        (0x90, &raw const QUEUE.avail as u64),
                        (0xa0, &raw const QUEUE.used as u64),
                    ] {
                        write(base, reg, address as u32);
                        write(base, reg + 4, (address >> 32) as u32);
                    }
                    write(base, 0x44, 1);
                }
                write(base, 0x70, if version == 2 { 15 } else { 7 });
                let mut filled = 0usize;
                let mut index = 0u16;
                let deadline = super::super::rng::monotonic_time_ns().saturating_add(50_000_000);
                while filled < 32 {
                    write_volatile(
                        &raw mut QUEUE.desc[0],
                        Desc {
                            addr: (&raw mut BUFFER.0 as *mut u8).add(filled) as u64,
                            len: (32 - filled) as u32,
                            flags: 2,
                            next: 0,
                        },
                    );
                    write_volatile(&raw mut QUEUE.avail.ring[(index as usize) % 8], 0);
                    fence(Ordering::SeqCst);
                    index = index.wrapping_add(1);
                    write_volatile(&raw mut QUEUE.avail.idx, index);
                    fence(Ordering::SeqCst);
                    write(base, 0x50, 0);
                    while read_volatile(&raw const QUEUE.used.idx) != index {
                        if super::super::rng::monotonic_time_ns() >= deadline {
                            write(base, 0x70, 0);
                            return None;
                        }
                        core::hint::spin_loop();
                    }
                    fence(Ordering::SeqCst);
                    let used = read_volatile(
                        &raw const QUEUE.used.ring[(index.wrapping_sub(1) as usize) % 8],
                    );
                    if used.id != 0 || used.len == 0 || used.len as usize > 32 - filled {
                        write(base, 0x70, 0);
                        return None;
                    }
                    filled += used.len as usize;
                }
                let seed = read_volatile(&raw const BUFFER.0);
                core::ptr::write_bytes(&raw mut BUFFER.0 as *mut u8, 0, 32);
                write(base, 0x70, 0);
                return Some(seed);
            }
            None
        }
    }
}
/// Seed the private cryptographic generator, with no timer fallback.
pub fn init() -> bool {
    #[cfg(target_os = "none")]
    if let Some(seed) = device::seed() {
        super::rng::install_secure_seed(seed);
        return true;
    }
    false
}
