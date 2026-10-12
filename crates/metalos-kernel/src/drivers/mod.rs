pub mod ata;
pub mod pic;
pub mod rng;
pub mod serial;
pub mod system_ui;
pub mod virtio_blk;
pub mod virtio_gpu;
pub mod virtio_input;
pub mod virtio_net;

#[cfg(any(test, feature = "host-tests"))]
pub mod tests;
