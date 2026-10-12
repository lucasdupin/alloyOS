//! VirtIO-Net MMIO Network Interface Driver for QEMU `-M virt` (`0x0a00_0000..0x0a00_4000`).
//!
//! Probes for `VIRTIO_DEV_NET = 1`, reads the 6-byte hardware MAC address from config space,
//! configures Virtqueue 0 (RX) and Virtqueue 1 (TX), and supports raw Ethernet frame
//! transmission/reception with a graceful virtual loopback fallback when running headless/offline.

use alloc::collections::VecDeque;
use alloc::vec::Vec;
use core::ptr::{read_volatile, write_volatile};
use core::sync::atomic::{fence, Ordering};
use spin::Mutex;

#[allow(unused_imports)]
use crate::drivers::virtio_blk::{VIRTIO_MMIO_BASE, VIRTIO_MMIO_COUNT, VIRTIO_MMIO_STRIDE};

const VIRTIO_MAGIC: u32 = 0x7472_6976; // "virt"
pub const VIRTIO_DEV_NET: u32 = 1;

const REG_MAGIC: usize = 0x000;
const REG_VERSION: usize = 0x004;
const REG_DEVICE_ID: usize = 0x008;
const REG_DEVICE_FEATURES: usize = 0x010;
const REG_DRIVER_FEATURES: usize = 0x020;
const REG_GUEST_PAGE_SIZE: usize = 0x028;
const REG_QUEUE_SEL: usize = 0x030;
const REG_QUEUE_NUM_MAX: usize = 0x034;
const REG_QUEUE_NUM: usize = 0x038;
const REG_QUEUE_ALIGN: usize = 0x03C;
const REG_QUEUE_PFN: usize = 0x040;
const REG_QUEUE_READY: usize = 0x044;
const REG_QUEUE_NOTIFY: usize = 0x050;
const REG_STATUS: usize = 0x070;
const REG_QUEUE_DESC_LOW: usize = 0x080;
const REG_QUEUE_DESC_HIGH: usize = 0x084;
const REG_QUEUE_AVAIL_LOW: usize = 0x090;
const REG_QUEUE_AVAIL_HIGH: usize = 0x094;
const REG_QUEUE_USED_LOW: usize = 0x0A0;
const REG_QUEUE_USED_HIGH: usize = 0x0A4;
const REG_CONFIG: usize = 0x100;

const STATUS_ACKNOWLEDGE: u32 = 1;
const STATUS_DRIVER: u32 = 2;
const STATUS_DRIVER_OK: u32 = 4;
const STATUS_FEATURES_OK: u32 = 8;

const VIRTIO_NET_F_MAC: u32 = 1 << 5;

const QUEUE_SIZE: usize = 16;
const VRING_DESC_F_WRITE: u16 = 2;
const MAX_FRAME_LEN: usize = 1526;

/// 10-byte legacy / 12-byte modern VirtIO-Net packet header.
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct VirtioNetHdr {
    pub flags: u8,
    pub gso_type: u8,
    pub hdr_len: u16,
    pub gso_size: u16,
    pub csum_start: u16,
    pub csum_offset: u16,
}

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

#[repr(C, align(4096))]
struct NetVirtQueueMemory {
    desc: [VirtqDesc; QUEUE_SIZE],
    avail: VirtqAvail,
    _pad0: [u8; 4096 - 256 - core::mem::size_of::<VirtqAvail>()],
    used: VirtqUsed,
}

#[repr(C, align(64))]
struct NetBouncePacket {
    hdr: VirtioNetHdr,
    frame: [u8; MAX_FRAME_LEN],
}

