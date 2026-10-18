//! Raw private anonymous mappings for protected native services.
//!
//! These calls never map files, share pages, or grant executable memory. Addresses
//! belong to the calling process and are bounded by its supervisor-assigned quota.
use alloyos_proto::process::*;

/// Reserve inaccessible virtual pages. Length must be a nonzero multiple of 4096.
pub fn reserve(length: usize) -> Result<u64, i64> {
    let result = unsafe { crate::syscall::syscall1(MEMORY_RESERVE, length as u64) };
    if result < 0 {
        Err(result)
    } else {
        Ok(result as u64)
    }
}

/// Commit zeroed pages, or change permissions while preserving committed bytes.
///
/// # Safety
/// The caller must own the reservation and synchronize every access to the range.
/// Changing permissions must not invalidate live Rust references.
pub unsafe fn commit(address: u64, length: usize, writable: bool) -> Result<(), i64> {
    let result = unsafe {
        crate::syscall::syscall3(
            MEMORY_COMMIT,
            address,
            length as u64,
            if writable {
                MEMORY_READ_WRITE
            } else {
                MEMORY_READ
            },
        )
    };
    status(result)
}

/// Discard committed bytes and revoke access; the reservation remains owned.
///
/// # Safety
/// No references or concurrent accesses to this range may remain live.
pub unsafe fn decommit(address: u64, length: usize) -> Result<(), i64> {
    status(unsafe { crate::syscall::syscall2(MEMORY_DECOMMIT, address, length as u64) })
}

/// Release one exact whole reservation, including all its committed pages.
///
/// # Safety
/// No references or concurrent accesses to any part of the reservation may remain.
pub unsafe fn release(address: u64, length: usize) -> Result<(), i64> {
    status(unsafe { crate::syscall::syscall2(MEMORY_RELEASE, address, length as u64) })
}

fn status(result: i64) -> Result<(), i64> {
    if result == 0 {
        Ok(())
    } else {
        Err(result)
    }
}
