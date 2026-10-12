//! ARMv8-A Exception Vector Table (`VBAR_EL1`) and `svc #0` Syscall Handler.

use core::ops::{Index, IndexMut};
use spin::Lazy;

pub const IRQ_FRAME_SIZE: usize = 816;

pub const TIMER_INTERRUPT_INDEX: u8 = 30; // ARM Generic Physical Timer PPI (INTID 30)
pub const KEYBOARD_INTERRUPT_INDEX: u8 = 33;
pub const SYSCALL_INTERRUPT_INDEX: u8 = 0x80;

/// ARMv8-A Exception Syndrome Register (`ESR_EL1`) Exception Class for `SVC` in AArch64.
pub const ESR_EC_SVC64: u64 = 0x15;

/// Saved AArch64 register state on synchronous exception or IRQ entry.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct TrapFrame {
    pub regs: [u64; 31], // x0 .. x30
    pub elr: u64,        // ELR_EL1
    pub spsr: u64,       // SPSR_EL1
    pub esr: u64,        // ESR_EL1
    pub far: u64,        // FAR_EL1
}

#[derive(Debug, Clone, Copy)]
pub struct HandlerAddr(u64);

impl HandlerAddr {
    pub const fn as_u64(self) -> u64 {
        self.0
    }
}

#[derive(Debug, Clone, Copy)]
pub struct VectorEntry {
    handler: u64,
}

impl VectorEntry {
    pub const fn empty() -> Self {
        Self { handler: 0 }
    }

    pub fn set_handler_fn(&mut self, f: fn()) -> &mut Self {
        self.handler = f as usize as u64;
        self
    }

    pub fn handler_addr(&self) -> HandlerAddr {
        HandlerAddr(self.handler)
    }
}

pub struct InterruptDescriptorTable {
    pub double_fault: VectorEntry,
    entries: [VectorEntry; 256],
}

impl InterruptDescriptorTable {
    pub const fn new() -> Self {
        Self {
            double_fault: VectorEntry::empty(),
            entries: [VectorEntry::empty(); 256],
        }
    }
}

impl Index<u8> for InterruptDescriptorTable {
    type Output = VectorEntry;
    fn index(&self, index: u8) -> &Self::Output {
        &self.entries[index as usize]
    }
}

impl IndexMut<u8> for InterruptDescriptorTable {
    fn index_mut(&mut self, index: u8) -> &mut Self::Output {
        &mut self.entries[index as usize]
    }
}

fn default_sync_fault_handler() {}
fn default_svc_handler() {}
fn default_timer_handler() {}

pub fn create_idt() -> InterruptDescriptorTable {
    let mut idt = InterruptDescriptorTable::new();
    idt.double_fault.set_handler_fn(default_sync_fault_handler);
    idt[TIMER_INTERRUPT_INDEX].set_handler_fn(default_timer_handler);
    idt[SYSCALL_INTERRUPT_INDEX].set_handler_fn(default_svc_handler);
    idt
}

pub static IDT: Lazy<InterruptDescriptorTable> = Lazy::new(create_idt);

/// Rust handler invoked by the AArch64 synchronous exception vector (`el1_sync` / `el0_sync`).
#[no_mangle]
pub extern "C" fn rust_sync_exception_handler(tf: &mut TrapFrame) {
    let ec = (tf.esr >> 26) & 0x3F;
    if ec == ESR_EC_SVC64 {
        // AArch64 syscall ABI: x8 = syscall number, x0..x5 = args 1..6, return in x0
        let num = tf.regs[8];
        let a1 = tf.regs[0];
        let a2 = tf.regs[1];
        let a3 = tf.regs[2];
        let a4 = tf.regs[3];
        let a5 = tf.regs[4];
        let a6 = tf.regs[5];
        let ret = crate::syscall::dispatch::dispatch_syscall(num, a1, a2, a3, a4, a5, a6);
        tf.regs[0] = ret as u64;
        return;
    }

    panic!(
        "AArch64 Synchronous Exception: EC={:#x}, ESR_EL1={:#018x}, ELR_EL1={:#018x}, FAR_EL1={:#018x}",
        ec, tf.esr, tf.elr, tf.far
    );
}

