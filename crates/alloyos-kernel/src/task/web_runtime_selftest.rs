//! Native EL0 acceptance for browser runtime memory; no host-side syscall mocks.
use super::{pcb::Task, scheduler};
use alloc::sync::Arc;

core::arch::global_asm!(include_str!("web_runtime_probe.S"));
extern "C" {
    static web_runtime_probe_start: u8;
    static web_runtime_probe_end: u8;
    static web_runtime_guard_start: u8;
    static web_runtime_guard_end: u8;
}

#[cfg(alloyos_cxx_probe)]
fn run_cxx() {
    let image = include_bytes!(env!("ALLOYOS_CXX_PROBE_ELF"));
    let first = Arc::new(Task::user_executable(image, 80, 128).expect("load native C++ probe ELF"));
    let peer_image = include_bytes!(env!("ALLOYOS_CXX_PEER_PROBE_ELF"));
    let second = Arc::new(Task::user_executable(peer_image, 80, 128).expect("load peer C++ probe ELF"));
    assert_ne!(first.address_space.lock().as_ref().unwrap().initial_thread_pointer(),
               second.address_space.lock().as_ref().unwrap().initial_thread_pointer(),
               "TLS pointer restoration needs distinct virtual addresses in this acceptance probe");
    assert_ne!(first.pid, second.pid);
    for task in [&first, &second] {
        assert_ne!(task.address_space.lock().as_ref().unwrap().initial_thread_pointer(), 0);
        scheduler::add_task(task.clone());
    }
    scheduler::start_preemption();
    let deadline = crate::drivers::rng::monotonic_time_ns() + 5_000_000_000;
    while crate::drivers::rng::monotonic_time_ns() < deadline {
        if first.exit_code().is_some() && second.exit_code().is_some() {
            for task in [&first, &second] {
                assert_eq!(task.exit_code(), Some(0), "native C++ TLS/constructor probe failed");
                crate::serial_println!("[web-runtime] TLS pid={} timer-peer-switches={}", task.pid, task.timer_preemptions());
                assert!(task.timer_preemptions() > 0, "TLS probe never switched to a peer through a timer IRQ");
                assert!(task.address_space.lock().is_none());
            }
            crate::serial_println!("[web-runtime] PASS: native C++ static ELF virtual dispatch and protected diagnostic report");
            crate::serial_println!("[web-runtime] PASS: native C++ constructors and timer-preempted two-process initialized/zero TLS separation");
            return;
        }
        crate::arch::wfi();
    }
    panic!("native C++ runtime probe timed out");
}

pub fn run() {
    #[cfg(alloyos_cxx_probe)]
    run_cxx();
    let image = unsafe {
        let start = core::ptr::addr_of!(web_runtime_probe_start);
        let length = core::ptr::addr_of!(web_runtime_probe_end) as usize - start as usize;
        core::slice::from_raw_parts(start, length)
    };
    let success = Arc::new(Task::user_process(image, 2, 12).expect("memory runtime probe"));
    // reserve(4096), then read the still-inaccessible first page.
    let guard_image = unsafe {
        let start = core::ptr::addr_of!(web_runtime_guard_start);
        let length = core::ptr::addr_of!(web_runtime_guard_end) as usize - start as usize;
        core::slice::from_raw_parts(start, length)
    };
    let guard = Arc::new(Task::user_process(guard_image, 2, 12).unwrap());
    scheduler::add_task(success.clone());
    scheduler::add_task(guard.clone());
    scheduler::start_preemption();
    let deadline = crate::drivers::rng::monotonic_time_ns() + 2_000_000_000;
    while crate::drivers::rng::monotonic_time_ns() < deadline {
        if success.exit_code().is_some() && guard.exit_code().is_some() {
            assert_eq!(success.exit_code(), Some(0), "memory syscall roundtrip failed");
            assert_eq!(guard.exit_code(), Some(-11), "uncommitted reservation was accessible");
            for task in [&success, &guard] {
                assert!(task.address_space.lock().is_none(), "private reservation survived teardown");
            }
            crate::serial_println!("[web-runtime] PASS: native EL0 reserve, zero commit, preserved commit, decommit zeroing, release, quota denial, executable denial, guard fault and teardown");
            return;
        }
        crate::arch::wfi();
    }
    panic!("native web runtime memory probe timed out");
}
