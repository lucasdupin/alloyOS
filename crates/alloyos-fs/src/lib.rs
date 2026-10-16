//! Shared no_std CobaltFS implementation for the guest kernel and host image tools.
#![no_std]
extern crate alloc;

pub mod block;
pub mod image;
pub mod cobaltfs;
pub mod vfs;