/// Rust handler invoked by the AArch64 IRQ vector (`el1_irq` / `el0_irq`).
#[no_mangle]
pub extern "C" fn rust_irq_exception_handler(tf: &mut TrapFrame) -> *mut TrapFrame {
    if crate::drivers::pic::acknowledge_timer() {
        crate::task::scheduler::preempt(tf)
    } else {
        tf
    }
}

#[cfg(all(target_arch = "aarch64", target_os = "none"))]
core::arch::global_asm!(
    r#"
.section .text.exceptions, "ax"
.global __exception_vectors
.balign 2048
__exception_vectors:
    // Current EL with SP0
    .balign 128
    b __sync_exception_stub
    .balign 128
    b __irq_exception_stub
    .balign 128
    b __unhandled_exception_stub
    .balign 128
    b __unhandled_exception_stub

    // Current EL with SPx (EL1h)
    .balign 128
    b __sync_exception_stub
    .balign 128
    b __irq_exception_stub
    .balign 128
    b __unhandled_exception_stub
    .balign 128
    b __unhandled_exception_stub

    // Lower EL using AArch64 (EL0)
    .balign 128
    b __sync_exception_stub
    .balign 128
    b __irq_exception_stub
    .balign 128
    b __unhandled_exception_stub
    .balign 128
    b __unhandled_exception_stub

    // Lower EL using AArch32
    .balign 128
    b __unhandled_exception_stub
    .balign 128
    b __unhandled_exception_stub
    .balign 128
    b __unhandled_exception_stub
    .balign 128
    b __unhandled_exception_stub

__sync_exception_stub:
    sub sp, sp, #288
    stp x0, x1, [sp, #0]
    stp x2, x3, [sp, #16]
    stp x4, x5, [sp, #32]
    stp x6, x7, [sp, #48]
    stp x8, x9, [sp, #64]
    stp x10, x11, [sp, #80]
    stp x12, x13, [sp, #96]
    stp x14, x15, [sp, #112]
    stp x16, x17, [sp, #128]
    stp x18, x19, [sp, #144]
    stp x20, x21, [sp, #160]
    stp x22, x23, [sp, #176]
    stp x24, x25, [sp, #192]
    stp x26, x27, [sp, #208]
    stp x28, x29, [sp, #224]
    mrs x0, elr_el1
    stp x30, x0, [sp, #240]
    mrs x1, spsr_el1
    mrs x2, esr_el1
    stp x1, x2, [sp, #256]
    mrs x3, far_el1
    str x3, [sp, #272]

    mov x0, sp
    bl rust_sync_exception_handler

    ldp x1, x2, [sp, #256]
    msr spsr_el1, x1
    ldp x30, x0, [sp, #240]
    msr elr_el1, x0
    ldp x28, x29, [sp, #224]
    ldp x26, x27, [sp, #208]
    ldp x24, x25, [sp, #192]
    ldp x22, x23, [sp, #176]
    ldp x20, x21, [sp, #160]
    ldp x18, x19, [sp, #144]
    ldp x16, x17, [sp, #128]
    ldp x14, x15, [sp, #112]
    ldp x12, x13, [sp, #96]
    ldp x10, x11, [sp, #80]
    ldp x8, x9, [sp, #64]
    ldp x6, x7, [sp, #48]
    ldp x4, x5, [sp, #32]
    ldp x2, x3, [sp, #16]
    ldp x0, x1, [sp, #0]
    add sp, sp, #288
    eret

__irq_exception_stub:
    sub sp, sp, #816
    stp x0, x1, [sp, #0]
    stp x2, x3, [sp, #16]
    stp x4, x5, [sp, #32]
    stp x6, x7, [sp, #48]
    stp x8, x9, [sp, #64]
    stp x10, x11, [sp, #80]
    stp x12, x13, [sp, #96]
    stp x14, x15, [sp, #112]
    stp x16, x17, [sp, #128]
    stp x18, x19, [sp, #144]
    stp x20, x21, [sp, #160]
    stp x22, x23, [sp, #176]
    stp x24, x25, [sp, #192]
    stp x26, x27, [sp, #208]
    stp x28, x29, [sp, #224]
    mrs x0, elr_el1
    stp x30, x0, [sp, #240]
    mrs x1, spsr_el1
    mrs x2, esr_el1
    stp x1, x2, [sp, #256]
    mrs x3, far_el1
    str x3, [sp, #272]

    stp q0, q1, [sp, #288]
    stp q2, q3, [sp, #320]
    stp q4, q5, [sp, #352]
    stp q6, q7, [sp, #384]
    stp q8, q9, [sp, #416]
    stp q10, q11, [sp, #448]
    stp q12, q13, [sp, #480]
    stp q14, q15, [sp, #512]
    stp q16, q17, [sp, #544]
    stp q18, q19, [sp, #576]
    stp q20, q21, [sp, #608]
    stp q22, q23, [sp, #640]
    stp q24, q25, [sp, #672]
    stp q26, q27, [sp, #704]
    stp q28, q29, [sp, #736]
    stp q30, q31, [sp, #768]
    mrs x0, fpcr
    mrs x1, fpsr
    str x0, [sp, #800]
    str x1, [sp, #808]
    mov x0, sp
    bl rust_irq_exception_handler
    mov sp, x0
    ldr x0, [sp, #800]
    ldr x1, [sp, #808]
    msr fpcr, x0
    msr fpsr, x1
    ldp q0, q1, [sp, #288]
    ldp q2, q3, [sp, #320]
    ldp q4, q5, [sp, #352]
    ldp q6, q7, [sp, #384]
    ldp q8, q9, [sp, #416]
    ldp q10, q11, [sp, #448]
    ldp q12, q13, [sp, #480]
    ldp q14, q15, [sp, #512]
    ldp q16, q17, [sp, #544]
    ldp q18, q19, [sp, #576]
    ldp q20, q21, [sp, #608]
    ldp q22, q23, [sp, #640]
    ldp q24, q25, [sp, #672]
    ldp q26, q27, [sp, #704]
    ldp q28, q29, [sp, #736]
    ldp q30, q31, [sp, #768]

    ldp x1, x2, [sp, #256]
    msr spsr_el1, x1
    ldp x30, x0, [sp, #240]
    msr elr_el1, x0
    ldp x28, x29, [sp, #224]
    ldp x26, x27, [sp, #208]
    ldp x24, x25, [sp, #192]
    ldp x22, x23, [sp, #176]
    ldp x20, x21, [sp, #160]
    ldp x18, x19, [sp, #144]
    ldp x16, x17, [sp, #128]
    ldp x14, x15, [sp, #112]
    ldp x12, x13, [sp, #96]
    ldp x10, x11, [sp, #80]
    ldp x8, x9, [sp, #64]
    ldp x6, x7, [sp, #48]
    ldp x4, x5, [sp, #32]
    ldp x2, x3, [sp, #16]
    ldp x0, x1, [sp, #0]
    add sp, sp, #816
    eret

__unhandled_exception_stub:
    wfe
    b __unhandled_exception_stub
"#
);

/// Install `VBAR_EL1` pointing to `__exception_vectors`.
pub fn init() {
    let _ = &*IDT;
    #[cfg(all(target_arch = "aarch64", target_os = "none"))]
    unsafe {
        extern "C" {
            static __exception_vectors: u8;
        }
        let vbar = &raw const __exception_vectors as usize as u64;
        core::arch::asm!(
            "msr vbar_el1, {vbar}",
            "isb",
            vbar = in(reg) vbar,
            options(nomem, nostack)
        );
    }
}
