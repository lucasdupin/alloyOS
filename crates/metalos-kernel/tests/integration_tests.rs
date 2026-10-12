use metalos_kernel::syscall::handlers::{sys_close, sys_open, sys_read, sys_write};
use metalos_kernel::task::pcb::{Task, TaskState};
use metalos_kernel::task::scheduler::Scheduler;
use std::sync::Arc;

#[test]
fn integration_vfs_and_syscall_io() {
    // Initialize VFS and FD table
    metalos_kernel::fs::init();

    let test_filename = b"/tmp_integration.txt\0";
    let fd = sys_open(test_filename.as_ptr() as u64, 0, 0);
    assert!(fd >= 3, "Opened FD should be >= 3, got: {}", fd);

    let message = b"metalOS integration test message";
    let written = sys_write(fd as u64, message.as_ptr() as u64, message.len() as u64);
    assert_eq!(written, message.len() as i64);

    // Close and reopen to verify persistence in RamFS
    assert_eq!(sys_close(fd as u64), 0);

    let reopened_fd = sys_open(test_filename.as_ptr() as u64, 0, 0);
    assert!(reopened_fd >= 3);

    let mut read_buf = [0u8; 64];
    let read_bytes = sys_read(
        reopened_fd as u64,
        read_buf.as_mut_ptr() as u64,
        read_buf.len() as u64,
    );
    assert_eq!(read_bytes, message.len() as i64);
    assert_eq!(&read_buf[..read_bytes as usize], message);

    assert_eq!(sys_close(reopened_fd as u64), 0);
}

#[test]
fn integration_devfs_devices() {
    metalos_kernel::fs::init();

    // /dev/zero
    let zero_path = b"/dev/zero\0";
    let fd_zero = sys_open(zero_path.as_ptr() as u64, 0, 0);
    assert!(fd_zero >= 3);

    let mut buf = [0xAAu8; 32];
    let read_zero = sys_read(fd_zero as u64, buf.as_mut_ptr() as u64, 32);
    assert_eq!(read_zero, 32);
    for b in buf {
        assert_eq!(b, 0x00);
    }
    assert_eq!(sys_close(fd_zero as u64), 0);

    // /dev/null
    let null_path = b"/dev/null\0";
    let fd_null = sys_open(null_path.as_ptr() as u64, 0, 0);
    assert!(fd_null >= 3);
    let read_null = sys_read(fd_null as u64, buf.as_mut_ptr() as u64, 32);
    assert_eq!(read_null, 0); // EOF
    assert_eq!(sys_close(fd_null as u64), 0);
}

#[test]
fn integration_scheduler_lifecycle() {
    let mut sched = Scheduler::new();
    let t1 = Arc::new(Task::new(10, None));
    let t2 = Arc::new(Task::new(20, None));

    sched.add_task(t1.clone());
    sched.add_task(t2.clone());

    let next1 = sched.next_task().unwrap();
    assert_eq!(next1.pid, 10);
    assert_eq!(next1.state(), TaskState::Running);

    sched.tick();
    assert_eq!(sched.current_task().unwrap().pid, 20);

    // Terminate task 2
    let curr = sched.current_task().unwrap();
    curr.set_exit_code(42);
    assert_eq!(curr.state(), TaskState::Zombie);

    // Next tick will only have t1
    sched.tick();
    assert_eq!(sched.current_task().unwrap().pid, 10);
}
