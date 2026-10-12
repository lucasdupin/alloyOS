#![no_std]
#![feature(abi_x86_interrupt, alloc_error_handler, naked_functions)]

#[cfg(any(test, feature = "host-tests"))]
extern crate std;

extern crate alloc;

pub mod arch;
pub mod drivers;
pub mod fs;
pub mod mm;
pub mod syscall;
pub mod task;
