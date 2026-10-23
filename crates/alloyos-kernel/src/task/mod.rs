pub mod app_process;
pub mod js_process;
pub mod ipc;
pub mod pcb;
mod thread_group;
pub mod scheduler;
#[cfg(feature = "process-selftest")]
pub mod selftest;
#[cfg(feature = "process-selftest")]
mod web_runtime_selftest;
#[cfg(feature = "process-selftest")]
mod native_threads_selftest;

#[cfg(test)]
pub mod tests;

use alloc::sync::Arc;

pub fn init() {
    let init_task = Arc::new(pcb::Task::new(1, None));
    scheduler::add_task(init_task);
}
