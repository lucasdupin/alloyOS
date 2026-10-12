#[cfg(target_os = "none")]
use linked_list_allocator::LockedHeap;
use crate::mm::paging::{FrameAllocator, OffsetPageTable, Size4KiB};

#[cfg(not(feature = "forge"))]
pub const HEAP_SIZE: usize = 64 * 1024 * 1024;
// Weights are borrowed from the reserved loader region, not copied into this
// heap. This budget covers model metadata, bounded KV cache, and working memory.
#[cfg(feature = "forge")]
pub const HEAP_SIZE: usize = 256 * 1024 * 1024;

#[cfg(target_os = "none")]
#[global_allocator]
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
