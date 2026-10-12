use spin::Lazy;
use x86_64::structures::gdt::{Descriptor, GlobalDescriptorTable, SegmentSelector};
use x86_64::structures::tss::TaskStateSegment;
use x86_64::VirtAddr;

pub const DOUBLE_FAULT_IST_INDEX: u16 = 0;

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

static mut DOUBLE_FAULT_STACK: AlignedStack = AlignedStack([0; 4096 * 5]);

pub fn create_tss() -> TaskStateSegment {
    let mut tss = TaskStateSegment::new();
    let stack_start = VirtAddr::from_ptr(unsafe { &raw const DOUBLE_FAULT_STACK.0 as *const u8 });
    let stack_end = stack_start + 4096 * 5;
    tss.interrupt_stack_table[DOUBLE_FAULT_IST_INDEX as usize] = stack_end;
    tss
}

pub static TSS: Lazy<TaskStateSegment> = Lazy::new(create_tss);

pub fn create_gdt(tss: &'static TaskStateSegment) -> (GlobalDescriptorTable, Selectors) {
    let mut gdt = GlobalDescriptorTable::new();
    let kernel_code = gdt.append(Descriptor::kernel_code_segment());
    let kernel_data = gdt.append(Descriptor::kernel_data_segment());
    let user_data = gdt.append(Descriptor::user_data_segment());
    let user_code = gdt.append(Descriptor::user_code_segment());
    let tss_sel = gdt.append(Descriptor::tss_segment(tss));

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
    #[cfg(not(test))]
    {
        use x86_64::instructions::segmentation::{Segment, CS, DS, ES, SS};
        use x86_64::instructions::tables::load_tss;

        GDT.0.load();
        unsafe {
            CS::set_reg(GDT.1.kernel_code);
            DS::set_reg(GDT.1.kernel_data);
            ES::set_reg(GDT.1.kernel_data);
            SS::set_reg(GDT.1.kernel_data);
            load_tss(GDT.1.tss);
        }
    }
}
