use crate::sync::IrqMutex as Mutex;
use crate::task::pcb::{Task, TaskState};
use alloc::collections::VecDeque;
use alloc::sync::Arc;

pub struct Scheduler {
    ready_queue: VecDeque<Arc<Task>>,
    current: Option<Arc<Task>>,
    blocked_queue: VecDeque<Arc<Task>>,
    idle: Option<Arc<Task>>,
    retired_contexts: VecDeque<Arc<Task>>,
}

impl Scheduler {
    pub const fn new() -> Self {
        Self {
            ready_queue: VecDeque::new(),
            current: None,
            blocked_queue: VecDeque::new(),
            idle: None,
            retired_contexts: VecDeque::new(),
        }
    }

    pub fn add_task(&mut self, task: Arc<Task>) {
        task.set_state(TaskState::Ready);
        self.ready_queue.push_back(task);
    }

    pub fn next_task(&mut self) -> Option<Arc<Task>> {
        // If current task is running, place it back on queue
        if let Some(prev) = self.current.take() {
            if self
                .idle
                .as_ref()
                .is_some_and(|idle| Arc::ptr_eq(idle, &prev))
            {
                // Idle is a fallback, never a peer competing with runnable work.
            } else if prev.state() == TaskState::Running {
                prev.set_state(TaskState::Ready);
                self.ready_queue.push_back(prev);
            } else if prev.state() == TaskState::Blocked {
                self.blocked_queue.push_back(prev);
            }
        }

        while let Some(task) = self.ready_queue.pop_front() {
            if task.state() == TaskState::Ready {
                task.set_state(TaskState::Running);
                self.current = Some(task.clone());
                return Some(task);
            }
        }

        if let Some(idle) = self.idle.clone() {
            idle.set_state(TaskState::Running);
            self.current = Some(idle.clone());
            return Some(idle);
        }
        None
    }

    fn expire_due(&mut self, now: u64) {
        for task in self
            .current
            .iter()
            .chain(self.ready_queue.iter())
            .chain(self.blocked_queue.iter())
        {
            if task.state() != TaskState::Zombie
                && task
                    .execution_deadline
                    .lock()
                    .is_some_and(|deadline| now >= deadline)
                && task.address_space.lock().is_some()
            {
                task.set_exit_code(-110);
                *task.wake_deadline.lock() = None;
            }
        }
        // Queued contexts never own the current exception stack or TTBR0, so
        // their backing can be released immediately after their budget expires.
        let release_zombie = |task: &Arc<Task>| {
            if task.state() != TaskState::Zombie {
                return true;
            }
            task.address_space.lock().take();
            task.ports.lock().clear();
            false
        };
        self.ready_queue.retain(release_zombie);
        self.blocked_queue.retain(release_zombie);
    }

    fn wake_due(&mut self, now: u64) {
        if let Some(current) = self.current.as_ref() {
            if current.state() == TaskState::Blocked
                && current
                    .wake_deadline
                    .lock()
                    .is_some_and(|deadline| deadline <= now)
            {
                *current.wake_deadline.lock() = None;
                current.set_state(TaskState::Running);
            }
        }
        while let Some(index) = self.blocked_queue.iter().position(|task| {
            task.wake_deadline
                .lock()
                .is_some_and(|deadline| deadline <= now)
        }) {
            let task = self.blocked_queue.remove(index).unwrap();
            *task.wake_deadline.lock() = None;
            self.add_task(task);
        }
    }

    pub fn current_task(&self) -> Option<Arc<Task>> {
        self.current.clone()
    }

    pub fn tick(&mut self) {
        // Time slice expired: switch to next task
        self.next_task();
    }
}

pub static SCHEDULER: Mutex<Scheduler> = Mutex::new(Scheduler::new());

pub fn add_task(task: Arc<Task>) {
    SCHEDULER.lock().add_task(task);
}

pub fn next_task() -> Option<Arc<Task>> {
    SCHEDULER.lock().next_task()
}

pub fn current_task() -> Option<Arc<Task>> {
    SCHEDULER.lock().current_task()
}

pub fn tick() {
    SCHEDULER.lock().tick();
}

