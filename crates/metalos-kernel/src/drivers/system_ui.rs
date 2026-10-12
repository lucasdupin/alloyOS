//! Driver bridge for the Ferrofluid-owned Slint System UI.
//! Slint lives on the boot/UI context, never in a shared driver static or IRQ.

use super::{rng, virtio_gpu, virtio_input};
use alloc::string::String;
use core::time::Duration;
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
    ui.set_status("Welder · resident local inference");
    loop {
        forge.poll();
        // Bound work per frame so a busy input device cannot starve rendering.
        for _ in 0..64 {
            let event = virtio_input::VIRTIO_INPUT.lock().poll_ui_event();
            let Some(event) = event else {
                break;
            };
            ui.handle_input(event);
        }
        while let Some(text) = ui.take_submission() {
            if transcript.len() > 16 * 1024 {
                transcript.clear();
            }
            transcript.push_str("\n\nYou: ");
            transcript.push_str(&text);
            match agent.handle_console_input(&text, &mut |_| {}) {
                Ok(output) => {
                    transcript.push_str("\n\nWelder: ");
                    transcript.push_str(&output.response_text);
                    ui.set_status("Welder · response ready");
                }
                Err(error) => {
                    ui.set_status(&alloc::format!("Welder unavailable · {}", error));
                }
            }
            ui.set_active_model(agent.active_model_id());
            ui.set_transcript(&transcript);
        }
        let elapsed = Duration::from_nanos(rng::monotonic_time_ns().saturating_sub(start));
        if let Some(damage) = ui.render(elapsed) {
            if let Some(gpu) = virtio_gpu::VIRTIO_GPU.lock().as_mut() {
                let size = ui.size();
                gpu.present_scaled_argb(ui.pixels(), size.width, size.height, Some(damage));
            }
        }
        // Timer/virtqueue IRQ wakeup isn't wired yet. Poll at most 60 Hz rather
        // than WFI, which could leave input waiting indefinitely.
        let deadline = start
            .saturating_add(elapsed.as_nanos() as u64)
            .saturating_add(16_666_667);
        while rng::monotonic_time_ns() < deadline {
            core::hint::spin_loop();
        }
    }
}
