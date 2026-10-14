//! ARM virtual timer and GICv2/v3 interrupt delivery.
pub const PIC_1_OFFSET: u8 = 32;
pub const PIC_2_OFFSET: u8 = PIC_1_OFFSET + 8;
pub const GICD_BASE: usize = 0x0800_0000;
pub const GICC_BASE: usize = 0x0801_0000;
#[cfg(all(target_arch = "aarch64", target_os = "none"))]
const TIMER_IRQ: u32 = 27;
#[cfg(all(target_arch = "aarch64", target_os = "none"))]
static GIC_VERSION: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(2);
pub fn init() {}
#[cfg(all(target_arch = "aarch64", target_os = "none"))]
unsafe fn rearm() {
    let frequency: u64;
    core::arch::asm!("mrs {}, cntfrq_el0", out(reg) frequency);
    core::arch::asm!("msr cntv_tval_el0, {}", in(reg) frequency / 1000);
    core::arch::asm!("msr cntv_ctl_el0, {}", in(reg) 1u64);
}
pub fn start_timer() {
    #[cfg(all(target_arch = "aarch64", target_os = "none"))]
    unsafe {
        // GICv2 has a 4 KiB distributor; reading the GICv3 PIDR2 offset
        // unconditionally causes an external abort on QEMU's default GICv2.
        let v2_id = (core::ptr::read_volatile((GICD_BASE + 0xfe8) as *const u32) >> 4) & 15;
        let version = if v2_id == 2 { 2 } else {
            (core::ptr::read_volatile((GICD_BASE + 0xffe8) as *const u32) >> 4) & 15
        };
        GIC_VERSION.store(version, core::sync::atomic::Ordering::Relaxed);
        if version >= 3 {
            // QEMU virt's CPU0 redistributor; SGI/PPI registers occupy its second page.
            let redistributor = 0x080a_0000usize;
            let waker = (redistributor + 0x14) as *mut u32;
            core::ptr::write_volatile(waker, core::ptr::read_volatile(waker) & !2);
            while core::ptr::read_volatile(waker) & 4 != 0 {
                core::hint::spin_loop();
            }
            let sgi = redistributor + 0x10000;
            core::ptr::write_volatile((sgi + 0x80) as *mut u32, u32::MAX);
            core::ptr::write_volatile((sgi + 0x400 + TIMER_IRQ as usize) as *mut u8, 0x80);
            core::ptr::write_volatile((sgi + 0x100) as *mut u32, 1 << TIMER_IRQ);
            core::ptr::write_volatile(GICD_BASE as *mut u32, 0x12);
            core::arch::asm!("msr ICC_SRE_EL1, {}", "isb", in(reg) 1u64);
            core::arch::asm!("msr ICC_PMR_EL1, {}", in(reg) 0xffu64);
            core::arch::asm!("msr ICC_BPR1_EL1, xzr", "msr ICC_IGRPEN1_EL1, {}", in(reg) 1u64);
        } else {
            core::ptr::write_volatile((GICD_BASE + 0x100) as *mut u32, 1 << TIMER_IRQ);
            core::ptr::write_volatile((GICD_BASE + 0x400 + TIMER_IRQ as usize) as *mut u8, 0x80);
            core::ptr::write_volatile(GICD_BASE as *mut u32, 1);
            core::ptr::write_volatile((GICC_BASE + 4) as *mut u32, 0xff);
            core::ptr::write_volatile(GICC_BASE as *mut u32, 1);
        }
        rearm();
        core::arch::asm!("dsb sy", "isb", "msr daifclr, #2");
    }
}
pub fn acknowledge_timer() -> bool {
    #[cfg(all(target_arch = "aarch64", target_os = "none"))]
    unsafe {
        let v3 = GIC_VERSION.load(core::sync::atomic::Ordering::Relaxed) >= 3;
        let id: u32;
        if v3 {
            let value: u64;
            core::arch::asm!("mrs {}, ICC_IAR1_EL1", out(reg) value);
            id = value as u32;
        } else {
            id = core::ptr::read_volatile((GICC_BASE + 0xc) as *const u32);
        }
        if id & 0x3ff == TIMER_IRQ {
            rearm();
        }
        if id & 0x3ff < 1020 {
            if v3 {
                core::arch::asm!("msr ICC_EOIR1_EL1, {}", in(reg) id as u64);
            } else {
                core::ptr::write_volatile((GICC_BASE + 0x10) as *mut u32, id);
            }
        }
        id & 0x3ff == TIMER_IRQ
    }
    #[cfg(not(all(target_arch = "aarch64", target_os = "none")))]
    {
        false
    }
}
pub fn notify_end_of_interrupt(_interrupt_id: u8) {}
