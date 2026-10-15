//! UI-thread live voice coordinator. Device/socket I/O remains in the host bridge.
use super::forge::ForgeService;
use alloc::{
    collections::{BTreeSet, VecDeque},
    string::{String, ToString},
    vec::Vec,
};
use alloyos_forge::{
    provider::ToolCall,
    voice::{LiveEvent, LiveSession, LiveState, GEMINI_LIVE_ENDPOINT},
};
use alloyos_mcp::display::VoiceState;
use alloyos_welder::AgentRuntime;
use core::task::Poll;
use serde_json::{json, Value};

pub struct Voice {
    pub output_muted: bool,
    live: Option<LiveSession>,
    next_id: u64,
    started: u64,
    startup_stage: StartupStage,
    host_ready: bool,
    direct_audio: bool,
    playback_active: bool,
    pub state: VoiceState,
    pub caption: String,
    pub error: String,
    pub level: u16,
    pub diagnostics: String,
    audio_stats: AudioDiagnostics,
    last_diagnostic_update: u64,
    level_target: u16,
    last_audio: u64,
    last_level_update: u64,
    input: String,
    output: String,
    interrupted_turn: bool,
    tools: VecDeque<ToolCall>,
    seen_tools: BTreeSet<String>,
    tool_count: usize,
    completed: VecDeque<(String, String)>,
    tool_outputs: VecDeque<alloyos_welder::tools::ToolExecutionOutput>,
}
impl Default for Voice {
    fn default() -> Self {
        Self {
            output_muted: false,
            live: None,
            next_id: 0,
            started: 0,
            startup_stage: StartupStage::Opening,
            host_ready: false,
            direct_audio: false,
            playback_active: false,
            state: VoiceState::Off,
            caption: String::new(),
            error: String::new(),
            level: 0,
            diagnostics: String::new(),
            audio_stats: AudioDiagnostics::default(),
            last_diagnostic_update: 0,
            level_target: 0,
            last_audio: 0,
            last_level_update: 0,
            input: String::new(),
            output: String::new(),
            interrupted_turn: false,
            tools: VecDeque::new(),
            seen_tools: BTreeSet::new(),
            tool_count: 0,
            completed: VecDeque::new(),
            tool_outputs: VecDeque::new(),
        }
    }
}
impl Voice {
    pub fn set_output_muted(&mut self, forge: &mut ForgeService, muted: bool) -> Result<(), &'static str> {
        if self.active() {
            self.command(forge, json!({"op":"output_mute","muted":muted}))?;
        }
        self.output_muted = muted;
        Ok(())
    }
    pub fn active(&self) -> bool {
        self.live.is_some()
    }
    fn command(&self, forge: &mut ForgeService, mut command: Value) -> Result<(), &'static str> {
        let id = self
            .live
            .as_ref()
            .ok_or("Voice is not connected")?
            .session_id;
        command["session"] = json!(id);
        forge.send_voice_command(id, &command)
    }
    fn commit(&mut self, agent: &mut AgentRuntime) {
        if self.input.is_empty() && self.output.is_empty() {
            return;
        }
        let user = core::mem::take(&mut self.input);
        let assistant = core::mem::take(&mut self.output);
        self.caption.clear();
        agent.record_voice_turn(&user, &assistant);
        if self.completed.len() < 16 {
            self.completed.push_back((user, assistant));
        }
    }
    pub fn take_completed(&mut self) -> Option<(String, String)> {
        self.completed.pop_front()
    }
    pub fn take_tool_output(&mut self) -> Option<alloyos_welder::tools::ToolExecutionOutput> {
        self.tool_outputs.pop_front()
    }
    pub fn stop(&mut self, forge: &mut ForgeService, agent: &mut AgentRuntime) {
        if let Some(live) = self.live.as_ref() {
            let _ = forge.send_voice_command(
                live.session_id,
                &json!({"op":"stop","session":live.session_id}),
            );
        }
        self.commit(agent);
        self.live = None;
        for call in &self.tools {
            agent.tool_registry.cancel_pending_tool(&call.id);
        }
        self.tools.clear();
        self.seen_tools.clear();
        self.state = VoiceState::Off;
        self.diagnostics.clear();
        self.level = 0;
        self.level_target = 0;
        self.caption.clear();
    }
    fn fail(&mut self, forge: &mut ForgeService, agent: &mut AgentRuntime, error: &str) {
        self.stop(forge, agent);
        self.state = VoiceState::Error;
        self.error = error.chars().take(512).collect();
    }
    pub fn toggle(
        &mut self,
        forge: &mut ForgeService,
        agent: &mut AgentRuntime,
        pending: bool,
        now: u64,
    ) {
        if self.active() {
            if self.state == VoiceState::Muted {
                let live = self.live.as_mut().unwrap();
                if live.set_muted(live.session_id, false).is_err()
                    || self
                        .command(forge, json!({"op":"mute","muted":false}))
                        .is_err()
                {
                    self.fail(forge, agent, "Could not resume voice capture");
                } else {
                    self.state = VoiceState::Listening;
                    self.error.clear();
                }
            } else {
                self.stop(forge, agent);
            }
            return;
        }
        self.error.clear();
        if pending {
            self.fail(
                forge,
                agent,
                "Wait for the current reply before starting voice",
            );
            return;
        }
        let Some(key) = forge.voice_key().map(String::from) else {
            self.fail(forge, agent, "Configure a Gemini API key to use voice");
            return;
        };
        self.next_id = self.next_id.saturating_add(1);
        let model = agent.voice_model().to_string();
        match agent.start_voice_session(self.next_id, &model) {
            Ok(live) => self.live = Some(live),
            Err(error) => {
                self.fail(forge, agent, &error.to_string());
                return;
            }
        }
        self.started = now;
        self.audio_stats = AudioDiagnostics::default();
        self.last_diagnostic_update = now;
        self.diagnostics = "Waiting for microphone frames…".into();
        self.startup_stage = StartupStage::Opening;
        self.host_ready = false;
        self.direct_audio = true;
        self.playback_active = false;
        self.tool_count = 0;
        self.state = VoiceState::Connecting;
        self.caption = "Opening microphone…".into();
        if self
            .command(
                forge,
                json!({"op":"start","endpoint":GEMINI_LIVE_ENDPOINT,"api_key":key,"direct_audio":true,"output_muted":self.output_muted}),
            )
            .is_err()
        {
            self.fail(forge, agent, "Voice transport is busy. Try again.");
        }
    }
    pub fn send_text(&mut self, text: &str) -> Result<(), &'static str> {
        let text = text.trim();
        let result = self
            .live
            .as_mut()
            .ok_or("Voice is not connected")
            .and_then(|live| {
                live.send_text(live.session_id, text)
                    .map_err(|_| "Voice is not ready or its input queue is full")
            });
        match result {
            Ok(()) => {
                append_bounded(&mut self.input, text);
                self.caption = self.input.clone();
                self.error.clear();
                Ok(())
            }
            Err(error) => {
                self.error = error.into();
                Err(error)
            }
        }
    }
    pub fn mute(&mut self, forge: &mut ForgeService) {
        self.level = 0;
        self.level_target = 0;
        if let Some(live) = self.live.as_mut() {
            let id = live.session_id;
            let protocol_result = live.set_muted(id, true);
            // Always send the device command, even if the provider queue is full.
            let device_result = self.command(forge, json!({"op":"mute","muted":true}));
            if protocol_result.is_err() || device_result.is_err() {
                self.error = "Voice mute could not be delivered; ending the session".into();
                let _ = forge.send_voice_command(id, &json!({"op":"stop","session":id}));
                self.live = None;
                // poll() cancels outstanding tools with access to AgentRuntime.
                self.state = VoiceState::Error;
            } else {
                self.state = VoiceState::Muted;
            }
        }
    }
    pub fn poll(&mut self, forge: &mut ForgeService, agent: &mut AgentRuntime, now: u64) {
        if self.live.is_none() {
            self.level = 0;
            self.level_target = 0;
            self.commit(agent);
            // Late capture/socket events after stop must not fill the inbox and
            // prevent the next session's handshake from being received.
            for _ in 0..64 {
                if forge.take_voice_event().is_none() { break; }
            }
            for call in &self.tools {
                agent.tool_registry.cancel_pending_tool(&call.id);
            }
            self.tools.clear();
            return;
        }
        for _ in 0..32 {
            let Some((id, text)) = forge.take_voice_event() else {
                break;
            };
            if self.live.as_ref().map(|l| l.session_id) != Some(id) {
                continue;
            }
            let Ok(value) = serde_json::from_str::<Value>(&text) else {
                self.fail(forge, agent, "Invalid voice bridge event");
                break;
            };
            if value["session"].as_u64() != Some(id) {
                continue;
            }
            match value["event"].as_str() {
                Some("audio_level") => {
                    if matches!(self.state, VoiceState::Listening | VoiceState::Responding) {
                        self.level_target = value["level"].as_u64().unwrap_or(0).min(1000) as u16;
                        self.last_audio = now;
                    }
                }
                Some("playback_active") => {
                    self.playback_active = value["active"].as_bool().unwrap_or(false);
                    if matches!(self.state, VoiceState::Listening | VoiceState::Responding) {
                        self.state = if value["active"].as_bool() == Some(true) { VoiceState::Responding } else { VoiceState::Listening };
                    }
                }
                Some("audio_stats") => {
                    self.audio_stats.captured = value["captured_frames"].as_u64().unwrap_or(0);
                    self.audio_stats.emitted = value["emitted_frames"].as_u64().unwrap_or(0);
                    self.audio_stats.sent = value["sent_frames"].as_u64().unwrap_or(0);
                    self.audio_stats.conversion_errors = value["conversion_errors"].as_u64().unwrap_or(0);
                    self.audio_stats.native_level = value["level"].as_u64().unwrap_or(0).min(1000) as u16;
                }
                Some("progress") => {
                    if let Some(stage) = value["stage"].as_str().and_then(StartupStage::parse) {
                        if self.live.as_ref().is_some_and(|live| !live.is_ready()) && stage > self.startup_stage {
                            self.startup_stage = stage;
                            self.started = now;
                            self.caption = stage.caption().into();
                        }
                    }
                }
                Some("ready") => {
                    self.host_ready = true;
                    if self.startup_stage < StartupStage::AudioReady {
                        self.startup_stage = StartupStage::AudioReady;
                        self.started = now;
                        self.caption = self.startup_stage.caption().into();
                    }
                } // setupComplete, not device readiness, enables input.
                Some("audio") => {
                    if self.state == VoiceState::Muted {
                        continue;
                    }
                    if let Some(pcm) = value["pcm"].as_str().and_then(decode_pcm) {
                        self.audio_stats.received = self.audio_stats.received.saturating_add(1);
                        self.level_target = pcm_level(&pcm);
                        self.last_audio = now;
                        if let Some(live) = self.live.as_mut() {
                            match live.send_audio(id, &pcm) {
                                Ok(()) => self.audio_stats.queued = self.audio_stats.queued.saturating_add(1),
                                Err(_) => self.audio_stats.dropped = self.audio_stats.dropped.saturating_add(1),
                            }
                        }
                    } else { self.audio_stats.dropped = self.audio_stats.dropped.saturating_add(1); }
                }
                Some("message") => {
                    let result = self
                        .live
                        .as_mut()
                        .unwrap()
                        .ingest(id, value["json"].as_str().unwrap_or(""));
                    if result.is_err() {
                        self.fail(forge, agent, "Invalid or overloaded live voice stream");
                        break;
                    }
                }
                Some("error") => {
                    self.fail(
                        forge,
                        agent,
                        value["message"]
                            .as_str()
                            .unwrap_or("Voice connection failed"),
                    );
                    break;
                }
                Some("stopped") => {
                    self.fail(
                        forge,
                        agent,
                        "Voice disconnected. Tap the mic to reconnect.",
                    );
                    break;
                }
                _ => {}
            }
        }
        for _ in 0..64 {
            let Some(event) = self.live.as_mut().and_then(LiveSession::poll_event) else {
                break;
            };
            match event {
                LiveEvent::State(state) => {
                    if state == LiveState::Listening && self.state == VoiceState::Connecting { self.caption.clear(); }
                    self.state = match state {
                        LiveState::Connecting => VoiceState::Connecting,
                        LiveState::Listening if self.direct_audio && self.playback_active => VoiceState::Responding,
                        LiveState::Listening => VoiceState::Listening,
                        LiveState::Responding => VoiceState::Responding,
                        LiveState::Muted => VoiceState::Muted,
                        LiveState::Stopped => VoiceState::Off,
                        LiveState::Error => VoiceState::Error,
                    }
                }
                LiveEvent::InterimInputTranscript { text } => {
                    self.caption = self.input.clone();
                    append_bounded(&mut self.caption, &text);
                }
                LiveEvent::InputTranscript { text, .. } => {
                    self.audio_stats.transcripts = self.audio_stats.transcripts.saturating_add(1);
                    // A new utterance may be transcribed before its interruption event.
                    if !self.output.is_empty() {
                        self.commit(agent);
                    }
                    append_bounded(&mut self.input, &text);
                    self.caption = self.input.clone();
                }
                LiveEvent::OutputTranscript { text, .. } => {
                    self.interrupted_turn = false;
                    append_bounded(&mut self.output, &text);
                    self.caption = self.output.clone();
                }
                LiveEvent::Audio {
                    generation_id, pcm, ..
                } => {
                    if !self.output_muted && self.command(forge,json!({"op":"playback","generation":generation_id,"pcm":encode_pcm(&pcm)})).is_err() {
                        self.fail(forge,agent,"Voice playback queue is full"); break;
                    }
                }
                LiveEvent::Interrupted { generation_id } => {
                    // Native transport clears playback immediately on provider interruption.
                    // Replaying that command after the UART round trip could cancel new audio.
                    if !self.direct_audio && self
                        .command(forge, json!({"op":"interrupt","generation":generation_id}))
                        .is_err()
                    {
                        self.fail(forge, agent, "Could not interrupt voice playback");
                        break;
                    }
                    for call in &self.tools {
                        agent.tool_registry.cancel_pending_tool(&call.id);
                    }
                    self.tools.clear();
                    if !self.output.is_empty() {
                        self.commit(agent);
                    }
                    self.interrupted_turn = true;
                }
                LiveEvent::TurnComplete => {
                    if !self.interrupted_turn || !self.output.is_empty() {
                        self.commit(agent);
                    }
                    self.interrupted_turn = false;
                    self.tool_count = 0;
                    self.seen_tools.clear();
                }
                LiveEvent::ToolCall { id, name, args } => {
                    if self.seen_tools.contains(&id) {
                        continue;
                    }
                    if self.tool_count >= 8 {
                        self.fail(forge, agent, "Voice tool budget exhausted");
                        break;
                    }
                    self.tool_count += 1;
                    self.seen_tools.insert(id.clone());
                    self.tools
                        .push_back(ToolCall::new(id, name, args.to_string()));
                }
                LiveEvent::ToolCancelled { ids } => {
                    for id in &ids {
                        agent.tool_registry.cancel_pending_tool(id);
                    }
                    self.tools.retain(|call| !ids.contains(&call.id))
                }
                LiveEvent::Error { .. } => {
                    self.fail(
                        forge,
                        agent,
                        "Live provider rejected the voice session. Check model and account access.",
                    );
                    break;
                }
                LiveEvent::GoAway => {
                    self.fail(
                        forge,
                        agent,
                        "Voice session expired. Tap the mic to reconnect.",
                    );
                    break;
                }
            }
        }
        if let Some(call) = self.tools.front().cloned() {
            if let Poll::Ready(result) = agent.poll_voice_tool(&call) {
                self.tools.pop_front();
                let response = match result {
                    Ok(output) => {
                        let model_output = match AgentRuntime::tool_result_for_model(&output) {
                            Ok(value) => value,
                            Err(error) => { self.fail(forge,agent,&error.to_string()); return; }
                        };
                        let response = serde_json::from_str(&model_output).unwrap_or(json!({"result":model_output}));
                        if self.tool_outputs.len() >= 8 { self.fail(forge,agent,"Voice UI output queue is full"); return; }
                        self.tool_outputs.push_back(output);
                        response
                    },
                    Err(error) => { self.fail(forge,agent,&error.to_string()); return; },
                };
                self.commit(agent);
                agent.record_voice_tool(&call, &response);
                if let Some(live) = self.live.as_mut() {
                    if live
                        .send_tool_result(live.session_id, &call.id, &call.name, response)
                        .is_err()
                    {
                        self.fail(forge, agent, "Voice tool result queue is full");
                    }
                }
            }
        }
        for _ in 0..if self.host_ready { 16 } else { 0 } {
            let Some(text) = self.live.as_mut().and_then(LiveSession::take_outbound) else {
                break;
            };
            if self
                .command(forge, json!({"op":"send","json":text}))
                .is_err()
            {
                self.fail(forge, agent, "Voice transport queue is full");
                break;
            }
        }
        let startup_timeout = if self.startup_stage == StartupStage::PermissionPending { 180_000_000_000 } else { 20_000_000_000 };
        if self.live.as_ref().is_some_and(|live| !live.is_ready())
            && now.saturating_sub(self.started) > startup_timeout
        {
            let message = self.startup_stage.timeout_message();
            self.fail(
                forge,
                agent,
                message,
            );
        }
        self.update_level(now);
        if self.active() && now.saturating_sub(self.last_diagnostic_update) >= 1_000_000_000 {
            self.last_diagnostic_update = now;
            let stats = &self.audio_stats;
            self.diagnostics = if self.direct_audio {
                alloc::format!("Mic {} · sent {} · text {}",stats.captured,stats.sent,stats.transcripts)
            } else {
                alloc::format!("Mic {} · received {} · sent {} · text {}",stats.captured,stats.received,stats.sent,stats.transcripts)
            };
            if stats.conversion_errors > 0 { self.diagnostics.push_str(" · conversion error"); }
            else if stats.captured > 0 && stats.native_level < 10 { self.diagnostics.push_str(" · quiet input"); }
            forge.queue_voice_diagnostics(&alloc::format!("[voice-input] mic={} emitted={} received={} queued={} sent={} transcripts={} drops={} conversion_errors={} level={}",stats.captured,stats.emitted,stats.received,stats.queued,stats.sent,stats.transcripts,stats.dropped,stats.conversion_errors,stats.native_level));
        }
    }
    fn update_level(&mut self, now: u64) {
        let elapsed = now.saturating_sub(self.last_level_update).min(100_000_000);
        self.last_level_update = now;
        if !matches!(self.state, VoiceState::Listening | VoiceState::Responding) {
            self.level = 0;
            self.level_target = 0;
            return;
        }
        if now.saturating_sub(self.last_audio) > 300_000_000 { self.level_target = 0; }
        let target = self.level_target as i64;
        let current = self.level as i64;
        // Fast attack, slower release. Depend on elapsed time, not frame count.
        let time_constant = if target > current { 8_000_000 } else { 85_000_000 };
        let mut step = (target - current) * elapsed as i64 / (time_constant + elapsed as i64);
        if step == 0 && elapsed > 0 { step = (target - current).signum(); }
        self.level = (current + step).clamp(0,1000) as u16;
        if target == 0 && self.level < 4 { self.level = 0; }
    }
}
#[derive(Default)]
struct AudioDiagnostics {
    captured: u64,
    emitted: u64,
    received: u64,
    queued: u64,
    sent: u64,
    transcripts: u64,
    dropped: u64,
    conversion_errors: u64,
    native_level: u16,
}
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum StartupStage { Opening, PermissionPending, PermissionGranted, AudioStarting, AudioReady, SocketOpen, SetupSent }
impl StartupStage {
    fn parse(stage: &str) -> Option<Self> {
        Some(match stage {
            "permission_pending" => Self::PermissionPending, "permission_granted" => Self::PermissionGranted,
            "audio_starting" => Self::AudioStarting, "audio_ready" => Self::AudioReady,
            "socket_open" => Self::SocketOpen, "setup_sent" => Self::SetupSent,
            _ => return None,
        })
    }
    fn caption(self) -> &'static str {
        match self {
            Self::Opening => "Opening microphone…", Self::PermissionPending => "Allow microphone access in macOS…",
            Self::PermissionGranted | Self::AudioStarting => "Starting microphone…",
            Self::AudioReady => "Connecting to Gemini…", Self::SocketOpen => "Setting up voice model…", Self::SetupSent => "Waiting for Gemini voice…",
        }
    }
    fn timeout_message(self) -> &'static str {
        match self {
            Self::Opening => "Voice host timed out before microphone startup. Restart the QEMU launcher and retry.",
            Self::PermissionPending => "Microphone permission timed out. Allow access in macOS, then tap the mic to retry.",
            Self::PermissionGranted | Self::AudioStarting => "Microphone device startup timed out. Check your input device and retry.",
            Self::AudioReady => "Gemini voice connection timed out. Check network access and retry.",
            Self::SocketOpen | Self::SetupSent => "Gemini voice setup timed out. Check the voice model and API access, then retry.",
        }
    }
}
/// PCM16LE RMS with a modest noise floor and speech gain. No FFT or provider work.
fn pcm_level(pcm: &[u8]) -> u16 {
    let count = pcm.len()/2;
    if count == 0 { return 0; }
    let sum: u64 = pcm.chunks_exact(2).map(|pair| {
        let sample = i16::from_le_bytes([pair[0],pair[1]]) as i64;
        (sample * sample) as u64
    }).sum();
    let rms = (sum / count as u64).isqrt();
    (rms.saturating_sub(160) * 1000 / 6000).min(1000) as u16
}
fn append_bounded(target: &mut String, text: &str) {
    for ch in text.chars() {
        if target.len() + ch.len_utf8() > 16384 {
            break;
        }
        target.push(ch);
    }
}
fn decode_pcm(hex: &str) -> Option<Vec<u8>> {
    if hex.is_empty() || hex.len() > 16384 || hex.len() % 4 != 0 {
        return None;
    }
    hex.as_bytes()
        .chunks_exact(2)
        .map(|pair| {
            let text = core::str::from_utf8(pair).ok()?;
            u8::from_str_radix(text, 16).ok()
        })
        .collect()
}
fn encode_pcm(pcm: &[u8]) -> String {
    use core::fmt::Write;
    let mut text = String::with_capacity(pcm.len() * 2);
    for byte in pcm {
        let _ = write!(text, "{byte:02x}");
    }
    text
}