static mut RX_QUEUE_MEM: NetVirtQueueMemory = NetVirtQueueMemory {
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

static mut TX_QUEUE_MEM: NetVirtQueueMemory = NetVirtQueueMemory {
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

static mut RX_BUF: NetBouncePacket = NetBouncePacket {
    hdr: VirtioNetHdr {
        flags: 0,
        gso_type: 0,
        hdr_len: 0,
        gso_size: 0,
        csum_start: 0,
        csum_offset: 0,
    },
    frame: [0; MAX_FRAME_LEN],
};

static mut TX_BUF: NetBouncePacket = NetBouncePacket {
    hdr: VirtioNetHdr {
        flags: 0,
        gso_type: 0,
        hdr_len: 0,
        gso_size: 0,
        csum_start: 0,
        csum_offset: 0,
    },
    frame: [0; MAX_FRAME_LEN],
};

pub struct VirtIoNetDevice {
    base: Option<usize>,
    pub mac: [u8; 6],
    rx_last_used: u16,
    tx_last_used: u16,
    loopback_rx: VecDeque<Vec<u8>>,
}

impl VirtIoNetDevice {
    pub const DEFAULT_MAC: [u8; 6] = [0x52, 0x54, 0x00, 0x12, 0x34, 0x56];

    pub const fn new_loopback() -> Self {
        Self {
            base: None,
            mac: Self::DEFAULT_MAC,
            rx_last_used: 0,
            tx_last_used: 0,
            loopback_rx: VecDeque::new(),
        }
    }

    unsafe fn read_reg(base: usize, offset: usize) -> u32 {
        read_volatile((base + offset) as *const u32)
    }

    unsafe fn write_reg(base: usize, offset: usize, val: u32) {
        write_volatile((base + offset) as *mut u32, val);
    }

    pub fn probe_or_loopback() -> Self {
        #[cfg(target_os = "none")]
        {
            for slot in 0..VIRTIO_MMIO_COUNT {
                let base = VIRTIO_MMIO_BASE + slot * VIRTIO_MMIO_STRIDE;
                if let Some(dev) = unsafe { Self::probe_at(base) } {
                    return dev;
                }
            }
        }
        Self::new_loopback()
    }

    #[allow(dead_code)]
    unsafe fn setup_queue(base: usize, version: u32, queue_idx: u32, q_mem: *mut NetVirtQueueMemory) -> bool {
        Self::write_reg(base, REG_QUEUE_SEL, queue_idx);
        let max_q = Self::read_reg(base, REG_QUEUE_NUM_MAX);
        if max_q < QUEUE_SIZE as u32 {
            return false;
        }
        Self::write_reg(base, REG_QUEUE_NUM, QUEUE_SIZE as u32);
        core::ptr::write_bytes(q_mem as *mut u8, 0, core::mem::size_of::<NetVirtQueueMemory>());

        let q_phys = q_mem as usize as u64;
        let desc_phys = &raw const (*q_mem).desc as usize as u64;
        let avail_phys = &raw const (*q_mem).avail as usize as u64;
        let used_phys = &raw const (*q_mem).used as usize as u64;

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
        true
    }

    #[allow(dead_code)]
    unsafe fn probe_at(base: usize) -> Option<Self> {
        if Self::read_reg(base, REG_MAGIC) != VIRTIO_MAGIC {
            return None;
        }
        let version = Self::read_reg(base, REG_VERSION);
        if version != 1 && version != 2 {
            return None;
        }
        if Self::read_reg(base, REG_DEVICE_ID) != VIRTIO_DEV_NET {
            return None;
        }

        Self::write_reg(base, REG_STATUS, 0);
        let mut status = STATUS_ACKNOWLEDGE | STATUS_DRIVER;
        Self::write_reg(base, REG_STATUS, status);

        let dev_features = Self::read_reg(base, REG_DEVICE_FEATURES);
        let negotiated = dev_features & VIRTIO_NET_F_MAC;
        Self::write_reg(base, REG_DRIVER_FEATURES, negotiated);

        if version == 2 {
            status |= STATUS_FEATURES_OK;
            Self::write_reg(base, REG_STATUS, status);
            if (Self::read_reg(base, REG_STATUS) & STATUS_FEATURES_OK) == 0 {
                return None;
            }
        } else {
            Self::write_reg(base, REG_GUEST_PAGE_SIZE, 4096);
        }

        // Queue 0 = RX, Queue 1 = TX
        if !Self::setup_queue(base, version, 0, &raw mut RX_QUEUE_MEM) {
            return None;
        }
        if !Self::setup_queue(base, version, 1, &raw mut TX_QUEUE_MEM) {
            return None;
        }

        let mut mac = Self::DEFAULT_MAC;
        if (negotiated & VIRTIO_NET_F_MAC) != 0 {
            for (i, byte) in mac.iter_mut().enumerate() {
                *byte = read_volatile((base + REG_CONFIG + i) as *const u8);
            }
        }

        // Pre-post an RX descriptor on Queue 0
        RX_QUEUE_MEM.desc[0] = VirtqDesc {
            addr: &raw mut RX_BUF as usize as u64,
            len: core::mem::size_of::<NetBouncePacket>() as u32,
            flags: VRING_DESC_F_WRITE,
            next: 0,
        };
        RX_QUEUE_MEM.avail.ring[0] = 0;
        fence(Ordering::SeqCst);
        RX_QUEUE_MEM.avail.idx = 1;
        fence(Ordering::SeqCst);

        status |= STATUS_DRIVER_OK;
        Self::write_reg(base, REG_STATUS, status);
        Self::write_reg(base, REG_QUEUE_NOTIFY, 0);

        Some(Self {
            base: Some(base),
            mac,
            rx_last_used: 0,
            tx_last_used: 0,
            loopback_rx: VecDeque::new(),
        })
    }

    pub fn is_hardware(&self) -> bool {
        self.base.is_some()
    }

    /// Inject a frame into the RX queue (used by loopback / virtual responder).
    pub fn inject_rx_frame(&mut self, frame: Vec<u8>) {
        self.loopback_rx.push_back(frame);
    }

    /// Transmit an Ethernet II frame.
    pub fn transmit_frame(&mut self, frame: &[u8]) -> Result<usize, &'static str> {
        if frame.is_empty() || frame.len() > MAX_FRAME_LEN {
            return Err("Invalid Ethernet frame size");
        }

        if let Some(base) = self.base {
            unsafe {
                TX_BUF.hdr = VirtioNetHdr::default();
                core::ptr::copy_nonoverlapping(
                    frame.as_ptr(),
                    &raw mut TX_BUF.frame as *mut u8,
                    frame.len(),
                );

                let total_len = core::mem::size_of::<VirtioNetHdr>() + frame.len();
                TX_QUEUE_MEM.desc[0] = VirtqDesc {
                    addr: &raw const TX_BUF as usize as u64,
                    len: total_len as u32,
                    flags: 0,
                    next: 0,
                };

                let avail_idx = read_volatile(&raw const TX_QUEUE_MEM.avail.idx);
                write_volatile(
                    &raw mut TX_QUEUE_MEM.avail.ring[(avail_idx as usize) % QUEUE_SIZE],
                    0,
                );
                fence(Ordering::SeqCst);
                write_volatile(&raw mut TX_QUEUE_MEM.avail.idx, avail_idx.wrapping_add(1));
                fence(Ordering::SeqCst);

                Self::write_reg(base, REG_QUEUE_NOTIFY, 1);

                let target_used = self.tx_last_used.wrapping_add(1);
                for _ in 0..1_000_000 {
                    fence(Ordering::SeqCst);
                    if read_volatile(&raw const TX_QUEUE_MEM.used.idx) == target_used {
                        self.tx_last_used = target_used;
                        break;
                    }
                    core::hint::spin_loop();
                }
            }
        } else {
            // Loopback fallback: keep a copy in the loopback queue
            self.loopback_rx.push_back(frame.to_vec());
        }

        Ok(frame.len())
    }

    /// Poll for a received Ethernet II frame.
    pub fn poll_receive(&mut self) -> Option<Vec<u8>> {
        if let Some(frame) = self.loopback_rx.pop_front() {
            return Some(frame);
        }

        let base = self.base?;
        unsafe {
            fence(Ordering::SeqCst);
            let used_idx = read_volatile(&raw const RX_QUEUE_MEM.used.idx);
            if used_idx == self.rx_last_used {
                return None;
            }

            let elem = read_volatile(
                &raw const RX_QUEUE_MEM.used.ring[(self.rx_last_used as usize) % QUEUE_SIZE],
            );
            self.rx_last_used = self.rx_last_used.wrapping_add(1);

            let hdr_len = core::mem::size_of::<VirtioNetHdr>();
            let total_len = elem.len as usize;
            let payload_len = total_len.saturating_sub(hdr_len).min(MAX_FRAME_LEN);
            let mut out = Vec::with_capacity(payload_len);
            if payload_len > 0 {
                let slice = core::slice::from_raw_parts(
                    &raw const RX_BUF.frame as *const u8,
                    payload_len,
                );
                out.extend_from_slice(slice);
            }

            // Re-arm RX descriptor 0
            let avail_idx = read_volatile(&raw const RX_QUEUE_MEM.avail.idx);
            write_volatile(
                &raw mut RX_QUEUE_MEM.avail.ring[(avail_idx as usize) % QUEUE_SIZE],
                0,
            );
            fence(Ordering::SeqCst);
            write_volatile(&raw mut RX_QUEUE_MEM.avail.idx, avail_idx.wrapping_add(1));
            fence(Ordering::SeqCst);
            Self::write_reg(base, REG_QUEUE_NOTIFY, 0);

            Some(out)
        }
    }
}

pub static VIRTIO_NET: Mutex<VirtIoNetDevice> = Mutex::new(VirtIoNetDevice::new_loopback());

pub fn init() -> [u8; 6] {
    let dev = VirtIoNetDevice::probe_or_loopback();
    let mac = dev.mac;
    *VIRTIO_NET.lock() = dev;
    mac
}