/// Adopt the boot/UI context and enable timer-driven kernel context switching.
pub fn start_preemption() {
    let mut scheduler = SCHEDULER.lock();
    let boot = Arc::new(Task::new(1, None));
    boot.set_state(TaskState::Running);
    scheduler.current = Some(boot);
    scheduler.idle = Some(Arc::new(Task::kernel_thread(3, idle_run)));
    scheduler
        .ready_queue
        .retain(|task| task.kernel_context || task.address_space.lock().is_some());
    drop(scheduler);
    crate::drivers::pic::start_timer();
}

pub fn spawn_kernel(pid: u64, entry: extern "C" fn() -> !) {
    add_task(Arc::new(Task::kernel_thread(pid, entry)));
}

pub fn preempt(frame: *mut crate::arch::idt::TrapFrame) -> *mut crate::arch::idt::TrapFrame {
    let mut scheduler = SCHEDULER.lock();
    // We have entered on the next context's exception stack. Previously retired
    // stacks are now safe to reap; never drop the stack this handler runs on.
    scheduler.retired_contexts.clear();
    scheduler.expire_due(crate::drivers::rng::monotonic_time_ns());
    let Some(current) = scheduler.current.as_ref() else {
        return frame;
    };
    let previous_root = current
        .address_space
        .lock()
        .as_ref()
        .map(|space| space.root_physical_address())
        .unwrap_or_else(crate::arch::mmu::kernel_root_address);
    let retired = if current.state() == TaskState::Zombie {
        Some(current.clone())
    } else {
        None
    };
    *current.stack_pointer.lock() = frame as u64;
    scheduler.wake_due(crate::drivers::rng::monotonic_time_ns());
    let Some(next) = scheduler.next_task() else {
        return frame;
    };
    let root = next
        .address_space
        .lock()
        .as_ref()
        .map(|space| space.root_physical_address())
        .unwrap_or_else(crate::arch::mmu::kernel_root_address);
    if root != previous_root {
        crate::arch::mmu::switch_address_space(root);
    }
    if let Some(retired) = retired {
        // TTBR0 no longer references these tables. Keep the current exception
        // stack alive until another context has entered an exception.
        retired.address_space.lock().take();
        retired.ports.lock().clear();
        scheduler.retired_contexts.push_back(retired);
    }
    let address = *next.stack_pointer.lock();
    if address == 0 {
        frame
    } else {
        address as *mut _
    }
}

extern "C" fn idle_run() -> ! {
    loop {
        crate::arch::wfi();
    }
}

/// Give unused frame time to other tasks; the timer wakes this task at its deadline.
pub fn sleep_until(deadline: u64) {
    let scheduler = SCHEDULER.lock();
    if crate::drivers::rng::monotonic_time_ns() >= deadline {
        return;
    }
    if let Some(current) = scheduler.current.as_ref() {
        *current.wake_deadline.lock() = Some(deadline);
        current.set_state(TaskState::Blocked);
    }
    drop(scheduler);
    crate::arch::wfi();
}

pub fn block_current() {
    if let Some(current) = SCHEDULER.lock().current.as_ref() {
        current.set_state(TaskState::Blocked);
    }
}
pub fn wake(pid: u64) {
    let mut scheduler = SCHEDULER.lock();
    if let Some(current) = scheduler.current.as_ref() {
        if current.pid == pid && current.state() == TaskState::Blocked {
            *current.wake_deadline.lock() = None;
            current.set_state(TaskState::Running);
        }
    }
    if let Some(index) = scheduler
        .blocked_queue
        .iter()
        .position(|task| task.pid == pid)
    {
        let task = scheduler.blocked_queue.remove(index).unwrap();
        *task.wake_deadline.lock() = None;
        scheduler.add_task(task);
    }
}

