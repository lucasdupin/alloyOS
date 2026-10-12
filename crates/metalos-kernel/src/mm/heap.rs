use linked_list_allocator::LockedHeap;
use x86_64::structures::paging::{
    FrameAllocator, Mapper, OffsetPageTable, Page, PageTableFlags, Size4KiB,
};
use x86_64::VirtAddr;

pub const HEAP_START: usize = 0xFFFF_8800_0000_0000;
pub const HEAP_SIZE: usize = 4 * 1024 * 1024; // 4 MiB

#[cfg(target_os = "none")]
#[global_allocator]
pub static ALLOCATOR: LockedHeap = LockedHeap::empty();

pub fn init_heap(
    page_table: &mut OffsetPageTable<'static>,
    frame_allocator: &mut impl FrameAllocator<Size4KiB>,
) -> Result<(), &'static str> {
    let page_range = {
        let heap_start = VirtAddr::new(HEAP_START as u64);
        let heap_end = heap_start + (HEAP_SIZE as u64) - 1u64;
        let heap_start_page = Page::containing_address(heap_start);
        let heap_end_page = Page::containing_address(heap_end);
        Page::range_inclusive(heap_start_page, heap_end_page)
    };

    for page in page_range {
        let frame = frame_allocator
            .allocate_frame()
            .ok_or("Out of physical frames for kernel heap")?;
        let flags = PageTableFlags::PRESENT | PageTableFlags::WRITABLE;
        unsafe {
            page_table
                .map_to(page, frame, flags, frame_allocator)
                .map_err(|_| "Failed to map heap page")?
                .flush();
        }
    }

    #[cfg(target_os = "none")]
    unsafe {
        ALLOCATOR.lock().init(HEAP_START as *mut u8, HEAP_SIZE);
    }

    Ok(())
}
