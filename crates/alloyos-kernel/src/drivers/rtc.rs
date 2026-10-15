//! UTC Unix time from the QEMU virt machine's ARM PL031 RTC.
//! Read-only: the guest never changes the host-provided clock.
pub fn unix_time_ms() -> Option<u64> {
    #[cfg(all(target_arch = "aarch64", target_os = "none"))]
    {
        // PL031 peripheral ID identifies the device before reading RTCDR.
        let base = 0x0901_0000usize;
        unsafe {
            let id = core::ptr::read_volatile((base + 0xfe0) as *const u32);
            if id & 0xff != 0x31 { return None; }
            Some(core::ptr::read_volatile(base as *const u32) as u64 * 1000)
        }
    }
    #[cfg(not(target_os = "none"))]
    {
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)
            .ok().map(|time| time.as_millis() as u64)
    }
    #[cfg(all(target_os = "none", not(target_arch = "aarch64")))]
    { None }
}