#[cfg(test)]
mod preemption_tests {
    use super::*;
    #[test]
    fn cancelled_queued_display_backing_is_reaped_without_deadline() {
        let mut scheduler = Scheduler::new();
        let boot = Arc::new(Task::new(940, None));
        let display = Arc::new(Task::user_process(&[1], 1024, 1025).unwrap());
        scheduler.add_task(boot.clone());
        scheduler.add_task(display.clone());
        assert_eq!(scheduler.next_task().unwrap().pid, boot.pid);
        display.set_exit_code(-125);
        scheduler.expire_due(0);
        assert!(display.address_space.lock().is_none());
        assert!(scheduler.ready_queue.is_empty());
        assert_eq!(scheduler.current_task().unwrap().pid, boot.pid);
    }
    #[test]
    fn execution_budget_expires_running_blocked_and_ready_private_tasks() {
        let mut scheduler = Scheduler::new();
        let running = Arc::new(Task::user_process(&[1], 1, 2).unwrap());
        let blocked = Arc::new(Task::user_process(&[1], 1, 2).unwrap());
        let ready = Arc::new(Task::user_process(&[1], 1, 2).unwrap());
        let kernel = Arc::new(Task::new(810, None));
        for task in [&running, &blocked, &ready, &kernel] {
            *task.execution_deadline.lock() = Some(100);
        }
        for task in [&blocked, &ready] {
            task.ports.lock().push(super::super::ipc::Port {
                endpoint: super::super::ipc::Endpoint::new(),
                send: true,
                receive: true,
            });
        }
        scheduler.add_task(running.clone());
        scheduler.add_task(blocked.clone());
        scheduler.add_task(ready.clone());
        scheduler.add_task(kernel.clone());
        scheduler.next_task();
        blocked.set_state(TaskState::Blocked);
        scheduler.blocked_queue.push_back(blocked.clone());
        scheduler.ready_queue.retain(|task| task.pid != blocked.pid);
        scheduler.expire_due(99);
        assert_eq!(running.state(), TaskState::Running);
        scheduler.expire_due(100);
        for task in [&running, &blocked, &ready] {
            assert_eq!(task.exit_code(), Some(-110));
        }
        assert_eq!(scheduler.next_task().unwrap().pid, kernel.pid);
        assert!(scheduler.blocked_queue.is_empty());
        assert!(blocked.address_space.lock().is_none());
        assert!(ready.address_space.lock().is_none());
        assert!(blocked.ports.lock().is_empty());
        assert!(ready.ports.lock().is_empty());
    }
    #[test]
    fn retired_exception_stack_survives_until_next_context_entry() {
        let mut scheduler = Scheduler::new();
        let retired = Arc::new(Task::new(711, None));
        let weak = Arc::downgrade(&retired);
        scheduler.retired_contexts.push_back(retired);
        assert!(weak.upgrade().is_some());
        scheduler.retired_contexts.clear();
        assert!(weak.upgrade().is_none());
    }

    #[test]
    fn private_faulted_task_is_skipped_and_peer_survives() {
        let mut scheduler = Scheduler::new();
        let user = Arc::new(Task::new(701, None));
        let peer = Arc::new(Task::new(702, None));
        *user.address_space.lock() =
            Some(super::super::app_process::AddressSpace::prepare(&[1], 1, 2).unwrap());
        scheduler.add_task(user.clone());
        scheduler.add_task(peer.clone());
        assert_eq!(scheduler.next_task().unwrap().pid, 701);
        assert!(user.contain_user_fault());
        assert_eq!(user.exit_code(), Some(-11));
        assert_eq!(scheduler.next_task().unwrap().pid, 702);
        assert!(!peer.contain_user_fault());
        assert_eq!(peer.state(), TaskState::Running);
    }

    #[test]
    fn user_stack_slot_matches_exception_assembly() {
        use crate::arch::idt::TrapFrame;
        assert_eq!(core::mem::offset_of!(TrapFrame, user_sp), 280);
        assert_eq!(core::mem::size_of::<TrapFrame>(), 288);
        assert_eq!(crate::arch::idt::IRQ_FRAME_SIZE, 816);
    }