#[cfg(test)]
#[path = "../../../../tests/support/apps.rs"]
mod fixture_apps;
#[cfg(test)]
mod tests {
    use super::*;
    fn runtime_with_apps() -> AgentRuntime {
        let backend = alloyos_welder::SharedMapVfsBackend::shared();
        backend.borrow_mut().files.extend(super::fixture_apps::files());
        AgentRuntime::with_vfs_backend(backend)
    }
    #[test]
    fn voice_mcp_discovery_execution_results_and_tiles_use_welder() {
        let (mut voice,mut forge,_) = active();
        let mut agent = runtime_with_apps();
        bridge(&mut forge,1,json!({"session":1,"event":"message","json":json!({"toolCall":{"functionCalls":[{"id":"discover","name":"mcp_list_tools","args":{}}]}}).to_string()}));
        voice.poll(&mut forge,&mut agent,0);
        let inventory = voice.take_tool_output().unwrap();
        assert!(inventory.output_json.contains("sample_timer.now"));
        let recorded = agent.history.iter().find(|message|message.tool_call_id.as_deref() == Some("discover")).unwrap();
        assert_eq!(serde_json::from_str::<Value>(&recorded.content).unwrap(),serde_json::from_str::<Value>(&AgentRuntime::tool_result_for_model(&inventory).unwrap()).unwrap());
        let trimmed: Value = serde_json::from_str(&recorded.content).unwrap();
        assert!(trimmed.get("skills").is_none());

        let args = json!({"app":"sample_timer","tool":"sample_timer.now","args":"{}"});
        bridge(&mut forge,1,json!({"session":1,"event":"message","json":json!({"toolCall":{"functionCalls":[{"id":"timer","name":"mcp_call_tool","args":args}]}}).to_string()}));
        voice.poll(&mut forge,&mut agent,16_666_667);
        let output = voice.take_tool_output().unwrap();
        assert_eq!(output.synthesized_view.as_ref().unwrap().tile.as_ref().unwrap().tile_id,"sample_timer.now");
        let mut typed = runtime_with_apps();
        let call = ToolCall::new("typed","mcp_call_tool",args.to_string());
        let expected = typed.tool_registry.execute_tool(&call,&mut typed.ui_synthesizer).unwrap();
        assert_eq!(output.output_json,expected.output_json);
        assert_eq!(output.synthesized_view,expected.synthesized_view);
        assert!(agent.history.iter().any(|message|message.tool_call_id.as_deref() == Some("timer")));

        let live = voice.live.as_mut().unwrap();
        live.take_outbound(); // initial setup
        live.ingest(1,r#"{"setupComplete":{}}"#).unwrap();
        let replies: Vec<Value> = core::iter::from_fn(||live.take_outbound()).map(|json|serde_json::from_str(&json).unwrap()).collect();
        assert!(replies.iter().any(|value|value["toolResponse"]["functionResponses"][0]["id"] == "timer"));
    }
    #[test]
    fn audio_envelope_tracks_volume_and_releases_in_silence() {
        assert_eq!(pcm_level(&[0;64]),0);
        assert_eq!(pcm_level(&[]),0);
        let quiet = [120i16.to_le_bytes();32].concat();
        let speech = [3000i16.to_le_bytes();32].concat();
        let loud = [i16::MIN.to_le_bytes();32].concat();
        assert_eq!(pcm_level(&quiet),0);
        assert!(pcm_level(&speech) > 400);
        assert_eq!(pcm_level(&loud),1000);
        let mut voice = Voice::default();
        voice.state = VoiceState::Listening;
        voice.level_target = pcm_level(&speech);
        voice.last_audio = 100_000_000;
        voice.update_level(100_000_000);
        assert!(voice.level > 0 && voice.level < voice.level_target);
        let attack = voice.level;
        for frame in 1..=70 { voice.update_level(100_000_000 + frame * 16_666_667); }
        assert!(voice.level < attack);
        assert_eq!(voice.level,0);
        voice.level = 500;
        voice.state = VoiceState::Muted;
        voice.update_level(1_400_000_000);
        assert_eq!(voice.level,0);
    }
    #[test]
    fn microphone_frames_drive_level_but_muted_frames_do_not() {
        let (mut voice,mut forge,mut agent) = active();
        voice.live.as_mut().unwrap().ingest(1,r#"{"setupComplete":{}}"#).unwrap();
        voice.poll(&mut forge,&mut agent,16_666_667);
        let speech = [3000i16.to_le_bytes();32].concat();
        bridge(&mut forge,1,json!({"session":1,"event":"audio","pcm":encode_pcm(&speech)}));
        voice.poll(&mut forge,&mut agent,33_333_334);
        assert!(voice.level > 0);
        voice.mute(&mut forge);
        bridge(&mut forge,1,json!({"session":1,"event":"audio","pcm":encode_pcm(&speech)}));
        voice.poll(&mut forge,&mut agent,50_000_001);
        assert_eq!(voice.level,0);
        assert_eq!(voice.level_target,0);
    }
    #[test]
    fn audio_diagnostics_show_each_stage_without_speech_contents() {
        let (mut voice,mut forge,mut agent) = active();
        voice.live.as_mut().unwrap().ingest(1,r#"{"setupComplete":{}}"#).unwrap();
        bridge(&mut forge,1,json!({"event":"audio_stats","session":1,"captured_frames":12,"emitted_frames":12,"sent_frames":11,"conversion_errors":0,"level":500}));
        bridge(&mut forge,1,json!({"event":"audio","session":1,"pcm":encode_pcm(&[3000i16.to_le_bytes();32].concat())}));
        bridge(&mut forge,1,json!({"event":"message","session":1,"json":r#"{"serverContent":{"inputTranscription":{"text":"private utterance"}}}"#}));
        voice.poll(&mut forge,&mut agent,1_000_000_000);
        for stage in ["Mic 12","received 1","sent 11","text 1"] { assert!(voice.diagnostics.contains(stage)); }
        assert_eq!(voice.audio_stats.queued,1);
        assert!(!voice.diagnostics.contains("private utterance"));
        assert_eq!(voice.caption,"private utterance");
    }
    fn bridge(forge: &mut ForgeService, id: u64, event: Value) {
        let frame = alloc::format!(
            "VOICE1\t{id}\t{}\n",
            alloyos_forge::protocol::encode_hex(&event.to_string())
        );
        for byte in frame.bytes() {
            forge.feed(byte);
        }
    }
    fn active() -> (Voice, ForgeService, AgentRuntime) {
        let mut voice = Voice::default();
        voice.live = Some(LiveSession::gemini(1, "fixture-live", "", Vec::new()).unwrap());
        voice.state = VoiceState::Connecting;
        (voice, ForgeService::new(), AgentRuntime::new())
    }
    #[test]
    fn setup_waits_for_host_ready_and_stale_events_are_ignored() {
        let (mut voice, mut forge, mut agent) = active();
        voice.poll(&mut forge, &mut agent, 0);
        assert!(!voice.live.as_ref().unwrap().state.eq(&LiveState::Listening));
        assert!(voice.live.as_mut().unwrap().take_outbound().is_some());
        // Fresh fixture to check actual handshake and late-frame rejection.
        let (mut voice, mut forge, mut agent) = active();
        bridge(
            &mut forge,
            2,
            json!({"session":2,"event":"message","json":"{\"setupComplete\":{}}"}),
        );
        voice.poll(&mut forge, &mut agent, 0);
        assert_eq!(voice.state, VoiceState::Connecting);
        bridge(&mut forge, 1, json!({"session":1,"event":"ready"}));
        voice.poll(&mut forge, &mut agent, 0);
        assert!(voice.live.as_mut().unwrap().take_outbound().is_none());
        bridge(
            &mut forge,
            1,
            json!({"session":1,"event":"message","json":"{\"setupComplete\":{}}"}),
        );
        voice.poll(&mut forge, &mut agent, 0);
        assert_eq!(voice.state, VoiceState::Listening);
    }
    #[test]
    fn direct_audio_levels_and_playback_survive_provider_turn_completion() {
        let (mut voice, mut forge, mut agent) = active();
        voice.direct_audio = true;
        bridge(&mut forge,1,json!({"session":1,"event":"message","json":r#"{"setupComplete":{}}"#}));
        voice.poll(&mut forge,&mut agent,0);
        bridge(&mut forge,1,json!({"session":1,"event":"audio_level","level":650}));
        bridge(&mut forge,1,json!({"session":1,"event":"playback_active","active":true}));
        bridge(&mut forge,1,json!({"session":1,"event":"message","json":r#"{"serverContent":{"turnComplete":true}}"#}));
        voice.poll(&mut forge,&mut agent,50_000_000);
        assert_eq!(voice.state,VoiceState::Responding);
        assert!(voice.level>0);
        assert_eq!(voice.audio_stats.received,0); // No UART PCM is needed for levels.
        bridge(&mut forge,1,json!({"session":1,"event":"playback_active","active":false}));
        voice.poll(&mut forge,&mut agent,100_000_000);
        assert_eq!(voice.state,VoiceState::Listening);
        voice.mute(&mut forge);
        bridge(&mut forge,1,json!({"session":1,"event":"audio_level","level":900}));
        bridge(&mut forge,1,json!({"session":1,"event":"playback_active","active":true}));
        voice.poll(&mut forge,&mut agent,150_000_000);
        assert_eq!(voice.state,VoiceState::Muted);
        assert_eq!(voice.level,0);
    }
    #[test]
    fn permission_wait_gets_its_own_deadline_and_duplicate_progress_cannot_extend_it() {
        let (mut voice,mut forge,mut agent) = active();
        bridge(&mut forge,1,json!({"session":1,"event":"progress","stage":"permission_pending"}));
        voice.poll(&mut forge,&mut agent,1_000_000_000);
        voice.poll(&mut forge,&mut agent,30_000_000_000);
        assert!(voice.active());
        assert!(voice.caption.contains("Allow microphone"));
        bridge(&mut forge,1,json!({"session":1,"event":"progress","stage":"permission_pending"}));
        voice.poll(&mut forge,&mut agent,182_000_000_000);
        assert!(!voice.active());
        assert!(voice.error.contains("permission timed out"));
    }
    #[test]
    fn startup_reports_provider_timeout_after_audio_and_socket_progress() {
        let (mut voice,mut forge,mut agent) = active();
        for (second,stage) in [(1,"permission_pending"),(25,"permission_granted"),(26,"audio_starting"),(27,"audio_ready"),(28,"socket_open"),(29,"setup_sent")] {
            bridge(&mut forge,1,json!({"session":1,"event":"progress","stage":stage}));
            voice.poll(&mut forge,&mut agent,second*1_000_000_000);
            assert!(voice.active());
        }
        assert_eq!(voice.caption,"Waiting for Gemini voice…");
        voice.poll(&mut forge,&mut agent,50_000_000_000);
        assert!(!voice.active());
        assert!(voice.error.contains("Gemini voice setup timed out"));
    }
    #[test]
    fn keyboard_mute_text_history_and_explicit_mic_resume() {
        let (mut voice, mut forge, mut agent) = active();
        voice.live.as_mut().unwrap().take_outbound();
        voice
            .live
            .as_mut()
            .unwrap()
            .ingest(1, r#"{"setupComplete":{}}"#)
            .unwrap();
        voice.poll(&mut forge, &mut agent, 0);
        voice.mute(&mut forge);
        assert_eq!(voice.state, VoiceState::Muted);
        voice.send_text(" typed question ").unwrap();
        assert_eq!(voice.caption, "typed question");
        bridge(
            &mut forge,
            1,
            json!({"session":1,"event":"message","json":r#"{"serverContent":{"outputTranscription":{"text":"reply"},"turnComplete":true}}"#}),
        );
        voice.poll(&mut forge, &mut agent, 0);
        assert_eq!(
            voice.take_completed(),
            Some(("typed question".into(), "reply".into()))
        );
        assert_eq!(voice.state, VoiceState::Muted);
        voice.toggle(&mut forge, &mut agent, false, 1);
        assert!(voice.active());
        assert_eq!(voice.state, VoiceState::Listening);
        voice.toggle(&mut forge, &mut agent, false, 2);
        assert!(!voice.active());
    }
    #[test]
    fn captures_are_bounded_and_failed_text_does_not_enter_history() {
        assert!(decode_pcm("0000").is_some());
        for bad in ["0", "000", "zzzz", ""] {
            assert!(decode_pcm(bad).is_none());
        }
        assert!(decode_pcm(&"00".repeat(8194)).is_none());
        let (mut voice, _, _) = active();
        assert!(voice.send_text("before setup").is_err());
        assert!(voice.input.is_empty());
        assert!(!voice.error.is_empty());
    }
    #[test]
    fn provisional_captions_do_not_duplicate_final_transcripts() {
        let (mut voice, mut forge, mut agent) = active();
        bridge(
            &mut forge,
            1,
            json!({"session":1,"event":"message","json":r#"{"serverContent":{"interimInputTranscription":{"text":"hello wor"}}}"#}),
        );
        voice.poll(&mut forge, &mut agent, 0);
        assert_eq!(voice.caption, "hello wor");
        assert!(voice.input.is_empty());
        bridge(
            &mut forge,
            1,
            json!({"session":1,"event":"message","json":r#"{"serverContent":{"inputTranscription":{"text":"hello world","finished":true},"outputTranscription":{"text":"Hi"},"turnComplete":true}}"#}),
        );
        voice.poll(&mut forge, &mut agent, 0);
        assert_eq!(
            voice.take_completed(),
            Some(("hello world".into(), "Hi".into()))
        );
        assert!(voice.caption.is_empty(), "completed chat replaces the live caption");
    }
    #[test]
    fn cancelled_and_interrupted_tools_are_never_executed() {
        let (mut voice, mut forge, mut agent) = active();
        bridge(
            &mut forge,
            1,
            json!({"session":1,"event":"message","json":r#"{"toolCall":{"functionCalls":[{"id":"c1","name":"file_write","args":{"path":"/cancelled","content":"no"}}]},"toolCallCancellation":{"ids":["c1"]}}"#}),
        );
        voice.poll(&mut forge, &mut agent, 0);
        assert!(voice.tools.is_empty());
        assert!(!agent
            .history
            .iter()
            .any(|m| m.tool_call_id.as_deref() == Some("c1")));
        voice.tools.push_back(ToolCall::new(
            "c2",
            "file_write",
            r#"{"path":"/cancelled","content":"no"}"#,
        ));
        bridge(
            &mut forge,
            1,
            json!({"session":1,"event":"message","json":r#"{"serverContent":{"interrupted":true}}"#}),
        );
        voice.poll(&mut forge, &mut agent, 0);
        assert!(voice.tools.is_empty());
    }
    #[test]
    fn muting_during_setup_does_not_disable_connection_timeout() {
        let (mut voice, mut forge, mut agent) = active();
        voice.mute(&mut forge);
        voice.poll(&mut forge, &mut agent, 21_000_000_000);
        assert!(!voice.active());
        assert_eq!(voice.state, VoiceState::Error);
        assert!(voice.error.contains("timed out"));
    }
}
