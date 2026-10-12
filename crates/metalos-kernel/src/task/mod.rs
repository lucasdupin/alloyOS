pub mod pcb;
pub mod scheduler;

#[cfg(any(test, feature = "host-tests"))]
pub mod tests;

use alloc::sync::Arc;

pub fn init() {
    let init_task = Arc::new(pcb::Task::new(1, None));
    scheduler::add_task(init_task);
}
