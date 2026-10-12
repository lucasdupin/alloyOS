use alloc::collections::VecDeque;
use alloc::sync::Arc;
use spin::Mutex;
use crate::task::pcb::{Task, TaskState};

pub struct Scheduler {
    ready_queue: VecDeque<Arc<Task>>,
    current: Option<Arc<Task>>,
}

impl Scheduler {
    pub const fn new() -> Self {
        Self {
            ready_queue: VecDeque::new(),
            current: None,
        }
    }

    pub fn add_task(&mut self, task: Arc<Task>) {
        task.set_state(TaskState::Ready);
        self.ready_queue.push_back(task);
    }

    pub fn next_task(&mut self) -> Option<Arc<Task>> {
        // If current task is running, place it back on queue
        if let Some(prev) = self.current.take() {
            if prev.state() == TaskState::Running {
                prev.set_state(TaskState::Ready);
                self.ready_queue.push_back(prev);
            }
        }

        while let Some(task) = self.ready_queue.pop_front() {
            if task.state() == TaskState::Ready {
                task.set_state(TaskState::Running);
                self.current = Some(task.clone());
                return Some(task);
            }
        }

        None
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
