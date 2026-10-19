//! Restricted EL0 syscall surface. Shared descriptor and network handlers are
//! deliberately unavailable until per-process ownership/capabilities exist.
use super::handlers::{EFAULT, ENOSYS, EPERM};
use alloyos_proto::SyscallNo;

pub fn dispatch(number: u64, a1: u64, a2: u64, a3: u64) -> i64 {
    let Some(task) = crate::task::scheduler::current_task() else {
        return EPERM;
    };
    let mut guard = task.address_space.lock();
    let Some(space) = guard.as_mut() else {
        return EPERM;
    };
    if (130..=134).contains(&number) {
        return crate::drivers::display_server::syscall(number, a1, a2, a3, &task, space);
    }
    if (120..=122).contains(&number) {
        return crate::task::js_process::dispatch(task.pid, space, number, a1, a2, a3);
    }
    if (alloyos_proto::process::MEMORY_RESERVE..=alloyos_proto::process::MEMORY_RELEASE).contains(&number) {
        use alloyos_proto::process::*;
        use crate::task::app_process::Error;
        let result = match number {
            MEMORY_RESERVE if a2 == 0 && a3 == 0 => space.reserve_private(a1 as usize).map(|address| address as i64),
            MEMORY_COMMIT if a3 == MEMORY_READ || a3 == MEMORY_READ_WRITE =>
                space.commit_private(a1, a2 as usize, a3 == MEMORY_READ_WRITE).map(|_| 0),
            MEMORY_DECOMMIT if a3 == 0 => space.decommit_private(a1, a2 as usize).map(|_| 0),
            MEMORY_RELEASE if a3 == 0 => space.release_private(a1, a2 as usize).map(|_| 0),
            _ => return super::handlers::EINVAL,
        };
        return result.unwrap_or_else(|error| match error {
            Error::MemoryLimit => -12,
            _ => super::handlers::EINVAL,
        });
    }
    if number == alloyos_proto::process::PANIC_REPORT {
        if a2 > 1024 { return -90; }
        let mut bytes = [0u8; 1024];
        if space.copy_in(a1, &mut bytes[..a2 as usize]).is_err() { return EFAULT; }
        crate::drivers::serial::emergency_print(format_args!("[crash] user-panic pid={} message=", task.pid));
        for byte in &bytes[..a2 as usize] {
            for escaped in core::ascii::escape_default(*byte) {
                crate::drivers::serial::emergency_print(format_args!("{}", escaped as char));
            }
        }
        crate::drivers::serial::emergency_print(format_args!("\n"));
        return 0;
    }
    if number == alloyos_proto::process::QUERY_ABI_VERSION {
        return alloyos_proto::process::ABI_VERSION as i64;
    }
    if number == alloyos_proto::process::IPC_SEND || number == alloyos_proto::process::IPC_RECEIVE {
        let ports = task.ports.lock();
        let Some(port) = ports.get(a1 as usize) else {
            return EPERM;
        };
        if a3 > super::super::task::app_process::MAX_MESSAGE as u64 + 8 {
            return -90;
        }
        if number == alloyos_proto::process::IPC_SEND {
            if !port.send {
                return EPERM;
            }
            if a3 > super::super::task::app_process::MAX_MESSAGE as u64 {
                return -90;
            }
            let mut bytes = alloc::vec![0; a3 as usize];
            if space.copy_in(a2, &mut bytes).is_err() {
                return EFAULT;
            }
            return port.send(task.pid, &bytes);
        }
        if !port.receive {
            return EPERM;
        }
        let mut queue = port.endpoint.queue.lock();
        let Some(message) = queue.front() else {
            return -11;
        };
        let length = 8 + message.payload.len();
        if a3 < length as u64 {
            return -90;
        }
        let mut bytes = alloc::vec::Vec::with_capacity(length);
        bytes.extend_from_slice(&message.sender.to_le_bytes());
        bytes.extend_from_slice(&message.payload);
        if space.copy_out(a2, &bytes).is_err() {
            return EFAULT;
        }
        queue.pop_front();
        return length as i64;
    }
    match SyscallNo::try_from_u64(number) {
        Ok(SyscallNo::GetRandom) => private_getrandom(space, a1, a2, a3),
        Ok(SyscallNo::Getpid) => task.pid as i64,
        Ok(SyscallNo::Yield) => 0, // Exception handler performs the context switch.
        Ok(SyscallNo::Exit) => {
            task.set_exit_code(a1 as i32);
            0
        }
        Ok(SyscallNo::ClockGettime) => {
            if a1 != 1 {
                return super::handlers::EINVAL;
            }
            let ns = crate::drivers::rng::monotonic_time_ns();
            let mut bytes = [0u8; 16];
            bytes[..8].copy_from_slice(&((ns / 1_000_000_000) as i64).to_ne_bytes());
            bytes[8..].copy_from_slice(&((ns % 1_000_000_000) as i64).to_ne_bytes());
            space.copy_out(a2, &bytes).map(|_| 0).unwrap_or(EFAULT)
        }
        Ok(_) => EPERM,
        Err(_) => ENOSYS,
    }
}

/// Bounded secure entropy for a protected process; never dereference EL0 memory.
fn private_getrandom(space: &mut crate::task::app_process::AddressSpace,
    address: u64, length: u64, flags: u64) -> i64 {
    if flags != 0 { return super::handlers::EINVAL; }
    if length > 256 { return -90; }
    if space.validate_range(address, length as usize, true).is_err() { return EFAULT; }
    if length == 0 { return 0; }
    let mut bytes = [0u8; 256];
    let output = &mut bytes[..length as usize];
    if crate::drivers::rng::try_fill_secure_random(output).is_err() { return -11; }
    let result = space.copy_out(address, output).map(|_| length as i64).unwrap_or(EFAULT);
    bytes.fill(0);
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::task::app_process::{AddressSpace, PAGE_SIZE, USER_BASE};
    #[test]
    fn protected_entropy_checks_private_permissions_and_fails_closed() {
        let mut space = AddressSpace::prepare(&[1], 2, 4).unwrap();
        let buffer = space.reserve_private(PAGE_SIZE).unwrap();
        assert_eq!(private_getrandom(&mut space, buffer, 32, 0), EFAULT);
        space.commit_private(buffer, PAGE_SIZE, true).unwrap();
        space.copy_out(buffer, &[0x5a; 32]).unwrap();
        assert_eq!(private_getrandom(&mut space, buffer, 32, 1), super::super::handlers::EINVAL);
        assert_eq!(private_getrandom(&mut space, buffer, 257, 0), -90);
        assert_eq!(private_getrandom(&mut space, USER_BASE, 32, 0), EFAULT);
        assert_eq!(private_getrandom(&mut space, buffer, 32, 0), -11);
        let mut unchanged = [0; 32];
        space.copy_in(buffer, &mut unchanged).unwrap();
        assert_eq!(unchanged, [0x5a; 32]);
        assert_eq!(private_getrandom(&mut space, buffer, 0, 0), 0);
    }
}
