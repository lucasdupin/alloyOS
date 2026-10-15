//! Driver bridge for the Ferrofluid-owned Slint System UI.
//! Slint lives on the boot/UI context, never in a shared driver static or IRQ.

use super::rng;
use alloc::{collections::VecDeque, string::{String, ToString}};
use core::{task::Poll, time::Duration};
use alloyos_mcp::display::{ChatItem, TileEvent};
use super::display_server::DisplayProxy as FerrofluidSystemUi;

pub fn create() -> FerrofluidSystemUi { FerrofluidSystemUi::new() }

pub fn run(
    mut ui: FerrofluidSystemUi,
    mut forge: super::forge::ForgeService,
    mut agent: alloyos_welder::AgentRuntime,
) -> ! {
    let mut console_rpc_id = None;
    let start = rng::monotonic_time_ns();
    let welcome = String::from("Welcome to alloyOS.\n\nYour workspace takes shape around what you need.");
    let mut transcript = welcome.clone();
    let mut chat_items: alloc::vec::Vec<ChatItem> = alloc::vec::Vec::new();
    chat_items.push(ChatItem::Text(welcome));
    ui.set_active_model(agent.active_model_id());
    if let Err(error) = agent.initialize_dashboard_tiles() {
        let err_msg = alloc::format!("Welder: {}", error);
        transcript.push_str(&alloc::format!("\n\n{}", err_msg));
        chat_items.push(ChatItem::Text(err_msg));
    }
    for id in agent.pinned_tile_ids().to_vec() {
        let _ = agent.refresh_tile(&id);
    }
    ui.set_available_tiles(&agent.synthesized_tiles());
    ui.restore_dashboard(agent.pinned_tile_ids());
    let mut chat_tiles: alloc::vec::Vec<alloyos_mcp::AppTile> = alloc::vec::Vec::new();
    ui.set_tiles(&chat_tiles);
    ui.set_transcript(&transcript);
    ui.set_chat_items(&chat_items);
    let mut dashboard_revision = ui.dashboard_state().revision();
    let mut last_dashboard_refresh = 0u64;
    let mut refresh_queue = VecDeque::new();
    let mut package_watcher = crate::fs::packages::PackageWatcher::default();
    let mut pending = false;
    let mut voice = super::voice::Voice::default();
    let mut partial = String::new();
    let mut booting = forge.is_boot_loading();
    if !booting {
        ui.set_booting(false);
    }

    #[cfg(feature="process-runtime-selftest")]
    ui.acceptance();
    loop {
        ui.poll();
        let frame_start = rng::monotonic_time_ns();
        forge.poll();
        while ui.take_voice_toggle() { voice.toggle(&mut forge, &mut agent, pending, frame_start); }
        while ui.take_voice_mute() { voice.mute(&mut forge); }
        voice.poll(&mut forge, &mut agent, frame_start);
        while let Some((user, assistant)) = voice.take_completed() {
            if !user.is_empty() {
                transcript.push_str(&alloc::format!("\n\nYou: {}", user));
                chat_items.push(ChatItem::Text(alloc::format!("You: {}", user)));
            }
            if !assistant.is_empty() {
                transcript.push_str(&alloc::format!("\n\nWelder: {}", assistant));
                chat_items.push(ChatItem::Text(alloc::format!("Welder: {}", assistant)));
            }
            ui.set_transcript(&transcript);
            ui.set_chat_items(&chat_items);
            ui.set_available_tiles(&agent.synthesized_tiles());
        }
        while let Some(output) = voice.take_tool_output() {
            if let Some(tile) = output.synthesized_view.and_then(|view|view.tile) {
                if let Some(existing) = chat_tiles.iter_mut().find(|t|t.tile_id == tile.tile_id) {
                    *existing = tile.clone();
                } else { chat_tiles.push(tile.clone()); }
                chat_items.push(ChatItem::Tile(tile));
                ui.set_tiles(&chat_tiles);
                ui.set_chat_items(&chat_items);
            }
            ui.set_available_tiles(&agent.synthesized_tiles());
        }
        ui.set_voice(voice.state, &voice.caption, &voice.error);
        ui.set_voice_level(voice.level);
        ui.set_voice_diagnostics(&voice.diagnostics);
        if !pending && !voice.active() {
            if let Some((id, input)) = forge.take_console_request() {
                crate::serial_println!("[welder-e2e] stage=chat_started");
                match agent.start_console_input(&input) {
                    Ok(None) => { console_rpc_id = Some(id); pending = true; ui.set_reply_pending(true); }
                    Ok(Some(output)) => { forge.finish_console_request(id, Ok(serde_json::json!({"text":output.response_text,"tools":[]}).to_string())); }
                    Err(error) => forge.finish_console_request(id, Err(error.to_string())),
                }
            }
        }
        if booting && !forge.is_boot_loading() {
            booting = false;
            ui.set_booting(false);
            crate::serial_println!("[systemui] Model boot phase finished.");
        }
        if let Some(error) = forge.take_boot_error() {
            let err_msg = alloc::format!("Welder: {}", error);
            transcript.push_str("\n\nWelder: ");
            transcript.push_str(error);
            chat_items.push(ChatItem::Text(err_msg));
            ui.set_transcript(&transcript);
            ui.set_chat_items(&chat_items);
        }
        while ui.take_model_cycle() {
            if !pending {
                voice.stop(&mut forge, &mut agent);
                match agent.cycle_model() {
                    Ok(()) => {
                        ui.set_active_model(agent.active_model_id());
                        ui.set_model_selection_status("");
                    }
                    Err(error) => {
                        crate::serial_println!("[systemui] Model selection failed: {}", error);
                        ui.set_model_selection_status(&alloc::format!("{}", error));
                    }
                }
            }
        }
        let dashboard = ui.dashboard_state();
        if dashboard.revision() != dashboard_revision {
            match agent.set_pinned_tile_ids(dashboard.pinned_ids()) {
                Ok(()) => dashboard_revision = dashboard.revision(),
                Err(error) => {
                    ui.restore_dashboard(agent.pinned_tile_ids());
                    dashboard_revision = ui.dashboard_state().revision();
                    let err_msg = alloc::format!("Welder: {}", error);
                    transcript.push_str(&alloc::format!("\n\nWelder: {}", error));
                    chat_items.push(ChatItem::Text(err_msg));
                    ui.set_transcript(&transcript);
                    ui.set_chat_items(&chat_items);
                }
            }
        }
        while let Some(event) = ui.take_tile_event() {
            if pending || voice.active() {
                continue;
            }
            let result = match event {
                TileEvent::Action { tile_id, action } => {
                    agent.execute_tile_action_for(&tile_id, &action)
                }
                TileEvent::Refresh { tile_id, .. } => {
                    agent.refresh_tile(&tile_id)
                }
            };
            match result {
                Ok(output) => {
                    if let Some(view) = output.synthesized_view {
                        if let Some(tile) = view.tile {
                            if let Some(existing) = chat_tiles.iter_mut().find(|t| t.tile_id == tile.tile_id) {
                                *existing = tile.clone();
                            }
                            for item in chat_items.iter_mut() {
                                if let ChatItem::Tile(existing) = item {
                                    if existing.tile_id == tile.tile_id {
                                        *existing = tile.clone();
                                    }
                                }
                            }
                        }
                    }
                    ui.set_available_tiles(&agent.synthesized_tiles());
                    ui.set_tiles(&chat_tiles);
                    ui.set_chat_items(&chat_items);
                }
                Err(error) => {
                    let err_msg = alloc::format!("Welder: {}", error);
                    transcript.push_str(&alloc::format!("\n\nWelder: {}", error));
                    chat_items.push(ChatItem::Text(err_msg));
                    ui.set_transcript(&transcript);
                    ui.set_chat_items(&chat_items);
                }
            }
        }
        if pending {
            if let Poll::Ready(result) = agent.poll_console_reply() {
                pending = false;
                ui.set_reply_pending(false);
                transcript.push_str("\n\nWelder: ");
                match result {
                    Ok(output) => {
                        if let Some(id) = console_rpc_id.take() {
                            let tools: alloc::vec::Vec<_> = output.tool_outputs.iter().map(|output| serde_json::json!({"name":output.tool_name,"output":serde_json::from_str::<serde_json::Value>(&output.output_json).unwrap_or(serde_json::Value::Null)})).collect();
                            crate::serial_println!("[welder-e2e] stage=chat_finished success=true tools={}", tools.len());
                            forge.finish_console_request(id, Ok(serde_json::json!({"text":output.response_text,"tools":tools}).to_string()));
                        }
                        transcript.push_str(&output.response_text);
                        let welder_msg = alloc::format!("Welder: {}", output.response_text);
                        if let Some(ChatItem::Text(last)) = chat_items.last_mut() {
                            if last.starts_with("Welder:") {
                                *last = welder_msg;
                            } else {
                                chat_items.push(ChatItem::Text(welder_msg));
                            }
                        } else {
                            chat_items.push(ChatItem::Text(welder_msg));
                        }
                        for tool_out in &output.tool_outputs {
                            if let Some(view) = &tool_out.synthesized_view {
                                if let Some(tile) = &view.tile {
                                    if let Some(existing) = chat_tiles.iter_mut().find(|t| t.tile_id == tile.tile_id) {
                                        *existing = tile.clone();
                                    } else {
                                        chat_tiles.push(tile.clone());
                                    }
                                    chat_items.push(ChatItem::Tile(tile.clone()));
                                }
                            }
                        }
                    }
                    Err(error) => {
                        if let Some(id) = console_rpc_id.take() {
                            crate::serial_println!("[welder-e2e] stage=chat_finished success=false");
                            forge.finish_console_request(id, Err(error.to_string()));
                        }
                        let err_msg = alloc::format!("Welder: {}", error);
                        transcript.push_str(&alloc::format!("{}", error));
                        if let Some(ChatItem::Text(last)) = chat_items.last_mut() {
                            if last.starts_with("Welder:") {
                                *last = err_msg;
                            } else {
                                chat_items.push(ChatItem::Text(err_msg));
                            }
                        } else {
                            chat_items.push(ChatItem::Text(err_msg));
                        }
                    }
                }
                ui.set_available_tiles(&agent.synthesized_tiles());
                ui.set_transcript(&transcript);
                ui.set_tiles(&chat_tiles);
                ui.set_chat_items(&chat_items);
            }
        }
        if pending {
            if let Some(text) = agent.partial_console_reply() {
                if text != partial {
                    partial = text;
                    // UART status frames can span UI ticks. Keep debug text
                    // out of the middle of an in-flight structured response.
                    if !super::forge::rpc_output_pending() {
                        crate::serial_println!("[welder] Reply text advanced.");
                    }
                    ui.update_streaming_reply(&transcript, &partial);
                    ui.update_streaming_chat_item(&partial);
                }
            }
        }
        if let Some(text) = if pending { None } else { ui.take_submission() } {
            if text.trim()=="/display restart" {voice.stop(&mut forge, &mut agent);ui.request_restart();continue;}
            if voice.active() {
                if let Err(error) = voice.send_text(&text) { ui.set_voice(voice.state, &voice.caption, error); }
                continue;
            }
            if transcript.len() > 16 * 1024 {
                transcript.clear();
            }
            if chat_items.len() > 100 {
                chat_items.drain(0..50);
            }
            transcript.push_str("\n\nYou: ");
            transcript.push_str(&text);
            chat_items.push(ChatItem::Text(alloc::format!("You: {}", text)));
            match agent.start_console_input(&text) {
                Ok(Some(output)) => {
                    transcript.push_str("\n\nWelder: ");
                    transcript.push_str(&output.response_text);
                    chat_items.push(ChatItem::Text(alloc::format!("Welder: {}", output.response_text)));
                    for tool_out in &output.tool_outputs {
                        if let Some(view) = &tool_out.synthesized_view {
                            if let Some(tile) = &view.tile {
                                if let Some(existing) = chat_tiles.iter_mut().find(|t| t.tile_id == tile.tile_id) {
                                    *existing = tile.clone();
                                } else {
                                    chat_tiles.push(tile.clone());
                                }
                                chat_items.push(ChatItem::Tile(tile.clone()));
                            }
                        }
                    }
                }
                Ok(None) => {
                    pending = true;
                    partial.clear();
                    ui.set_reply_pending(true);
                }
                Err(error) => {
                    let err_msg = alloc::format!("Welder: {}", error);
                    transcript.push_str(&alloc::format!("\n\nWelder: {}", error));
                    chat_items.push(ChatItem::Text(err_msg));
                }
            }
            ui.set_active_model(agent.active_model_id());
            ui.set_available_tiles(&agent.synthesized_tiles());
            ui.set_transcript(&transcript);
            ui.set_tiles(&chat_tiles);
            ui.set_chat_items(&chat_items);
        }
        let elapsed = Duration::from_nanos(rng::monotonic_time_ns().saturating_sub(start));
        if !voice.active() {
            let _ = crate::fs::automation::tick_kernel_automations(&mut agent, elapsed.as_millis() as u64);
        }
        let now_ms = elapsed.as_millis() as u64;
        if let Ok(changes) = package_watcher.poll(now_ms) {
            if !changes.is_empty() {
                let _ = package_watcher.publish_inventory();
                let _ = agent.tool_registry.refresh_app_packages();
                for change in changes {
                    if let alloyos_mcp::packages::PackageChange::Updated(status) = change {
                        if !status.ready {
                            for diagnostic in status.diagnostics {
                                let diag_msg = alloc::format!("Welder: App {}: {} ({})",
                                    status.app_id, diagnostic.message, diagnostic.path);
                                transcript.push_str(&alloc::format!("\n\n{}", diag_msg));
                                chat_items.push(ChatItem::Text(diag_msg));
                            }
                            ui.set_transcript(&transcript);
                            ui.set_chat_items(&chat_items);
                        }
                    }
                }
            }
        }
        if !pending && !voice.active() && refresh_queue.is_empty() && now_ms.saturating_sub(last_dashboard_refresh) >= 5_000 {
            last_dashboard_refresh = now_ms;
            refresh_queue.extend(ui.dashboard_state().pinned_ids().iter().cloned());
        }
        if !pending && !voice.active() {
            if let Some(id) = refresh_queue.pop_front() {
                // One card per frame keeps large dashboards from monopolizing
                // rendering. Unpinned cards need no background refresh.
                if ui.dashboard_state().pinned_ids().contains(&id) {
                    let _ = agent.refresh_tile(&id);
                    ui.set_available_tiles(&agent.synthesized_tiles());
                    ui.set_tiles(&chat_tiles);
                    ui.set_chat_items(&chat_items);
                }
            }
        }
        ui.flush();
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
    use liballoy::tls::Tls13ServerConfig;
    use alloyos_forge::{cloud::ApiProvider, Forge};

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
        let mut agent = alloyos_welder::AgentRuntime::with_forge(shared);
        agent.cycle_model().unwrap();
        assert_eq!(agent.active_model_id(), ApiProvider::OpenAi.default_model());

        let base_transcript = String::from("Welcome to alloyOS.\n\nYou: test live stream");
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
            "Welcome to alloyOS.\n\nYou: test live stream\n\nWelder: Streaming live tokens"
        );
        with_net_stack(|net| net.clear_tls_endpoints());
    }
}
