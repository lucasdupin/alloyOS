use x86_64::registers::control::Cr3;
use x86_64::structures::paging::{
    FrameAllocator, Mapper, OffsetPageTable, Page, PageTable, PageTableFlags, PhysFrame, Size4KiB,
    Translate,
};
use x86_64::{PhysAddr, VirtAddr};

pub unsafe fn init(physical_memory_offset: VirtAddr) -> OffsetPageTable<'static> {
    let (level_4_table_frame, _) = Cr3::read();
    let phys = level_4_table_frame.start_address();
    let virt = physical_memory_offset + phys.as_u64();
    let page_table_ptr: *mut PageTable = virt.as_mut_ptr();

    unsafe { OffsetPageTable::new(&mut *page_table_ptr, physical_memory_offset) }
}

pub fn map_page<A>(
    va: VirtAddr,
    pa: PhysAddr,
    flags: PageTableFlags,
    allocator: &mut A,
    page_table: &mut OffsetPageTable<'static>,
) -> Result<(), &'static str>
where
    A: FrameAllocator<Size4KiB>,
{
    let page: Page<Size4KiB> = Page::containing_address(va);
    let frame: PhysFrame<Size4KiB> = PhysFrame::containing_address(pa);

    unsafe {
        page_table
            .map_to(page, frame, flags, allocator)
            .map_err(|_| "Failed to map page")?
            .flush();
    }

    Ok(())
}

pub fn translate(va: VirtAddr, page_table: &OffsetPageTable) -> Option<PhysAddr> {
    page_table.translate_addr(va)
}
