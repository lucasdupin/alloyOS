//! ARM Generic Interrupt Controller (GICv2) stub & interrupt notification for `aarch64`.

pub const PIC_1_OFFSET: u8 = 32;
pub const PIC_2_OFFSET: u8 = PIC_1_OFFSET + 8;

pub const GICD_BASE: usize = 0x0800_0000;
pub const GICC_BASE: usize = 0x0801_0000;

pub fn init() {
    // GICv2 / Generic Timer initialized on demand when preemptive timer IRQs are unmasked.
}

pub fn notify_end_of_interrupt(_interrupt_id: u8) {}
