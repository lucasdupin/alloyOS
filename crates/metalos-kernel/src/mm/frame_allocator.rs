use x86_64::structures::paging::{FrameAllocator, FrameDeallocator, PhysFrame, Size4KiB};
use x86_64::PhysAddr;

pub const MAX_BITMAP_WORDS: usize = 4096; // 4096 * 64 = 262,144 frames = 1 GiB capacity

pub struct BitmapFrameAllocator {
    bitmap: [u64; MAX_BITMAP_WORDS],
    start_addr: u64,
    total_frames: usize,
    allocated_count: usize,
}

impl BitmapFrameAllocator {
    pub const fn new(start_addr: u64, total_frames: usize) -> Self {
        let max_frames = MAX_BITMAP_WORDS * 64;
        let frames = if total_frames > max_frames {
            max_frames
        } else {
            total_frames
        };
        Self {
            bitmap: [0; MAX_BITMAP_WORDS],
            start_addr,
            total_frames: frames,
            allocated_count: 0,
        }
    }

    pub fn total_frames(&self) -> usize {
        self.total_frames
    }

    pub fn allocated_count(&self) -> usize {
        self.allocated_count
    }

    pub fn available_frames(&self) -> usize {
        self.total_frames.saturating_sub(self.allocated_count)
    }

    pub fn allocate_frame_custom(&mut self) -> Option<PhysFrame<Size4KiB>> {
        for word_idx in 0..MAX_BITMAP_WORDS {
            let word = self.bitmap[word_idx];
            if word == u64::MAX {
                continue;
            }

            let bit_idx = (!word).trailing_zeros() as usize;
            let frame_idx = word_idx * 64 + bit_idx;
            if frame_idx >= self.total_frames {
                return None;
            }

            self.bitmap[word_idx] |= 1 << bit_idx;
            self.allocated_count += 1;

            let phys_addr = PhysAddr::new(self.start_addr + (frame_idx as u64) * 4096);
            return Some(PhysFrame::containing_address(phys_addr));
        }

        None
    }

    pub fn deallocate_frame_custom(&mut self, frame: PhysFrame<Size4KiB>) {
        let addr = frame.start_address().as_u64();
        if addr < self.start_addr {
            return;
        }

        let frame_idx = ((addr - self.start_addr) / 4096) as usize;
        if frame_idx >= self.total_frames {
            return;
        }

        let word_idx = frame_idx / 64;
        let bit_idx = frame_idx % 64;

        if (self.bitmap[word_idx] & (1 << bit_idx)) != 0 {
            self.bitmap[word_idx] &= !(1 << bit_idx);
            self.allocated_count = self.allocated_count.saturating_sub(1);
        }
    }

    pub fn init_from_bootloader_regions(
        regions: &bootloader::info::MemoryRegions,
    ) -> Option<Self> {
        for region in regions.iter() {
            if region.kind == bootloader::info::MemoryRegionKind::Usable {
                let start = (region.start + 0xFFF) & !0xFFF;
                let end = region.end & !0xFFF;
                if end > start {
                    let total_frames = ((end - start) / 4096) as usize;
                    return Some(Self::new(start, total_frames));
                }
            }
        }
        None
    }
}

unsafe impl FrameAllocator<Size4KiB> for BitmapFrameAllocator {
    fn allocate_frame(&mut self) -> Option<PhysFrame<Size4KiB>> {
        self.allocate_frame_custom()
    }
}

impl FrameDeallocator<Size4KiB> for BitmapFrameAllocator {
    unsafe fn deallocate_frame(&mut self, frame: PhysFrame<Size4KiB>) {
        self.deallocate_frame_custom(frame);
    }
}
