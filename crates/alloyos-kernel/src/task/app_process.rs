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
struct PrivateReservation {
    first: usize,
    pages: Vec<Option<Box<Page>>>,
    writable: Vec<bool>,
}

pub struct AddressSpace {
    root: Box<L1PageTable>,
    _middle: Box<Table>,
    _leaf: Vec<Box<Table>>,
    pages: Pages,
    writable: Vec<bool>,
    mapped: Vec<bool>,
    heap_start: u64,
    initial_thread_pointer: u64,
    max_pages: usize,
    reservations: Vec<PrivateReservation>,
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
            initial_thread_pointer: 0,
            max_pages,
            reservations: Vec::new(),
        })
    }
    /// Load static ELF64 AArch64 with one local-exec TLS module (Variant I).
    /// Dynamic images, overlapping load segments and W+X pages are rejected.
    /// TLS gets a private zero TCB plus initialized/zeroed template before the heap.
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
        let mut tls = None;
        for i in 0..number {
            let o = ph.checked_add(i * 56).ok_or(Error::InvalidImage)?;
            let kind = u32at(image, o)?;
            // Dynamic linkage and interpreter startup remain unsupported.
            if kind == 2 || kind == 3 {
                return Err(Error::InvalidImage);
            }
            if kind == 7 {
                if tls.is_some() { return Err(Error::InvalidImage); }
                let flags = u32at(image, o + 4)?;
                let file = usize::try_from(u64at(image, o + 8)?).map_err(|_| Error::InvalidImage)?;
                let va = u64at(image, o + 16)?;
                let filesz = usize::try_from(u64at(image, o + 32)?).map_err(|_| Error::InvalidImage)?;
                let memsz = usize::try_from(u64at(image, o + 40)?).map_err(|_| Error::InvalidImage)?;
                let alignment = usize::try_from(u64at(image, o + 48)?).map_err(|_| Error::InvalidImage)?.max(1);
                if flags & !6 != 0 || flags & 4 == 0 || filesz > memsz || memsz == 0
                    || !alignment.is_power_of_two() || alignment > PAGE_SIZE
                    || va % alignment as u64 != 0 || file % alignment != 0
                    || image.get(file..file.checked_add(filesz).ok_or(Error::InvalidImage)?).is_none()
                    || memsz > max_pages * PAGE_SIZE {
                    return Err(Error::InvalidImage);
                }
                tls = Some((file, va, filesz, memsz, alignment));
                continue;
            }
            if kind != 1 { continue; }
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
        let tls_bytes = if let Some((file, va, filesz, memsz, alignment)) = tls {
            // The initialized template must be part of a non-executable readable
            // load segment with matching file/virtual offsets. .tbss is copied
            // as zero storage, not read from the file or template mapping.
            let covered = segments.iter().any(|&(_, _, load_va, load_file, load_filesz, flags)| {
                va >= load_va && flags & 1 == 0 && flags & 4 != 0
                    && (va - load_va) == file.saturating_sub(load_file) as u64
                    && file >= load_file && file.checked_add(filesz).is_some_and(|n| n <= load_file + load_filesz)
            });
            if !covered { return Err(Error::InvalidImage); }
            let offset = 16usize.div_ceil(alignment) * alignment;
            offset.checked_add(memsz).ok_or(Error::MemoryLimit)?
        } else { 0 };
        let tls_pages = tls_bytes.div_ceil(PAGE_SIZE);
        let image_pages = code_pages.checked_add(tls_pages).ok_or(Error::MemoryLimit)?;
        if code_pages == 0 || image_pages.checked_add(writable_pages).ok_or(Error::MemoryLimit)? > max_pages {
            return Err(Error::MemoryLimit);
        }
        let mut space = Self::prepare(&[0], image_pages + writable_pages - 1, max_pages)?;
        space.heap_start = USER_BASE + (image_pages * PAGE_SIZE) as u64;
        if let Some((file, _, filesz, _, alignment)) = tls {
            space.initial_thread_pointer = USER_BASE + (code_pages * PAGE_SIZE) as u64;
            let offset = code_pages * PAGE_SIZE + 16usize.div_ceil(alignment) * alignment;
            for (n, byte) in image[file..file + filesz].iter().enumerate() {
                space.pages[(offset + n) / PAGE_SIZE].0[(offset + n) % PAGE_SIZE] = *byte;
            }
        }
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
    pub fn initial_thread_pointer(&self) -> u64 {
        self.initial_thread_pointer
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
    /// Reserve anonymous private address space without allocating physical backing.
    /// Image, initial heap/stack, and reservations share the process page quota.
    pub fn reserve_private(&mut self, length: usize) -> Result<u64, Error> {
        let count = Self::page_count(length)?;
        let used = self
            .reservations
            .iter()
            .try_fold(self.pages.len(), |n, r| {
                n.checked_add(r.pages.len()).ok_or(Error::MemoryLimit)
            })?;
        if count > self.max_pages.saturating_sub(used) {
            return Err(Error::MemoryLimit);
        }
        let mut first = self.pages.len();
        for reservation in &self.reservations {
            if reservation.first.saturating_sub(first) >= count {
                break;
            }
            first = reservation.first + reservation.pages.len();
        }
        let end = first.checked_add(count).ok_or(Error::MemoryLimit)?;
        if end > 512 * 512 {
            return Err(Error::MemoryLimit);
        }
        let mut pages = Vec::new();
        pages
            .try_reserve_exact(count)
            .map_err(|_| Error::MemoryLimit)?;
        pages.resize_with(count, || None);
        let writable = allocate_flags(count, false)?;
        self.reservations
            .try_reserve(1)
            .map_err(|_| Error::MemoryLimit)?;
        // Allocate table storage before publishing the reservation. Empty table
        // entries expose no physical memory, including gaps between reservations.
        let needed = end.div_ceil(512);
        self._leaf
            .try_reserve(needed.saturating_sub(self._leaf.len()))
            .map_err(|_| Error::MemoryLimit)?;
        while self._leaf.len() < needed {
            let table = allocate_box(Table([0; 512]))?;
            let index = self._leaf.len();
            self._middle.0[index] = (&*table as *const Table as u64 & ADDRESS_MASK) | 3;
            self._leaf.push(table);
        }
        let index = self.reservations.partition_point(|r| r.first < first);
        self.reservations.insert(
            index,
            PrivateReservation {
                first,
                pages,
                writable,
            },
        );
        Ok(USER_BASE + (first * PAGE_SIZE) as u64)
    }

    fn page_count(length: usize) -> Result<usize, Error> {
        if length == 0 || length % PAGE_SIZE != 0 {
            return Err(Error::InvalidRange);
        }
        Ok(length / PAGE_SIZE)
    }

    fn private_interval(
        &self,
        address: u64,
        length: usize,
    ) -> Result<(usize, usize, usize), Error> {
        let count = Self::page_count(length)?;
        let offset = address.checked_sub(USER_BASE).ok_or(Error::InvalidRange)?;
        if offset % PAGE_SIZE as u64 != 0 {
            return Err(Error::InvalidRange);
        }
        let first = usize::try_from(offset / PAGE_SIZE as u64).map_err(|_| Error::InvalidRange)?;
        let end = first.checked_add(count).ok_or(Error::InvalidRange)?;
        self.reservations
            .iter()
            .enumerate()
            .find_map(|(index, r)| {
                (first >= r.first && end <= r.first + r.pages.len())
                    .then(|| (index, first - r.first, count))
            })
            .ok_or(Error::InvalidRange)
    }

    /// Commit zero-filled pages, or change existing committed pages to RO/RW.
    /// Allocation failure leaves the entire requested interval unchanged.
    pub fn commit_private(
        &mut self,
        address: u64,
        length: usize,
        writable: bool,
    ) -> Result<(), Error> {
        let (index, offset, count) = self.private_interval(address, length)?;
        let reservation = &mut self.reservations[index];
        let mut fresh = Vec::new();
        fresh
            .try_reserve_exact(count)
            .map_err(|_| Error::MemoryLimit)?;
        for i in offset..offset + count {
            if reservation.pages[i].is_none() {
                fresh.push((i, allocate_box(Page([0; PAGE_SIZE]))?));
            }
        }
        for (i, page) in fresh {
            reservation.pages[i] = Some(page);
        }
        for i in offset..offset + count {
            reservation.writable[i] = writable;
            let physical = &**reservation.pages[i].as_ref().unwrap() as *const Page as u64;
            let page_index = reservation.first + i;
            let permissions = (if writable { 1 } else { 3 }) << 6 | (3 << 53);
            self._leaf[page_index / 512].0[page_index % 512] =
                (physical & ADDRESS_MASK) | 3 | (1 << 2) | (3 << 8) | (1 << 10) | permissions;
        }
        Self::flush_private_tables();
        Ok(())
    }

    pub fn decommit_private(&mut self, address: u64, length: usize) -> Result<(), Error> {
        let (index, offset, count) = self.private_interval(address, length)?;
        let reservation = &mut self.reservations[index];
        for i in offset..offset + count {
            let page_index = reservation.first + i;
            self._leaf[page_index / 512].0[page_index % 512] = 0;
        }
        // Invalidate translations before backing can return to the allocator.
        Self::flush_private_tables();
        for i in offset..offset + count {
            reservation.pages[i] = None;
            reservation.writable[i] = false;
        }
        Ok(())
    }

    pub fn release_private(&mut self, address: u64, length: usize) -> Result<(), Error> {
        let (index, offset, count) = self.private_interval(address, length)?;
        if offset != 0 || count != self.reservations[index].pages.len() {
            return Err(Error::InvalidRange);
        }
        self.decommit_private(address, length)?;
        self.reservations.remove(index);
        Ok(())
    }

    fn flush_private_tables() {
        #[cfg(all(target_arch = "aarch64", target_os = "none"))]
        unsafe {
            core::arch::asm!("dsb ishst", "tlbi vmalle1is", "dsb ish", "isb");
        }
    }

    fn page(&self, index: usize, write: bool) -> Result<&Page, Error> {
        if index < self.pages.len() {
            if self.mapped[index] && (!write || self.writable[index]) {
                return Ok(&self.pages[index]);
            }
        } else {
            for r in &self.reservations {
                if index >= r.first && index - r.first < r.pages.len() {
                    let offset = index - r.first;
                    if !write || r.writable[offset] {
                        return r.pages[offset].as_deref().ok_or(Error::InvalidRange);
                    }
                    break;
                }
            }
        }
        Err(Error::InvalidRange)
    }

    pub fn validate_range(&self, address: u64, length: usize, write: bool) -> Result<(), Error> {
        let offset = address.checked_sub(USER_BASE).ok_or(Error::InvalidRange)?;
        let end = offset
            .checked_add(length as u64)
            .ok_or(Error::InvalidRange)?;
        if end > (512 * 512 * PAGE_SIZE) as u64 {
            return Err(Error::InvalidRange);
        }
        if length == 0 {
            // Retain the original empty-range convention at the static end.
            if offset <= (self.pages.len() * PAGE_SIZE) as u64 {
                return Ok(());
            }
            return self.page(offset as usize / PAGE_SIZE, write).map(|_| ());
        }
        for index in offset as usize / PAGE_SIZE..=((end - 1) as usize / PAGE_SIZE) {
            self.page(index, write)?;
        }
        Ok(())
    }

    /// Acquire-load a naturally aligned shared word through owned page backing.
    pub fn load_atomic_u32(&self, address: u64) -> Result<u32, Error> {
        if address & 3 != 0 { return Err(Error::InvalidRange); }
        self.validate_range(address, 4, false)?;
        let offset = (address - USER_BASE) as usize;
        let page = self.page(offset / PAGE_SIZE, false)?;
        let pointer = unsafe { page.0.as_ptr().add(offset % PAGE_SIZE) }
            as *const core::sync::atomic::AtomicU32;
        // Alignment was validated and no page boundary can split a u32.
        Ok(unsafe { &*pointer }.load(core::sync::atomic::Ordering::Acquire))
    }

    /// Validate an instruction range in existing read-only EL0 executable pages.
    /// Private anonymous reservations are never executable.
    pub fn validate_executable(&self, address: u64, length: usize) -> Result<(), Error> {
        if length == 0 || address & 3 != 0 { return Err(Error::InvalidRange); }
        self.validate_range(address, length, false)?;
        let first = ((address - USER_BASE) / PAGE_SIZE as u64) as usize;
        let last = ((address - USER_BASE + length as u64 - 1) / PAGE_SIZE as u64) as usize;
        for index in first..=last {
            if index >= self.pages.len() || self.writable[index]
                || self._leaf[index / 512].0[index % 512] & (1 << 54) != 0 {
                return Err(Error::InvalidRange);
            }
        }
        Ok(())
    }

    /// Copy through kernel-owned backing, never dereference a caller's pointer.
    pub fn copy_out(&mut self, address: u64, bytes: &[u8]) -> Result<(), Error> {
        self.validate_range(address, bytes.len(), true)?;
        let mut offset = (address - USER_BASE) as usize;
        for byte in bytes {
            let index = offset / PAGE_SIZE;
            if index < self.pages.len() {
                self.pages[index].0[offset % PAGE_SIZE] = *byte;
            } else {
                let r = self
                    .reservations
                    .iter_mut()
                    .find(|r| index >= r.first && index - r.first < r.pages.len())
                    .unwrap();
                r.pages[index - r.first].as_mut().unwrap().0[offset % PAGE_SIZE] = *byte;
            }
            offset += 1;
        }
        Ok(())
    }

    pub fn copy_in(&self, address: u64, bytes: &mut [u8]) -> Result<(), Error> {
        self.validate_range(address, bytes.len(), false)?;
        let mut offset = (address - USER_BASE) as usize;
        for byte in bytes {
            *byte = self.page(offset / PAGE_SIZE, false)?.0[offset % PAGE_SIZE];
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
    #[test]
    fn private_reservations_commit_protect_decommit_and_reuse_within_quota() {
        let mut space = AddressSpace::prepare(&[1], 2, 8).unwrap();
        let address = space.reserve_private(3 * PAGE_SIZE).unwrap();
        assert_eq!(address, space.stack_top());
        assert_eq!(
            space.validate_range(address, 1, false),
            Err(Error::InvalidRange)
        );
        space.commit_private(address, 2 * PAGE_SIZE, true).unwrap();
        let mut bytes = [1; 4];
        space
            .copy_in(address + PAGE_SIZE as u64 - 2, &mut bytes)
            .unwrap();
        assert_eq!(bytes, [0; 4]);
        space
            .copy_out(address + PAGE_SIZE as u64 - 2, b"test")
            .unwrap();
        space.commit_private(address, 2 * PAGE_SIZE, false).unwrap();
        assert_eq!(space.copy_out(address, b"x"), Err(Error::InvalidRange));
        space
            .copy_in(address + PAGE_SIZE as u64 - 2, &mut bytes)
            .unwrap();
        assert_eq!(&bytes, b"test");
        let index = ((address - USER_BASE) / PAGE_SIZE as u64) as usize;
        assert_eq!(space._leaf[index / 512].0[index % 512] & (3 << 53), 3 << 53);
        space.decommit_private(address, PAGE_SIZE).unwrap();
        assert_eq!(
            space.validate_range(address, 1, false),
            Err(Error::InvalidRange)
        );
        space.commit_private(address, PAGE_SIZE, true).unwrap();
        space.copy_in(address, &mut bytes).unwrap();
        assert_eq!(bytes, [0; 4]);
        assert_eq!(
            space.reserve_private(3 * PAGE_SIZE),
            Err(Error::MemoryLimit)
        );
        assert_eq!(
            space.release_private(address, PAGE_SIZE),
            Err(Error::InvalidRange)
        );
        space.release_private(address, 3 * PAGE_SIZE).unwrap();
        assert_eq!(
            space.validate_range(address, 1, false),
            Err(Error::InvalidRange)
        );
        assert_eq!(space.reserve_private(3 * PAGE_SIZE).unwrap(), address);
    }

    #[test]
    fn private_operations_reject_static_unaligned_overflow_and_cross_reservation_ranges() {
        let mut space = AddressSpace::prepare(&[1], 2, 1028).unwrap();
        assert_eq!(space.reserve_private(0), Err(Error::InvalidRange));
        assert_eq!(
            space.reserve_private(PAGE_SIZE - 1),
            Err(Error::InvalidRange)
        );
        let a = space.reserve_private(512 * PAGE_SIZE).unwrap();
        let b = space.reserve_private(PAGE_SIZE).unwrap();
        assert_eq!(
            space.commit_private(USER_BASE, PAGE_SIZE, true),
            Err(Error::InvalidRange)
        );
        assert_eq!(
            space.decommit_private(space.heap_start(), PAGE_SIZE),
            Err(Error::InvalidRange)
        );
        assert_eq!(
            space.commit_private(a + 1, PAGE_SIZE, true),
            Err(Error::InvalidRange)
        );
        assert_eq!(
            space.commit_private(u64::MAX - 4095, PAGE_SIZE, true),
            Err(Error::InvalidRange)
        );
        assert_eq!(
            space.commit_private(b - PAGE_SIZE as u64, 2 * PAGE_SIZE, true),
            Err(Error::InvalidRange)
        );
        space
            .commit_private(a + 508 * PAGE_SIZE as u64, 2 * PAGE_SIZE, true)
            .unwrap();
        space
            .copy_out(a + 509 * PAGE_SIZE as u64 - 1, b"ok")
            .unwrap();
        let mut bytes = [0; 2];
        space
            .copy_in(a + 509 * PAGE_SIZE as u64 - 1, &mut bytes)
            .unwrap();
        assert_eq!(&bytes, b"ok");
        assert_ne!(space._middle.0[1], 0);
        space.release_private(a, 512 * PAGE_SIZE).unwrap();
        // Releasing one reservation does not revoke adjacent private backing.
        space.commit_private(b, PAGE_SIZE, true).unwrap();
        space.copy_out(b, b"peer").unwrap();
        assert_eq!(space.reserve_private(PAGE_SIZE).unwrap(), a);
        let mut result = [0; 4];
        space.copy_in(b, &mut result).unwrap();
        assert_eq!(&result, b"peer");
    }

    #[test]
    fn private_dynamic_pages_belong_to_one_process_and_drop_with_its_address_space() {
        let mut first = AddressSpace::prepare(&[1], 2, 4).unwrap();
        let mut second = AddressSpace::prepare(&[1], 2, 4).unwrap();
        let address = first.reserve_private(PAGE_SIZE).unwrap();
        assert_eq!(second.reserve_private(PAGE_SIZE).unwrap(), address);
        first.commit_private(address, PAGE_SIZE, true).unwrap();
        second.commit_private(address, PAGE_SIZE, true).unwrap();
        first.copy_out(address, b"one").unwrap();
        second.copy_out(address, b"two").unwrap();
        let index = ((address - USER_BASE) / PAGE_SIZE as u64) as usize;
        assert_ne!(
            first._leaf[index / 512].0[index % 512] & ADDRESS_MASK,
            second._leaf[index / 512].0[index % 512] & ADDRESS_MASK
        );
        let mut process = AppProcess::prepared(81, first, 100);
        process.terminate(ExitReason::Fault);
        assert!(process.address_space.is_none());
        let mut bytes = [0; 3];
        second.copy_in(address, &mut bytes).unwrap();
        assert_eq!(&bytes, b"two");
    }

    #[test]
    fn atomic_word_reads_require_aligned_owned_backing() {
        let mut space = space();
        let address = USER_BASE + PAGE_SIZE as u64;
        space.copy_out(address, &0x12345678u32.to_le_bytes()).unwrap();
        assert_eq!(space.load_atomic_u32(address).unwrap(), 0x12345678);
        assert!(space.load_atomic_u32(USER_BASE).is_ok());
        for bad in [address + 1, USER_BASE - 4, space.stack_top(), u64::MAX - 3] {
            assert!(space.load_atomic_u32(bad).is_err());
        }
        space.max_pages += 1;
        let private = space.reserve_private(PAGE_SIZE).unwrap();
        assert!(space.load_atomic_u32(private).is_err());
        space.commit_private(private, PAGE_SIZE, true).unwrap();
        space.copy_out(private, &17u32.to_le_bytes()).unwrap();
        assert_eq!(space.load_atomic_u32(private).unwrap(), 17);
        space.commit_private(private, PAGE_SIZE, false).unwrap();
        assert_eq!(space.load_atomic_u32(private).unwrap(), 17);
        space.decommit_private(private, PAGE_SIZE).unwrap();
        assert!(space.load_atomic_u32(private).is_err());
    }

    #[test]
    fn thread_entry_requires_actual_read_only_executable_mapping() {
        let mut space = space();
        assert!(space.validate_executable(USER_BASE, 4).is_ok());
        for (address, length) in [(USER_BASE + 1, 4), (USER_BASE, 0),
            (USER_BASE + PAGE_SIZE as u64, 4), (USER_BASE + PAGE_SIZE as u64 - 4, 8),
            (USER_BASE - 4, 4), (u64::MAX - 3, 8)] {
            assert!(space.validate_executable(address, length).is_err());
        }
        // Anonymous committed read-only data must remain NX.
        space.max_pages += 1;
        let data = space.reserve_private(PAGE_SIZE).unwrap();
        space.commit_private(data, PAGE_SIZE, false).unwrap();
        assert!(space.validate_executable(data, 4).is_err());
        let (elf_space, _) = AddressSpace::load_elf(&elf(), 65, 100).unwrap();
        assert!(elf_space.validate_executable(USER_BASE, 4).is_ok());
        assert!(elf_space.validate_executable(USER_BASE + PAGE_SIZE as u64, 4).is_err());
        assert!(elf_space.validate_executable(USER_BASE + 2 * PAGE_SIZE as u64, 4).is_err());
    }

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
    fn elf_initializes_private_static_tls_and_zero_tcb() {
        let mut image = elf();
        // A TLS template normally overlaps a writable PT_LOAD. Keep both valid
        // load segments intact, then append a well-formed PT_TLS descriptor.
        image[56..58].copy_from_slice(&3u16.to_le_bytes());
        let header = 64 + 2 * 56;
        image[header..header + 4].copy_from_slice(&7u32.to_le_bytes());
        image[header + 4..header + 8].copy_from_slice(&4u32.to_le_bytes());
        image[header + 8..header + 16].copy_from_slice(&0x2000u64.to_le_bytes());
        image[header + 16..header + 24].copy_from_slice(&(USER_BASE + 8192).to_le_bytes());
        image[header + 32..header + 40].copy_from_slice(&4u64.to_le_bytes());
        image[header + 40..header + 48].copy_from_slice(&8u64.to_le_bytes());
        image[header + 48..header + 56].copy_from_slice(&8u64.to_le_bytes());
        let (mut first, _) = AddressSpace::load_elf(&image, 65, 100).unwrap();
        let (second, _) = AddressSpace::load_elf(&image, 65, 100).unwrap();
        let tp = first.initial_thread_pointer();
        assert_eq!(tp % PAGE_SIZE as u64, 0);
        assert_eq!(first.heap_start(), tp + PAGE_SIZE as u64);
        let mut bytes = [0u8; 24];
        first.copy_in(tp, &mut bytes).unwrap();
        assert_eq!(&bytes[..16], &[0u8; 16]);
        assert_eq!(&bytes[16..20], b"data");
        assert_eq!(&bytes[20..], &[0u8; 4]);
        first.copy_out(tp + 16, b"peer").unwrap();
        second.copy_in(tp, &mut bytes).unwrap();
        assert_eq!(&bytes[16..20], b"data");
        for (offset, value) in [(4, 5u64), (32, 9), (40, 0), (48, 3), (48, 8192)] {
            let mut bad = image.clone();
            if offset == 4 { bad[header + offset..header + offset + 4].copy_from_slice(&(value as u32).to_le_bytes()); }
            else { bad[header + offset..header + offset + 8].copy_from_slice(&value.to_le_bytes()); }
            assert!(AddressSpace::load_elf(&bad, 65, 100).is_err());
        }
        assert!(matches!(AddressSpace::load_elf(&image, 65, 69), Err(Error::MemoryLimit)));
        let mut aligned = image.clone();
        aligned[header + 48..header + 56].copy_from_slice(&64u64.to_le_bytes());
        let (aligned_space, _) = AddressSpace::load_elf(&aligned, 65, 100).unwrap();
        let mut template = [0u8; 8];
        aligned_space.copy_in(aligned_space.initial_thread_pointer() + 64, &mut template).unwrap();
        assert_eq!(&template[..4], b"data");
        assert_eq!(&template[4..], &[0; 4]);
        let mut duplicate = image.clone();
        duplicate[56..58].copy_from_slice(&4u16.to_le_bytes());
        duplicate.copy_within(header..header + 56, header + 56);
        assert!(AddressSpace::load_elf(&duplicate, 65, 100).is_err());
        let mut unbacked = image.clone();
        unbacked[header + 16..header + 24].copy_from_slice(&USER_BASE.to_le_bytes());
        assert!(AddressSpace::load_elf(&unbacked, 65, 100).is_err());
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
