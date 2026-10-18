//! Guest acceptance probes: actual EL0 syscalls, memory faults and preemption.
use super::{pcb::Task, scheduler};
use alloc::{sync::Arc, vec::Vec};

fn probe(words: &[u32]) -> Arc<Task> {
    let bytes: Vec<u8> = words.iter().flat_map(|word| word.to_le_bytes()).collect();
    Arc::new(Task::user_process(&bytes, 2, 3).expect("prepare EL0 probe"))
}

pub fn run() {
    super::web_runtime_selftest::run();
    scheduler::start_preemption();
    // getpid; exit(returned pid)
    let pid = probe(&[0xd28004e8, 0xd4000001, 0xd2800788, 0xd4000001]);
    // clock_gettime(CLOCK_MONOTONIC, NULL); exit(returned errno)
    let pointer = probe(&[
        0xd2800020, 0xd2800001, 0xd2801c88, 0xd4000001, 0xd2800788, 0xd4000001,
    ]);
    // Load privileged kernel RAM: must terminate this process, not the kernel.
    let fault = probe(&[0xd2a80000, 0xf9400000, 0x14000000]);
    // Give the faulting probe a valid private frame chain. The reporter must
    // copy it through owned backing while the fault itself remains contained.
    {
        let mut space = fault.address_space.lock();
        let space = space.as_mut().unwrap();
        let top = space.stack_top();
        let mut records = [0u8; 32];
        records[..8].copy_from_slice(&(top - 16).to_le_bytes());
        records[8..16].copy_from_slice(&(super::app_process::USER_BASE + 8).to_le_bytes());
        records[24..].copy_from_slice(&(super::app_process::USER_BASE + 4).to_le_bytes());
        space.copy_out(top - 32, &records).unwrap();
        let frame = *fault.stack_pointer.lock() as *mut crate::arch::idt::TrapFrame;
        unsafe { (*frame).regs[29] = top - 32; (*frame).user_sp = top - 64; }
    }
    // Report a message from read-only user memory, then trap as a real EL0
    // panic handler does. It must never reach shared descriptor handlers.
    let message = b"intentional EL0 panic";
    let words = [0xd2c00200, 0x91006000, 0xd2800001 | ((message.len() as u32) << 5), 0xd2803268, 0xd4000001, 0xd4200000];
    let mut image: Vec<u8> = words.iter().flat_map(|word| word.to_le_bytes()).collect();
    image.extend_from_slice(message);
    let panic_probe = Arc::new(Task::user_process(&image, 2, 3).unwrap());
    // Never yield: timer must still schedule the other probes and boot context.
    let spinner = probe(&[0x14000000]);
    // Preserve x19 across an explicit yield and resume at the following SVC.
    let yielding = probe(&[
        0xd28004e8, 0xd4000001, 0xaa0003f3, 0xd2800308, 0xd4000001, 0xaa1303e0, 0xd2800788,
        0xd4000001,
    ]);
    let abi = probe(&[0xd2803248, 0xd4000001, 0xd2800788, 0xd4000001]);
    // A protected process must not reach the host's shared file descriptors.
    let denied = probe(&[0xd2800028, 0xd4000001, 0xd2800788, 0xd4000001]);
    // Validated output into the private writable stack succeeds.
    let clock = probe(&[
        0xd10043ff, 0xd2800020, 0x910003e1, 0xd2801c88, 0xd4000001, 0xd2800788, 0xd4000001,
    ]);
    // Send four executable bytes through a supervisor-granted endpoint.
    let sender = probe(&[
        0xd2800000, 0xd2c00201, 0xd2800082, 0xd2803208, 0xd4000001, 0xd2800788, 0xd4000001,
    ]);
    let receiver = probe(&[
        0xd10043ff, 0xd2800000, 0x910003e1, 0xd2800182, 0xd2803228, 0xd4000001, 0xf100301f,
        0x54000041, 0xf94003e0, 0xd2800788, 0xd4000001,
    ]);
    let endpoint = super::ipc::Endpoint::new();
    sender.ports.lock().push(super::ipc::Port {
        endpoint: endpoint.clone(),
        send: true,
        receive: false,
    });
    receiver.ports.lock().push(super::ipc::Port {
        endpoint,
        send: false,
        receive: true,
    });
    for task in [
        &pid, &pointer, &fault, &panic_probe, &spinner, &sender, &receiver, &abi, &denied, &clock, &yielding,
    ] {
        scheduler::add_task(task.clone());
    }
    let deadline = crate::drivers::rng::monotonic_time_ns() + 2_000_000_000;
    while crate::drivers::rng::monotonic_time_ns() < deadline {
        if [
            &pid, &pointer, &fault, &panic_probe, &sender, &receiver, &abi, &denied, &clock, &yielding,
        ]
        .iter()
        .all(|task| task.exit_code().is_some())
        {
            assert_eq!(pid.exit_code(), Some(pid.pid as i32));
            assert_eq!(pointer.exit_code(), Some(-14));
            assert_eq!(fault.exit_code(), Some(-11));
            assert_eq!(panic_probe.exit_code(), Some(-11));
            assert_eq!(sender.exit_code(), Some(4));
            assert_eq!(receiver.exit_code(), Some(sender.pid as i32));
            assert_eq!(yielding.exit_code(), Some(yielding.pid as i32));
            assert_eq!(abi.exit_code(), Some(1));
            assert_eq!(denied.exit_code(), Some(-1));
            assert_eq!(clock.exit_code(), Some(0));
            for task in [
                &pid, &pointer, &fault, &panic_probe, &sender, &receiver, &abi, &denied, &clock, &yielding,
            ] {
                assert!(
                    task.address_space.lock().is_none(),
                    "dead process retains private mappings"
                );
                assert!(
                    task.ports.lock().is_empty(),
                    "dead process retains endpoint grants"
                );
            }
            spinner.set_exit_code(0);
            crate::serial_println!("[process-selftest] PASS: EL0 syscalls, IPC, invalid pointers, privileged memory fault, timer preemption and kernel survival");
            loop {
                crate::arch::wfi();
            }
        }
        crate::arch::wfi();
    }
    panic!("EL0 process acceptance timed out");
}
