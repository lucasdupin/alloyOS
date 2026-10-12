pub mod block;
pub mod cobaltfs;
pub mod devfs;
pub mod ramfs;
pub mod vfs;

#[cfg(any(test, feature = "host-tests"))]
pub mod tests;

pub fn init() {
    vfs::init();
}
