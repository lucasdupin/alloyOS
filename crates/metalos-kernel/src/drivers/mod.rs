pub mod ata;
pub mod pic;
pub mod serial;
pub mod virtio_blk;

#[cfg(any(test, feature = "host-tests"))]
pub mod tests;
