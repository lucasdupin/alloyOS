//! ARM Power State Coordination Interface (PSCI) for `aarch64`.

pub const PSCI_SYSTEM_OFF: u32 = 0x8400_0008;
pub const PSCI_SYSTEM_RESET: u32 = 0x8400_0009;

/// Power off the system via PSCI `SYSTEM_OFF` (`hvc #0`).
pub fn system_off() -> ! {
    #[cfg(all(target_arch = "aarch64", target_os = "none"))]
    unsafe {
        core::arch::asm!(
            "hvc #0",
            in("w0") PSCI_SYSTEM_OFF,
            options(nomem, nostack)
        );
    }
    loop {
        crate::arch::wfi();
    }
}
