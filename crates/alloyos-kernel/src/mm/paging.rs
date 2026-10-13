//! Physical and virtual memory address and page frame abstractions for `aarch64`.

use bitflags::bitflags;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
#[repr(transparent)]
pub struct PhysAddr(u64);

impl PhysAddr {
    #[inline]
    pub const fn new(addr: u64) -> Self {
        Self(addr)
    }

    #[inline]
    pub const fn as_u64(self) -> u64 {
        self.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
#[repr(transparent)]
pub struct VirtAddr(u64);

impl VirtAddr {
    #[inline]
    pub const fn new(addr: u64) -> Self {
        Self(addr)
    }

    #[inline]
    pub const fn as_u64(self) -> u64 {
        self.0
    }

    #[inline]
    pub const fn as_mut_ptr<T>(self) -> *mut T {
        self.0 as usize as *mut T
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Size4KiB;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PhysFrame<S = Size4KiB> {
    start: PhysAddr,
    _marker: core::marker::PhantomData<S>,
}

impl<S> PhysFrame<S> {
    #[inline]
    pub const fn containing_address(addr: PhysAddr) -> Self {
        Self {
            start: PhysAddr::new(addr.as_u64() & !0xFFF),
            _marker: core::marker::PhantomData,
        }
    }

    #[inline]
    pub const fn start_address(self) -> PhysAddr {
        self.start
    }
}

pub unsafe trait FrameAllocator<S> {
    fn allocate_frame(&mut self) -> Option<PhysFrame<S>>;
}

pub trait FrameDeallocator<S> {
    unsafe fn deallocate_frame(&mut self, frame: PhysFrame<S>);
}

bitflags! {
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub struct PageTableFlags: u64 {
        const PRESENT = 1 << 0;
        const WRITABLE = 1 << 1;
        const USER_ACCESSIBLE = 1 << 2;
        const NO_EXECUTE = 1 << 54;
    }
}

pub struct OffsetPageTable {
    phys_offset: VirtAddr,
}

impl OffsetPageTable {
    pub const unsafe fn new(phys_offset: VirtAddr) -> Self {
        Self { phys_offset }
    }

    pub fn translate_addr(&self, va: VirtAddr) -> Option<PhysAddr> {
        Some(PhysAddr::new(va.as_u64().wrapping_sub(self.phys_offset.as_u64())))
    }
}

pub unsafe fn init(physical_memory_offset: VirtAddr) -> OffsetPageTable {
    OffsetPageTable::new(physical_memory_offset)
}

pub fn map_page<A>(
    _va: VirtAddr,
    _pa: PhysAddr,
    _flags: PageTableFlags,
    _allocator: &mut A,
    _page_table: &mut OffsetPageTable,
) -> Result<(), &'static str>
where
    A: FrameAllocator<Size4KiB>,
{
    // Identity-mapped in TTBR0_EL1 (1 GiB Normal WB block for 0x4000_0000..0x8000_0000)
    Ok(())
}

pub fn translate(va: VirtAddr, page_table: &OffsetPageTable) -> Option<PhysAddr> {
    page_table.translate_addr(va)
}
