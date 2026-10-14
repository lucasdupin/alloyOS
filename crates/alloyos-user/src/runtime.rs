//! Reusable allocator for static EL0 executables. The loader supplies the private
//! heap in x0/x1; the final stack pages are excluded from this range.
use core::alloc::{GlobalAlloc, Layout};
use linked_list_allocator::LockedHeap;

pub struct ProcessAllocator(LockedHeap);
impl ProcessAllocator {
    pub const fn new() -> Self {
        Self(LockedHeap::empty())
    }
    /// Initialize once, before any allocation, from the executable entry args.
    ///
    /// # Safety
    /// The range must be private writable memory and remain live for the process.
    pub unsafe fn init(&self, start: usize, length: usize) {
        unsafe {
            self.0.lock().init(start as *mut u8, length);
        }
    }
}
impl Default for ProcessAllocator {
    fn default() -> Self {
        Self::new()
    }
}
unsafe impl GlobalAlloc for ProcessAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        unsafe { self.0.alloc(layout) }
    }
    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        unsafe {
            self.0.dealloc(pointer, layout);
        }
    }
}

/// Allocation-free panic message followed by an EL0 breakpoint. The kernel
/// reports saved registers and walks the private stack before containing it.
#[cfg(all(target_arch = "aarch64", target_os = "none"))]
pub fn panic_report(info: &core::panic::PanicInfo) -> ! {
    struct Buffer { bytes: [u8; 1024], len: usize }
    impl core::fmt::Write for Buffer {
        fn write_str(&mut self, text: &str) -> core::fmt::Result {
            let count = text.len().min(self.bytes.len() - self.len);
            self.bytes[self.len..self.len + count].copy_from_slice(&text.as_bytes()[..count]);
            self.len += count;
            Ok(())
        }
    }
    use core::fmt::Write;
    let mut buffer = Buffer { bytes: [0; 1024], len: 0 };
    let _ = write!(buffer, "{}", info);
    unsafe {
        crate::syscall::syscall2(alloyos_proto::process::PANIC_REPORT, buffer.bytes.as_ptr() as u64, buffer.len as u64);
        core::arch::asm!("brk #0", options(noreturn));
    }
}
