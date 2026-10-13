//! Shared phone geometry for boot, Ferrofluid, and the QEMU display.
pub const PHONE_LOGICAL_WIDTH: u32 = 402;
pub const PHONE_LOGICAL_HEIGHT: u32 = 874;
pub const PHONE_SCALE: u32 = 3;
pub const PHONE_WIDTH: u32 = PHONE_LOGICAL_WIDTH * PHONE_SCALE;
pub const PHONE_HEIGHT: u32 = PHONE_LOGICAL_HEIGHT * PHONE_SCALE;
