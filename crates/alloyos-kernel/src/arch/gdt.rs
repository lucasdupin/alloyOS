//! ARMv8-A Exception Level (`EL1` / `EL0`) state & exception stack configuration.

use spin::Lazy;

pub const DOUBLE_FAULT_IST_INDEX: u16 = 0;

/// Logical privilege/exception level selector representation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SegmentSelector(pub u16);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Selectors {
    pub kernel_code: SegmentSelector,
    pub kernel_data: SegmentSelector,
    pub user_code: SegmentSelector,
    pub user_data: SegmentSelector,
    pub tss: SegmentSelector,
}

#[repr(align(16))]
struct AlignedStack([u8; 4096 * 5]);

static EXCEPTION_STACK: AlignedStack = AlignedStack([0; 4096 * 5]);

#[derive(Debug, Clone)]
pub struct TaskStateSegment {
    pub interrupt_stack_table: [u64; 7],
}

impl TaskStateSegment {
    pub const fn new() -> Self {
        Self {
            interrupt_stack_table: [0; 7],
        }
    }
}

pub fn create_tss() -> TaskStateSegment {
    let mut tss = TaskStateSegment::new();
    let stack_start = &raw const EXCEPTION_STACK.0 as *const u8 as usize as u64;
    let stack_end = stack_start + (4096 * 5) as u64;
    tss.interrupt_stack_table[DOUBLE_FAULT_IST_INDEX as usize] = stack_end;
    tss
}

pub static TSS: Lazy<TaskStateSegment> = Lazy::new(create_tss);

pub struct GlobalDescriptorTable {
    count: u16,
}

impl GlobalDescriptorTable {
    pub const fn new() -> Self {
        Self { count: 1 }
    }

    pub fn append(&mut self) -> SegmentSelector {
        let sel = SegmentSelector(self.count * 8);
        self.count += 1;
        sel
    }
}

pub fn create_gdt(_tss: &'static TaskStateSegment) -> (GlobalDescriptorTable, Selectors) {
    let mut gdt = GlobalDescriptorTable::new();
    let kernel_code = gdt.append();
    let kernel_data = gdt.append();
    let user_data = gdt.append();
    let user_code = gdt.append();
    let tss_sel = gdt.append();

    (
        gdt,
        Selectors {
            kernel_code,
            kernel_data,
            user_code,
            user_data,
            tss: tss_sel,
        },
    )
}

pub static GDT: Lazy<(GlobalDescriptorTable, Selectors)> = Lazy::new(|| create_gdt(&TSS));

pub fn init() {
    let _ = &*GDT;
}
