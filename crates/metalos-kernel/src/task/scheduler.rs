use crate::sync::IrqMutex as Mutex;
use crate::task::pcb::{Task, TaskState};
use alloc::collections::VecDeque;
use alloc::sync::Arc;

pub struct Scheduler {
    ready_queue: VecDeque<Arc<Task>>,
    current: Option<Arc<Task>>,
    blocked_queue: VecDeque<Arc<Task>>,
    idle: Option<Arc<Task>>,
}

impl Scheduler {
    pub const fn new() -> Self {
        Self {
            ready_queue: VecDeque::new(),
            current: None,
            blocked_queue: VecDeque::new(),
            idle: None,
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
    scheduler.ready_queue.retain(|task| task.kernel_context);
    drop(scheduler);
    crate::drivers::pic::start_timer();
}

pub fn spawn_kernel(pid: u64, entry: extern "C" fn() -> !) {
    add_task(Arc::new(Task::kernel_thread(pid, entry)));
}

pub fn preempt(frame: *mut crate::arch::idt::TrapFrame) -> *mut crate::arch::idt::TrapFrame {
    let mut scheduler = SCHEDULER.lock();
    let Some(current) = scheduler.current.as_ref() else {
        return frame;
    };
    *current.stack_pointer.lock() = frame as u64;
    scheduler.wake_due(crate::drivers::rng::monotonic_time_ns());
    let Some(next) = scheduler.next_task() else {
        return frame;
    };
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
}
