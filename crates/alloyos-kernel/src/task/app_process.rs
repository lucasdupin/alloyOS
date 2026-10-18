//! Private process memory, static executable loading, and bounded messages.
//! Service capabilities and JavaScript hosting are managed by the supervisors.
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

fn allocate_box<T>(value: T) -> Result<Box<T>, Error> {
    let layout = core::alloc::Layout::new::<T>();
    let pointer = unsafe { alloc::alloc::alloc(layout) as *mut T };
    if pointer.is_null() {
        return Err(Error::MemoryLimit);
    }
    unsafe {
        pointer.write(value);
        Ok(Box::from_raw(pointer))
    }
}
// Bounded aligned chunks avoid requiring a single large free heap extent when
// restarting a process after allocating its ELF input buffer. One alignment
// pad per MiB keeps overhead bounded without the per-page allocation penalty.
const BACKING_CHUNK_PAGES: usize = 256;
struct Pages {
    chunks: Vec<Box<[Page]>>,
    count: usize,
}
impl Pages {
    fn len(&self) -> usize {
        self.count
    }
    #[cfg(any(test, all(target_arch = "aarch64", target_os = "none")))]
    fn iter(&self) -> impl Iterator<Item = &Page> {
        self.chunks.iter().flat_map(|chunk| chunk.iter())
    }
}
impl core::ops::Index<usize> for Pages {
    type Output = Page;
    fn index(&self, index: usize) -> &Page {
        &self.chunks[index / BACKING_CHUNK_PAGES][index % BACKING_CHUNK_PAGES]
    }
}
impl core::ops::IndexMut<usize> for Pages {
    fn index_mut(&mut self, index: usize) -> &mut Page {
        &mut self.chunks[index / BACKING_CHUNK_PAGES][index % BACKING_CHUNK_PAGES]
    }
}
fn allocate_pages(count: usize) -> Result<Pages, Error> {
    let mut chunks = Vec::new();
    chunks
        .try_reserve_exact(count.div_ceil(BACKING_CHUNK_PAGES))
        .map_err(|_| Error::MemoryLimit)?;
    let mut remaining = count;
    while remaining > 0 {
        let length = remaining.min(BACKING_CHUNK_PAGES);
        let layout = core::alloc::Layout::array::<Page>(length).map_err(|_| Error::MemoryLimit)?;
        let pointer = unsafe { alloc::alloc::alloc_zeroed(layout) as *mut Page };
        if pointer.is_null() {
            #[cfg(all(target_arch = "aarch64", target_os = "none"))]
            {
                let _interrupts = crate::sync::InterruptGuard::new();
                let heap = crate::mm::heap::ALLOCATOR.lock();
                crate::serial_println!("[process-memory] chunk allocation failed bytes={} remaining_pages={} free={} used={}",layout.size(),remaining,heap.free(),heap.used());
            }
            return Err(Error::MemoryLimit);
        }
        chunks.push(unsafe { Box::from_raw(core::ptr::slice_from_raw_parts_mut(pointer, length)) });
        remaining -= length;
    }
    Ok(Pages { chunks, count })
}
fn allocate_flags(count: usize, value: bool) -> Result<Vec<bool>, Error> {
    let mut flags = Vec::new();
    flags
        .try_reserve_exact(count)
        .map_err(|_| Error::MemoryLimit)?;
    flags.resize(count, value);
    Ok(flags)
}

