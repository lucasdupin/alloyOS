#[cfg(test)]
pub mod tests {
    use alloc::boxed::Box;
    use crate::arch::gdt::{create_gdt, create_tss, DOUBLE_FAULT_IST_INDEX};
    use crate::arch::idt::{create_idt, TrapFrame, SYSCALL_INTERRUPT_INDEX};
    use crate::arch::mmu::{IDENTITY_L1_TABLE, DESC_AF, DESC_VALID_BLOCK};

    #[test]
    fn gdt_has_five_entries() {
        let tss = Box::leak(Box::new(create_tss()));
        let (_gdt, selectors) = create_gdt(tss);

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
        let stack_ptr = tss.interrupt_stack_table[DOUBLE_FAULT_IST_INDEX as usize];
        assert_ne!(stack_ptr, 0, "Exception stack pointer should not be null");
        assert_eq!(stack_ptr % 16, 0, "Exception stack pointer must be 16-byte aligned");
    }

    #[test]
    fn idt_double_fault_has_handler() {
        let idt = create_idt();
        let double_fault_entry = idt.double_fault;
        let handler_addr = double_fault_entry.handler_addr();
        assert_ne!(handler_addr.as_u64(), 0, "Sync fault handler address must not be zero");
    }

    #[test]
    fn idt_syscall_configured() {
        let idt = create_idt();
        let syscall_entry = idt[SYSCALL_INTERRUPT_INDEX];
        assert_ne!(syscall_entry.handler_addr().as_u64(), 0);
    }

    #[test]
    fn aarch64_mmu_identity_table_valid() {
        let dev_entry = IDENTITY_L1_TABLE.entries[0];
        let ram_entry = IDENTITY_L1_TABLE.entries[1];
        assert_eq!(dev_entry & 0b11, DESC_VALID_BLOCK);
        assert_ne!(dev_entry & DESC_AF, 0);
        assert_eq!(ram_entry & 0b11, DESC_VALID_BLOCK);
        assert_ne!(ram_entry & DESC_AF, 0);
        assert_eq!(ram_entry & 0x0000_FFFF_C000_0000, 0x4000_0000);
        assert_eq!(core::mem::size_of::<TrapFrame>(), 38 * 8);
        assert_eq!(core::mem::offset_of!(TrapFrame, elr), 248);
        assert_eq!(core::mem::offset_of!(TrapFrame, spsr), 256);
        assert_eq!(core::mem::offset_of!(TrapFrame, esr), 264);
        assert_eq!(core::mem::offset_of!(TrapFrame, far), 272);
        assert_eq!(core::mem::offset_of!(TrapFrame, user_sp), 280);
        assert_eq!(core::mem::offset_of!(TrapFrame, thread_pointer), 288);
        assert_eq!(core::mem::align_of::<TrapFrame>(), 16);
    }
}
