//! ATA PIO (Programmed I/O) 28-bit LBA Block Driver.
//!
//! Operates on the primary IDE channel (`0x1F0`–`0x1F7`, control `0x3F6`).
//! By default, BIOS boot disks occupy Drive 0 (Master), so `AtaPio` supports
//! selecting either Master (`drive = 0`) or Slave (`drive = 1`, where `xtask`
//! attaches the persistent CobaltFS disk image).

use spin::Mutex;
use x86_64::instructions::port::Port;

use crate::fs::block::{BlockDevice, BlockError};

const ATA_PRIMARY_BASE: u16 = 0x1F0;
const ATA_PRIMARY_CTRL: u16 = 0x3F6;

const STATUS_BSY:  u8 = 0x80;
const STATUS_DRQ:  u8 = 0x08;
const STATUS_ERR:  u8 = 0x01;
const STATUS_DF:   u8 = 0x20;

const CMD_IDENTIFY:     u8 = 0xEC;
const CMD_READ_PIO:     u8 = 0x20;
const CMD_WRITE_PIO:    u8 = 0x30;
const CMD_CACHE_FLUSH:  u8 = 0xE7;

struct AtaRegisters {
    data:         Port<u16>,
    _error:       Port<u8>,
    sector_count: Port<u8>,
    lba_low:      Port<u8>,
    lba_mid:      Port<u8>,
    lba_high:     Port<u8>,
    drive_select: Port<u8>,
    command_stat: Port<u8>,
    alt_status:   Port<u8>,
}

impl AtaRegisters {
    const fn new() -> Self {
        Self {
            data:         Port::new(ATA_PRIMARY_BASE),
            _error:       Port::new(ATA_PRIMARY_BASE + 1),
            sector_count: Port::new(ATA_PRIMARY_BASE + 2),
            lba_low:      Port::new(ATA_PRIMARY_BASE + 3),
            lba_mid:      Port::new(ATA_PRIMARY_BASE + 4),
            lba_high:     Port::new(ATA_PRIMARY_BASE + 5),
            drive_select: Port::new(ATA_PRIMARY_BASE + 6),
            command_stat: Port::new(ATA_PRIMARY_BASE + 7),
            alt_status:   Port::new(ATA_PRIMARY_CTRL),
        }
    }

    /// 400ns delay by reading the alternate status register 4 times.
    unsafe fn io_wait(&mut self) {
        for _ in 0..4 {
            let _ = self.alt_status.read();
        }
    }

    unsafe fn wait_not_busy(&mut self) -> Result<(), BlockError> {
        for _ in 0..100_000 {
            let s = self.command_stat.read();
            if s == 0xFF {
                return Err(BlockError::IoError); // Floating bus
            }
            if s & STATUS_BSY == 0 {
                return Ok(());
            }
        }
        Err(BlockError::IoError)
    }

    unsafe fn wait_drq(&mut self) -> Result<(), BlockError> {
        for _ in 0..100_000 {
            let s = self.command_stat.read();
            if s & (STATUS_ERR | STATUS_DF) != 0 {
                return Err(BlockError::IoError);
            }
            if s & STATUS_BSY == 0 && s & STATUS_DRQ != 0 {
                return Ok(());
            }
        }
        Err(BlockError::IoError)
    }
}

/// Primary-bus ATA PIO drive (Master = `0`, Slave = `1`).
pub struct AtaPio {
    drive:   u8,
    sectors: u64,
    regs:    Mutex<AtaRegisters>,
}

impl AtaPio {
    /// Probe the specified drive (`0` = Primary Master, `1` = Primary Slave)
    /// using the ATA `IDENTIFY` command. Returns `Some(AtaPio)` if a drive is
    /// present and responds with non-zero sector count.
    pub fn probe(drive: u8) -> Option<Self> {
        if drive > 1 {
            return None;
        }
        let mut regs = AtaRegisters::new();
        let sectors = unsafe {
            let sel = 0xA0 | ((drive & 1) << 4);
            regs.drive_select.write(sel);
            regs.io_wait();

            regs.sector_count.write(0);
            regs.lba_low.write(0);
            regs.lba_mid.write(0);
            regs.lba_high.write(0);
            regs.command_stat.write(CMD_IDENTIFY);
            regs.io_wait();

            let status = regs.command_stat.read();
            if status == 0 || status == 0xFF {
                return None;
            }
            if regs.wait_not_busy().is_err() {
                return None;
            }
            // Check LBA mid/high to ensure it's standard ATA (not ATAPI)
            if regs.lba_mid.read() != 0 || regs.lba_high.read() != 0 {
                return None;
            }
            if regs.wait_drq().is_err() {
                return None;
            }

            let mut ident = [0u16; 256];
            for word in ident.iter_mut() {
                *word = regs.data.read();
            }
            // Words 60..=61 hold the total number of 28-bit LBA addressable sectors
            let count = (ident[60] as u32) | ((ident[61] as u32) << 16);
            if count == 0 {
                return None;
            }
            count as u64
        };

        Some(Self {
            drive,
            sectors,
            regs: Mutex::new(regs),
        })
    }
}

impl BlockDevice for AtaPio {
    fn block_size(&self) -> usize {
        512
    }

    fn block_count(&self) -> u64 {
        self.sectors
    }

    fn read_block(&self, lba: u64, buf: &mut [u8]) -> Result<(), BlockError> {
        if buf.len() != 512 {
            return Err(BlockError::InvalidBufferSize);
        }
        if lba >= self.sectors || lba > 0x0FFF_FFFF {
            return Err(BlockError::OutOfBounds);
        }

        let mut regs = self.regs.lock();
        unsafe {
            regs.wait_not_busy()?;
            let head = (0xE0 | ((self.drive & 1) << 4)) | (((lba >> 24) as u8) & 0x0F);
            regs.drive_select.write(head);
            regs.io_wait();
            regs.sector_count.write(1);
            regs.lba_low.write(lba as u8);
            regs.lba_mid.write((lba >> 8) as u8);
            regs.lba_high.write((lba >> 16) as u8);
            regs.command_stat.write(CMD_READ_PIO);
            regs.io_wait();

            regs.wait_drq()?;

            for chunk in buf.chunks_exact_mut(2) {
                let word = regs.data.read();
                let bytes = word.to_le_bytes();
                chunk[0] = bytes[0];
                chunk[1] = bytes[1];
            }
        }
        Ok(())
    }

    fn write_block(&self, lba: u64, buf: &[u8]) -> Result<(), BlockError> {
        if buf.len() != 512 {
            return Err(BlockError::InvalidBufferSize);
        }
        if lba >= self.sectors || lba > 0x0FFF_FFFF {
            return Err(BlockError::OutOfBounds);
        }

        let mut regs = self.regs.lock();
        unsafe {
            regs.wait_not_busy()?;
            let head = (0xE0 | ((self.drive & 1) << 4)) | (((lba >> 24) as u8) & 0x0F);
            regs.drive_select.write(head);
            regs.io_wait();
            regs.sector_count.write(1);
            regs.lba_low.write(lba as u8);
            regs.lba_mid.write((lba >> 8) as u8);
            regs.lba_high.write((lba >> 16) as u8);
            regs.command_stat.write(CMD_WRITE_PIO);
            regs.io_wait();

            regs.wait_drq()?;

            for chunk in buf.chunks_exact(2) {
                let word = u16::from_le_bytes([chunk[0], chunk[1]]);
                regs.data.write(word);
            }

            regs.command_stat.write(CMD_CACHE_FLUSH);
            regs.io_wait();
            regs.wait_not_busy()?;
        }
        Ok(())
    }
}
