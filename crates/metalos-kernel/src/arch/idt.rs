use spin::Lazy;
use x86_64::PrivilegeLevel;
use x86_64::structures::idt::{InterruptDescriptorTable, InterruptStackFrame, PageFaultErrorCode};
use crate::arch::gdt;
use crate::drivers::pic::{PICS, PIC_1_OFFSET};

pub const TIMER_INTERRUPT_INDEX: u8 = PIC_1_OFFSET;
pub const KEYBOARD_INTERRUPT_INDEX: u8 = PIC_1_OFFSET + 1;
pub const SYSCALL_INTERRUPT_INDEX: u8 = 0x80;

pub fn create_idt() -> InterruptDescriptorTable {
    let mut idt = InterruptDescriptorTable::new();

    // 0-31 CPU exceptions
    idt.divide_error.set_handler_fn(divide_error_handler);
    idt.debug.set_handler_fn(debug_handler);
    idt.non_maskable_interrupt.set_handler_fn(nmi_handler);
    idt.breakpoint.set_handler_fn(breakpoint_handler);
    idt.overflow.set_handler_fn(overflow_handler);
    idt.bound_range_exceeded.set_handler_fn(bound_range_handler);
    idt.invalid_opcode.set_handler_fn(invalid_opcode_handler);
    idt.device_not_available.set_handler_fn(device_not_available_handler);

    unsafe {
        idt.double_fault
            .set_handler_fn(double_fault_handler)
            .set_stack_index(gdt::DOUBLE_FAULT_IST_INDEX);
    }

    idt.invalid_tss.set_handler_fn(invalid_tss_handler);
    idt.segment_not_present.set_handler_fn(segment_not_present_handler);
    idt.stack_segment_fault.set_handler_fn(stack_segment_fault_handler);
    idt.general_protection_fault.set_handler_fn(general_protection_fault_handler);
    idt.page_fault.set_handler_fn(page_fault_handler);
    idt.x87_floating_point.set_handler_fn(x87_floating_point_handler);
    idt.alignment_check.set_handler_fn(alignment_check_handler);
    idt.machine_check.set_handler_fn(machine_check_handler);
    idt.simd_floating_point.set_handler_fn(simd_floating_point_handler);
    idt.virtualization.set_handler_fn(virtualization_handler);
    idt.cp_protection_exception.set_handler_fn(cp_protection_handler);
    idt.hv_injection_exception.set_handler_fn(hv_injection_handler);
    idt.vmm_communication_exception.set_handler_fn(vmm_communication_handler);
    idt.security_exception.set_handler_fn(security_exception_handler);

    // IRQs 32-47
    idt[TIMER_INTERRUPT_INDEX].set_handler_fn(timer_interrupt_handler);
    idt[KEYBOARD_INTERRUPT_INDEX].set_handler_fn(keyboard_interrupt_handler);

    for irq in 2..16 {
        let vector = PIC_1_OFFSET + irq;
        idt[vector].set_handler_fn(generic_irq_handler);
    }

    // Vector 0x80 syscall
    idt[SYSCALL_INTERRUPT_INDEX]
        .set_handler_fn(syscall_interrupt_handler)
        .set_privilege_level(PrivilegeLevel::Ring3);

    idt
}

pub static IDT: Lazy<InterruptDescriptorTable> = Lazy::new(create_idt);

pub fn init() {
    #[cfg(not(test))]
    IDT.load();
}

extern "x86-interrupt" fn divide_error_handler(stack_frame: InterruptStackFrame) {
    panic!("EXCEPTION: DIVIDE ERROR\n{:#?}", stack_frame);
}

extern "x86-interrupt" fn debug_handler(_stack_frame: InterruptStackFrame) {}

extern "x86-interrupt" fn nmi_handler(stack_frame: InterruptStackFrame) {
    panic!("EXCEPTION: NON-MASKABLE INTERRUPT\n{:#?}", stack_frame);
}

extern "x86-interrupt" fn breakpoint_handler(_stack_frame: InterruptStackFrame) {}

extern "x86-interrupt" fn overflow_handler(_stack_frame: InterruptStackFrame) {}

extern "x86-interrupt" fn bound_range_handler(stack_frame: InterruptStackFrame) {
    panic!("EXCEPTION: BOUND RANGE EXCEEDED\n{:#?}", stack_frame);
}

extern "x86-interrupt" fn invalid_opcode_handler(stack_frame: InterruptStackFrame) {
    panic!("EXCEPTION: INVALID OPCODE\n{:#?}", stack_frame);
}

extern "x86-interrupt" fn device_not_available_handler(_stack_frame: InterruptStackFrame) {}

