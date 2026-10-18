pub mod automation;
pub mod accounts;
pub mod packages;
pub use alloyos_fs::{block, cobaltfs};
pub mod devfs;
pub mod ramfs;
pub mod vfs;

#[cfg(test)]
pub mod tests;

pub fn init() -> &'static str {
    vfs::init()
}
