#![no_std]
#![feature(alloc_error_handler)]
#![no_main]
extern crate alloc;
use alloc::vec;
use alloyos_ferrofluid::FerrofluidSystemUi;
use alloyos_mcp::display::{self, Event, State};
use alloyos_proto::{InputEvent, InputEventKind};
use liballoy::{runtime::ProcessAllocator, syscall::syscall3};
#[global_allocator]
static HEAP: ProcessAllocator = ProcessAllocator::new();
fn call(n: u64, a: u64, b: u64, c: u64) -> i64 {
    unsafe { syscall3(n, a, b, c) }
}
fn send(event: Event) {
    if let Ok(b) = serde_json::to_vec(&event) {
        loop {
            let status = call(display::DISPLAY_EVENT, b.as_ptr() as u64, b.len() as u64, 0);
            if status >= 0 {
                break;
            }
            if status == -11 {
                call(alloyos_proto::SyscallNo::Yield.as_u64(), 0, 0, 0);
                continue;
            }
            exit(71)
        }
    }
}
fn time() -> u64 {
    let mut t = [0i64; 2];
    call(
        alloyos_proto::SyscallNo::ClockGettime.as_u64(),
        1,
        t.as_mut_ptr() as u64,
        0,
    );
    (t[0] as u64) * 1_000_000_000 + t[1] as u64
}
#[no_mangle]
#[link_section = ".text._start"]
pub extern "C" fn _start(heap_start: usize, heap_length: usize) -> ! {
    unsafe { HEAP.init(heap_start, heap_length) };
    if call(alloyos_proto::process::QUERY_ABI_VERSION, 0, 0, 0)
        != alloyos_proto::process::ABI_VERSION as i64
    {
        exit(72)
    }
    let mut dimensions = [0u32; 2];
    if call(display::DISPLAY_INFO, dimensions.as_mut_ptr() as u64, 0, 0) < 0 {
        exit(1)
    }
    let mut ui = FerrofluidSystemUi::new(
        dimensions[0],
        dimensions[1],
        alloyos_proto::display::PHONE_SCALE as f32,
    )
    .unwrap();
    let mut bytes = vec![0; display::MAX_STATE];
    let mut revision = u64::MAX;
    let mut pins = alloc::vec::Vec::new();
    let start = time();
    send(Event::Ready);
    loop {
        let frame = time();
        let n = call(
            display::DISPLAY_STATE,
            bytes.as_mut_ptr() as u64,
            bytes.len() as u64,
            0,
        );
        if n > 0 {
            if let Ok(s) = serde_json::from_slice::<State>(&bytes[..n as usize]) {
                if s.revision != revision {
                    if s.fault_probe {
                        unsafe {
                            core::ptr::read_volatile(0x4008_0000 as *const u64);
                        }
                    }
                    revision = s.revision;
                    ui.set_booting(s.booting);
                    ui.set_reply_pending(s.pending);
                    ui.set_voice_state(s.voice_state, &s.voice_caption, &s.voice_error);
                    ui.set_voice_level(s.voice_level);
                    ui.set_output_muted(s.output_muted);
                    ui.set_voice_diagnostics(&s.voice_diagnostics);
                    ui.set_active_model(&s.model);
                    ui.set_model_selection_status(&s.model_status);
                    ui.set_transcript(&s.transcript);
                    ui.set_available_tiles(&s.available);
                    ui.set_tile_discovery_status(s.tile_catalog_loaded, &s.tile_catalog_error);
                    ui.set_pending_remote_install(s.pending_remote_install.as_ref());
                    ui.set_tiles(&s.tiles);
                    ui.set_chat_items(&s.items);
                    if s.pinned != pins {
                        ui.restore_dashboard(&s.pinned);
                        pins = s.pinned;
                    }
                    send(Event::Applied {
                        revision: s.revision,
                        digest: display::state_digest(
                            &ui.transcript(),
                            ui.dashboard_state().pinned_ids(),
                        ),
                    });
                    if !s.partial.is_empty() {
                        ui.update_streaming_reply(&s.transcript, &s.partial);
                        ui.update_streaming_chat_item(&s.partial);
                    }
                }
            }
        }
        for _ in 0..64 {
            let mut b = [0u8; 16];
            if call(display::DISPLAY_INPUT, b.as_mut_ptr() as u64, 0, 0) < 0 {
                break;
            }
            let kind = match b[0] {
                1 => InputEventKind::KeyDown,
                2 => InputEventKind::KeyUp,
                3 => InputEventKind::PointerMove,
                4 => InputEventKind::PointerButtonDown,
                5 => InputEventKind::PointerButtonUp,
                _ => continue,
            };
            ui.handle_input(InputEvent {
                kind,
                code: u16::from_le_bytes([b[2], b[3]]),
                x: i32::from_le_bytes(b[4..8].try_into().unwrap()),
                y: i32::from_le_bytes(b[8..12].try_into().unwrap()),
            });
        }
        if let Some(s) = ui.take_submission() {
            send(Event::Submit(s))
        }
        while ui.take_output_mute_toggle() { send(Event::OutputMuteToggle); }
        while ui.take_voice_toggle() { send(Event::VoiceToggle); }
        while ui.take_voice_mute() { send(Event::VoiceMute); }
        while ui.take_model_cycle() {
            send(Event::CycleModel)
        }
        if ui.take_tile_discovery_request() {
            send(Event::DiscoverTiles);
        }
        while let Some((id, approved)) = ui.take_remote_install_decision() {
            send(Event::RemoteInstallDecision { id, approved });
        }
        while let Some(e) = ui.take_tile_event() {
            send(Event::Tile(e))
        }
        let d = ui.dashboard_state();
        if d.pinned_ids() != pins {
            pins = d.pinned_ids().to_vec();
            send(Event::Dashboard(pins.clone()));
        }
        if ui
            .render(core::time::Duration::from_nanos(
                time().saturating_sub(start),
            ))
            .is_some()
        {
            let size = ui.size();
            call(
                display::DISPLAY_PRESENT,
                ui.pixels().as_ptr() as u64,
                size.width as u64,
                size.height as u64,
            );
        }
        while time() < frame.saturating_add(16_666_667) {
            call(alloyos_proto::SyscallNo::Yield.as_u64(), 0, 0, 0);
        }
    }
}
fn exit(code: u64) -> ! {
    call(alloyos_proto::SyscallNo::Exit.as_u64(), code, 0, 0);
    loop {
        call(alloyos_proto::SyscallNo::Yield.as_u64(), 0, 0, 0);
    }
}
#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    liballoy::runtime::panic_report(info)
}

#[alloc_error_handler]
fn allocation_failed(layout: core::alloc::Layout) -> ! {
    // The panic reporter uses a fixed stack buffer, so an exhausted heap still
    // produces a useful fault report instead of silently awaiting the watchdog.
    panic!("Ferrofluid heap exhausted allocating {} bytes", layout.size());
}