extern "x86-interrupt" fn double_fault_handler(
    stack_frame: InterruptStackFrame,
    _error_code: u64,
) -> ! {
    panic!("EXCEPTION: DOUBLE FAULT\n{:#?}", stack_frame);
}

extern "x86-interrupt" fn invalid_tss_handler(stack_frame: InterruptStackFrame, error_code: u64) {
    panic!("EXCEPTION: INVALID TSS (code: {})\n{:#?}", error_code, stack_frame);
}

extern "x86-interrupt" fn segment_not_present_handler(
    stack_frame: InterruptStackFrame,
    error_code: u64,
) {
    panic!("EXCEPTION: SEGMENT NOT PRESENT (code: {})\n{:#?}", error_code, stack_frame);
}

extern "x86-interrupt" fn stack_segment_fault_handler(
    stack_frame: InterruptStackFrame,
    error_code: u64,
) {
    panic!("EXCEPTION: STACK SEGMENT FAULT (code: {})\n{:#?}", error_code, stack_frame);
}

extern "x86-interrupt" fn general_protection_fault_handler(
    stack_frame: InterruptStackFrame,
    error_code: u64,
) {
    panic!("EXCEPTION: GENERAL PROTECTION FAULT (code: {})\n{:#?}", error_code, stack_frame);
}

extern "x86-interrupt" fn page_fault_handler(
    stack_frame: InterruptStackFrame,
    error_code: PageFaultErrorCode,
) {
    use x86_64::registers::control::Cr2;
    panic!(
        "EXCEPTION: PAGE FAULT at {:?}, flags: {:?}\n{:#?}",
        Cr2::read(),
        error_code,
        stack_frame
    );
}

extern "x86-interrupt" fn x87_floating_point_handler(stack_frame: InterruptStackFrame) {
    panic!("EXCEPTION: x87 FLOATING POINT\n{:#?}", stack_frame);
}

extern "x86-interrupt" fn alignment_check_handler(stack_frame: InterruptStackFrame, error_code: u64) {
    panic!("EXCEPTION: ALIGNMENT CHECK (code: {})\n{:#?}", error_code, stack_frame);
}

extern "x86-interrupt" fn machine_check_handler(stack_frame: InterruptStackFrame) -> ! {
    panic!("EXCEPTION: MACHINE CHECK\n{:#?}", stack_frame);
}

extern "x86-interrupt" fn simd_floating_point_handler(stack_frame: InterruptStackFrame) {
    panic!("EXCEPTION: SIMD FLOATING POINT\n{:#?}", stack_frame);
}

extern "x86-interrupt" fn virtualization_handler(stack_frame: InterruptStackFrame) {
    panic!("EXCEPTION: VIRTUALIZATION\n{:#?}", stack_frame);
}

extern "x86-interrupt" fn cp_protection_handler(stack_frame: InterruptStackFrame, error_code: u64) {
    panic!("EXCEPTION: CONTROL PROTECTION (code: {})\n{:#?}", error_code, stack_frame);
}

extern "x86-interrupt" fn hv_injection_handler(stack_frame: InterruptStackFrame) {
    panic!("EXCEPTION: HYPERVISOR INJECTION\n{:#?}", stack_frame);
}

extern "x86-interrupt" fn vmm_communication_handler(
    stack_frame: InterruptStackFrame,
    error_code: u64,
) {
    panic!("EXCEPTION: VMM COMMUNICATION (code: {})\n{:#?}", error_code, stack_frame);
}

extern "x86-interrupt" fn security_exception_handler(
    stack_frame: InterruptStackFrame,
    error_code: u64,
) {
    panic!("EXCEPTION: SECURITY EXCEPTION (code: {})\n{:#?}", error_code, stack_frame);
}

extern "x86-interrupt" fn timer_interrupt_handler(_stack_frame: InterruptStackFrame) {
    crate::task::scheduler::tick();
    unsafe {
        PICS.lock().notify_end_of_interrupt(TIMER_INTERRUPT_INDEX);
    }
}

extern "x86-interrupt" fn keyboard_interrupt_handler(_stack_frame: InterruptStackFrame) {
    unsafe {
        PICS.lock().notify_end_of_interrupt(KEYBOARD_INTERRUPT_INDEX);
    }
}

extern "x86-interrupt" fn generic_irq_handler(_stack_frame: InterruptStackFrame) {
    unsafe {
        PICS.lock().notify_end_of_interrupt(PIC_1_OFFSET);
    }
}

extern "x86-interrupt" fn syscall_interrupt_handler(_stack_frame: InterruptStackFrame) {
    // Syscall handler via interrupt gate
}
