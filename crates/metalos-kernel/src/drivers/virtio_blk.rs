//! VirtIO-Blk MMIO Block Device Driver for QEMU `-M virt` (`0x0a00_0000..0x0a00_4000`).
//!
//! Supports both VirtIO MMIO Version 1 (Legacy) and Version 2 (Modern) transports,
//! providing a 512-byte sector `BlockDevice` implementation to back **CobaltFS**.

use core::ptr::{read_volatile, write_volatile};
use core::sync::atomic::{fence, Ordering};
use spin::Mutex;

use crate::fs::block::{BlockDevice, BlockError};

pub const VIRTIO_MMIO_BASE: usize = 0x0a00_0000;
pub const VIRTIO_MMIO_STRIDE: usize = 0x200;
pub const VIRTIO_MMIO_COUNT: usize = 32;

const VIRTIO_MAGIC: u32 = 0x7472_6976; // "virt"
const VIRTIO_DEV_BLK: u32 = 2;

// VirtIO MMIO Register Offsets
const REG_MAGIC: usize = 0x000;
const REG_VERSION: usize = 0x004;
const REG_DEVICE_ID: usize = 0x008;
const REG_DEVICE_FEATURES: usize = 0x010;
const REG_DRIVER_FEATURES: usize = 0x020;
const REG_GUEST_PAGE_SIZE: usize = 0x028; // v1 only
const REG_QUEUE_SEL: usize = 0x030;
const REG_QUEUE_NUM_MAX: usize = 0x034;
const REG_QUEUE_NUM: usize = 0x038;
const REG_QUEUE_ALIGN: usize = 0x03C; // v1 only
const REG_QUEUE_PFN: usize = 0x040; // v1 only
const REG_QUEUE_READY: usize = 0x044; // v2 only
const REG_QUEUE_NOTIFY: usize = 0x050;
const REG_STATUS: usize = 0x070;
const REG_QUEUE_DESC_LOW: usize = 0x080; // v2 only
const REG_QUEUE_DESC_HIGH: usize = 0x084;
const REG_QUEUE_AVAIL_LOW: usize = 0x090; // v2 only
const REG_QUEUE_AVAIL_HIGH: usize = 0x094;
const REG_QUEUE_USED_LOW: usize = 0x0A0; // v2 only
const REG_QUEUE_USED_HIGH: usize = 0x0A4;
const REG_CONFIG: usize = 0x100;

// Device status bits
const STATUS_ACKNOWLEDGE: u32 = 1;
const STATUS_DRIVER: u32 = 2;
const STATUS_DRIVER_OK: u32 = 4;
const STATUS_FEATURES_OK: u32 = 8;

const QUEUE_SIZE: usize = 16;
const VRING_DESC_F_NEXT: u16 = 1;
const VRING_DESC_F_WRITE: u16 = 2;

const VIRTIO_BLK_T_IN: u32 = 0;
const VIRTIO_BLK_T_OUT: u32 = 1;

#[repr(C, align(16))]
#[derive(Clone, Copy)]
struct VirtqDesc {
    addr: u64,
    len: u32,
    flags: u16,
    next: u16,
}

#[repr(C, align(2))]
struct VirtqAvail {
    flags: u16,
    idx: u16,
    ring: [u16; QUEUE_SIZE],
    used_event: u16,
}

#[repr(C, align(4))]
#[derive(Clone, Copy)]
struct VirtqUsedElem {
    id: u32,
    len: u32,
}

#[repr(C, align(4))]
struct VirtqUsed {
    flags: u16,
    idx: u16,
    ring: [VirtqUsedElem; QUEUE_SIZE],
    avail_event: u16,
}

/// 2-page contiguous Virtqueue layout compatible with both VirtIO MMIO v1 (4096-byte page align)
/// and VirtIO MMIO v2 (explicit descriptor/avail/used physical pointers).
#[repr(C, align(4096))]
struct VirtQueueMemory {
    desc: [VirtqDesc; QUEUE_SIZE], // 256 bytes (0..256)
    avail: VirtqAvail,             // 38 bytes (256..294)
    _pad0: [u8; 4096 - 256 - core::mem::size_of::<VirtqAvail>()],
    used: VirtqUsed,               // starts at offset 4096
}

#[repr(C)]
struct VirtioBlkReqHeader {
    type_: u32,
    reserved: u32,
    sector: u64,
}

#[repr(C, align(64))]
struct RequestBounceBuffer {
    header: VirtioBlkReqHeader,
    data: [u8; 512],
    status: u8,
}

