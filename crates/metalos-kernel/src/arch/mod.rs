pub mod gdt;
pub mod idt;

#[cfg(any(test, feature = "host-tests"))]
pub mod tests;

pub fn init() {
    gdt::init();
    idt::init();
}
