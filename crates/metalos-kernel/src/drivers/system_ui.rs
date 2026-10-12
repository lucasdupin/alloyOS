//! Driver bridge for the Ferrofluid-owned Slint System UI.
//! Slint lives on the boot/UI context, never in a shared driver static or IRQ.

use super::{rng, virtio_gpu, virtio_input};
use alloc::string::String;
use core::{task::Poll, time::Duration};
use metalos_ferrofluid::system_ui::FerrofluidSystemUi;

pub fn create() -> FerrofluidSystemUi {
    let size = virtio_gpu::VIRTIO_GPU
        .lock()
        .as_ref()
        .expect("GPU initialized")
        .framebuffer_info();
    virtio_input::VIRTIO_INPUT
        .lock()
        .set_display_size(size.width, size.height);
    FerrofluidSystemUi::new(
        size.width,
        size.height,
        metalos_proto::display::PHONE_SCALE as f32,
    )
    .expect("initialize Ferrofluid Slint System UI")
}

pub fn run(
    mut ui: FerrofluidSystemUi,
    mut forge: super::forge::ForgeService,
    mut agent: metalos_welder::AgentRuntime,
) -> ! {
    let start = rng::monotonic_time_ns();
    let mut transcript =
        String::from("Welcome to metalOS.\n\nYour workspace takes shape around what you need.");
    ui.set_active_model(agent.active_model_id());
    let mut pending = false;
    let mut partial = String::new();
    let mut booting = forge.is_boot_loading();
    if !booting {
        ui.set_booting(false);
    }

    loop {
        let frame_start = rng::monotonic_time_ns();
        forge.poll();
        if booting && !forge.is_boot_loading() {
            booting = false;
            ui.set_booting(false);
            crate::serial_println!("[systemui] Model boot phase finished.");
        }
        if let Some(error) = forge.take_boot_error() {
            transcript.push_str("\n\nWelder: ");
            transcript.push_str(error);
            ui.set_transcript(&transcript);
        }
        // Bound work per frame so a busy input device cannot starve rendering.
        for _ in 0..64 {
            let event = virtio_input::VIRTIO_INPUT.lock().poll_ui_event();
            let Some(event) = event else {
                break;
            };
            ui.handle_input(event);
            forge.note_ui_input();
        }
        while ui.take_model_cycle() {
            if !pending {
                match agent.cycle_model() {
                    Ok(()) => ui.set_active_model(agent.active_model_id()),
                    Err(error) => {
                        crate::serial_println!("[systemui] Model selection failed: {}", error);
                    }
                }
            }
        }
        if pending {
            if let Poll::Ready(result) = agent.poll_console_reply() {
                pending = false;
                ui.set_reply_pending(false);
                transcript.push_str("\n\nWelder: ");
                match result {
                    Ok(output) => transcript.push_str(&output.response_text),
                    Err(error) => transcript.push_str(&alloc::format!("{}", error)),
                }
                ui.set_transcript(&transcript);
            }
        }
        if pending {
            if let Some(text) = agent.partial_console_reply() {
                if text != partial {
                    partial = text;
                    crate::serial_println!("[welder] Reply text advanced.");
                    ui.set_transcript(&alloc::format!("{}\n\nWelder: {}", transcript, partial));
                }
            }
        }
        if let Some(text) = if pending { None } else { ui.take_submission() } {
            if transcript.len() > 16 * 1024 {
                transcript.clear();
            }
            transcript.push_str("\n\nYou: ");
            transcript.push_str(&text);
            match agent.start_console_input(&text) {
                Ok(Some(output)) => {
                    transcript.push_str("\n\nWelder: ");
                    transcript.push_str(&output.response_text);
                }
                Ok(None) => {
                    pending = true;
                    partial.clear();
                    ui.set_reply_pending(true);
                }
                Err(error) => {
                    transcript.push_str("\n\nWelder: ");
                    transcript.push_str(&alloc::format!("{}", error));
                }
            }
            ui.set_active_model(agent.active_model_id());
            if pending {
                ui.set_transcript(&alloc::format!("{}\n\nWelder: Thinking…", transcript));
            } else {
                ui.set_transcript(&transcript);
            }
        }
        let elapsed = Duration::from_nanos(rng::monotonic_time_ns().saturating_sub(start));
        if let Some(damage) = ui.render(elapsed) {
            if let Some(gpu) = virtio_gpu::VIRTIO_GPU.lock().as_mut() {
                let size = ui.size();
                gpu.present_scaled_argb(ui.pixels(), size.width, size.height, Some(damage));
            }
        }
        forge.note_ui_frame();
        // Timer/virtqueue IRQ wakeup isn't wired yet. Poll at most 60 Hz rather
        // than WFI, which could leave input waiting indefinitely.
        let deadline = frame_start.saturating_add(16_666_667);
        while rng::monotonic_time_ns() < deadline {
            core::hint::spin_loop();
        }
    }
}
