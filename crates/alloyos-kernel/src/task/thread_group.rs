//! Bounded native thread lifetime and stack ownership within one process.
use super::app_process::Error;

pub const MAX_THREADS: usize = 8;
#[derive(Clone, Copy)]
struct Child {
    tid: u64,
    stack_start: u64,
    stack_end: u64,
    exit: Option<i32>,
}

pub(super) struct ThreadGroup {
    pub process_id: u64,
    pub enabled: bool,
    pub exit: Option<i32>,
    active: usize,
    leader_stack: (u64, u64),
    children: [Option<Child>; MAX_THREADS - 1],
}
impl ThreadGroup {
    pub fn new(process_id: u64) -> Self {
        Self {
            process_id,
            enabled: false,
            exit: None,
            active: 1,
            leader_stack: (0, 0),
            children: [None; MAX_THREADS - 1],
        }
    }
    pub fn set_leader_stack(&mut self, start: u64, end: u64) {
        self.leader_stack = (start, end);
    }
    pub fn insert(&mut self, tid: u64, start: u64, end: u64) -> Result<(), Error> {
        if !self.enabled || self.exit.is_some() {
            return Err(Error::Identity);
        }
        let overlaps = |a: u64, b: u64| start < b && a < end;
        if overlaps(self.leader_stack.0, self.leader_stack.1)
            || self
                .children
                .iter()
                .flatten()
                .any(|c| overlaps(c.stack_start, c.stack_end))
        {
            return Err(Error::InvalidRange);
        }
        let slot = self
            .children
            .iter_mut()
            .find(|slot| slot.is_none())
            .ok_or(Error::MemoryLimit)?;
        *slot = Some(Child {
            tid,
            stack_start: start,
            stack_end: end,
            exit: None,
        });
        self.active += 1;
        Ok(())
    }
    pub fn retire(&mut self, tid: u64, code: i32) -> bool {
        if let Some(child) = self.children.iter_mut().flatten().find(|c| c.tid == tid) {
            child.exit = Some(code);
        }
        self.active -= 1;
        self.active == 0
    }
    pub fn join(&mut self, caller: u64, tid: u64) -> Result<Option<i32>, Error> {
        if tid == caller || tid == self.process_id {
            return Err(Error::InvalidRange);
        }
        let slot = self
            .children
            .iter_mut()
            .find(|slot| slot.is_some_and(|c| c.tid == tid))
            .ok_or(Error::Identity)?;
        let code = slot.as_ref().unwrap().exit;
        if code.is_some() {
            *slot = None;
        }
        Ok(code)
    }
}
