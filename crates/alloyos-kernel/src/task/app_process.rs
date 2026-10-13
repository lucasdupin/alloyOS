//! Protected-app preparation. This module does not launch EL0 or host JavaScript.
//! Activation must wait for user stack switching and validated syscall copy-in/out.
use crate::arch::mmu::L1PageTable;
use alloc::{boxed::Box, collections::VecDeque, vec::Vec};

pub const USER_BASE: u64 = 0x10_0000_0000;
pub const PAGE_SIZE: usize = 4096;
pub const MAX_MESSAGE: usize = 4096;
pub const MAX_QUEUE: usize = 16;
const ADDRESS_MASK: u64 = 0x0000_ffff_ffff_f000;

#[repr(C, align(4096))]
struct Page([u8; PAGE_SIZE]);
#[repr(C, align(4096))]
struct Table([u64; 512]);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Error {
    InvalidImage,
    MemoryLimit,
    InvalidRange,
    QueueFull,
    MessageTooLarge,
    Dead,
    Identity,
    Timeout,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExitReason {
    Fault,
    MemoryLimit,
    Timeout,
    Cancelled,
}

/// Real three-level ARMv8 tables and private page backing. Kernel identity mappings
/// retain EL1-only AP permissions; only the dedicated high user window grants EL0.
/// Physical addresses assume the current kernel's identity-mapped allocator.
pub struct AddressSpace {
    root: Box<L1PageTable>,
    _middle: Box<Table>,
    _leaf: Box<Table>,
    pages: Vec<Box<Page>>,
    code_pages: usize,
}
impl AddressSpace {
    pub fn prepare(image: &[u8], writable_pages: usize, max_pages: usize) -> Result<Self, Error> {
        if image.is_empty() {
            return Err(Error::InvalidImage);
        }
        let code_pages = image
            .len()
            .checked_add(PAGE_SIZE - 1)
            .ok_or(Error::MemoryLimit)?
            / PAGE_SIZE;
        let count = code_pages
            .checked_add(writable_pages)
            .ok_or(Error::MemoryLimit)?;
        if writable_pages == 0 || count > max_pages || count > 512 {
            return Err(Error::MemoryLimit);
        }
        let mut root = Box::new(L1PageTable::new_identity_2gib());
        let mut middle = Box::new(Table([0; 512]));
        let mut leaf = Box::new(Table([0; 512]));
        let mut pages = Vec::with_capacity(count);
        for index in 0..count {
            let mut page = Box::new(Page([0; PAGE_SIZE]));
            if index < code_pages {
                let start = index * PAGE_SIZE;
                let end = core::cmp::min(start + PAGE_SIZE, image.len());
                page.0[..end - start].copy_from_slice(&image[start..end]);
            }
            let physical = &*page as *const Page as u64;
            // AP=11: EL0/EL1 read-only executable; AP=01: EL0 RW, XN.
            let permissions = if index < code_pages {
                (3 << 6) | (1 << 53)
            } else {
                (1 << 6) | (3 << 53)
            };
            leaf.0[index] =
                (physical & ADDRESS_MASK) | 3 | (1 << 2) | (3 << 8) | (1 << 10) | permissions;
            pages.push(page);
        }
        middle.0[0] = (&*leaf as *const Table as u64 & ADDRESS_MASK) | 3;
        root.entries[(USER_BASE >> 30) as usize] =
            (&*middle as *const Table as u64 & ADDRESS_MASK) | 3;
        Ok(Self {
            root,
            _middle: middle,
            _leaf: leaf,
            pages,
            code_pages,
        })
    }
    pub fn root_physical_address(&self) -> u64 {
        &*self.root as *const L1PageTable as u64
    }
    pub fn stack_top(&self) -> u64 {
        USER_BASE + (self.pages.len() * PAGE_SIZE) as u64
    }
    pub fn validate_range(&self, address: u64, length: usize, write: bool) -> Result<(), Error> {
        let offset = address.checked_sub(USER_BASE).ok_or(Error::InvalidRange)?;
        let end = offset
            .checked_add(length as u64)
            .ok_or(Error::InvalidRange)?;
        if end > (self.pages.len() * PAGE_SIZE) as u64
            || (write && offset < (self.code_pages * PAGE_SIZE) as u64)
        {
            return Err(Error::InvalidRange);
        }
        Ok(())
    }
}

pub struct Message {
    pub caller_pid: u64,
    pub request: u64,
    pub payload: Vec<u8>,
}
/// Owner identity is kernel-provided at creation and verified against scheduler PID.
/// Never construct caller identity from JSON supplied by an app.
pub struct AppProcess {
    pid: u64,
    pub address_space: Option<AddressSpace>,
    deadline: u64,
    exit: Option<ExitReason>,
    inbox: VecDeque<Message>,
}
impl AppProcess {
    pub fn prepared(pid: u64, address_space: AddressSpace, deadline: u64) -> Self {
        Self {
            pid,
            address_space: Some(address_space),
            deadline,
            exit: None,
            inbox: VecDeque::new(),
        }
    }
    pub fn enqueue_current(&mut self, request: u64, payload: &[u8], now: u64) -> Result<(), Error> {
        let caller = super::scheduler::current_task().ok_or(Error::Identity)?;
        self.enqueue_from(caller.pid, request, payload, now)
    }
    fn enqueue_from(
        &mut self,
        scheduled_pid: u64,
        request: u64,
        payload: &[u8],
        now: u64,
    ) -> Result<(), Error> {
        if self.exit.is_some() {
            return Err(Error::Dead);
        }
        if now >= self.deadline {
            self.terminate(ExitReason::Timeout);
            return Err(Error::Timeout);
        }
        if scheduled_pid != self.pid {
            return Err(Error::Identity);
        }
        if payload.len() > MAX_MESSAGE {
            return Err(Error::MessageTooLarge);
        }
        if self.inbox.len() >= MAX_QUEUE {
            return Err(Error::QueueFull);
        }
        self.inbox.push_back(Message {
            caller_pid: self.pid,
            request,
            payload: payload.to_vec(),
        });
        Ok(())
    }
    pub fn receive(&mut self) -> Option<Message> {
        self.inbox.pop_front()
    }
    pub fn terminate(&mut self, reason: ExitReason) {
        self.exit = Some(reason);
        self.inbox.clear();
        self.address_space = None; // drops private pages and tables
    }
    pub fn exit_reason(&self) -> Option<ExitReason> {
        self.exit
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn space() -> AddressSpace {
        AddressSpace::prepare(&[0xc0, 3, 0x5f, 0xd6], 2, 3).unwrap()
    }
    #[test]
    fn private_tables_enforce_permissions_and_leave_kernel_privileged() {
        let first = space();
        let second = space();
        assert_ne!(
            first.root_physical_address(),
            second.root_physical_address()
        );
        assert_ne!(
            first._leaf.0[0] & ADDRESS_MASK,
            second._leaf.0[0] & ADDRESS_MASK
        );
        assert_eq!(first.root.entries[1] & (3 << 6), 0);
        assert_eq!(first._leaf.0[0] & (3 << 6), 3 << 6);
        assert_eq!(first._leaf.0[1] & (3 << 53), 3 << 53);
        assert_ne!(first._middle.0[0], 0);
        assert!(first.validate_range(USER_BASE, 4, false).is_ok());
        assert_eq!(
            first.validate_range(USER_BASE, 4, true),
            Err(Error::InvalidRange)
        );
        assert!(first.validate_range(USER_BASE + 4096, 8192, true).is_ok());
        assert_eq!(
            first.validate_range(0x4000_0000, 1, false),
            Err(Error::InvalidRange)
        );
        assert_eq!(
            first.validate_range(u64::MAX, 2, false),
            Err(Error::InvalidRange)
        );
    }
    #[test]
    fn cancelling_one_prepared_app_keeps_other_private_pages_and_queue_alive() {
        let mut first = AppProcess::prepared(81, space(), 100);
        let mut second = AppProcess::prepared(82, space(), 100);
        for first_page in &first.address_space.as_ref().unwrap().pages {
            for second_page in &second.address_space.as_ref().unwrap().pages {
                assert_ne!(&**first_page as *const Page, &**second_page as *const Page);
            }
        }
        second.enqueue_from(82, 9, b"alive", 0).unwrap();
        first.terminate(ExitReason::Fault);
        assert!(first.address_space.is_none());
        assert!(second.address_space.is_some());
        assert_eq!(second.receive().unwrap().payload, b"alive");
        assert!(second.address_space.as_ref().unwrap()._leaf.0[3..]
            .iter()
            .all(|entry| *entry == 0));
        assert_eq!(
            second.address_space.as_ref().unwrap().validate_range(
                second.address_space.as_ref().unwrap().stack_top(),
                1,
                true
            ),
            Err(Error::InvalidRange)
        );
    }
    #[test]
    fn memory_and_ipc_are_bounded_and_teardown_releases_backing() {
        assert!(matches!(
            AddressSpace::prepare(&[1], 3, 3),
            Err(Error::MemoryLimit)
        ));
        let mut process = AppProcess::prepared(71, space(), 100);
        assert_eq!(
            process.enqueue_from(72, 1, b"forged", 0),
            Err(Error::Identity)
        );
        assert_eq!(
            process.enqueue_from(71, 1, &[0; MAX_MESSAGE + 1], 0),
            Err(Error::MessageTooLarge)
        );
        for request in 0..MAX_QUEUE {
            process.enqueue_from(71, request as u64, b"ok", 0).unwrap();
        }
        assert_eq!(
            process.enqueue_from(71, 99, b"ok", 0),
            Err(Error::QueueFull)
        );
        assert_eq!(process.receive().unwrap().caller_pid, 71);
        assert_eq!(
            process.enqueue_from(71, 99, b"ok", 100),
            Err(Error::Timeout)
        );
        assert!(process.address_space.is_none());
        assert!(process.receive().is_none());
        assert_eq!(process.enqueue_from(71, 1, b"ok", 101), Err(Error::Dead));
    }
}
