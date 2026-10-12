#![no_std]
#![cfg_attr(target_os = "none", no_main)]
#![feature(abi_x86_interrupt, alloc_error_handler, naked_functions)]

#[cfg(not(target_os = "none"))]
extern crate std;

extern crate alloc;

#[cfg(target_os = "none")]
use bootloader::BootloaderConfig;
#[cfg(target_os = "none")]
use bootloader::{entry_point, BootInfo};
use metalos_kernel::{arch, drivers, fs, mm, serial_println, task};

#[cfg(target_os = "none")]
pub static BOOTLOADER_CONFIG: BootloaderConfig = {
    let mut config = BootloaderConfig::new_default();
    config.mappings.physical_memory = Some(bootloader::config::Mapping::Dynamic);
    config
};

#[cfg(target_os = "none")]
entry_point!(kernel_main, config = &BOOTLOADER_CONFIG);

#[cfg(target_os = "none")]
fn kernel_main(boot_info: &'static mut BootInfo) -> ! {
    // 1. Initialize serial (for early debug output)
    drivers::serial::init();

    // 2. Print banner: "metalOS v0.1.0 booting..."
    serial_println!("metalOS v0.1.0 booting...");

    // 3. Init GDT
    arch::gdt::init();

    // 4. Init IDT
    arch::idt::init();

    // 5. Init memory (buddy allocator + heap)
    mm::init(boot_info);

    // 6. Init PIC/timer
    drivers::pic::init();

    // 7. Init VFS
    fs::init();

    // 8. Init scheduler, spawn init task
    task::init();

    // 9. Enable interrupts, enter scheduler loop
    x86_64::instructions::interrupts::enable();

    serial_println!("metalOS initialized successfully. Entering scheduler loop.");

    loop {
        x86_64::instructions::hlt();
    }
}

#[cfg(not(target_os = "none"))]
fn main() {}

#[cfg(target_os = "none")]
#[alloc_error_handler]
fn alloc_error(layout: core::alloc::Layout) -> ! {
    panic!("allocation error: {:?}", layout);
}

#[cfg(target_os = "none")]
#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    serial_println!("{}", info);
    loop {
        x86_64::instructions::hlt();
    }
}
