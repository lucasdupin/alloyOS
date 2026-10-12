//! VirtIO-GPU 2D MMIO Driver (`VIRTIO_DEV_GPU = 16`) & Linear Framebuffer (`ramfb`) Manager.
//!
//! Manages a `1206x2622` portrait 32-bpp (`B8G8R8A8_UNORM` / `ARGB8888`) surface, supports partial
//! `DirtyRect` damage updates, and accelerates scanline blits using ARM64 NEON 128-bit SIMD
//! loads/stores (`vld1q_u8` / `vst1q_u8`).

use alloc::vec;
use alloc::vec::Vec;
use core::ptr::{read_volatile, write_volatile};
use core::sync::atomic::{fence, Ordering};
use metalos_proto::{DirtyRect, FramebufferInfo};
use spin::Mutex;

use crate::drivers::virtio_blk::{VIRTIO_MMIO_BASE, VIRTIO_MMIO_COUNT, VIRTIO_MMIO_STRIDE};

const VIRTIO_MAGIC: u32 = 0x7472_6976;
pub const VIRTIO_DEV_GPU: u32 = 16;

pub use metalos_proto::display::{
    PHONE_WIDTH as DEFAULT_FB_WIDTH, PHONE_HEIGHT as DEFAULT_FB_HEIGHT,
    PHONE_LOGICAL_WIDTH as UI_WIDTH, PHONE_LOGICAL_HEIGHT as UI_HEIGHT,
};
pub const DEFAULT_FB_BPP: u32 = 4;

// VirtIO-GPU 2D command constants
pub const VIRTIO_GPU_CMD_GET_DISPLAY_INFO: u32 = 0x0100;
pub const VIRTIO_GPU_CMD_RESOURCE_CREATE_2D: u32 = 0x0101;
pub const VIRTIO_GPU_CMD_RESOURCE_UNREF: u32 = 0x0102;
pub const VIRTIO_GPU_CMD_SET_SCANOUT: u32 = 0x0103;
pub const VIRTIO_GPU_CMD_RESOURCE_FLUSH: u32 = 0x0104;
pub const VIRTIO_GPU_CMD_TRANSFER_TO_HOST_2D: u32 = 0x0105;
pub const VIRTIO_GPU_CMD_RESOURCE_ATTACH_BACKING: u32 = 0x0106;
pub const VIRTIO_GPU_FORMAT_B8G8R8A8_UNORM: u32 = 1;
pub const VIRTIO_GPU_RESP_OK_NODATA: u32 = 0x1100;

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

const STATUS_ACKNOWLEDGE: u32 = 1;
const STATUS_DRIVER: u32 = 2;
const STATUS_DRIVER_OK: u32 = 4;
const STATUS_FEATURES_OK: u32 = 8;

const QUEUE_SIZE: usize = 16;
const VRING_DESC_F_NEXT: u16 = 1;
const VRING_DESC_F_WRITE: u16 = 2;

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
struct VirtQueueMemory {
    desc: [VirtqDesc; QUEUE_SIZE],
    avail: VirtqAvail,
    _pad0: [u8; 4096 - 256 - core::mem::size_of::<VirtqAvail>()],
    used: VirtqUsed,
}

