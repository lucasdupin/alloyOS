use crate::sync::IrqMutex as Mutex;
use alloc::sync::Arc;
use alloc::vec;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskState {
    Ready,
    Running,
    Blocked,
    Zombie,
}

// PIDs 1..=3 are reserved for boot, Forge worker and idle.
static NEXT_PID: AtomicU64 = AtomicU64::new(4);

pub fn allocate_pid() -> u64 {
    NEXT_PID.fetch_add(1, Ordering::Relaxed)
}

pub struct Task {
    pub pid: u64,
    pub kernel_context: bool,
    /// Private tables for protected EL0 contexts.
    pub address_space: Arc<Mutex<Option<super::app_process::AddressSpace>>>,
    pub parent_pid: Option<u64>,
    pub state: Mutex<TaskState>,
    pub wake_deadline: Mutex<Option<u64>>,
    /// Absolute monotonic execution budget; only protected apps are expired.
    pub execution_deadline: Arc<Mutex<Option<u64>>>,
    pub kernel_stack: Vec<u8>,
    pub stack_pointer: Mutex<u64>,
    pub exit_code: Mutex<Option<i32>>,
    pub ports: Arc<Mutex<Vec<super::ipc::Port>>>,
    group: Arc<Mutex<super::thread_group::ThreadGroup>>,
    retired: AtomicBool,
    pub(super) wait_word: Mutex<Option<u64>>,
    #[cfg(feature = "process-selftest")]
    timer_preemptions: AtomicU64,
}

impl Task {
    pub const DEFAULT_STACK_SIZE: usize = 16 * 1024; // flat isolation probes
    /// Production syscall handlers parse bounded nested JSON and invoke kernel
    /// Rust in dev builds. Charge this fallible backing to the shared kernel heap.
    pub const EXECUTABLE_STACK_SIZE: usize = 128 * 1024;

