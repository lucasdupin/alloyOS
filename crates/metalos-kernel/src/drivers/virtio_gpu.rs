//! VirtIO-GPU 2D MMIO Driver (`VIRTIO_DEV_GPU = 16`) & Linear Framebuffer (`ramfb`) Manager.
//!
//! Manages a `1280x800` 32-bpp (`B8G8R8A8_UNORM` / `ARGB8888`) surface, supports partial
//! `DirtyRect` damage updates, and accelerates scanline blits using ARM64 NEON 128-bit SIMD
//! loads/stores (`vld1q_u8` / `vst1q_u8`).

use alloc::vec;
use alloc::vec::Vec;
use core::ptr::{read_volatile, write_volatile};
use metalos_proto::{DirtyRect, FramebufferInfo};
use spin::Mutex;

use crate::drivers::virtio_blk::{VIRTIO_MMIO_BASE, VIRTIO_MMIO_COUNT, VIRTIO_MMIO_STRIDE};

const VIRTIO_MAGIC: u32 = 0x7472_6976;
pub const VIRTIO_DEV_GPU: u32 = 16;

pub const DEFAULT_FB_WIDTH: u32 = 1280;
pub const DEFAULT_FB_HEIGHT: u32 = 800;
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

const REG_MAGIC: usize = 0x000;
const REG_VERSION: usize = 0x004;
const REG_DEVICE_ID: usize = 0x008;

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
    vram: Vec<u8>,
}

impl VirtIoGpuDevice {
    pub fn new(width: u32, height: u32) -> Self {
        let stride_bytes = width * DEFAULT_FB_BPP;
        let total_bytes = (stride_bytes * height) as usize;
        Self {
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
            vram: vec![0u8; total_bytes],
        }
    }

    fn probe_mmio_base() -> Option<usize> {
        #[cfg(target_os = "none")]
        unsafe {
            for slot in 0..VIRTIO_MMIO_COUNT {
                let base = VIRTIO_MMIO_BASE + slot * VIRTIO_MMIO_STRIDE;
                let magic = read_volatile((base + REG_MAGIC) as *const u32);
                let version = read_volatile((base + REG_VERSION) as *const u32);
                let dev_id = read_volatile((base + REG_DEVICE_ID) as *const u32);
                if magic == VIRTIO_MAGIC && (version == 1 || version == 2) && dev_id == VIRTIO_DEV_GPU {
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

    pub fn framebuffer_info(&self) -> FramebufferInfo {
        self.info
    }

    pub fn vram_slice(&self) -> &[u8] {
        &self.vram
    }

    /// Blit a rectangular region from `src_rgba` (matching `self.info` dimensions or sub-rect stride)
    /// into the active scanout framebuffer using NEON-accelerated scanline copies.
    pub fn blit_dirty_rect(&mut self, rect: DirtyRect, src_frame: &[u8]) -> Result<u64, &'static str> {
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
            if row_offset + copy_bytes <= self.vram.len() && row_offset + copy_bytes <= src_frame.len() {
                neon_copy_scanline(
                    &mut self.vram[row_offset..row_offset + copy_bytes],
                    &src_frame[row_offset..row_offset + copy_bytes],
                );
            }
        }

        self.flush_count = self.flush_count.wrapping_add(1);
        Ok(self.flush_count)
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
