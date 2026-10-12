#![no_std]
#![cfg_attr(target_os = "none", no_main)]
#![cfg_attr(target_os = "none", feature(alloc_error_handler))]

#[cfg(not(target_os = "none"))]
extern crate std;

extern crate alloc;

#[cfg(target_os = "none")]
use bootloader::BootloaderConfig;
#[cfg(target_os = "none")]
use bootloader::{entry_point, BootInfo};
#[cfg(target_os = "none")]
use metalos_kernel::{arch, drivers, fs, mm, serial_println, task};

#[cfg(target_os = "none")]
pub static BOOTLOADER_CONFIG: BootloaderConfig = {
    let mut config = BootloaderConfig::new_default();
    config.mappings.physical_memory = Some(bootloader::config::Mapping::Dynamic);
    config.kernel_stack_size = 512 * 1024;
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

    // 4. Init IDT and x86_64 syscall MSRs
    arch::idt::init();

    // 5. Init memory (bitmap frame allocator + heap)
    mm::init(boot_info);
    serial_println!("[mm] Frame allocator and 4 MiB kernel heap online.");

    // 6. Init PIC/timer
    drivers::pic::init();

    // 7. Init VFS (probes ATA for CobaltFS volume, falls back to RamFS)
    let root_backend = fs::init();
    serial_println!("[fs] POSIX VFS (/ [{}], /dev, /proc) online.", root_backend);

    // 8. Init scheduler, spawn init task
    task::init();

    // 9. Enable interrupts, launch interactive POSIX shell
    x86_64::instructions::interrupts::enable();

    serial_println!("metalOS initialized successfully. Launching metal-sh...");
    libmetal::shell::run_shell();

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
