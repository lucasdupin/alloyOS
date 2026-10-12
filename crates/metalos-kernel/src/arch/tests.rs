#[cfg(any(test, feature = "host-tests"))]
pub mod tests {
    use alloc::boxed::Box;
    use crate::arch::gdt::{create_gdt, create_tss, DOUBLE_FAULT_IST_INDEX};
    use crate::arch::idt::create_idt;

    #[test]
    fn gdt_has_five_entries() {
        // We create a leak-safe static TSS for testing
        let tss = Box::leak(Box::new(create_tss()));
        let (_gdt, selectors) = create_gdt(tss);

        // Verify all 5 selectors are distinct and non-zero
        assert_ne!(selectors.kernel_code.0, 0);
        assert_ne!(selectors.kernel_data.0, 0);
        assert_ne!(selectors.user_code.0, 0);
        assert_ne!(selectors.user_data.0, 0);
        assert_ne!(selectors.tss.0, 0);

        assert_ne!(selectors.kernel_code, selectors.kernel_data);
        assert_ne!(selectors.kernel_code, selectors.user_code);
        assert_ne!(selectors.user_code, selectors.user_data);
        assert_ne!(selectors.tss, selectors.kernel_code);
    }

    #[test]
    fn tss_stack_aligned() {
        let tss = create_tss();
        let stack_ptr = tss.interrupt_stack_table[DOUBLE_FAULT_IST_INDEX as usize].as_u64();
        assert_ne!(stack_ptr, 0, "IST1 stack pointer should not be null");
        assert_eq!(stack_ptr % 16, 0, "IST1 stack pointer must be 16-byte aligned");
    }

    #[test]
    fn idt_double_fault_has_handler() {
        let idt = create_idt();
        let double_fault_entry = idt.double_fault;
        let handler_addr = double_fault_entry.handler_addr();
        assert_ne!(handler_addr.as_u64(), 0, "Double fault handler address must not be zero");
    }

    #[test]
    fn idt_syscall_configured() {
        let idt = create_idt();
        let syscall_entry = idt[crate::arch::idt::SYSCALL_INTERRUPT_INDEX];
        assert_ne!(syscall_entry.handler_addr().as_u64(), 0);
    }
}
