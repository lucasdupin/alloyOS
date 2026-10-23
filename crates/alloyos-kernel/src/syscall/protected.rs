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
    if (alloyos_proto::process::THREAD_CREATE..=alloyos_proto::process::GETTID).contains(&number) {
        use crate::task::app_process::Error;
        use alloyos_proto::process::*;
        if !task.native_threads_enabled() {
            return EPERM;
        }
        return match number {
            THREAD_CREATE => {
                if a2 != THREAD_DESCRIPTOR_BYTES as u64 || a3 != 0 {
                    return super::handlers::EINVAL;
                }
                let mut descriptor = [0u8; THREAD_DESCRIPTOR_BYTES];
                if space.copy_in(a1, &mut descriptor).is_err() {
                    return EFAULT;
                }
                let field = |i: usize| {
                    u64::from_le_bytes(descriptor[i * 8..i * 8 + 8].try_into().unwrap())
                };
                if field(5) != 0 {
                    return super::handlers::EINVAL;
                }
                let child = match task.user_thread(
                    space,
                    field(0),
                    field(1),
                    field(2),
                    field(3) as usize,
                    field(4),
                ) {
                    Ok(child) => child,
                    Err(Error::MemoryLimit) => return -12,
                    Err(Error::Identity) => return EPERM,
                    Err(_) => return super::handlers::EINVAL,
                };
                let tid = child.pid as i64;
                drop(guard);
                crate::task::scheduler::add_task(alloc::sync::Arc::new(child));
                tid
            }
            THREAD_EXIT => {
                if a2 != 0 || a3 != 0 {
                    return super::handlers::EINVAL;
                }
                task.exit_thread(a1 as i32);
                0
            }
            THREAD_JOIN => {
                if a3 != 0 {
                    return super::handlers::EINVAL;
                }
                if space.validate_range(a2, 4, true).is_err() {
                    return EFAULT;
                }
                match task.join_thread(a1) {
                    Ok(Some(code)) => space
                        .copy_out(a2, &code.to_le_bytes())
                        .map(|_| 0)
                        .unwrap_or(EFAULT),
                    Ok(None) => -11,
                    Err(Error::Identity) => -3,
                    Err(_) => super::handlers::EINVAL,
                }
            }
            WORD_WAIT => {
                if a2 > u32::MAX as u64 {
                    return super::handlers::EINVAL;
                }
                crate::task::scheduler::wait_word(&task, space, a1, a2 as u32, a3)
            }
            WORD_WAKE => {
                if a3 != 0 || a2 > u32::MAX as u64 || a1 % 4 != 0 {
                    return super::handlers::EINVAL;
                }
                if space.load_atomic_u32(a1).is_err() {
                    return EFAULT;
                }
                drop(guard);
                crate::task::scheduler::wake_word(&task, a1, a2 as usize)
            }
            GETTID => {
                if a1 != 0 || a2 != 0 || a3 != 0 {
                    return super::handlers::EINVAL;
                }
                task.pid as i64
            }
            _ => ENOSYS,
        };
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
        Ok(SyscallNo::Getpid) => task.process_id() as i64,
        Ok(SyscallNo::Yield) => 0, // Exception handler performs the context switch.
        Ok(SyscallNo::Exit) => {
            task.exit_process(a1 as i32);
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
    use crate::task::pcb::{Task, TaskState};
    use crate::task::scheduler::SCHEDULER;
    use alloyos_proto::process::*;
    use alloc::sync::Arc;

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

    fn write_descriptor(
        space: &mut AddressSpace,
        address: u64,
        fields: [u64; 6],
    ) {
        let mut bytes = [0u8; THREAD_DESCRIPTOR_BYTES];
        for (i, field) in fields.into_iter().enumerate() {
            bytes[i * 8..i * 8 + 8].copy_from_slice(&field.to_le_bytes());
        }
        space.copy_out(address, &bytes).unwrap();
    }

    #[test]
    fn thread_syscalls_require_supervisor_enable_and_validate_descriptors() {
        let _clean = SchedulerTestGuard::new();
        let leader = Arc::new(Task::user_process(&[0xc0, 3, 0x5f, 0xd6], 16, 32).unwrap());
        SCHEDULER.lock().add_task(leader.clone());
        assert_eq!(SCHEDULER.lock().next_task().unwrap().pid, leader.pid);

        // Without enable_native_threads(), all thread syscalls fail with EPERM.
        for number in THREAD_CREATE..=GETTID {
            assert_eq!(dispatch(number, 0, 0, 0), EPERM);
        }

        leader.enable_native_threads();
        assert_eq!(dispatch(GETTID, 0, 0, 0), leader.pid as i64);
        assert_eq!(dispatch(GETTID, 1, 0, 0), super::super::handlers::EINVAL);
        assert_eq!(dispatch(SyscallNo::Getpid as u64, 0, 0, 0), leader.process_id() as i64);

        let desc_addr = USER_BASE + PAGE_SIZE as u64;
        let stack_base = USER_BASE + 2 * PAGE_SIZE as u64;
        let stack_len = 4 * PAGE_SIZE;
        let tp = USER_BASE + 6 * PAGE_SIZE as u64;

        // Invalid descriptor size or trailing argument.
        assert_eq!(dispatch(THREAD_CREATE, desc_addr, 40, 0), super::super::handlers::EINVAL);
        assert_eq!(dispatch(THREAD_CREATE, desc_addr, 48, 1), super::super::handlers::EINVAL);
        // Unmapped descriptor pointer.
        assert_eq!(dispatch(THREAD_CREATE, USER_BASE + 64 * PAGE_SIZE as u64, 48, 0), EFAULT);

        // Non-zero reserved field(5).
        write_descriptor(
            leader.address_space.lock().as_mut().unwrap(),
            desc_addr,
            [USER_BASE, 7, stack_base, stack_len as u64, tp, 1],
        );
        assert_eq!(dispatch(THREAD_CREATE, desc_addr, 48, 0), super::super::handlers::EINVAL);

        // Non-executable entry (writable data page).
        write_descriptor(
            leader.address_space.lock().as_mut().unwrap(),
            desc_addr,
            [desc_addr, 7, stack_base, stack_len as u64, tp, 0],
        );
        assert_eq!(dispatch(THREAD_CREATE, desc_addr, 48, 0), super::super::handlers::EINVAL);

        // Undersized stack (< 4 pages), unaligned stack base, or unaligned TLS pointer.
        for bad in [
            [USER_BASE, 7, stack_base, (3 * PAGE_SIZE) as u64, tp, 0],
            [USER_BASE, 7, stack_base + 16, stack_len as u64, tp, 0],
            [USER_BASE, 7, stack_base, stack_len as u64, tp + 8, 0],
            // Overlapping the leader's stack window.
            [USER_BASE, 7, USER_BASE + 12 * PAGE_SIZE as u64, stack_len as u64, tp, 0],
        ] {
            write_descriptor(leader.address_space.lock().as_mut().unwrap(), desc_addr, bad);
            assert_eq!(dispatch(THREAD_CREATE, desc_addr, 48, 0), super::super::handlers::EINVAL);
        }

        // Valid child creation via private committed stack.
        let private_stack = dispatch(MEMORY_RESERVE, stack_len as u64, 0, 0) as u64;
        // Uncommitted stack must be rejected.
        write_descriptor(
            leader.address_space.lock().as_mut().unwrap(),
            desc_addr,
            [USER_BASE, 99, private_stack, stack_len as u64, tp, 0],
        );
        assert_eq!(dispatch(THREAD_CREATE, desc_addr, 48, 0), super::super::handlers::EINVAL);
        // Read-only stack must be rejected.
        assert_eq!(dispatch(MEMORY_COMMIT, private_stack, stack_len as u64, MEMORY_READ), 0);
        assert_eq!(dispatch(THREAD_CREATE, desc_addr, 48, 0), super::super::handlers::EINVAL);
        // Read-write committed stack succeeds.
        assert_eq!(dispatch(MEMORY_COMMIT, private_stack, stack_len as u64, MEMORY_READ_WRITE), 0);
        let child_tid = dispatch(THREAD_CREATE, desc_addr, 48, 0);
        assert!(child_tid > leader.pid as i64);

        // Overlapping an active child stack must be rejected.
        assert_eq!(dispatch(THREAD_CREATE, desc_addr, 48, 0), super::super::handlers::EINVAL);
    }

    #[test]
    fn thread_join_and_word_wait_wake_enforce_isolation_and_preserve_exit_status() {
        let _clean = SchedulerTestGuard::new();
        let leader = Arc::new(Task::user_process(&[0xc0, 3, 0x5f, 0xd6], 16, 40).unwrap());
        let peer = Arc::new(Task::user_process(&[0xc0, 3, 0x5f, 0xd6], 16, 40).unwrap());
        leader.enable_native_threads();
        peer.enable_native_threads();

        SCHEDULER.lock().add_task(leader.clone());
        assert_eq!(SCHEDULER.lock().next_task().unwrap().pid, leader.pid);

        let desc_addr = USER_BASE + PAGE_SIZE as u64;
        let word_addr = USER_BASE + PAGE_SIZE as u64 + 64;
        let status_addr = USER_BASE + PAGE_SIZE as u64 + 128;
        let tp = USER_BASE + 2 * PAGE_SIZE as u64;
        let stack_len = 4 * PAGE_SIZE;
        let child_stack = dispatch(MEMORY_RESERVE, stack_len as u64, 0, 0) as u64;
        assert_eq!(dispatch(MEMORY_COMMIT, child_stack, stack_len as u64, MEMORY_READ_WRITE), 0);
        write_descriptor(
            leader.address_space.lock().as_mut().unwrap(),
            desc_addr,
            [USER_BASE, 123, child_stack, stack_len as u64, tp, 0],
        );
        let child_tid = dispatch(THREAD_CREATE, desc_addr, 48, 0) as u64;

        // Self-join and leader-join return EINVAL; unknown TID returns ESRCH (-3).
        assert_eq!(dispatch(THREAD_JOIN, leader.pid, status_addr, 0), super::super::handlers::EINVAL);
        assert_eq!(dispatch(THREAD_JOIN, 999_999, status_addr, 0), -3);
        // Running child returns EAGAIN (-11).
        assert_eq!(dispatch(THREAD_JOIN, child_tid, status_addr, 0), -11);

        // WORD_WAIT validation: unaligned address, value mismatch, expired deadline.
        leader
            .address_space
            .lock()
            .as_mut()
            .unwrap()
            .copy_out(word_addr, &7u32.to_le_bytes())
            .unwrap();
        assert_eq!(dispatch(WORD_WAIT, word_addr + 2, 7, 0), super::super::handlers::EINVAL);
        assert_eq!(dispatch(WORD_WAIT, word_addr, u32::MAX as u64 + 1, 0), super::super::handlers::EINVAL);
        assert_eq!(dispatch(WORD_WAIT, word_addr, 8, 0), -11);
        assert_eq!(dispatch(WORD_WAIT, word_addr, 7, 1), -110);

        // Block leader on word_addr == 7, then switch to child.
        assert_eq!(dispatch(WORD_WAIT, word_addr, 7, 0), 0);
        assert_eq!(leader.state(), TaskState::Blocked);
        let child = SCHEDULER.lock().next_task().unwrap();
        assert_eq!(child.pid, child_tid);
        assert_eq!(dispatch(GETTID, 0, 0, 0), child_tid as i64);
        assert_eq!(dispatch(SyscallNo::Getpid as u64, 0, 0, 0), leader.pid as i64);

        // Switch to peer process: waking the same virtual address from another process must not wake leader.
        SCHEDULER.lock().add_task(peer.clone());
        assert_eq!(SCHEDULER.lock().next_task().unwrap().pid, peer.pid);
        assert_eq!(dispatch(WORD_WAKE, word_addr, 1, 0), 0);
        assert_eq!(leader.state(), TaskState::Blocked);

        // Switch back to child and wake leader.
        assert_eq!(SCHEDULER.lock().next_task().unwrap().pid, child_tid);
        assert_eq!(dispatch(WORD_WAKE, word_addr, 1, 0), 1);
        assert_eq!(leader.state(), TaskState::Ready);

        // Child exits with code 55; before retirement join still returns EAGAIN, after retirement it reaps once.
        assert_eq!(dispatch(THREAD_EXIT, 55, 0, 0), 0);
        assert_eq!(child.state(), TaskState::Zombie);
        child.retire_context();

        assert_eq!(SCHEDULER.lock().next_task().unwrap().pid, peer.pid);
        assert_eq!(SCHEDULER.lock().next_task().unwrap().pid, leader.pid);

        // Invalid status pointer fails with EFAULT BEFORE consuming the retired child slot.
        assert_eq!(dispatch(THREAD_JOIN, child_tid, USER_BASE, 0), EFAULT);
        assert_eq!(dispatch(THREAD_JOIN, child_tid, status_addr, 0), 0);
        let status = leader
            .address_space
            .lock()
            .as_ref()
            .unwrap()
            .load_atomic_u32(status_addr)
            .unwrap();
        assert_eq!(status as i32, 55);
        // Second join after reap returns ESRCH (-3).
        assert_eq!(dispatch(THREAD_JOIN, child_tid, status_addr, 0), -3);
    }

    static TEST_SCHEDULER_LOCK: spin::Mutex<()> = spin::Mutex::new(());
    struct SchedulerTestGuard(#[allow(dead_code)] spin::MutexGuard<'static, ()>);
    impl SchedulerTestGuard {
        fn new() -> Self {
            let guard = TEST_SCHEDULER_LOCK.lock();
            *SCHEDULER.lock() = crate::task::scheduler::Scheduler::new();
            Self(guard)
        }
    }
    impl Drop for SchedulerTestGuard {
        fn drop(&mut self) {
            *SCHEDULER.lock() = crate::task::scheduler::Scheduler::new();
        }
    }
}