static mut GPU_VIRTQ_MEM: VirtQueueMemory = VirtQueueMemory {
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

#[repr(C, align(64))]
struct GpuCmdScratch {
    req: [u8; 128],
    resp: VirtioGpuCtrlHdr,
}

static mut GPU_CMD_SCRATCH: GpuCmdScratch = GpuCmdScratch {
    req: [0; 128],
    resp: VirtioGpuCtrlHdr {
        type_: 0,
        flags: 0,
        fence_id: 0,
        ctx_id: 0,
        padding: 0,
    },
};

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct VirtioGpuCtrlHdr {
    pub type_: u32,
    pub flags: u32,
    pub fence_id: u64,
    pub ctx_id: u32,
    pub padding: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct VirtioGpuRect {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct VirtioGpuResourceCreate2d {
    pub hdr: VirtioGpuCtrlHdr,
    pub resource_id: u32,
    pub format: u32,
    pub width: u32,
    pub height: u32,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct VirtioGpuMemEntry {
    pub addr: u64,
    pub length: u32,
    pub padding: u32,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct VirtioGpuResourceAttachBacking1 {
    pub hdr: VirtioGpuCtrlHdr,
    pub resource_id: u32,
    pub nr_entries: u32,
    pub entry: VirtioGpuMemEntry,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct VirtioGpuSetScanout {
    pub hdr: VirtioGpuCtrlHdr,
    pub r: VirtioGpuRect,
    pub scanout_id: u32,
    pub resource_id: u32,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct VirtioGpuTransferToHost2d {
    pub hdr: VirtioGpuCtrlHdr,
    pub r: VirtioGpuRect,
    pub offset: u64,
    pub resource_id: u32,
    pub padding: u32,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct VirtioGpuResourceFlush {
    pub hdr: VirtioGpuCtrlHdr,
    pub r: VirtioGpuRect,
    pub resource_id: u32,
    pub padding: u32,
}

/// Copy a scanline using 128-bit ARM64 NEON vector registers when compiled for `aarch64`.
#[inline]
pub fn neon_copy_scanline(dst: &mut [u8], src: &[u8]) {
    let len = core::cmp::min(dst.len(), src.len());
    #[cfg(target_arch = "aarch64")]
    unsafe {
        use core::arch::aarch64::{vld1q_u8, vst1q_u8};
        let mut i = 0usize;
        let dst_ptr = dst.as_mut_ptr();
        let src_ptr = src.as_ptr();
        while i + 16 <= len {
            let chunk = vld1q_u8(src_ptr.add(i));
            vst1q_u8(dst_ptr.add(i), chunk);
            i += 16;
        }
        while i < len {
            *dst_ptr.add(i) = *src_ptr.add(i);
            i += 1;
        }
    }
    #[cfg(not(target_arch = "aarch64"))]
    {
        dst[..len].copy_from_slice(&src[..len]);
    }
}

pub struct VirtIoGpuDevice {
    pub mmio_base: Option<usize>,
    pub info: FramebufferInfo,
    pub resource_id: u32,
    pub scanout_configured: bool,
    pub flush_count: u64,
    last_used_idx: u16,
    vram: Vec<u8>,
}

impl VirtIoGpuDevice {
    pub fn new(width: u32, height: u32) -> Self {
        let stride_bytes = width * DEFAULT_FB_BPP;
        let total_bytes = (stride_bytes * height) as usize;
        let mut dev = Self {
            mmio_base: Self::probe_mmio_base(),
            info: FramebufferInfo {
                width,
                height,
                stride_bytes,
                bytes_per_pixel: DEFAULT_FB_BPP,
            },
            resource_id: 1,
            scanout_configured: true,
            flush_count: 0,
            last_used_idx: 0,
            vram: vec![0u8; total_bytes],
        };
        dev.init_hardware_scanout();
        dev
    }

    #[inline]
    unsafe fn read_reg(base: usize, offset: usize) -> u32 {
        read_volatile((base + offset) as *const u32)
    }

    #[inline]
    unsafe fn write_reg(base: usize, offset: usize, val: u32) {
        write_volatile((base + offset) as *mut u32, val);
    }

    fn probe_mmio_base() -> Option<usize> {
        #[cfg(target_os = "none")]
        unsafe {
            for slot in 0..VIRTIO_MMIO_COUNT {
                let base = VIRTIO_MMIO_BASE + slot * VIRTIO_MMIO_STRIDE;
                let magic = read_volatile((base + REG_MAGIC) as *const u32);
                let version = read_volatile((base + REG_VERSION) as *const u32);
                let dev_id = read_volatile((base + REG_DEVICE_ID) as *const u32);
                if magic == VIRTIO_MAGIC
                    && (version == 1 || version == 2)
                    && dev_id == VIRTIO_DEV_GPU
                {
                    let _ = write_volatile::<u32>;
                    return Some(base);
                }
            }
        }
        let _ = (
            VIRTIO_MMIO_BASE,
            VIRTIO_MMIO_COUNT,
            VIRTIO_MMIO_STRIDE,
            VIRTIO_MAGIC,
            REG_MAGIC,
            REG_VERSION,
            REG_DEVICE_ID,
            read_volatile::<u32>,
            write_volatile::<u32>,
        );
        None
    }

    fn init_hardware_scanout(&mut self) {
        #[cfg(target_os = "none")]
        if let Some(base) = self.mmio_base {
            unsafe {
                let version = Self::read_reg(base, REG_VERSION);
                Self::write_reg(base, REG_STATUS, 0);
                let mut status = STATUS_ACKNOWLEDGE | STATUS_DRIVER;
                Self::write_reg(base, REG_STATUS, status);

                let _dev_features = Self::read_reg(base, REG_DEVICE_FEATURES);
                Self::write_reg(base, REG_DRIVER_FEATURES, 0);

                if version == 2 {
                    status |= STATUS_FEATURES_OK;
                    Self::write_reg(base, REG_STATUS, status);
                    if (Self::read_reg(base, REG_STATUS) & STATUS_FEATURES_OK) == 0 {
                        self.mmio_base = None;
                        return;
                    }
                } else {
                    Self::write_reg(base, REG_GUEST_PAGE_SIZE, 4096);
                }

                Self::write_reg(base, REG_QUEUE_SEL, 0);
                let max_queue = Self::read_reg(base, REG_QUEUE_NUM_MAX);
                if max_queue < QUEUE_SIZE as u32 {
                    self.mmio_base = None;
                    return;
                }
                Self::write_reg(base, REG_QUEUE_NUM, QUEUE_SIZE as u32);

                core::ptr::write_bytes(
                    &raw mut GPU_VIRTQ_MEM as *mut u8,
                    0,
                    core::mem::size_of::<VirtQueueMemory>(),
                );

                let q_phys = &raw const GPU_VIRTQ_MEM as usize as u64;
                let desc_phys = &raw const GPU_VIRTQ_MEM.desc as usize as u64;
                let avail_phys = &raw const GPU_VIRTQ_MEM.avail as usize as u64;
                let used_phys = &raw const GPU_VIRTQ_MEM.used as usize as u64;

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

                status |= STATUS_DRIVER_OK;
                Self::write_reg(base, REG_STATUS, status);

                // 1. RESOURCE_CREATE_2D
                let create_cmd = VirtioGpuResourceCreate2d {
                    hdr: VirtioGpuCtrlHdr {
                        type_: VIRTIO_GPU_CMD_RESOURCE_CREATE_2D,
                        flags: 0,
                        fence_id: 0,
                        ctx_id: 0,
                        padding: 0,
                    },
                    resource_id: self.resource_id,
                    format: VIRTIO_GPU_FORMAT_B8G8R8A8_UNORM,
                    width: self.info.width,
                    height: self.info.height,
                };
                self.submit_cmd(base, &create_cmd);

                // 2. RESOURCE_ATTACH_BACKING
                let attach_cmd = VirtioGpuResourceAttachBacking1 {
                    hdr: VirtioGpuCtrlHdr {
                        type_: VIRTIO_GPU_CMD_RESOURCE_ATTACH_BACKING,
                        flags: 0,
                        fence_id: 0,
                        ctx_id: 0,
                        padding: 0,
                    },
                    resource_id: self.resource_id,
                    nr_entries: 1,
                    entry: VirtioGpuMemEntry {
                        addr: self.vram.as_ptr() as u64,
                        length: self.vram.len() as u32,
                        padding: 0,
                    },
                };
                self.submit_cmd(base, &attach_cmd);

                // 3. SET_SCANOUT
                let scanout_cmd = VirtioGpuSetScanout {
                    hdr: VirtioGpuCtrlHdr {
                        type_: VIRTIO_GPU_CMD_SET_SCANOUT,
                        flags: 0,
                        fence_id: 0,
                        ctx_id: 0,
                        padding: 0,
                    },
                    r: VirtioGpuRect {
                        x: 0,
                        y: 0,
                        width: self.info.width,
                        height: self.info.height,
                    },
                    scanout_id: 0,
                    resource_id: self.resource_id,
                };
                self.submit_cmd(base, &scanout_cmd);
                self.transfer_and_flush_rows(0, self.info.height);
            }
        }
        let _ = (
            REG_DEVICE_FEATURES,
            REG_DRIVER_FEATURES,
            REG_GUEST_PAGE_SIZE,
            REG_QUEUE_SEL,
            REG_QUEUE_NUM_MAX,
            REG_QUEUE_NUM,
            REG_QUEUE_ALIGN,
            REG_QUEUE_PFN,
            REG_QUEUE_READY,
            REG_QUEUE_NOTIFY,
            REG_STATUS,
            REG_QUEUE_DESC_LOW,
            REG_QUEUE_DESC_HIGH,
            REG_QUEUE_AVAIL_LOW,
            REG_QUEUE_AVAIL_HIGH,
            REG_QUEUE_USED_LOW,
            REG_QUEUE_USED_HIGH,
            STATUS_ACKNOWLEDGE,
            STATUS_DRIVER,
            STATUS_DRIVER_OK,
            STATUS_FEATURES_OK,
            VRING_DESC_F_NEXT,
            VRING_DESC_F_WRITE,
            self.last_used_idx,
        );
    }

    #[cfg(target_os = "none")]
    unsafe fn submit_cmd<T: Copy>(&mut self, base: usize, cmd: &T) {
        let cmd_len = core::mem::size_of::<T>();
        if cmd_len > 128 {
            return;
        }
        core::ptr::copy_nonoverlapping(
            cmd as *const T as *const u8,
            &raw mut GPU_CMD_SCRATCH.req as *mut u8,
            cmd_len,
        );
        GPU_CMD_SCRATCH.resp = VirtioGpuCtrlHdr::default();

        GPU_VIRTQ_MEM.desc[0] = VirtqDesc {
            addr: &raw const GPU_CMD_SCRATCH.req as usize as u64,
            len: cmd_len as u32,
            flags: VRING_DESC_F_NEXT,
            next: 1,
        };
        GPU_VIRTQ_MEM.desc[1] = VirtqDesc {
            addr: &raw mut GPU_CMD_SCRATCH.resp as usize as u64,
            len: core::mem::size_of::<VirtioGpuCtrlHdr>() as u32,
            flags: VRING_DESC_F_WRITE,
            next: 0,
        };

        let avail_idx = read_volatile(&raw const GPU_VIRTQ_MEM.avail.idx);
        write_volatile(
            &raw mut GPU_VIRTQ_MEM.avail.ring[(avail_idx as usize) % QUEUE_SIZE],
            0,
        );
        fence(Ordering::SeqCst);
        write_volatile(&raw mut GPU_VIRTQ_MEM.avail.idx, avail_idx.wrapping_add(1));
        fence(Ordering::SeqCst);

        Self::write_reg(base, REG_QUEUE_NOTIFY, 0);

        let target_used = self.last_used_idx.wrapping_add(1);
        for _ in 0..10_000_000 {
            fence(Ordering::SeqCst);
            let cur_used = read_volatile(&raw const GPU_VIRTQ_MEM.used.idx);
            if cur_used == target_used {
                self.last_used_idx = target_used;
                break;
            }
            core::hint::spin_loop();
        }
    }

    pub fn transfer_and_flush_rows(&mut self, y_start: u32, height: u32) {
        #[cfg(target_os = "none")]
        if let Some(base) = self.mmio_base {
            let y0 = y_start.min(self.info.height);
            let h = height.min(self.info.height.saturating_sub(y0));
            if h == 0 {
                return;
            }
            let offset = (y0 as u64) * (self.info.stride_bytes as u64);
            let rect = VirtioGpuRect {
                x: 0,
                y: y0,
                width: self.info.width,
                height: h,
            };
            let transfer_cmd = VirtioGpuTransferToHost2d {
                hdr: VirtioGpuCtrlHdr {
                    type_: VIRTIO_GPU_CMD_TRANSFER_TO_HOST_2D,
                    flags: 0,
                    fence_id: 0,
                    ctx_id: 0,
                    padding: 0,
                },
                r: rect,
                offset,
                resource_id: self.resource_id,
                padding: 0,
            };
            let flush_cmd = VirtioGpuResourceFlush {
                hdr: VirtioGpuCtrlHdr {
                    type_: VIRTIO_GPU_CMD_RESOURCE_FLUSH,
                    flags: 0,
                    fence_id: 0,
                    ctx_id: 0,
                    padding: 0,
                },
                r: rect,
                resource_id: self.resource_id,
                padding: 0,
            };
            unsafe {
                self.submit_cmd(base, &transfer_cmd);
                self.submit_cmd(base, &flush_cmd);
            }
        }
        let _ = (y_start, height);
    }

    pub fn framebuffer_info(&self) -> FramebufferInfo {
        self.info
    }

    pub fn vram_slice(&self) -> &[u8] {
        &self.vram
    }

    /// Blit a rectangular region from `src_rgba` (matching `self.info` dimensions or sub-rect stride)
    /// into the active scanout framebuffer using NEON-accelerated scanline copies.
    pub fn blit_dirty_rect(
        &mut self,
        rect: DirtyRect,
        src_frame: &[u8],
    ) -> Result<u64, &'static str> {
        let x_end = rect.x.saturating_add(rect.width).min(self.info.width);
        let y_end = rect.y.saturating_add(rect.height).min(self.info.height);
        if rect.x >= x_end || rect.y >= y_end {
            return Ok(self.flush_count);
        }

        let bpp = self.info.bytes_per_pixel as usize;
        let stride = self.info.stride_bytes as usize;
        let copy_bytes = ((x_end - rect.x) as usize) * bpp;

        for row in rect.y..y_end {
            let row_offset = (row as usize) * stride + (rect.x as usize) * bpp;
            if row_offset + copy_bytes <= self.vram.len()
                && row_offset + copy_bytes <= src_frame.len()
            {
                neon_copy_scanline(
                    &mut self.vram[row_offset..row_offset + copy_bytes],
                    &src_frame[row_offset..row_offset + copy_bytes],
                );
            }
        }

        self.transfer_and_flush_rows(rect.y, y_end - rect.y);
        self.flush_count = self.flush_count.wrapping_add(1);
        Ok(self.flush_count)
    }

    /// Scale an ARGB8888 `u32` buffer (`src_w x src_h`) into the portrait scanout VRAM
    /// and flush the updated scanlines to the hardware VirtIO-GPU display.
    pub fn present_scaled_argb(
        &mut self,
        src: &[u32],
        src_w: u32,
        src_h: u32,
        dirty: Option<DirtyRect>,
    ) {
        if src_w == 0 || src_h == 0 || src.len() < (src_w as usize) * (src_h as usize) {
            return;
        }
        let scale_x = (self.info.width / src_w).max(1);
        let scale_y = (self.info.height / src_h).max(1);

        let (sy0, sy1) = if let Some(d) = dirty {
            let y0 = d.y.min(src_h);
            let y1 = d.y.saturating_add(d.height).min(src_h);
            (y0, y1)
        } else {
            (0, src_h)
        };
        if sy0 >= sy1 {
            return;
        }

        let dst_w = self.info.width as usize;
        let vram_u32 = unsafe {
            core::slice::from_raw_parts_mut(
                self.vram.as_mut_ptr() as *mut u32,
                self.vram.len() / 4,
            )
        };

        let sx_scale = scale_x as usize;
        let sy_scale = scale_y as usize;
        let src_w_usize = src_w as usize;

        for sy in sy0..sy1 {
            let src_row = &src[(sy as usize) * src_w_usize..(sy as usize + 1) * src_w_usize];
            let base_dy = (sy as usize) * sy_scale;
            if base_dy >= self.info.height as usize {
                break;
            }
            let first_dst_start = base_dy * dst_w;
            for (sx, &px) in src_row.iter().enumerate() {
                let dx_start = first_dst_start + sx * sx_scale;
                for dx in 0..sx_scale {
                    if dx_start + dx < vram_u32.len() {
                        vram_u32[dx_start + dx] = px;
                    }
                }
            }
            // Duplicate the scaled scanline vertically for `scale_y - 1` rows
            for dy_off in 1..sy_scale {
                let dst_y = base_dy + dy_off;
                if dst_y >= self.info.height as usize {
                    break;
                }
                let dst_start = dst_y * dst_w;
                vram_u32.copy_within(first_dst_start..first_dst_start + dst_w, dst_start);
            }
        }

        let flush_y0 = sy0 * scale_y;
        let flush_h = (sy1 - sy0) * scale_y;
        self.transfer_and_flush_rows(flush_y0, flush_h);
        self.flush_count = self.flush_count.wrapping_add(1);
    }

    /// Read the 32-bit ARGB8888 pixel at `(x, y)`.
    pub fn read_pixel(&self, x: u32, y: u32) -> Option<u32> {
        if x >= self.info.width || y >= self.info.height {
            return None;
        }
        let offset = (y * self.info.stride_bytes + x * self.info.bytes_per_pixel) as usize;
        let bytes = self.vram.get(offset..offset + 4)?;
        Some(u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
    }
}

pub static VIRTIO_GPU: Mutex<Option<VirtIoGpuDevice>> = Mutex::new(None);

pub fn init() -> FramebufferInfo {
    let dev = VirtIoGpuDevice::new(DEFAULT_FB_WIDTH, DEFAULT_FB_HEIGHT);
    let info = dev.framebuffer_info();
    *VIRTIO_GPU.lock() = Some(dev);
    info
}

pub fn has_hardware_display() -> bool {
    VIRTIO_GPU
        .lock()
        .as_ref()
        .and_then(|d| d.mmio_base)
        .is_some()
}

/// Present an already rendered frame at the shared phone geometry.
pub fn present_ui_frame(pixels: &[u32], width: u32, height: u32) {
    if let Some(gpu) = VIRTIO_GPU.lock().as_mut() {
        if gpu.mmio_base.is_some() {
            gpu.present_scaled_argb(pixels, width, height, None);
        }
    }
}

/// Fade the static boot scanout into Ferrofluid over half a second.
pub fn crossfade_to_frame(pixels: &[u32], width: u32, height: u32) {
    let mut guard = VIRTIO_GPU.lock();
    let Some(gpu) = guard.as_mut() else { return; };
    if gpu.mmio_base.is_none() || width != gpu.info.width || height != gpu.info.height
        || pixels.len() < width as usize * height as usize
    {
        return;
    }
    let source = gpu.vram.clone();
    const DURATION_NS: u64 = 500_000_000;
    const FRAME_NS: u64 = 16_666_667;
    let start = super::rng::monotonic_time_ns();
    loop {
        let frame_start = super::rng::monotonic_time_ns();
        let elapsed = frame_start.saturating_sub(start).min(DURATION_NS);
        let t = elapsed * 255 / DURATION_NS;
        // Smoothstep eases both ends without moving or deforming the logo.
        let alpha = (t * t * (765 - 2 * t) / (255 * 255)) as u32;
        for y in 0..height as usize {
            for x in 0..width as usize {
                let offset = y * gpu.info.stride_bytes as usize + x * 4;
                let target = pixels[y * width as usize + x].to_le_bytes();
                for channel in 0..3 {
                    gpu.vram[offset + channel] = ((source[offset + channel] as u32 * (255 - alpha)
                        + target[channel] as u32 * alpha + 127) / 255) as u8;
                }
                gpu.vram[offset + 3] = 255;
            }
        }
        gpu.transfer_and_flush_rows(0, height);
        gpu.flush_count = gpu.flush_count.wrapping_add(1);
        if elapsed == DURATION_NS { break; }
        while super::rng::monotonic_time_ns().saturating_sub(frame_start) < FRAME_NS {
            core::hint::spin_loop();
        }
    }
}