static mut VIRTQ_MEM: VirtQueueMemory = VirtQueueMemory {
    desc: [VirtqDesc {
        addr: 0,
        len: 0,
        flags: 0,
        next: 0,
    }; QUEUE_SIZE],
    avail: VirtqAvail {
        flags: 0,
        idx: 0,
        ring: [0; QUEUE_SIZE],
        used_event: 0,
    },
    _pad0: [0; 4096 - 256 - core::mem::size_of::<VirtqAvail>()],
    used: VirtqUsed {
        flags: 0,
        idx: 0,
        ring: [VirtqUsedElem { id: 0, len: 0 }; QUEUE_SIZE],
        avail_event: 0,
    },
};

static mut REQ_BUF: RequestBounceBuffer = RequestBounceBuffer {
    header: VirtioBlkReqHeader {
        type_: 0,
        reserved: 0,
        sector: 0,
    },
    data: [0; 512],
    status: 0,
};

struct VirtIoBlkInner {
    base: usize,
    last_used_idx: u16,
}

pub struct VirtIoBlk {
    sectors: u64,
    inner: Mutex<VirtIoBlkInner>,
}

impl VirtIoBlk {
    unsafe fn read_reg(base: usize, offset: usize) -> u32 {
        read_volatile((base + offset) as *const u32)
    }

    unsafe fn write_reg(base: usize, offset: usize, val: u32) {
        write_volatile((base + offset) as *mut u32, val);
    }

    /// Scan all 32 VirtIO MMIO slots on QEMU `-M virt` for a VirtIO Block Device (`DeviceID == 2`).
    pub fn probe_any() -> Option<Self> {
        for slot in 0..VIRTIO_MMIO_COUNT {
            let base = VIRTIO_MMIO_BASE + slot * VIRTIO_MMIO_STRIDE;
            if let Some(dev) = unsafe { Self::probe_at(base) } {
                return Some(dev);
            }
        }
        None
    }

    unsafe fn probe_at(base: usize) -> Option<Self> {
        let magic = Self::read_reg(base, REG_MAGIC);
        if magic != VIRTIO_MAGIC {
            return None;
        }
        let version = Self::read_reg(base, REG_VERSION);
        if version != 1 && version != 2 {
            return None;
        }
        let device_id = Self::read_reg(base, REG_DEVICE_ID);
        if device_id != VIRTIO_DEV_BLK {
            return None;
        }

        // Read-only model disks belong to Forge, never to the writable root FS.
        Self::write_reg(base, 0x014, 0);
        if Self::read_reg(base, REG_DEVICE_FEATURES) & (1 << 5) != 0 {
            return None;
        }

        // 1. Reset device
        Self::write_reg(base, REG_STATUS, 0);

        // 2. Set ACKNOWLEDGE and DRIVER status bits
        let mut status = STATUS_ACKNOWLEDGE | STATUS_DRIVER;
        Self::write_reg(base, REG_STATUS, status);

        // 3. Negotiate features (0 = standard basic block read/write)
        let _dev_features = Self::read_reg(base, REG_DEVICE_FEATURES);
        Self::write_reg(base, REG_DRIVER_FEATURES, 0);

        if version == 2 {
            status |= STATUS_FEATURES_OK;
            Self::write_reg(base, REG_STATUS, status);
            if (Self::read_reg(base, REG_STATUS) & STATUS_FEATURES_OK) == 0 {
                return None;
            }
        } else {
            Self::write_reg(base, REG_GUEST_PAGE_SIZE, 4096);
        }

        // 4. Configure Virtqueue 0
        Self::write_reg(base, REG_QUEUE_SEL, 0);
        let max_queue = Self::read_reg(base, REG_QUEUE_NUM_MAX);
        if max_queue < QUEUE_SIZE as u32 {
            return None;
        }
        Self::write_reg(base, REG_QUEUE_NUM, QUEUE_SIZE as u32);

        // Zero queue memory
        core::ptr::write_bytes(&raw mut VIRTQ_MEM as *mut u8, 0, core::mem::size_of::<VirtQueueMemory>());

        let q_phys = &raw const VIRTQ_MEM as usize as u64;
        let desc_phys = &raw const VIRTQ_MEM.desc as usize as u64;
        let avail_phys = &raw const VIRTQ_MEM.avail as usize as u64;
        let used_phys = &raw const VIRTQ_MEM.used as usize as u64;

        if version == 1 {
            Self::write_reg(base, REG_QUEUE_ALIGN, 4096);
            Self::write_reg(base, REG_QUEUE_PFN, (q_phys >> 12) as u32);
        } else {
            Self::write_reg(base, REG_QUEUE_DESC_LOW, desc_phys as u32);
            Self::write_reg(base, REG_QUEUE_DESC_HIGH, (desc_phys >> 32) as u32);
            Self::write_reg(base, REG_QUEUE_AVAIL_LOW, avail_phys as u32);
            Self::write_reg(base, REG_QUEUE_AVAIL_HIGH, (avail_phys >> 32) as u32);
            Self::write_reg(base, REG_QUEUE_USED_LOW, used_phys as u32);
            Self::write_reg(base, REG_QUEUE_USED_HIGH, (used_phys >> 32) as u32);
            Self::write_reg(base, REG_QUEUE_READY, 1);
        }

        // 5. Mark DRIVER_OK
        status |= STATUS_DRIVER_OK;
        Self::write_reg(base, REG_STATUS, status);

        // Read capacity in 512-byte sectors from config space
        let cap_low = Self::read_reg(base, REG_CONFIG) as u64;
        let cap_high = Self::read_reg(base, REG_CONFIG + 4) as u64;
        let sectors = cap_low | (cap_high << 32);
        if sectors == 0 {
            return None;
        }

        Some(Self {
            sectors,
            inner: Mutex::new(VirtIoBlkInner {
                base,
                last_used_idx: 0,
            }),
        })
    }