    pub fn new(pid: u64, parent_pid: Option<u64>) -> Self {
        Self::try_new(pid, parent_pid).expect("kernel task stack allocation failed")
    }
    fn try_new(pid: u64, parent_pid: Option<u64>) -> Result<Self, super::app_process::Error> {
        Self::try_new_with_stack(pid, parent_pid, Self::DEFAULT_STACK_SIZE)
    }
    fn try_new_with_stack(
        pid: u64,
        parent_pid: Option<u64>,
        stack_size: usize,
    ) -> Result<Self, super::app_process::Error> {
        NEXT_PID.fetch_max(pid.saturating_add(1), Ordering::Relaxed);
        let mut stack = Vec::new();
        stack
            .try_reserve_exact(stack_size)
            .map_err(|_| super::app_process::Error::MemoryLimit)?;
        stack.resize(stack_size, 0);
        let stack_top = stack.as_ptr() as u64 + stack_size as u64;

        Ok(Self {
            pid,
            kernel_context: false,
            address_space: Arc::new(Mutex::new(None)),
            parent_pid,
            state: Mutex::new(TaskState::Ready),
            wake_deadline: Mutex::new(None),
            execution_deadline: Arc::new(Mutex::new(None)),
            kernel_stack: stack,
            stack_pointer: Mutex::new(stack_top),
            exit_code: Mutex::new(None),
            ports: Arc::new(Mutex::new(Vec::new())),
            group: Arc::new(Mutex::new(super::thread_group::ThreadGroup::new(pid))),
            retired: AtomicBool::new(false),
            wait_word: Mutex::new(None),
            #[cfg(feature = "process-selftest")]
            timer_preemptions: AtomicU64::new(0),
        })
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

    /// Construct an exception frame that the scheduler restores with ERET to EL0.
    /// The initial executable format is a bounded flat image linked at USER_BASE.
    pub fn user_process(
        image: &[u8],
        writable_pages: usize,
        max_pages: usize,
    ) -> Result<Self, super::app_process::Error> {
        let space = super::app_process::AddressSpace::prepare(image, writable_pages, max_pages)?;
        let task = Self::try_new(allocate_pid(), Some(1))?;
        let top = (task.kernel_stack.as_ptr() as usize + task.kernel_stack.len()) & !15;
        let address = top - crate::arch::idt::IRQ_FRAME_SIZE;
        unsafe {
            core::ptr::write_bytes(address as *mut u8, 0, crate::arch::idt::IRQ_FRAME_SIZE);
            let frame = &mut *(address as *mut crate::arch::idt::TrapFrame);
            frame.elr = super::app_process::USER_BASE;
            frame.spsr = 0x340; // EL0t, IRQ enabled.
            frame.user_sp = space.stack_top() & !15;
        }
        *task.stack_pointer.lock() = address as u64;
        task.group.lock().set_leader_stack(
            space.heap_start().max(
                space
                    .stack_top()
                    .saturating_sub((64 * super::app_process::PAGE_SIZE) as u64),
            ),
            space.stack_top(),
        );
        *task.address_space.lock() = Some(space);
        Ok(task)
    }

    pub fn user_executable(
        image: &[u8],
        writable_pages: usize,
        max_pages: usize,
    ) -> Result<Self, super::app_process::Error> {
        if writable_pages <= 64 {
            return Err(super::app_process::Error::MemoryLimit);
        }
        let (space, entry) =
            super::app_process::AddressSpace::load_elf(image, writable_pages, max_pages)?;
        let task = Self::try_new_with_stack(allocate_pid(), Some(1), Self::EXECUTABLE_STACK_SIZE)?;
        let top = (task.kernel_stack.as_ptr() as usize + task.kernel_stack.len()) & !15;
        let address = top - crate::arch::idt::IRQ_FRAME_SIZE;
        unsafe {
            core::ptr::write_bytes(address as *mut u8, 0, crate::arch::idt::IRQ_FRAME_SIZE);
            let frame = &mut *(address as *mut crate::arch::idt::TrapFrame);
            frame.elr = entry;
            frame.spsr = 0x340;
            frame.user_sp = space.stack_top() & !15;
            frame.thread_pointer = space.initial_thread_pointer();
            frame.regs[0] = space.heap_start();
            frame.regs[1] = space.heap_length();
        }
        *task.stack_pointer.lock() = address as u64;
        task.group.lock().set_leader_stack(
            space.stack_top() - (64 * super::app_process::PAGE_SIZE) as u64,
            space.stack_top(),
        );
        *task.address_space.lock() = Some(space);
        Ok(task)
    }

    /// Mark an identified private user context dead; retain tables until the
    /// scheduler has switched away so TTBR0 cannot reference freed memory.
    pub fn contain_user_fault(&self) -> bool {
        if self.address_space.lock().is_none() {
            return false;
        }
        self.exit_process(-11);
        true
    }

    pub fn process_id(&self) -> u64 {
        self.group.lock().process_id
    }

    /// Supervisor-only policy; protected code has no syscall to grant this.
    pub fn enable_native_threads(&self) {
        self.group.lock().enabled = true;
    }
    pub fn native_threads_enabled(&self) -> bool {
        self.group.lock().enabled
    }
    pub fn process_exit_code(&self) -> Option<i32> {
        self.group.lock().exit
    }
    pub fn exit_process(&self, code: i32) {
        let mut group = self.group.lock();
        if group.exit.is_none() {
            group.exit = Some(code);
        }
        drop(group);
        self.set_exit_code(code);
    }
    pub fn exit_thread(&self, code: i32) { self.set_exit_code(code); }

    pub fn join_thread(&self, tid: u64) -> Result<Option<i32>, super::app_process::Error> {
        self.group.lock().join(self.pid, tid)
    }

    /// Construct a real EL0 context sharing this process's private page tables.
    pub fn user_thread(
        &self,
        space: &super::app_process::AddressSpace,
        entry: u64,
        argument: u64,
        stack_start: u64,
        stack_len: usize,
        thread_pointer: u64,
    ) -> Result<Self, super::app_process::Error> {
        use super::app_process::{Error, PAGE_SIZE};
        if !self.native_threads_enabled() {
            return Err(Error::Identity);
        }
        if stack_len < 4 * PAGE_SIZE
            || stack_len % PAGE_SIZE != 0
            || stack_start % PAGE_SIZE as u64 != 0
            || thread_pointer % 16 != 0
        {
            return Err(Error::InvalidRange);
        }
        let stack_end = stack_start
            .checked_add(stack_len as u64)
            .ok_or(Error::InvalidRange)?;
        space.validate_executable(entry, 4)?;
        space.validate_range(stack_start, stack_len, true)?;
        space.validate_range(thread_pointer, 16, true)?;
        let mut task = Self::try_new_with_stack(
            allocate_pid(),
            Some(self.process_id()),
            Self::EXECUTABLE_STACK_SIZE,
        )?;
        let top = (task.kernel_stack.as_ptr() as usize + task.kernel_stack.len()) & !15;
        let address = top - crate::arch::idt::IRQ_FRAME_SIZE;
        unsafe {
            core::ptr::write_bytes(address as *mut u8, 0, crate::arch::idt::IRQ_FRAME_SIZE);
            let frame = &mut *(address as *mut crate::arch::idt::TrapFrame);
            frame.elr = entry;
            frame.spsr = 0x340;
            frame.user_sp = stack_end;
            frame.thread_pointer = thread_pointer;
            frame.regs[0] = argument;
        }
        *task.stack_pointer.lock() = address as u64;
        self.group.lock().insert(task.pid, stack_start, stack_end)?;
        task.address_space = self.address_space.clone();
        task.ports = self.ports.clone();
        task.execution_deadline = self.execution_deadline.clone();
        task.group = self.group.clone();
        Ok(task)
    }

    /// Called only once its exception stack and TTBR are no longer in use.
    pub(crate) fn retire_context(&self) {
        if self.retired.swap(true, Ordering::AcqRel) {
            return;
        }
        let final_thread = self
            .group
            .lock()
            .retire(self.pid, self.exit_code().unwrap_or(-125));
        if final_thread {
            self.address_space.lock().take();
            self.ports.lock().clear();
        }
    }

    /// Native acceptance evidence: timer IRQs from EL0 which switched to a peer.
    #[cfg(feature = "process-selftest")]
    pub fn timer_preemptions(&self) -> u64 {
        self.timer_preemptions.load(Ordering::Relaxed)
    }

    #[cfg(feature = "process-selftest")]
    pub(super) fn record_timer_preemption(&self) {
        self.timer_preemptions.fetch_add(1, Ordering::Relaxed);
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

#[cfg(test)]
mod stack_budget_tests {
    use super::*;
    #[test]
    fn production_exception_stack_is_fallible_and_preserves_frame_headroom() {
        let task = Task::try_new_with_stack(910, Some(1), Task::EXECUTABLE_STACK_SIZE).unwrap();
        assert_eq!(task.kernel_stack.len(), 128 * 1024);
        let base = task.kernel_stack.as_ptr() as usize;
        let top = (base + task.kernel_stack.len()) & !15;
        let frame = top - crate::arch::idt::IRQ_FRAME_SIZE;
        assert_eq!(frame % 16, 0);
        assert!(frame - base > 127 * 1024);
        assert!(matches!(
            Task::try_new_with_stack(911, Some(1), usize::MAX),
            Err(super::super::app_process::Error::MemoryLimit)
        ));
    }
}
