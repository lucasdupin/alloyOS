#![no_std]
#![feature(abi_x86_interrupt)]

#[cfg(not(target_os = "none"))]
extern crate std;

extern crate alloc;

pub mod arch;
pub mod drivers;
pub mod fs;
pub mod mm;
pub mod syscall;
pub mod task;
