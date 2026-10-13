#[cfg(test)]
pub mod tests {
    use crate::task::pcb::{Task, TaskState};
    use crate::task::scheduler::Scheduler;
    use alloc::collections::BTreeSet;
    use alloc::sync::Arc;

    #[test]
    fn task_initial_state_is_ready() {
        let task = Task::new_with_auto_pid(None);
        assert_eq!(task.state(), TaskState::Ready);
    }

    #[test]
    fn task_pid_is_unique() {
        let mut pids = BTreeSet::new();
        for _ in 0..5 {
            let task = Task::new_with_auto_pid(None);
            let inserted = pids.insert(task.pid);
            assert!(inserted, "Duplicate PID encountered: {}", task.pid);
        }
        assert_eq!(pids.len(), 5);
    }

    #[test]
    fn scheduler_round_robin_order() {
        let mut sched = Scheduler::new();
        let task_a = Arc::new(Task::new(101, None));
        let task_b = Arc::new(Task::new(102, None));
        let task_c = Arc::new(Task::new(103, None));

        sched.add_task(task_a.clone());
        sched.add_task(task_b.clone());
        sched.add_task(task_c.clone());

        let mut observed_pids = alloc::vec::Vec::new();
        for _ in 0..6 {
            let next = sched.next_task().expect("Task expected");
            observed_pids.push(next.pid);
        }

        assert_eq!(observed_pids, alloc::vec![101, 102, 103, 101, 102, 103]);
    }

    #[test]
    fn scheduler_empty_returns_none() {
        let mut sched = Scheduler::new();
        assert!(sched.next_task().is_none());
    }

    #[test]
    fn task_state_transition() {
        let task = Task::new_with_auto_pid(None);
        assert_eq!(task.state(), TaskState::Ready);

        task.set_state(TaskState::Running);
        assert_eq!(task.state(), TaskState::Running);

        task.set_state(TaskState::Blocked);
        assert_eq!(task.state(), TaskState::Blocked);

        task.set_state(TaskState::Ready);
        assert_eq!(task.state(), TaskState::Ready);

        task.set_exit_code(0);
        assert_eq!(task.state(), TaskState::Zombie);
        assert_eq!(task.exit_code(), Some(0));
    }
}
