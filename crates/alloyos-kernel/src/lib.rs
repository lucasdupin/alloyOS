#![no_std]

#[cfg(not(target_os = "none"))]
extern crate std;

extern crate alloc;

pub mod arch;
pub mod drivers;
pub mod fs;
pub mod mm;
pub mod net;
pub mod syscall;
pub mod task;
pub mod sync;
