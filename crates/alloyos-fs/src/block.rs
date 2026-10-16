/// Errors returned by block device operations.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlockError {
    /// The underlying hardware signalled an I/O failure.
    IoError,
    /// The requested block address is beyond the device's capacity.
    OutOfBounds,
    /// The supplied buffer length does not match the device's block size.
    InvalidBufferSize,
}

/// A block-addressable storage device.
///
/// Implementors provide sector-level read and write access. All I/O is issued
/// in whole blocks whose size is reported by [`BlockDevice::block_size`].
pub trait BlockDevice: Send + Sync {
    /// Size of a single block in bytes (e.g. 512 for a raw ATA sector).
    fn block_size(&self) -> usize;

    /// Total number of addressable blocks on the device.
    fn block_count(&self) -> u64;

    /// Read block `lba` into `buf`.
    ///
    /// `buf` must be exactly [`block_size`] bytes; returns
    /// [`BlockError::InvalidBufferSize`] otherwise.
    fn read_block(&self, lba: u64, buf: &mut [u8]) -> Result<(), BlockError>;

    /// Write `buf` to block `lba`.
    ///
    /// `buf` must be exactly [`block_size`] bytes; returns
    /// [`BlockError::InvalidBufferSize`] otherwise.
    fn write_block(&self, lba: u64, buf: &[u8]) -> Result<(), BlockError>;
}