    fn do_request(&self, lba: u64, write: bool, buf: &mut [u8]) -> Result<(), BlockError> {
        let mut guard = self.inner.lock();
        unsafe {
            REQ_BUF.header.type_ = if write {
                VIRTIO_BLK_T_OUT
            } else {
                VIRTIO_BLK_T_IN
            };
            REQ_BUF.header.reserved = 0;
            REQ_BUF.header.sector = lba;
            REQ_BUF.status = 0xFF;

            if write {
                core::ptr::copy_nonoverlapping(
                    buf.as_ptr(),
                    &raw mut REQ_BUF.data as *mut u8,
                    512,
                );
            }

            // Descriptor 0: Header
            VIRTQ_MEM.desc[0] = VirtqDesc {
                addr: &raw const REQ_BUF.header as usize as u64,
                len: core::mem::size_of::<VirtioBlkReqHeader>() as u32,
                flags: VRING_DESC_F_NEXT,
                next: 1,
            };

            // Descriptor 1: 512-byte Sector Data
            VIRTQ_MEM.desc[1] = VirtqDesc {
                addr: &raw mut REQ_BUF.data as usize as u64,
                len: 512,
                flags: VRING_DESC_F_NEXT | if write { 0 } else { VRING_DESC_F_WRITE },
                next: 2,
            };

            // Descriptor 2: 1-byte Status
            VIRTQ_MEM.desc[2] = VirtqDesc {
                addr: &raw mut REQ_BUF.status as usize as u64,
                len: 1,
                flags: VRING_DESC_F_WRITE,
                next: 0,
            };

            let avail_idx = read_volatile(&raw const VIRTQ_MEM.avail.idx);
            write_volatile(
                &raw mut VIRTQ_MEM.avail.ring[(avail_idx as usize) % QUEUE_SIZE],
                0,
            );
            fence(Ordering::SeqCst);
            write_volatile(&raw mut VIRTQ_MEM.avail.idx, avail_idx.wrapping_add(1));
            fence(Ordering::SeqCst);

            Self::write_reg(guard.base, REG_QUEUE_NOTIFY, 0);

            let target_used = guard.last_used_idx.wrapping_add(1);
            for _ in 0..10_000_000 {
                fence(Ordering::SeqCst);
                let cur_used = read_volatile(&raw const VIRTQ_MEM.used.idx);
                if cur_used == target_used {
                    guard.last_used_idx = target_used;
                    let status = read_volatile(&raw const REQ_BUF.status);
                    if status != 0 {
                        return Err(BlockError::IoError);
                    }
                    if !write {
                        core::ptr::copy_nonoverlapping(
                            &raw const REQ_BUF.data as *const u8,
                            buf.as_mut_ptr(),
                            512,
                        );
                    }
                    return Ok(());
                }
                core::hint::spin_loop();
            }

            Err(BlockError::IoError)
        }
    }
}

impl BlockDevice for VirtIoBlk {
    fn block_size(&self) -> usize {
        512
    }

    fn block_count(&self) -> u64 {
        self.sectors
    }

    fn read_block(&self, lba: u64, buf: &mut [u8]) -> Result<(), BlockError> {
        if buf.len() != 512 {
            return Err(BlockError::InvalidBufferSize);
        }
        if lba >= self.sectors {
            return Err(BlockError::OutOfBounds);
        }
        self.do_request(lba, false, buf)
    }

    fn write_block(&self, lba: u64, buf: &[u8]) -> Result<(), BlockError> {
        if buf.len() != 512 {
            return Err(BlockError::InvalidBufferSize);
        }
        if lba >= self.sectors {
            return Err(BlockError::OutOfBounds);
        }
        let mut temp = [0u8; 512];
        temp.copy_from_slice(buf);
        self.do_request(lba, true, &mut temp)
    }
}
