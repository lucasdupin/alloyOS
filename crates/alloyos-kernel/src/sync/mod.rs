//! Short shared critical sections must not be preempted by another kernel task.
pub struct InterruptGuard {
    #[cfg(all(target_arch = "aarch64", target_os = "none"))]
    flags: u64,
}
impl InterruptGuard {
    pub fn new() -> Self {
        #[cfg(all(target_arch = "aarch64", target_os = "none"))]
        unsafe {
            let flags;
            core::arch::asm!("mrs {}, daif", "msr daifset, #2", out(reg) flags, options(nostack, preserves_flags));
            Self { flags }
        }
        #[cfg(not(all(target_arch = "aarch64", target_os = "none")))]
        Self {}
    }
}
impl Drop for InterruptGuard {
    fn drop(&mut self) {
        #[cfg(all(target_arch = "aarch64", target_os = "none"))]
        unsafe {
            core::arch::asm!("msr daif, {}", in(reg) self.flags, options(nostack, preserves_flags));
        }
    }
}
pub struct IrqMutex<T>(spin::Mutex<T>);
pub struct IrqMutexGuard<'a, T> {
    guard: spin::MutexGuard<'a, T>,
    _interrupts: InterruptGuard,
}
impl<T> IrqMutex<T> {
    pub const fn new(value: T) -> Self {
        Self(spin::Mutex::new(value))
    }
    pub fn lock(&self) -> IrqMutexGuard<'_, T> {
        let interrupts = InterruptGuard::new();
        IrqMutexGuard {
            guard: self.0.lock(),
            _interrupts: interrupts,
        }
    }
}
impl<T> core::ops::Deref for IrqMutexGuard<'_, T> {
    type Target = T;
    fn deref(&self) -> &T {
        &self.guard
    }
}
impl<T> core::ops::DerefMut for IrqMutexGuard<'_, T> {
    fn deref_mut(&mut self) -> &mut T {
        &mut self.guard
    }
}
