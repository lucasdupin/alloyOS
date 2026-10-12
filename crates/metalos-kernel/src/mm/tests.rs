#[cfg(any(test, feature = "host-tests"))]
pub mod tests {
    use crate::mm::frame_allocator::BitmapFrameAllocator;
    use crate::mm::paging::FrameAllocator;
    use core::alloc::{GlobalAlloc, Layout};
    use linked_list_allocator::LockedHeap;

    #[test]
    fn bitmap_alloc_first_frame() {
        let mut allocator = BitmapFrameAllocator::new(0x10_0000, 16);
        let frame = allocator.allocate_frame().expect("Should allocate frame");
        let addr = frame.start_address().as_u64();

        assert_eq!(addr, 0x10_0000);
        assert_eq!(addr % 4096, 0);
        assert_ne!(addr, 0);
    }

    #[test]
    fn bitmap_alloc_dealloc_realloc() {
        let mut allocator = BitmapFrameAllocator::new(0x20_0000, 8);
        let frame1 = allocator.allocate_frame().expect("Frame 1");
        let frame2 = allocator.allocate_frame().expect("Frame 2");
        assert_ne!(frame1.start_address(), frame2.start_address());

        allocator.deallocate_frame_custom(frame1);
        let frame1_realloc = allocator.allocate_frame().expect("Reallocated frame 1");
        assert_eq!(frame1.start_address(), frame1_realloc.start_address());
    }

    #[test]
    fn bitmap_oom_returns_none() {
        let count = 4;
        let mut allocator = BitmapFrameAllocator::new(0x30_0000, count);
        for _ in 0..count {
            assert!(allocator.allocate_frame().is_some());
        }
        assert!(allocator.allocate_frame().is_none(), "Allocator should be out of memory");
    }

    #[test]
    fn heap_alloc_vec() {
        let heap = LockedHeap::empty();
        let mut buffer = [0u8; 32 * 1024];
        unsafe {
            heap.lock().init(buffer.as_mut_ptr(), buffer.len());
        }

        // Allocate space for 100 u64 values
        let layout = Layout::array::<u64>(100).unwrap();
        let ptr = unsafe { heap.alloc(layout) as *mut u64 };
        assert!(!ptr.is_null());

        unsafe {
            for i in 0..100 {
                ptr.add(i).write(i as u64);
            }
            for i in 0..100 {
                assert_eq!(ptr.add(i).read(), i as u64);
            }
            heap.dealloc(ptr as *mut u8, layout);
        }
    }

    #[test]
    fn heap_alloc_alignment() {
        let heap = LockedHeap::empty();
        let mut buffer = [0u8; 16 * 1024];
        unsafe {
            heap.lock().init(buffer.as_mut_ptr(), buffer.len());
        }

        let layout = Layout::from_size_align(128, 64).unwrap();
        let ptr = unsafe { heap.alloc(layout) };
        assert!(!ptr.is_null());
        assert_eq!(ptr as usize % 64, 0, "Allocated pointer must be 64-byte aligned");

        unsafe {
            heap.dealloc(ptr, layout);
        }
    }
}
