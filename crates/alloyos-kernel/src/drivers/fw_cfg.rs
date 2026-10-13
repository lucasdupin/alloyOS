//! QEMU Arm virt fw_cfg MMIO file reader. Boot secrets remain in volatile RAM.
use alloc::vec::Vec;
use core::ptr::{read_volatile, write_volatile};
use spin::Mutex;
const BASE: usize = 0x0902_0000;
const MAX_FILES: usize = 256;
static ACCESS: Mutex<()> = Mutex::new(());

unsafe fn select(selector: u16) {
    write_volatile((BASE + 8) as *mut u16, selector.to_be());
}
unsafe fn byte() -> u8 {
    read_volatile(BASE as *const u8)
}
unsafe fn u32_be() -> u32 {
    u32::from_be_bytes([byte(), byte(), byte(), byte()])
}

/// Bounded file read. Never prints credential names or contents.
/// Caller must invoke only on QEMU Arm virt with its fw_cfg mapping present.
pub fn read_file(name: &str, max_length: usize) -> Option<Vec<u8>> {
    let _guard = ACCESS.lock();
    unsafe {
        select(0);
        if [byte(), byte(), byte(), byte()] != *b"QEMU" {
            return None;
        }
        select(0x19);
        let count = u32_be() as usize;
        if count > MAX_FILES {
            return None;
        }
        let mut found = None;
        for _ in 0..count {
            let size = u32_be() as usize;
            let selector = u16::from_be_bytes([byte(), byte()]);
            byte();
            byte(); // reserved
            let mut filename = [0u8; 56];
            for item in &mut filename {
                *item = byte();
            }
            let len = filename.iter().position(|b| *b == 0).unwrap_or(56);
            if &filename[..len] == name.as_bytes() {
                found = Some((size, selector));
            }
        }
        let (size, selector) = found?;
        if size > max_length {
            return None;
        }
        select(selector);
        let mut bytes = Vec::with_capacity(size);
        for _ in 0..size {
            bytes.push(byte());
        }
        Some(bytes)
    }
}

/// Owning secret bytes without Debug, with best effort erase on drop.
pub struct BootAuth(Vec<u8>);
impl BootAuth {
    pub fn read() -> Option<Self> {
        read_file("opt/alloyos/auth", 16384).map(Self)
    }
    pub fn get(&self, name: &str) -> Option<&str> {
        for entry in self.0.split(|b| *b == 0) {
            let split = entry.iter().position(|b| *b == b'=')?;
            if &entry[..split] == name.as_bytes() {
                return core::str::from_utf8(&entry[split + 1..]).ok();
            }
        }
        None
    }
}
impl Drop for BootAuth {
    fn drop(&mut self) {
        for byte in &mut self.0 {
            unsafe {
                write_volatile(byte, 0);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn credentials_are_named_and_bounded_by_entry() {
        let auth = BootAuth(alloc::vec::Vec::from(
            &b"HF_TOKEN=fixture\0API_TOKEN=second=value\0"[..],
        ));
        assert_eq!(auth.get("HF_TOKEN"), Some("fixture"));
        assert_eq!(auth.get("API_TOKEN"), Some("second=value"));
        assert_eq!(auth.get("TOKEN"), None);
    }
    #[test]
    fn malformed_credentials_are_not_exposed() {
        let auth = BootAuth(alloc::vec::Vec::from(&b"invalid\0HF_TOKEN=fixture\0"[..]));
        assert_eq!(auth.get("HF_TOKEN"), None);
    }
}
