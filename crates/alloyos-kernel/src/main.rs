#![no_std]
#![cfg_attr(target_os = "none", no_main)]
#![cfg_attr(target_os = "none", feature(alloc_error_handler))]

#[cfg(not(target_os = "none"))]
extern crate std;

extern crate alloc;

// The userland dependency also belongs to the serial-shell build variant.
// Forge builds omit that entry point but retain the workspace dependency.
#[cfg(feature = "forge")]
use liballoy as _;

#[cfg(target_os = "none")]
use alloyos_kernel::{arch, drivers, fs, mm, serial_println, task};
#[cfg(target_os = "none")]
use alloyos_proto::{BootInfo, MemoryRegion, MemoryRegionKind};

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
    mov x29, xzr               // terminate the initial frame-pointer chain
    bl aarch64_kernel_entry
    b .Lpark
"#
);

// Initialized once on the boot CPU before the allocator sees a static slice.
#[cfg(target_os = "none")]
static mut VIRT_MEMORY_REGIONS: [MemoryRegion; 2] = [
    MemoryRegion::new(0x4000_0000, 0, MemoryRegionKind::KernelAndModules),
    MemoryRegion::new(0x4000_0000, 0, MemoryRegionKind::Usable),
];

#[cfg(target_os = "none")]
#[no_mangle]
pub extern "C" fn aarch64_kernel_entry() -> ! {
    // 1. Init ARMv8 Exception Vector Table (VBAR_EL1) and MMU identity map first
    // so exclusive atomics (ldaxr/stlxr in spin::Mutex) operate on Normal Cacheable RAM on Apple M4 HVF
    arch::init();

    // 2. Initialize PL011 UART (for early debug output)
    drivers::serial::init();
    #[cfg(feature = "crash-selftest")]
    alloyos_kernel::diagnostics::selftest();

    // 3. Print boot banner
    serial_println!("alloyOS v0.1.0 booting...");

    // 4. Init memory (bitmap frame allocator + bounded kernel heap)
    // Slint and embedded fonts make the image larger than the old fixed 4 MiB
    // reservation. Reserve the entire linked image and stack before allocating.
    unsafe extern "C" { static __kernel_end: u8; }
    let usable_start = (core::ptr::addr_of!(__kernel_end) as u64 + 4095) & !4095;
    #[cfg(not(feature = "forge"))]
    let allocator_end = 0x6000_0000; // 512 MiB standard guest
    #[cfg(feature = "forge")]
    let allocator_end = 0x8000_0000;
    assert!(usable_start < allocator_end, "kernel exceeds QEMU RAM");
    let memory_regions = [
        MemoryRegion::new(0x4000_0000, usable_start - 0x4000_0000, MemoryRegionKind::KernelAndModules),
        MemoryRegion::new(usable_start, allocator_end - usable_start, MemoryRegionKind::Usable),
    ];
    // SAFETY: only CPU 0 reaches boot initialization; this map is never mutated
    // again after publishing the immutable reference required by BootInfo.
    let memory_regions = unsafe {
        core::ptr::addr_of_mut!(VIRT_MEMORY_REGIONS).write(memory_regions);
        &*core::ptr::addr_of!(VIRT_MEMORY_REGIONS)
    };
    let boot_info = BootInfo {
        memory_regions,
        physical_memory_offset: 0,
        kernel_image_offset: 0x4008_0000,
        rsdp_addr: None,
    };
    mm::init(&boot_info);
    serial_println!("[mm] Frame allocator and {} MiB kernel heap online.", mm::heap::HEAP_SIZE / (1024 * 1024));

    // 5. Init GIC/timer & Hardware RNG
    drivers::pic::init();
    let _ = drivers::rng::next_u64();
    let secure_entropy = drivers::virtio_rng::init();
    let mut entropy_probe=[0u8;32];
    let crypto_available=drivers::rng::try_fill_secure_random(&mut entropy_probe).is_ok();
    entropy_probe.fill(0);
    alloyos_kernel::serial_println!("[entropy] secure source {} crypto_api={}", if secure_entropy {"ready"} else {"unavailable; crypto requests fail closed"}, crypto_available);

    // Hardware-owned splash until the protected display executable is loaded.
    let boot_display = drivers::virtio_gpu::init();
    drivers::display_server::boot_splash(boot_display);
    drivers::virtio_input::init();
    // 7. Init VFS (probes VirtIO-Blk for CobaltFS volume, falls back to RamFS)
    let root_backend = fs::init();
    serial_println!("[fs] POSIX VFS (/ [{}], /dev, /proc) online.", root_backend);

    // 8. Init VirtIO-Net & TCP/IP stack (Phase 2)
    let _ = drivers::virtio_net::init();
    alloyos_kernel::net::with_net_stack(|_| {});
    serial_println!("[net] VirtIO-Net + TCP/IP + DNS + TLS 1.3 stack online.");

    let system_ui = if drivers::virtio_gpu::has_hardware_display() && !cfg!(feature="debug-shell") {
        Some(drivers::system_ui::create())
    } else { None };
    #[cfg(any(not(feature = "forge"), feature = "debug-shell"))]
    let mut system_ui = system_ui;
    #[cfg(feature = "debug-shell")]
    { drop(system_ui.take()); serial_println!("[ferrofluidd] System UI disabled (serial debug shell)."); }
    #[cfg(all(not(feature = "debug-shell"), not(feature = "forge")))]
    if let Some(ui) = system_ui.as_mut() { ui.set_booting(false); ui.flush(); }
    // 10. Init Embedded JS Engine (V8 / JS Isolate) & Event/Time Scheduler (`scriptd`) (Phase 6)
    #[cfg(feature = "forge")]
    let forge_service = drivers::forge::ForgeService::from_boot();
    #[cfg(not(feature = "forge"))]
    let forge_service = drivers::forge::ForgeService::new();
    let mut js_host = alloyos_js::VirtualHostEnv::new();
    let shared_forge = forge_service.shared();
    js_host.set_forge_handler(alloc::boxed::Box::new(move |prompt, model, max_tokens| {
        let request = alloyos_forge::GenerationRequest {
            model: model.map(alloc::string::String::from),
            prompt: alloc::string::String::from(prompt),
            max_tokens,
        };
        shared_forge.borrow_mut().generate(&request)
            .map(|output| output.text).map_err(|error| alloc::format!("{}", error))
    }));
    let mut js_scheduler = alloyos_js::ScriptScheduler::new();
    let _ = js_scheduler.dispatch_event(&alloyos_js::OsEvent::SystemBoot, &mut js_host);
    serial_println!("[scriptd] Embedded V8/JS runtime & event/time scheduler online.");

    // 11. Init Headless MCP Developer App Registry (Phase 7)
    let mut mcp_registry = alloyos_mcp::McpRegistry::new();
    if let Some(vfs) = fs::vfs::VFS.lock().as_ref() {
        if let Ok(snapshot) = fs::packages::snapshot(vfs) {
            let _ = mcp_registry.discover_and_register_packages(&snapshot, None);
        }
    }
    serial_println!(
        "[mcp] Headless MCP app platform online ({} apps registered; zero app-owned UI).",
        mcp_registry.app_count()
    );

    // 12. Init Welder Agent Runtime — Shell-Free Agent Operator & Live JS Automation (Phases 5 & 6)
    let ai_runtime = fs::automation::init_kernel_agent_runtime(forge_service.shared());
    serial_println!(
        "[welder] Agent runtime online (active={}, shell-access=disabled).",
        ai_runtime.active_provider_name()
    );

    // 13. Init scheduler, spawn init task
    task::init();
    #[cfg(feature = "process-selftest")]
    task::selftest::run();
    {
        task::scheduler::spawn_kernel(2, drivers::forge_worker::run);
        task::scheduler::start_preemption();
        serial_println!("[scheduler] Timer-preempted UI and Forge tasks online.");
    }

    task::js_process::install();
    #[cfg(feature="process-runtime-selftest")]
    task::js_process::acceptance();
    #[cfg(feature="remote-mcp-selftest")]
    drivers::mcp_https::remote_acceptance();

    #[cfg(feature="oauth-selftest")]
    drivers::oauth_acceptance::run();

    // 14. Ferrofluid owns the Slint System UI and its execution context.
    #[cfg(not(feature = "debug-shell"))]
    if let Some(ui) = system_ui {
        serial_println!("[systemui] Slint window online; Ferrofluid owns platform and input.");
        drivers::system_ui::run(ui, forge_service, ai_runtime);
    }
    #[cfg(feature = "forge")]
    forge_service.run();
    #[cfg(not(feature = "forge"))]
    {
        // Serial bring-up fallback, or explicitly selected debug-shell build.
        serial_println!("alloyOS initialized successfully. Launching alloy-sh...");
        liballoy::shell::run_shell();

        loop {
            arch::wfi();
        }
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
    alloyos_kernel::diagnostics::panic_report(info);
    loop {
        arch::wfi();
    }
}
