use crate::mm::paging::{FrameAllocator, OffsetPageTable, Size4KiB};
#[cfg(target_os = "none")]
use linked_list_allocator::LockedHeap;

#[cfg(not(feature = "forge"))]
// Private executable pages, UI heap, ELF archive, framebuffers and app engines
// all draw from this allocator. A 256 MiB guest leaves room for kernel/initrd.
pub const HEAP_SIZE: usize = 160 * 1024 * 1024;
// Weights are borrowed from the reserved loader region, not copied into this
// heap. This budget covers model metadata, bounded KV cache, and working memory.
#[cfg(feature = "forge")]
pub const HEAP_SIZE: usize = 256 * 1024 * 1024;

#[cfg(target_os = "none")]
pub static ALLOCATOR: LockedHeap = LockedHeap::empty();

pub fn init_heap(
    _page_table: &mut OffsetPageTable,
    frame_allocator: &mut impl FrameAllocator<Size4KiB>,
) -> Result<(), &'static str> {
    let num_frames = HEAP_SIZE / 4096;
    let mut heap_start_phys: Option<u64> = None;

    for _ in 0..num_frames {
        let frame = frame_allocator
            .allocate_frame()
            .ok_or("Out of physical frames for kernel heap")?;
        if heap_start_phys.is_none() {
            heap_start_phys = Some(frame.start_address().as_u64());
        }
    }

    #[cfg(target_os = "none")]
    unsafe {
        let start = heap_start_phys.ok_or("Empty kernel heap")? as usize;
        ALLOCATOR.lock().init(start as *mut u8, HEAP_SIZE);
    }

    Ok(())
}

#[cfg(target_os = "none")]
struct PreemptionSafeAllocator;
#[cfg(target_os = "none")]
#[global_allocator]
static GLOBAL_ALLOCATOR: PreemptionSafeAllocator = PreemptionSafeAllocator;
#[cfg(target_os = "none")]
unsafe impl core::alloc::GlobalAlloc for PreemptionSafeAllocator {
    unsafe fn alloc(&self, layout: core::alloc::Layout) -> *mut u8 {
        let _interrupts = crate::sync::InterruptGuard::new();
        core::alloc::GlobalAlloc::alloc(&ALLOCATOR, layout)
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: core::alloc::Layout) {
        let _interrupts = crate::sync::InterruptGuard::new();
        core::alloc::GlobalAlloc::dealloc(&ALLOCATOR, ptr, layout)
    }
}