/// Real three-level ARMv8 tables and private page backing. Kernel identity mappings
/// retain EL1-only AP permissions; only the dedicated high user window grants EL0.
/// Physical addresses assume the current kernel's identity-mapped allocator.
pub struct AddressSpace {
    root: Box<L1PageTable>,
    _middle: Box<Table>,
    _leaf: Vec<Box<Table>>,
    pages: Pages,
    writable: Vec<bool>,
    mapped: Vec<bool>,
    heap_start: u64,
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
        if writable_pages == 0 || count > max_pages || count > 512 * 512 {
            return Err(Error::MemoryLimit);
        }
        let mut pages = allocate_pages(count)?;
        let mut root = allocate_box(L1PageTable::new_identity_2gib())?;
        let mut middle = allocate_box(Table([0; 512]))?;
        let mut leaf: Vec<Box<Table>> = Vec::new();
        leaf.try_reserve_exact(count.div_ceil(512))
            .map_err(|_| Error::MemoryLimit)?;
        for _ in 0..count.div_ceil(512) {
            leaf.push(allocate_box(Table([0; 512]))?);
        }
        let mut writable = allocate_flags(count, true)?;
        writable[..code_pages].fill(false);
        let mapped = allocate_flags(count, true)?;
        for index in 0..count {
            let page = &mut pages[index];
            if index < code_pages {
                let start = index * PAGE_SIZE;
                let end = core::cmp::min(start + PAGE_SIZE, image.len());
                page.0[..end - start].copy_from_slice(&image[start..end]);
                // Publish freshly copied executable bytes to the instruction
                // side before any EL0 context can fetch them.
                #[cfg(all(target_arch = "aarch64", target_os = "none"))]
                unsafe {
                    // CTR_EL0.DminLine describes the minimum data cache line.
                    let ctr: u64;
                    core::arch::asm!("mrs {ctr}, ctr_el0", ctr = out(reg) ctr);
                    let line = 4usize << ((ctr >> 16) & 15);
                    let base = page.0.as_ptr() as usize;
                    for address in (base..base + PAGE_SIZE).step_by(line) {
                        core::arch::asm!("dc cvau, {address}", address = in(reg) address);
                    }
                    core::arch::asm!("dsb ish", "ic iallu", "dsb ish", "isb");
                }
            }
            let physical = &*page as *const Page as u64;
            // AP=11: EL0/EL1 read-only executable; AP=01: EL0 RW, XN.
            let permissions = if index < code_pages {
                (3 << 6) | (1 << 53)
            } else {
                (1 << 6) | (3 << 53)
            };
            leaf[index / 512].0[index % 512] =
                (physical & ADDRESS_MASK) | 3 | (1 << 2) | (3 << 8) | (1 << 10) | permissions;
        }
        for (index, table) in leaf.iter().enumerate() {
            middle.0[index] = (&**table as *const Table as u64 & ADDRESS_MASK) | 3;
        }
        root.entries[(USER_BASE >> 30) as usize] =
            (&*middle as *const Table as u64 & ADDRESS_MASK) | 3;
        Ok(Self {
            root,
            _middle: middle,
            _leaf: leaf,
            pages,
            writable,
            mapped,
            heap_start: USER_BASE + (code_pages * PAGE_SIZE) as u64,
        })
    }
    /// Load a static ELF64 AArch64 executable. Reject dynamic images, ELF TLS,
    /// overlapping segments and writable executable pages; gaps remain inaccessible
    /// to EL0. Thread-local storage requires runtime setup this loader does not provide.
    pub fn load_elf(
        image: &[u8],
        writable_pages: usize,
        max_pages: usize,
    ) -> Result<(Self, u64), Error> {
        fn u16at(b: &[u8], o: usize) -> Result<u16, Error> {
            Ok(u16::from_le_bytes(
                b.get(o..o.checked_add(2).ok_or(Error::InvalidImage)?)
                    .ok_or(Error::InvalidImage)?
                    .try_into()
                    .unwrap(),
            ))
        }
        fn u32at(b: &[u8], o: usize) -> Result<u32, Error> {
            Ok(u32::from_le_bytes(
                b.get(o..o.checked_add(4).ok_or(Error::InvalidImage)?)
                    .ok_or(Error::InvalidImage)?
                    .try_into()
                    .unwrap(),
            ))
        }
        fn u64at(b: &[u8], o: usize) -> Result<u64, Error> {
            Ok(u64::from_le_bytes(
                b.get(o..o.checked_add(8).ok_or(Error::InvalidImage)?)
                    .ok_or(Error::InvalidImage)?
                    .try_into()
                    .unwrap(),
            ))
        }
        if image.get(..7) != Some(&b"\x7fELF\x02\x01\x01"[..])
            || u16at(image, 16)? != 2
            || u16at(image, 18)? != 183
            || u32at(image, 20)? != 1
            || u16at(image, 52)? != 64
            || u16at(image, 54)? != 56
        {
            return Err(Error::InvalidImage);
        }
        if max_pages == 0 || max_pages > 512 * 512 {
            return Err(Error::MemoryLimit);
        }
        let entry = u64at(image, 24)?;
        let ph = usize::try_from(u64at(image, 32)?).map_err(|_| Error::InvalidImage)?;
        let number = u16at(image, 56)? as usize;
        if number == 0 || number > 128 {
            return Err(Error::InvalidImage);
        }
        let ph_end = ph
            .checked_add(number.checked_mul(56).ok_or(Error::InvalidImage)?)
            .ok_or(Error::InvalidImage)?;
        image.get(ph..ph_end).ok_or(Error::InvalidImage)?;
        let mut segments = Vec::new();
        segments
            .try_reserve_exact(number)
            .map_err(|_| Error::MemoryLimit)?;
        let mut end = USER_BASE;
        for i in 0..number {
            let o = ph.checked_add(i * 56).ok_or(Error::InvalidImage)?;
            let kind = u32at(image, o)?;
            // PT_DYNAMIC, PT_INTERP and PT_TLS require runtime support. Ignoring
            // PT_TLS would accept binaries whose thread-local variables have no
            // initialized storage or thread pointer, even with static linking.
            if kind == 2 || kind == 3 || kind == 7 {
                return Err(Error::InvalidImage);
            }
            if kind != 1 {
                continue;
            }
            let flags = u32at(image, o + 4)?;
            let file = usize::try_from(u64at(image, o + 8)?).map_err(|_| Error::InvalidImage)?;
            let va = u64at(image, o + 16)?;
            let filesz = usize::try_from(u64at(image, o + 32)?).map_err(|_| Error::InvalidImage)?;
            let memsz = u64at(image, o + 40)?;
            let alignment = u64at(image, o + 48)?;
            if flags & !7 != 0
                || flags & 4 == 0
                || (alignment > 1
                    && (!alignment.is_power_of_two()
                        || va % alignment != (file as u64) % alignment))
            {
                return Err(Error::InvalidImage);
            }
            let limit = va.checked_add(memsz).ok_or(Error::InvalidImage)?;
            if va < USER_BASE
                || limit > USER_BASE + (max_pages as u64) * PAGE_SIZE as u64
                || filesz as u64 > memsz
                || flags & 3 == 3
                || image
                    .get(file..file.checked_add(filesz).ok_or(Error::InvalidImage)?)
                    .is_none()
            {
                return Err(Error::InvalidImage);
            }
            if memsz == 0 {
                continue;
            }
            let first = ((va - USER_BASE) / PAGE_SIZE as u64) as usize;
            let last = ((limit - USER_BASE).div_ceil(PAGE_SIZE as u64)) as usize;
            if segments
                .iter()
                .any(|&(a, b, _, _, _, _)| first < b && last > a)
            {
                return Err(Error::InvalidImage);
            }
            segments.push((first, last, va, file, filesz, flags));
            end = end.max(limit);
        }
        let code_pages = ((end - USER_BASE).div_ceil(PAGE_SIZE as u64)) as usize;
        if code_pages == 0
            || code_pages
                .checked_add(writable_pages)
                .ok_or(Error::MemoryLimit)?
                > max_pages
        {
            return Err(Error::MemoryLimit);
        }
        // Allocation and table construction are shared with the flat probe loader.
        let mut space = Self::prepare(&[0], code_pages + writable_pages - 1, max_pages)?;
        space.heap_start = USER_BASE + (code_pages * PAGE_SIZE) as u64;
        for i in 0..code_pages {
            space.mapped[i] = false;
            space.writable[i] = false;
            space._leaf[i / 512].0[i % 512] = 0;
        }
        let mut valid_entry = false;
        for (first, last, va, file, filesz, flags) in segments {
            for i in first..last {
                space.mapped[i] = true;
                space.writable[i] = flags & 2 != 0;
                let physical = &space.pages[i] as *const Page as u64;
                let permissions = if flags & 2 != 0 {
                    (1 << 6) | (3 << 53)
                } else if flags & 1 != 0 {
                    (3 << 6) | (1 << 53)
                } else {
                    (3 << 6) | (3 << 53)
                };
                space._leaf[i / 512].0[i % 512] =
                    (physical & ADDRESS_MASK) | 3 | (1 << 2) | (3 << 8) | (1 << 10) | permissions;
            }
            let offset = (va - USER_BASE) as usize;
            for (n, byte) in image[file..file + filesz].iter().enumerate() {
                space.pages[(offset + n) / PAGE_SIZE].0[(offset + n) % PAGE_SIZE] = *byte;
            }
            valid_entry |= flags & 1 != 0 && entry >= va && entry < va + filesz as u64;
        }
        if !valid_entry {
            return Err(Error::InvalidImage);
        }
        #[cfg(all(target_arch = "aarch64", target_os = "none"))]
        unsafe {
            let ctr: u64;
            core::arch::asm!("mrs {ctr}, ctr_el0",ctr=out(reg) ctr);
            let line = 4usize << ((ctr >> 16) & 15);
            for page in space.pages.iter().take(code_pages) {
                let base = page.0.as_ptr() as usize;
                for address in (base..base + PAGE_SIZE).step_by(line) {
                    core::arch::asm!("dc cvau, {address}",address=in(reg) address);
                }
            }
            core::arch::asm!("dsb ish", "ic iallu", "dsb ish", "isb");
        }
        Ok((space, entry))
    }
    pub fn heap_start(&self) -> u64 {
        self.heap_start
    }
    pub fn heap_length(&self) -> u64 {
        self.stack_top()
            .saturating_sub(self.heap_start + (64 * PAGE_SIZE) as u64)
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
        if end > (self.pages.len() * PAGE_SIZE) as u64 {
            return Err(Error::InvalidRange);
        }
        if length != 0 {
            for index in offset as usize / PAGE_SIZE..=((end - 1) as usize / PAGE_SIZE) {
                if !self.mapped[index] || (write && !self.writable[index]) {
                    return Err(Error::InvalidRange);
                }
            }
        }
        Ok(())
    }

    /// Copy through kernel-owned backing, never dereference a caller's pointer.
    pub fn copy_out(&mut self, address: u64, bytes: &[u8]) -> Result<(), Error> {
        self.validate_range(address, bytes.len(), true)?;
        let mut offset = (address - USER_BASE) as usize;
        for byte in bytes {
            self.pages[offset / PAGE_SIZE].0[offset % PAGE_SIZE] = *byte;
            offset += 1;
        }
        Ok(())
    }

    pub fn copy_in(&self, address: u64, bytes: &mut [u8]) -> Result<(), Error> {
        self.validate_range(address, bytes.len(), false)?;
        let mut offset = (address - USER_BASE) as usize;
        for byte in bytes {
            *byte = self.pages[offset / PAGE_SIZE].0[offset % PAGE_SIZE];
            offset += 1;
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
    fn elf() -> Vec<u8> {
        let mut image = alloc::vec![0u8; 0x2004];
        image[..7].copy_from_slice(b"\x7fELF\x02\x01\x01");
        image[16..18].copy_from_slice(&2u16.to_le_bytes());
        image[18..20].copy_from_slice(&183u16.to_le_bytes());
        image[20..24].copy_from_slice(&1u32.to_le_bytes());
        image[24..32].copy_from_slice(&USER_BASE.to_le_bytes());
        image[32..40].copy_from_slice(&64u64.to_le_bytes());
        image[52..54].copy_from_slice(&64u16.to_le_bytes());
        image[54..56].copy_from_slice(&56u16.to_le_bytes());
        image[56..58].copy_from_slice(&2u16.to_le_bytes());
        for (i, flags, va, file, memsz) in [
            (0, 5u32, USER_BASE, 0x1000u64, 4u64),
            (1, 6, USER_BASE + 8192, 0x2000, 8192),
        ] {
            let o = 64 + i * 56;
            image[o..o + 4].copy_from_slice(&1u32.to_le_bytes());
            image[o + 4..o + 8].copy_from_slice(&flags.to_le_bytes());
            image[o + 8..o + 16].copy_from_slice(&file.to_le_bytes());
            image[o + 16..o + 24].copy_from_slice(&va.to_le_bytes());
            image[o + 32..o + 40].copy_from_slice(&4u64.to_le_bytes());
            image[o + 40..o + 48].copy_from_slice(&memsz.to_le_bytes());
        }
        image[0x1000..0x1004].copy_from_slice(&[0xc0, 3, 0x5f, 0xd6]);
        image[0x2000..0x2004].copy_from_slice(b"data");
        image
    }
    #[test]
    fn elf_rejects_tls_even_when_template_is_in_a_valid_load_segment() {
        let mut image = elf();
        // A TLS template normally overlaps a writable PT_LOAD. Keep both valid
        // load segments intact, then append a well-formed PT_TLS descriptor.
        image[56..58].copy_from_slice(&3u16.to_le_bytes());
        let header = 64 + 2 * 56;
        image[header..header + 4].copy_from_slice(&7u32.to_le_bytes());
        image[header + 4..header + 8].copy_from_slice(&4u32.to_le_bytes());
        image[header + 8..header + 16].copy_from_slice(&0x2000u64.to_le_bytes());
        image[header + 16..header + 24]
            .copy_from_slice(&(USER_BASE + 8192).to_le_bytes());
        image[header + 32..header + 40].copy_from_slice(&4u64.to_le_bytes());
        image[header + 40..header + 48].copy_from_slice(&8u64.to_le_bytes());
        image[header + 48..header + 56].copy_from_slice(&8u64.to_le_bytes());
        assert!(matches!(
            AddressSpace::load_elf(&image, 65, 100),
            Err(Error::InvalidImage)
        ));
        // An inert metadata header at the same location is accepted: rejection
        // above must be due to TLS, not malformed ELF or missing executable code.
        image[header..header + 4].copy_from_slice(&0u32.to_le_bytes());
        assert!(AddressSpace::load_elf(&image, 65, 100).is_ok());
    }
    #[test]
    fn elf_rejects_overflowing_headers_sizes_addresses_and_bad_alignment() {
        for (offset, value) in [
            (32, u64::MAX - 3),
            (72, u64::MAX),
            (96, u64::MAX),
            (104, u64::MAX),
            (80, u64::MAX),
            (112, 3),
        ] {
            let mut image = elf();
            image[offset..offset + 8].copy_from_slice(&value.to_le_bytes());
            assert!(
                AddressSpace::load_elf(&image, 65, 100).is_err(),
                "offset {offset}"
            );
        }
        assert!(matches!(
            AddressSpace::load_elf(&elf(), 65, usize::MAX),
            Err(Error::MemoryLimit)
        ));
        assert!(matches!(
            AddressSpace::load_elf(&elf(), 65, 1),
            Err(Error::InvalidImage) | Err(Error::MemoryLimit)
        ));
    }
    #[test]
    fn private_page_backing_chunks_are_aligned_and_map_across_boundaries() {
        let mut memory = AddressSpace::prepare(&[1], 513, 514).unwrap();
        assert_eq!(memory.pages.chunks.len(), 3);
        for chunk in &memory.pages.chunks {
            let base = chunk.as_ptr() as usize;
            assert_eq!(base % PAGE_SIZE, 0);
            for (index, page) in chunk.iter().enumerate() {
                assert_eq!(page as *const Page as usize, base + index * PAGE_SIZE);
            }
        }
        for index in [255usize, 511] {
            let address = USER_BASE + ((index + 1) * PAGE_SIZE - 2) as u64;
            memory.copy_out(address, b"cross").unwrap();
            let mut bytes = [0; 5];
            memory.copy_in(address, &mut bytes).unwrap();
            assert_eq!(&bytes, b"cross");
            assert_eq!(
                memory._leaf[(index + 1) / 512].0[(index + 1) % 512] & ADDRESS_MASK,
                &memory.pages[index + 1] as *const Page as u64 & ADDRESS_MASK
            );
        }
    }
    #[test]
    fn elf_loads_segments_bss_and_rejects_bad_entry_wx_and_overlap() {
        let image = elf();
        let (mut memory, entry) = AddressSpace::load_elf(&image, 65, 100).unwrap();
        assert_eq!(entry, USER_BASE);
        let mut data = [0u8; 8];
        memory.copy_in(USER_BASE + 8192, &mut data).unwrap();
        assert_eq!(&data, b"data\0\0\0\0");
        memory.copy_out(USER_BASE + 8192, b"ok").unwrap();
        assert_eq!(
            memory.validate_range(USER_BASE + 4096, 1, false),
            Err(Error::InvalidRange)
        );
        assert_eq!(memory.copy_out(USER_BASE, b"x"), Err(Error::InvalidRange));
        let mut malformed = image.clone();
        malformed[24..32].copy_from_slice(&(USER_BASE + 8192).to_le_bytes());
        assert!(matches!(
            AddressSpace::load_elf(&malformed, 65, 100),
            Err(Error::InvalidImage)
        ));
        malformed = image.clone();
        malformed[68..72].copy_from_slice(&7u32.to_le_bytes());
        assert!(matches!(
            AddressSpace::load_elf(&malformed, 65, 100),
            Err(Error::InvalidImage)
        ));
        malformed = image;
        malformed[136..144].copy_from_slice(&USER_BASE.to_le_bytes());
        assert!(matches!(
            AddressSpace::load_elf(&malformed, 65, 100),
            Err(Error::InvalidImage)
        ));
    }
    #[test]
    fn address_space_maps_more_than_one_leaf_table() {
        let memory = AddressSpace::prepare(&[1], 513, 514).unwrap();
        assert_eq!(memory._leaf.len(), 2);
        assert_ne!(memory._middle.0[1], 0);
        assert!(memory
            .validate_range(USER_BASE + (513 * PAGE_SIZE) as u64, PAGE_SIZE, true)
            .is_ok());
    }
    #[test]
    fn copies_cross_pages_and_reject_code_and_overflow() {
        let mut memory = space();
        let address = USER_BASE + (2 * PAGE_SIZE - 2) as u64;
        memory.copy_out(address, b"abcd").unwrap();
        let mut result = [0; 4];
        memory.copy_in(address, &mut result).unwrap();
        assert_eq!(&result, b"abcd");
        assert_eq!(memory.copy_out(USER_BASE, b"x"), Err(Error::InvalidRange));
        assert_eq!(
            memory.copy_in(u64::MAX, &mut result),
            Err(Error::InvalidRange)
        );
        assert_eq!(
            memory.copy_out(memory.stack_top() - 1, b"ab"),
            Err(Error::InvalidRange)
        );
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
            first._leaf[0].0[0] & ADDRESS_MASK,
            second._leaf[0].0[0] & ADDRESS_MASK
        );
        assert_eq!(first.root.entries[1] & (3 << 6), 0);
        assert_eq!(first._leaf[0].0[0] & (3 << 6), 3 << 6);
        assert_eq!(first._leaf[0].0[1] & (3 << 53), 3 << 53);
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
        for first_page in first.address_space.as_ref().unwrap().pages.iter() {
            for second_page in second.address_space.as_ref().unwrap().pages.iter() {
                assert_ne!(first_page as *const Page, second_page as *const Page);
            }
        }
        second.enqueue_from(82, 9, b"alive", 0).unwrap();
        first.terminate(ExitReason::Fault);
        assert!(first.address_space.is_none());
        assert!(second.address_space.is_some());
        assert_eq!(second.receive().unwrap().payload, b"alive");
        assert!(second.address_space.as_ref().unwrap()._leaf[0].0[3..]
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
