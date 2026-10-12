pub mod gdt;
pub mod idt;
pub mod mmu;
pub mod psci;

#[cfg(any(test, feature = "host-tests"))]
pub mod tests;

pub fn init() {
    mmu::init();
    gdt::init();
    idt::init();
}

/// Wait for interrupt (`wfi` on AArch64).
#[inline]
pub fn wfi() {
    #[cfg(all(target_arch = "aarch64", target_os = "none"))]
    unsafe {
        core::arch::asm!("wfi", options(nomem, nostack, preserves_flags));
    }
}

/// Enable IRQ interrupts in `DAIF` (`msr daifclr, #2`).
#[inline]
pub fn enable_interrupts() {
    #[cfg(all(target_arch = "aarch64", target_os = "none"))]
    unsafe {
        core::arch::asm!("msr daifclr, #2", options(nomem, nostack));
    }
}

/// Execute closure with IRQs masked in `DAIF`.
#[inline]
pub fn without_interrupts<F, R>(f: F) -> R
where
    F: FnOnce() -> R,
{
    #[cfg(all(target_arch = "aarch64", target_os = "none"))]
    unsafe {
        let daif: u64;
        core::arch::asm!(
            "mrs {daif}, daif",
            "msr daifset, #2",
            daif = out(reg) daif,
            options(nomem, nostack)
        );
        let ret = f();
        core::arch::asm!(
            "msr daif, {daif}",
            daif = in(reg) daif,
            options(nomem, nostack)
        );
        ret
    }
    #[cfg(not(all(target_arch = "aarch64", target_os = "none")))]
    {
        f()
    }
}
