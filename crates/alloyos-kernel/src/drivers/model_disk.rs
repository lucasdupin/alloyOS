//! Read-only model disk: direct DMA into reserved RAM, polled without waiting.
use super::virtio_blk::{VIRTIO_MMIO_BASE, VIRTIO_MMIO_COUNT, VIRTIO_MMIO_STRIDE};
use core::{
    ptr::{read_volatile, write_volatile},
    sync::atomic::{fence, Ordering},
    task::Poll,
};
const QUEUE_SIZE: usize = 16;
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
    ring: [u16; QUEUE_SIZE],
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
    ring: [UsedElem; QUEUE_SIZE],
    event: u16,
}
#[repr(C, align(4096))]
struct Queue {
    desc: [Desc; QUEUE_SIZE],
    avail: Avail,
    pad: [u8; 4096 - 256 - core::mem::size_of::<Avail>()],
    used: Used,
}
static mut QUEUE: Queue = Queue {
    desc: [Desc {
        addr: 0,
        len: 0,
        flags: 0,
        next: 0,
    }; QUEUE_SIZE],
    avail: Avail {
        flags: 0,
        idx: 0,
        ring: [0; QUEUE_SIZE],
        event: 0,
    },
    pad: [0; 4096 - 256 - core::mem::size_of::<Avail>()],
    used: Used {
        flags: 0,
        idx: 0,
        ring: [UsedElem { id: 0, len: 0 }; QUEUE_SIZE],
        event: 0,
    },
};
#[repr(C)]
struct Request {
    kind: u32,
    reserved: u32,
    sector: u64,
    status: u8,
}
static mut REQUEST: Request = Request {
    kind: 0,
    reserved: 0,
    sector: 0,
    status: 0,
};
pub struct ModelDisk {
    base: usize,
    pub sectors: u64,
    used: u16,
    pending: bool,
    started: u64,
}
unsafe fn read(base: usize, offset: usize) -> u32 {
    read_volatile((base + offset) as *const u32)
}
unsafe fn write(base: usize, offset: usize, value: u32) {
    write_volatile((base + offset) as *mut u32, value);
}
impl ModelDisk {
    pub fn probe() -> Option<Self> {
        for slot in 0..VIRTIO_MMIO_COUNT {
            let base = VIRTIO_MMIO_BASE + slot * VIRTIO_MMIO_STRIDE;
            unsafe {
                if read(base, 0) != 0x74726976 || read(base, 8) != 2 {
                    continue;
                }
                write(base, 0x14, 0);
                if read(base, 0x10) & (1 << 5) == 0 {
                    continue;
                } // model disk is read-only
                let version = read(base, 4);
                if version != 1 && version != 2 {
                    continue;
                }
                write(base, 0x70, 0);
                write(base, 0x70, 3);
                write(base, 0x24, 0);
                write(base, 0x20, 1 << 5);
                if version == 2 {
                    write(base, 0x14, 1);
                    if read(base, 0x10) & 1 == 0 {
                        continue;
                    }
                    write(base, 0x24, 1);
                    write(base, 0x20, 1); // VIRTIO_F_VERSION_1
                    write(base, 0x24, 0);
                    write(base, 0x70, 11);
                    if read(base, 0x70) & 8 == 0 {
                        continue;
                    }
                } else {
                    write(base, 0x28, 4096);
                }
                write(base, 0x30, 0);
                if read(base, 0x34) < QUEUE_SIZE as u32 {
                    continue;
                }
                write(base, 0x38, QUEUE_SIZE as u32);
                core::ptr::write_bytes(&raw mut QUEUE as *mut u8, 0, core::mem::size_of::<Queue>());
                let desc = &raw const QUEUE.desc as u64;
                let avail = &raw const QUEUE.avail as u64;
                let used = &raw const QUEUE.used as u64;
                if version == 1 {
                    write(base, 0x3c, 4096);
                    write(base, 0x40, (&raw const QUEUE as u64 >> 12) as u32);
                } else {
                    for (reg, addr) in [(0x80, desc), (0x90, avail), (0xa0, used)] {
                        write(base, reg, addr as u32);
                        write(base, reg + 4, (addr >> 32) as u32);
                    }
                    write(base, 0x44, 1);
                }
                write(base, 0x70, if version == 2 { 15 } else { 7 });
                let sectors = read(base, 0x100) as u64 | ((read(base, 0x104) as u64) << 32);
                return Some(Self {
                    base,
                    sectors,
                    used: 0,
                    pending: false,
                    started: 0,
                });
            }
        }
        None
    }
    /// Destination must remain reserved and DMA-accessible until completion.
    pub unsafe fn start_read(
        &mut self,
        sector: u64,
        destination: usize,
        length: usize,
    ) -> Result<(), ()> {
        if self.pending
            || length == 0
            || length % 512 != 0
            || length > 16 * 1024 * 1024
            || sector
                .checked_add((length / 512) as u64)
                .is_none_or(|end| end > self.sectors)
        {
            return Err(());
        }
        REQUEST.kind = 0;
        REQUEST.reserved = 0;
        REQUEST.sector = sector;
        REQUEST.status = 255;
        QUEUE.desc[0] = Desc {
            addr: &raw const REQUEST as u64,
            len: 16,
            flags: 1,
            next: 1,
        };
        QUEUE.desc[1] = Desc {
            addr: destination as u64,
            len: length as u32,
            flags: 3,
            next: 2,
        };
        QUEUE.desc[2] = Desc {
            addr: &raw const REQUEST.status as u64,
            len: 1,
            flags: 2,
            next: 0,
        };
        let idx = read_volatile(&raw const QUEUE.avail.idx);
        write_volatile(&raw mut QUEUE.avail.ring[idx as usize % QUEUE_SIZE], 0);
        fence(Ordering::SeqCst);
        write_volatile(&raw mut QUEUE.avail.idx, idx.wrapping_add(1));
        fence(Ordering::SeqCst);
        write(self.base, 0x50, 0);
        self.pending = true;
        self.started = super::rng::monotonic_time_ns();
        Ok(())
    }
    pub fn poll(&mut self) -> Poll<Result<(), ()>> {
        if !self.pending {
            return Poll::Ready(Err(()));
        }
        unsafe {
            fence(Ordering::SeqCst);
            if read_volatile(&raw const QUEUE.used.idx) == self.used.wrapping_add(1) {
                self.used = self.used.wrapping_add(1);
                self.pending = false;
                return Poll::Ready(if read_volatile(&raw const REQUEST.status) == 0 {
                    Ok(())
                } else {
                    Err(())
                });
            }
            if super::rng::monotonic_time_ns().saturating_sub(self.started) > 30_000_000_000 {
                // Reset before releasing a destination that could still be receiving DMA.
                write(self.base, 0x70, 0);
                self.pending = false;
                return Poll::Ready(Err(()));
            }
        }
        Poll::Pending
    }
}
