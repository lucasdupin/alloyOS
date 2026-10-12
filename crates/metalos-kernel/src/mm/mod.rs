pub mod frame_allocator;
pub mod heap;
pub mod paging;

#[cfg(test)]
pub mod tests;

use metalos_proto::BootInfo;
use crate::mm::paging::VirtAddr;

pub fn init(boot_info: &BootInfo) {
    let mut mapper = unsafe { paging::init(VirtAddr::new(boot_info.physical_memory_offset)) };
    frame_allocator::BitmapFrameAllocator::init_global_from_regions(boot_info.memory_regions)
        .expect("No usable memory regions found");

    let mut frame_allocator = frame_allocator::FRAME_ALLOCATOR.lock();
    heap::init_heap(&mut mapper, &mut *frame_allocator).expect("Heap initialization failed");
}
