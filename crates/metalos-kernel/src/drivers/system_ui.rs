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
    ui.set_tiles(&agent.synthesized_tiles());
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
        while let Some(action) = ui.take_tile_action() {
            match agent.execute_tile_action(&action) {
                Ok(_) => ui.set_tiles(&agent.synthesized_tiles()),
                Err(error) => {
                    transcript.push_str("\n\nWelder: ");
                    transcript.push_str(&alloc::format!("{}", error));
                    ui.set_transcript(&transcript);
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
                ui.set_tiles(&agent.synthesized_tiles());
            }
        }
        if pending {
            if let Some(text) = agent.partial_console_reply() {
                if text != partial {
                    partial = text;
                    crate::serial_println!("[welder] Reply text advanced.");
                    ui.update_streaming_reply(&transcript, &partial);
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
            ui.set_transcript(&transcript);
            ui.set_tiles(&agent.synthesized_tiles());
        }
        let elapsed = Duration::from_nanos(rng::monotonic_time_ns().saturating_sub(start));
        let _ = crate::fs::automation::tick_kernel_automations(&mut agent, elapsed.as_millis() as u64);
        if let Some(damage) = ui.render(elapsed) {
            if let Some(gpu) = virtio_gpu::VIRTIO_GPU.lock().as_mut() {
                let size = ui.size();
                gpu.present_scaled_argb(ui.pixels(), size.width, size.height, Some(damage));
            }
        }
        forge.note_ui_frame();
        // Sleep until the next frame; Forge can use the remaining CPU time.
        let deadline = frame_start.saturating_add(16_666_667);
        while rng::monotonic_time_ns() < deadline {
            crate::task::scheduler::sleep_until(deadline);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::drivers::forge::forge_https::GuestHttps;
    use crate::net::{with_net_stack, DnsResolver, TlsEndpoint, TEST_NET_LOCK};
    use alloc::{boxed::Box, rc::Rc, vec, vec::Vec};
    use core::cell::RefCell;
    use libmetal::tls::Tls13ServerConfig;
    use metalos_forge::{cloud::ApiProvider, Forge};

    #[test]
    fn streaming_tls13_cloud_reply_advances_welder_and_transcript_live() {
        let _net_guard = TEST_NET_LOCK.lock();
        let host = "api.openai.com";
        let ip = DnsResolver::resolve_hostname(host);
        with_net_stack(|net| {
            net.clear_tls_endpoints();
            net.register_tls_endpoint(
                ip,
                443,
                TlsEndpoint {
                    server_config: Tls13ServerConfig::trusted_for_host(host),
                    status: 200,
                    content_type: String::from("text/event-stream"),
                    body_chunks: vec![
                        b"data: {\"type\":\"response.output_text.delta\",\"delta\":\"Streaming\"}\n\n".to_vec(),
                        b"data: {\"type\":\"response.output_text.delta\",\"delta\":\" live\"}\n\n".to_vec(),
                        b"data: {\"type\":\"response.output_text.delta\",\"delta\":\" tokens\",\"usage\":{\"output_tokens\":3}}\n\ndata: [DONE]\n\n".to_vec(),
                    ],
                    require_header: Some((
                        String::from("Authorization"),
                        String::from("Bearer live-key"),
                    )),
                },
            );
        });

        let mut forge = Forge::new(0);
        forge
            .connect_api(
                ApiProvider::OpenAi,
                ApiProvider::OpenAi.default_model(),
                "live-key",
                Box::new(GuestHttps::new()),
            )
            .unwrap();
        let shared = Rc::new(RefCell::new(forge));
        let mut agent = metalos_welder::AgentRuntime::with_forge(shared);
        agent.cycle_model().unwrap();
        assert_eq!(agent.active_model_id(), ApiProvider::OpenAi.default_model());

        let base_transcript = String::from("Welcome to metalOS.\n\nYou: test live stream");
        assert!(agent.start_console_input("test live stream").unwrap().is_none());

        let mut observed_partials = Vec::new();
        let mut final_reply = None;
        for _ in 0..16 {
            match agent.poll_console_reply() {
                Poll::Pending => {
                    if let Some(p) = agent.partial_console_reply() {
                        if observed_partials.last() != Some(&p) {
                            observed_partials.push(p);
                        }
                    }
                }
                Poll::Ready(res) => {
                    final_reply = Some(res.unwrap().response_text);
                    break;
                }
            }
        }
        assert_eq!(
            observed_partials,
            vec![String::from("Streaming"), String::from("Streaming live")]
        );
        assert_eq!(
            final_reply.as_deref(),
            Some("Streaming live tokens")
        );
        assert_eq!(
            alloc::format!("{}\n\nWelder: {}", base_transcript, final_reply.unwrap()),
            "Welcome to metalOS.\n\nYou: test live stream\n\nWelder: Streaming live tokens"
        );
        with_net_stack(|net| net.clear_tls_endpoints());
    }
}
