use pic8259::ChainedPics;
use spin::Mutex;

pub const PIC_1_OFFSET: u8 = 32;
pub const PIC_2_OFFSET: u8 = PIC_1_OFFSET + 8;

pub static PICS: Mutex<ChainedPics> =
    Mutex::new(unsafe { ChainedPics::new(PIC_1_OFFSET, PIC_2_OFFSET) });

pub fn init() {
    #[cfg(not(test))]
    unsafe {
        PICS.lock().initialize();
    }
}

pub fn notify_end_of_interrupt(_interrupt_id: u8) {
    #[cfg(not(test))]
    unsafe {
        PICS.lock().notify_end_of_interrupt(_interrupt_id);
    }
}
