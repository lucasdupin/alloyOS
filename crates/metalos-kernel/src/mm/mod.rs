pub mod frame_allocator;
pub mod heap;
pub mod paging;

#[cfg(any(test, feature = "host-tests"))]
pub mod tests;

use bootloader::BootInfo;
use x86_64::VirtAddr;

pub fn init(boot_info: &'static mut BootInfo) {
    let phys_mem_offset = boot_info
        .physical_memory_offset
        .into_option()
        .expect("Physical memory offset missing from bootloader");

    let mut mapper = unsafe { paging::init(VirtAddr::new(phys_mem_offset)) };
    let mut frame_allocator = frame_allocator::BitmapFrameAllocator::init_from_bootloader_regions(
        &boot_info.memory_regions,
    )
    .expect("No usable memory regions found");

    heap::init_heap(&mut mapper, &mut frame_allocator).expect("Heap initialization failed");
}
