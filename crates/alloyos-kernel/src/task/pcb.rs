use crate::sync::IrqMutex as Mutex;
use alloc::vec;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicU64, Ordering};

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
    pub kernel_context: bool,
    /// Prepared private tables; no EL0 launch path is enabled yet.
    pub address_space: Mutex<Option<super::app_process::AddressSpace>>,
    pub parent_pid: Option<u64>,
    pub state: Mutex<TaskState>,
    pub wake_deadline: Mutex<Option<u64>>,
    pub kernel_stack: Vec<u8>,
    pub stack_pointer: Mutex<u64>,
    pub exit_code: Mutex<Option<i32>>,
}

impl Task {
    pub const DEFAULT_STACK_SIZE: usize = 16 * 1024; // 16 KiB

    pub fn new(pid: u64, parent_pid: Option<u64>) -> Self {
        NEXT_PID.fetch_max(pid.saturating_add(1), Ordering::Relaxed);
        let stack = vec![0u8; Self::DEFAULT_STACK_SIZE];
        let stack_top = stack.as_ptr() as u64 + Self::DEFAULT_STACK_SIZE as u64;

        Self {
            pid,
            kernel_context: false,
            address_space: Mutex::new(None),
            parent_pid,
            state: Mutex::new(TaskState::Ready),
            wake_deadline: Mutex::new(None),
            kernel_stack: stack,
            stack_pointer: Mutex::new(stack_top),
            exit_code: Mutex::new(None),
        }
    }

    pub fn kernel_thread(pid: u64, entry: extern "C" fn() -> !) -> Self {
        let mut task = Self::new(pid, Some(1));
        task.kernel_stack = vec![0u8; 256 * 1024];
        let top = (task.kernel_stack.as_ptr() as usize + task.kernel_stack.len()) & !15;
        let address = top - crate::arch::idt::IRQ_FRAME_SIZE;
        unsafe {
            core::ptr::write_bytes(address as *mut u8, 0, crate::arch::idt::IRQ_FRAME_SIZE);
            let frame = address as *mut crate::arch::idt::TrapFrame;
            (*frame).elr = entry as usize as u64;
            (*frame).spsr = 0x345; // EL1h, IRQ enabled, other exception masks retained.
        }
        *task.stack_pointer.lock() = address as u64;
        task.kernel_context = true;
        task
    }
    pub fn new_with_auto_pid(parent_pid: Option<u64>) -> Self {
        Self::new(allocate_pid(), parent_pid)
    }

    /// Mark an identified private user context dead; retain tables until the
    /// scheduler has switched away so TTBR0 cannot reference freed memory.
    pub fn contain_user_fault(&self) -> bool {
        if self.address_space.lock().is_none() {
            return false;
        }
        self.set_exit_code(-11);
        true
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
