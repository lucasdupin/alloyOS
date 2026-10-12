#![no_std]
#![cfg_attr(target_os = "none", no_main)]
#![cfg_attr(target_os = "none", feature(alloc_error_handler))]

#[cfg(not(target_os = "none"))]
extern crate std;

extern crate alloc;

#[cfg(target_os = "none")]
use metalos_kernel::{arch, drivers, fs, mm, serial_println, task};
#[cfg(target_os = "none")]
use metalos_proto::{BootInfo, MemoryRegion, MemoryRegionKind};

#[cfg(all(target_arch = "aarch64", target_os = "none"))]
core::arch::global_asm!(
    r#"
.section .text.boot, "ax"
.global _start
_start:
    // Only allow CPU 0 to boot; park secondary cores
    mrs x0, mpidr_el1
    and x0, x0, #3
    cbz x0, .Lprimary_cpu
.Lpark:
    wfe
    b .Lpark

.Lprimary_cpu:
    // Check current Exception Level (bits [3:2] of CurrentEL)
    mrs x0, CurrentEL
    lsr x0, x0, #2
    cmp x0, #2
    b.ne .Lel1_entry

    // Booted in EL2: configure EL1 for AArch64 and drop to EL1h
    mov x0, #(1 << 31)          // HCR_EL2.RW = 1 (EL1 is AArch64)
    msr hcr_el2, x0

    mrs x0, cnthctl_el2
    orr x0, x0, #3              // Enable EL1 physical timer & counter access
    msr cnthctl_el2, x0
    msr cntvoff_el2, xzr

    mov x0, #0x3c5              // SPSR_EL2: EL1h (0b0101) + DAIF masked (0x3c0)
    msr spsr_el2, x0
    adr x0, .Lel1_entry
    msr elr_el2, x0
    eret

.Lel1_entry:
    // Enable FP/SIMD (NEON) in CPACR_EL1 (FPEN = bits [21:20] = 0b11)
    mov x0, #(3 << 20)
    msr cpacr_el1, x0
    isb

    // Initialize kernel stack pointer
    ldr x0, =__stack_top
    mov sp, x0

    // Zero .bss section
    ldr x0, =__bss_start
    ldr x1, =__bss_end
.Lbss_loop:
    cmp x0, x1
    b.hs .Lbss_done
    str xzr, [x0], #8
    b .Lbss_loop

.Lbss_done:
    bl aarch64_kernel_entry
    b .Lpark
"#
);

/// Static memory map for QEMU `-M virt` (256 MiB RAM at `0x4000_0000..0x5000_0000`).
/// Usable frame allocation starts at `0x4040_0000` (4 MiB into RAM, above kernel & boot stack).
#[cfg(target_os = "none")]
static VIRT_MEMORY_REGIONS: [MemoryRegion; 2] = [
    MemoryRegion::new(
        0x4000_0000,
        0x0040_0000,
        MemoryRegionKind::KernelAndModules,
    ),
    MemoryRegion::new(0x4040_0000, 0x0FC0_0000, MemoryRegionKind::Usable),
];

#[cfg(target_os = "none")]
#[no_mangle]
pub extern "C" fn aarch64_kernel_entry() -> ! {
    // 1. Init ARMv8 Exception Vector Table (VBAR_EL1) and MMU identity map first
    // so exclusive atomics (ldaxr/stlxr in spin::Mutex) operate on Normal Cacheable RAM on Apple M4 HVF
    arch::init();

    // 2. Initialize PL011 UART (for early debug output)
    drivers::serial::init();

    // 3. Print boot banner
    serial_println!("metalOS v0.1.0 booting...");

    // 4. Init memory (bitmap frame allocator + 4 MiB kernel heap)
    let boot_info = BootInfo {
        memory_regions: &VIRT_MEMORY_REGIONS,
        physical_memory_offset: 0,
        kernel_image_offset: 0x4008_0000,
        rsdp_addr: None,
    };
    mm::init(&boot_info);
    serial_println!("[mm] Frame allocator and 4 MiB kernel heap online.");

    // 5. Init GIC/timer & Hardware RNG
    drivers::pic::init();
    let _ = drivers::rng::next_u64();

    // 6. Init VirtIO-GPU framebuffer & Liquid Metal Drip Boot Splash (Phase 3)
    let _ = drivers::virtio_gpu::init();
    drivers::virtio_input::init();
    {
        let mut splash = metalos_boot::splash::LiquidSplashAnimation::new(240, 432);
        for dt in [160u32, 180, 160] {
            splash.step_frame(dt);
            drivers::virtio_gpu::present_splash_frame(&splash);
        }
        serial_println!(
            "[splash] Liquid Metal Drip animation active (coalesce={}%).",
            splash.coalesce_progress
        );
    }

    // 7. Init VFS (probes VirtIO-Blk for CobaltFS volume, falls back to RamFS)
    let root_backend = fs::init();
    serial_println!("[fs] POSIX VFS (/ [{}], /dev, /proc) online.", root_backend);

    // 8. Init VirtIO-Net & TCP/IP stack (Phase 2)
    let _ = drivers::virtio_net::init();
    metalos_kernel::net::with_net_stack(|_| {});
    serial_println!("[net] VirtIO-Net + TCP/IP + DNS + TLS 1.3 stack online.");

    // 9. Init Mercury Display Server (`mercuryd`) in Single-Window AI Boot Mode (Phase 4)
    let (mw, mh) = drivers::virtio_gpu::init_mercury_console();
    serial_println!(
        "[mercuryd] Mercury display server online ({}x{}, agent-exclusive UI mode).",
        mw,
        mh
    );

    // 10. Init Embedded JS Engine (V8 / JS Isolate) & Event/Time Scheduler (`scriptd`) (Phase 6)
    let mut js_host = metalos_js::VirtualHostEnv::new();
    let mut js_scheduler = metalos_js::ScriptScheduler::new();
    let _ = js_scheduler.dispatch_event(&metalos_js::OsEvent::SystemBoot, &mut js_host);
    serial_println!("[scriptd] Embedded V8/JS runtime & event/time scheduler online.");

    // 11. Init Headless MCP Developer App Registry (Phase 7)
    let mut mcp_registry = metalos_mcp::McpRegistry::new();
    let _ = metalos_mcp::register_builtin_apps(&mut mcp_registry);
    serial_println!(
        "[mcp] Headless MCP app platform online ({} apps registered; zero app-owned UI).",
        mcp_registry.app_count()
    );

    // 12. Init Pluggable AI Runtime (`libmetal-ai`) — Shell-Free Agent Operator (Phase 5)
    let ai_runtime = metalos_ai::AiRuntime::new();
    serial_println!(
        "[ai] Pluggable AI runtime online (active={}, shell-access=disabled).",
        ai_runtime.active_provider_name()
    );

    // 13. Init scheduler, spawn init task
    task::init();

    // 14. Launch console / debug bring-up loop
    serial_println!("metalOS initialized successfully. Launching metal-sh...");
    libmetal::shell::run_shell();

    loop {
        arch::wfi();
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
        arch::wfi();
    }
}
