//! Actual EL0 thread creation, shared memory, TLS, atomic waits and group faults.
use super::{pcb::Task, scheduler};
use alloc::{sync::Arc, vec::Vec};

core::arch::global_asm!(include_str!("native_threads_probe.S"));
extern "C" {
    static native_threads_probe_start: u8;
    static native_threads_probe_end: u8;
    static native_threads_fault_start: u8;
    static native_threads_fault_end: u8;
}

pub fn run() {
    let success_image = unsafe {
        let start = core::ptr::addr_of!(native_threads_probe_start);
        let len = core::ptr::addr_of!(native_threads_probe_end) as usize - start as usize;
        core::slice::from_raw_parts(start, len)
    };
    let fault_image = unsafe {
        let start = core::ptr::addr_of!(native_threads_fault_start);
        let len = core::ptr::addr_of!(native_threads_fault_end) as usize - start as usize;
        core::slice::from_raw_parts(start, len)
    };
    let success = Arc::new(Task::user_process(success_image, 80, 128).unwrap());
    let fault = Arc::new(Task::user_process(fault_image, 80, 128).unwrap());
    success.enable_native_threads();
    fault.enable_native_threads();
    let peer_words: [u32; 3] = [0xd2800000, 0xd2800788, 0xd4000001];
    let peer_image: Vec<u8> = peer_words.iter().flat_map(|word| word.to_le_bytes()).collect();
    let peer = Arc::new(Task::user_process(&peer_image, 2, 3).unwrap());
    // Without an explicit supervisor grant, even malformed thread calls are denied.
    let denied_words: [u32; 4] = [0xd2803308, 0xd4000001, 0xd2800788, 0xd4000001];
    let denied_image: Vec<u8> = denied_words.iter().flat_map(|word| word.to_le_bytes()).collect();
    let denied = Arc::new(Task::user_process(&denied_image, 2, 3).unwrap());
    for task in [&success, &fault, &peer, &denied] {
        scheduler::add_task(task.clone());
    }
    scheduler::start_preemption();
    let deadline = crate::drivers::rng::monotonic_time_ns() + 5_000_000_000;
    while crate::drivers::rng::monotonic_time_ns() < deadline {
        if [&success, &fault, &peer, &denied].iter().all(|task| task.exit_code().is_some()) {
            assert_eq!(success.exit_code(), Some(0), "native thread handshake failed");
            assert_eq!(fault.exit_code(), Some(-11), "thread fault did not terminate its process");
            assert_eq!(peer.exit_code(), Some(0), "independent process did not survive thread fault");
            assert_eq!(denied.exit_code(), Some(-1), "thread creation lacked supervisor policy");
            if [&success, &fault, &peer, &denied].iter().all(|task| task.address_space.lock().is_none()) {
                crate::serial_println!("[web-runtime] PASS: native shared-address-space threads, distinct TLS, atomic wait/wake race, timeout, join, group fault containment and teardown");
                return;
            }
        }
        crate::arch::wfi();
    }
    panic!("native thread acceptance timed out");
}
