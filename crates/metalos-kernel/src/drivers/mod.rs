pub mod ata;
pub mod pic;
pub mod rng;
pub mod serial;
pub mod fw_cfg;
pub mod forge;
pub mod system_ui;
pub mod virtio_blk;
pub mod virtio_gpu;
pub mod virtio_input;
pub mod virtio_net;

#[cfg(test)]
pub mod tests;
