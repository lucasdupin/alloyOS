use alloc::vec;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicU64, Ordering};
use spin::Mutex;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskState {
    Ready,
    Running,
    Blocked,
    Zombie,
}

static NEXT_PID: AtomicU64 = AtomicU64::new(1);

pub fn allocate_pid() -> u64 {
    NEXT_PID.fetch_add(1, Ordering::Relaxed)
}

pub struct Task {
    pub pid: u64,
    pub parent_pid: Option<u64>,
    pub state: Mutex<TaskState>,
    pub kernel_stack: Vec<u8>,
    pub stack_pointer: Mutex<u64>,
    pub exit_code: Mutex<Option<i32>>,
}

impl Task {
    pub const DEFAULT_STACK_SIZE: usize = 16 * 1024; // 16 KiB

    pub fn new(pid: u64, parent_pid: Option<u64>) -> Self {
        let stack = vec![0u8; Self::DEFAULT_STACK_SIZE];
        let stack_top = stack.as_ptr() as u64 + Self::DEFAULT_STACK_SIZE as u64;

        Self {
            pid,
            parent_pid,
            state: Mutex::new(TaskState::Ready),
            kernel_stack: stack,
            stack_pointer: Mutex::new(stack_top),
            exit_code: Mutex::new(None),
        }
    }

    pub fn new_with_auto_pid(parent_pid: Option<u64>) -> Self {
        Self::new(allocate_pid(), parent_pid)
    }

    pub fn state(&self) -> TaskState {
        *self.state.lock()
    }

    pub fn set_state(&self, new_state: TaskState) {
        *self.state.lock() = new_state;
    }

    pub fn exit_code(&self) -> Option<i32> {
        *self.exit_code.lock()
    }

    pub fn set_exit_code(&self, code: i32) {
        *self.exit_code.lock() = Some(code);
        self.set_state(TaskState::Zombie);
    }
}