    #[test]
    fn blocked_context_is_retained_for_wakeup() {
        let mut scheduler = Scheduler::new();
        let ui = Arc::new(Task::new(501, None));
        let worker = Arc::new(Task::new(502, None));
        scheduler.add_task(ui.clone());
        scheduler.add_task(worker.clone());
        assert_eq!(scheduler.next_task().unwrap().pid, ui.pid);
        assert_eq!(scheduler.next_task().unwrap().pid, worker.pid);
        worker.set_state(TaskState::Blocked);
        assert_eq!(scheduler.next_task().unwrap().pid, ui.pid);
        assert_eq!(scheduler.blocked_queue.front().unwrap().pid, worker.pid);
        let restored = scheduler.blocked_queue.pop_front().unwrap();
        scheduler.add_task(restored);
        assert_eq!(scheduler.next_task().unwrap().pid, worker.pid);
    }
    #[test]
    fn idle_is_only_used_until_a_sleeping_task_is_due() {
        let mut scheduler = Scheduler::new();
        let ui = Arc::new(Task::new(601, None));
        let idle = Arc::new(Task::new(603, None));
        scheduler.idle = Some(idle.clone());
        scheduler.add_task(ui.clone());
        assert_eq!(scheduler.next_task().unwrap().pid, ui.pid);
        *ui.wake_deadline.lock() = Some(100);
        ui.set_state(TaskState::Blocked);
        assert_eq!(scheduler.next_task().unwrap().pid, idle.pid);
        scheduler.wake_due(99);
        assert_eq!(scheduler.next_task().unwrap().pid, idle.pid);
        scheduler.wake_due(100);
        assert_eq!(scheduler.next_task().unwrap().pid, ui.pid);
        assert!(ui.wake_deadline.lock().is_none());
        assert!(scheduler.blocked_queue.is_empty());
    }

    #[test]
    fn runnable_worker_gets_cpu_while_ui_sleeps() {
        let mut scheduler = Scheduler::new();
        let ui = Arc::new(Task::new(611, None));
        let worker = Arc::new(Task::new(612, None));
        scheduler.idle = Some(Arc::new(Task::new(613, None)));
        scheduler.add_task(ui.clone());
        scheduler.add_task(worker.clone());
        assert_eq!(scheduler.next_task().unwrap().pid, ui.pid);
        *ui.wake_deadline.lock() = Some(100);
        ui.set_state(TaskState::Blocked);
        for _ in 0..5 {
            assert_eq!(scheduler.next_task().unwrap().pid, worker.pid);
        }
        scheduler.wake_due(100);
        assert_eq!(scheduler.next_task().unwrap().pid, ui.pid);
    }

    #[test]
    fn kernel_task_starts_with_aligned_exception_frame() {
        extern "C" fn entry() -> ! {
            panic!("not executed by this test")
        }
        let worker = Task::kernel_thread(503, entry);
        let address = *worker.stack_pointer.lock() as usize;
        assert_eq!(address % 16, 0);
        assert!(address >= worker.kernel_stack.as_ptr() as usize);
        let frame = unsafe { &*(address as *const crate::arch::idt::TrapFrame) };
        assert_eq!(frame.elr, entry as *const () as usize as u64);
        assert_eq!(
            frame.spsr & 0x80,
            0,
            "initial context must allow timer IRQs"
        );
    }

    #[test]
    fn user_task_starts_at_el0_with_private_aligned_stacks() {
        let process = Task::user_process(&[0; 4], 2, 3).unwrap();
        let address = *process.stack_pointer.lock() as usize;
        let frame = unsafe { &*(address as *const crate::arch::idt::TrapFrame) };
        assert_eq!(address % 16, 0);
        assert_eq!(frame.elr, super::super::app_process::USER_BASE);
        assert_eq!(frame.spsr & 15, 0);
        assert_eq!(frame.spsr & 0x80, 0);
        assert_eq!(frame.user_sp % 16, 0);
        assert!(!process.kernel_context);
        assert!(process.address_space.lock().is_some());
    }
}

/// Fault recovery is restricted to explicitly prepared user contexts. An EL1
/// fault or an unidentified EL0 context remains a kernel error.
pub fn contain_user_fault() -> bool {
    let scheduler = SCHEDULER.lock();
    if let Some(task) = scheduler.current.as_ref() {
        return task.contain_user_fault();
    }
    false
}

/// Crash diagnostics must never wait for a lock held by the faulting code.
pub fn try_current_task() -> Option<Arc<Task>> {
    SCHEDULER.try_lock()?.current_task()
}
