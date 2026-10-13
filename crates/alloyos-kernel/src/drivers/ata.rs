//! Block device compatibility wrapper delegating to `VirtIoBlk` on `aarch64`.

use crate::drivers::virtio_blk::VirtIoBlk;
use crate::fs::block::{BlockDevice, BlockError};

pub struct AtaPio {
    inner: VirtIoBlk,
}

impl AtaPio {
    pub fn probe(_drive: u8) -> Option<Self> {
        VirtIoBlk::probe_any().map(|inner| Self { inner })
    }
}

impl BlockDevice for AtaPio {
    fn block_size(&self) -> usize {
        self.inner.block_size()
    }

    fn block_count(&self) -> u64 {
        self.inner.block_count()
    }

    fn read_block(&self, lba: u64, buf: &mut [u8]) -> Result<(), BlockError> {
        self.inner.read_block(lba, buf)
    }

    fn write_block(&self, lba: u64, buf: &[u8]) -> Result<(), BlockError> {
        self.inner.write_block(lba, buf)
    }
}
