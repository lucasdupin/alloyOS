//! Bounded, transport-independent Gemini Live protocol adapter.
//! The caller owns the authenticated WebSocket and must close it on `stop`.
use crate::ForgeError;
use alloc::{
    collections::VecDeque,
    string::{String, ToString},
    vec::Vec,
};
use serde_json::{json, Value};

pub const GEMINI_LIVE_ENDPOINT: &str = "wss://generativelanguage.googleapis.com/ws/google.ai.generativelanguage.v1beta.GenerativeService.BidiGenerateContent";
// Official WebSocket quickstart model, checked 2026-10-14. Override via caller configuration.
// https://ai.google.dev/gemini-api/docs/live-api/get-started-websocket
pub const DEFAULT_LIVE_MODEL: &str = "gemini-3.8-live";
pub const MAX_INPUT_AUDIO_BYTES: usize = 8192;
pub const MAX_OUTPUT_AUDIO_BYTES: usize = 32768;
pub const MAX_LIVE_MESSAGE_BYTES: usize = 131072;
const QUEUE_LIMIT: usize = 64;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LiveState {
    Connecting,
    Listening,
    Responding,
    Muted,
    Stopped,
    Error,
}
#[derive(Debug, Clone, PartialEq)]
pub enum LiveEvent {
    State(LiveState),
    InterimInputTranscript {
        text: String,
    },
    InputTranscript {
        text: String,
        finished: bool,
    },
    OutputTranscript {
        text: String,
        finished: bool,
    },
    Audio {
        generation_id: u64,
        pcm: Vec<u8>,
        sample_rate: u32,
    },
    Interrupted {
        generation_id: u64,
    },
    ToolCall {
        id: String,
        name: String,
        args: Value,
    },
    ToolCancelled {
        ids: Vec<String>,
    },
    TurnComplete,
    Error {
        message: String,
    },
    GoAway,
}

pub struct LiveSession {
    pub session_id: u64,
    pub generation_id: u64,
    pub state: LiveState,
    muted: bool,
    ready: bool,
    setup_sent: bool,
    outbound: VecDeque<String>,
    events: VecDeque<LiveEvent>,
}
impl LiveSession {
    /// `tools` contains Gemini Tool objects (`functionDeclarations`), never credentials.
    pub fn gemini(
        session_id: u64,
        model: &str,
        instruction: &str,
        tools: Vec<Value>,
    ) -> Result<Self, ForgeError> {
        if session_id == 0
            || model.is_empty()
            || model.len() > 128
            || !model
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"-._/".contains(&b))
            || instruction.len() > 32768
        {
            return Err(ForgeError::InvalidRequest);
        }
        let model = if model.starts_with("models/") {
            model.to_string()
        } else {
            alloc::format!("models/{model}")
        };
        let setup = json!({"setup":{"model":model,"generationConfig":{"responseModalities":["AUDIO"]},"systemInstruction":{"parts":[{"text":instruction}]},"tools":tools,"inputAudioTranscription":{},"outputAudioTranscription":{}}});
        let mut session = Self {
            session_id,
            generation_id: 1,
            state: LiveState::Connecting,
            muted: false,
            ready: false,
            setup_sent: false,
            outbound: VecDeque::new(),
            events: VecDeque::new(),
        };
        session.enqueue(setup)?;
        session
            .events
            .push_back(LiveEvent::State(LiveState::Connecting));
        Ok(session)
    }
    fn active(&self, id: u64) -> Result<(), ForgeError> {
        if id != self.session_id || matches!(self.state, LiveState::Stopped | LiveState::Error) {
            Err(ForgeError::InvalidRequest)
        } else {
            Ok(())
        }
    }
    fn enqueue(&mut self, message: Value) -> Result<(), ForgeError> {
        if self.outbound.len() >= QUEUE_LIMIT {
            return Err(ForgeError::Busy);
        }
        let text = message.to_string();
        if text.len() > MAX_LIVE_MESSAGE_BYTES {
            return Err(ForgeError::InvalidRequest);
        }
        self.outbound.push_back(text);
        Ok(())
    }
    fn event(&mut self, event: LiveEvent) -> Result<(), ForgeError> {
        if self.events.len() >= QUEUE_LIMIT {
            return Err(ForgeError::Busy);
        }
        self.events.push_back(event);
        Ok(())
    }
    fn state(&mut self, state: LiveState) -> Result<(), ForgeError> {
        self.event(LiveEvent::State(state))?;
        self.state = state;
        Ok(())
    }
    pub fn is_ready(&self) -> bool {
        self.ready
    }
    pub fn take_outbound(&mut self) -> Option<String> {
        if !self.ready && self.setup_sent {
            return None;
        }
        let message = self.outbound.pop_front()?;
        self.setup_sent = true;
        Some(message)
    }
    pub fn poll_event(&mut self) -> Option<LiveEvent> {
        self.events.pop_front()
    }
    /// History consists of Gemini Content objects with role and parts.
    pub fn seed_conversation(&mut self, id: u64, turns: Vec<Value>) -> Result<(), ForgeError> {
        self.active(id)?;
        self.enqueue(json!({"clientContent":{"turns":turns,"turnComplete":false}}))
    }
    pub fn send_audio(&mut self, id: u64, pcm: &[u8]) -> Result<(), ForgeError> {
        self.active(id)?;
        if !self.ready || self.muted {
            return Err(ForgeError::Busy);
        }
        if pcm.is_empty() || pcm.len() > MAX_INPUT_AUDIO_BYTES || pcm.len() % 2 != 0 {
            return Err(ForgeError::InvalidRequest);
        }
        self.enqueue(json!({"realtimeInput":{"audio":{"mimeType":"audio/pcm;rate=16000","data":encode_base64(pcm)}}}))
    }
    pub fn send_text(&mut self, id: u64, text: &str) -> Result<(), ForgeError> {
        self.active(id)?;
        if !self.ready || text.is_empty() || text.len() > 16384 {
            return Err(ForgeError::InvalidRequest);
        }
        self.enqueue(json!({"realtimeInput":{"text":text}}))
    }
    pub fn set_muted(&mut self, id: u64, muted: bool) -> Result<(), ForgeError> {
        self.active(id)?;
        if muted && !self.muted {
            // Captured frames queued before the mute must never leave after the mute.
            self.outbound
                .retain(|wire| !wire.contains("\"mimeType\":\"audio/pcm;rate=16000\""));
            self.enqueue(json!({"realtimeInput":{"audioStreamEnd":true}}))?;
        }
        self.muted = muted;
        self.state(if muted {
            LiveState::Muted
        } else if self.ready {
            LiveState::Listening
        } else {
            LiveState::Connecting
        })
    }
    pub fn send_tool_result(
        &mut self,
        id: u64,
        call_id: &str,
        name: &str,
        response: Value,
    ) -> Result<(), ForgeError> {
        self.active(id)?;
        self.enqueue(json!({"toolResponse":{"functionResponses":[{"id":call_id,"name":name,"response":response}]}}))
    }
    pub fn stop(&mut self, id: u64) -> Result<(), ForgeError> {
        self.active(id)?;
        self.outbound.clear();
        self.events.clear();
        self.ready = false;
        self.muted = true;
        self.generation_id = self
            .generation_id
            .checked_add(1)
            .ok_or(ForgeError::InvalidResponse)?;
        self.state(LiveState::Stopped)
    }
    pub fn ingest(&mut self, id: u64, text: &str) -> Result<(), ForgeError> {
        self.active(id)?;
        if text.len() > MAX_LIVE_MESSAGE_BYTES {
            return Err(ForgeError::InvalidResponse);
        }
        let value: Value = serde_json::from_str(text).map_err(|_| ForgeError::InvalidResponse)?;
        // Process transactionally: backpressure or malformed frames cannot partially advance state.
        let checkpoint = (
            self.events.clone(),
            self.outbound.clone(),
            self.state,
            self.generation_id,
            self.ready,
        );
        match self.ingest_value(&value) {
            Ok(()) => Ok(()),
            Err(error) => {
                (
                    self.events,
                    self.outbound,
                    self.state,
                    self.generation_id,
                    self.ready,
                ) = checkpoint;
                Err(error)
            }
        }
    }
    fn ingest_value(&mut self, v: &Value) -> Result<(), ForgeError> {
        if !v.is_object() {
            return Err(ForgeError::InvalidResponse);
        }
        if let Some(error) = v.get("error") {
            self.outbound.clear();
            self.ready = false;
            self.event(LiveEvent::Error {
                message: error
                    .get("message")
                    .and_then(Value::as_str)
                    .unwrap_or("Live provider error")
                    .chars()
                    .take(1024)
                    .collect(),
            })?;
            return self.state(LiveState::Error);
        }
        if v.get("setupComplete").is_some() {
            self.ready = true;
            self.state(if self.muted {
                LiveState::Muted
            } else {
                LiveState::Listening
            })?;
        }
        if v.get("goAway").is_some() {
            self.event(LiveEvent::GoAway)?;
        }
        if let Some(content) = v.get("serverContent") {
            if content.get("interrupted").and_then(Value::as_bool) == Some(true) {
                self.generation_id = self
                    .generation_id
                    .checked_add(1)
                    .ok_or(ForgeError::InvalidResponse)?;
                self.events
                    .retain(|e| !matches!(e, LiveEvent::Audio { .. }));
                self.event(LiveEvent::Interrupted {
                    generation_id: self.generation_id,
                })?;
            }
            for (field, input) in [
                ("inputTranscription", true),
                ("interimInputTranscription", true),
                ("outputTranscription", false),
            ] {
                if let Some(transcript) = content.get(field) {
                    let text = transcript
                        .get("text")
                        .and_then(Value::as_str)
                        .ok_or(ForgeError::InvalidResponse)?
                        .to_string();
                    if text.len() > 16384 {
                        return Err(ForgeError::InvalidResponse);
                    }
                    let finished = transcript
                        .get("finished")
                        .and_then(Value::as_bool)
                        .unwrap_or(false);
                    self.event(if field == "interimInputTranscription" {
                        LiveEvent::InterimInputTranscript { text }
                    } else if input {
                        LiveEvent::InputTranscript { text, finished }
                    } else {
                        LiveEvent::OutputTranscript { text, finished }
                    })?;
                }
            }
            if let Some(parts) = content
                .pointer("/modelTurn/parts")
                .and_then(Value::as_array)
            {
                for part in parts {
                    if let Some(blob) = part.get("inlineData") {
                        let mime = blob
                            .get("mimeType")
                            .and_then(Value::as_str)
                            .ok_or(ForgeError::InvalidResponse)?;
                        if !matches!(
                            mime,
                            "audio/pcm;rate=24000" | "audio/pcm;rate=24000;channels=1"
                        ) {
                            return Err(ForgeError::InvalidResponse);
                        }
                        let pcm = decode_base64(
                            blob.get("data")
                                .and_then(Value::as_str)
                                .ok_or(ForgeError::InvalidResponse)?,
                            MAX_OUTPUT_AUDIO_BYTES,
                        )?;
                        if pcm.is_empty() || pcm.len() % 2 != 0 {
                            return Err(ForgeError::InvalidResponse);
                        }
                        self.state(if self.muted {
                            LiveState::Muted
                        } else {
                            LiveState::Responding
                        })?;
                        self.event(LiveEvent::Audio {
                            generation_id: self.generation_id,
                            pcm,
                            sample_rate: 24000,
                        })?;
                    }
                }
            }
            if content.get("turnComplete").and_then(Value::as_bool) == Some(true) {
                self.event(LiveEvent::TurnComplete)?;
                self.generation_id = self
                    .generation_id
                    .checked_add(1)
                    .ok_or(ForgeError::InvalidResponse)?;
                self.state(if self.muted {
                    LiveState::Muted
                } else {
                    LiveState::Listening
                })?;
            }
        }
        if let Some(calls) = v
            .pointer("/toolCall/functionCalls")
            .and_then(Value::as_array)
        {
            for call in calls {
                let id = call
                    .get("id")
                    .and_then(Value::as_str)
                    .ok_or(ForgeError::InvalidResponse)?;
                let name = call
                    .get("name")
                    .and_then(Value::as_str)
                    .ok_or(ForgeError::InvalidResponse)?;
                if id.len() > 256 || name.len() > 256 {
                    return Err(ForgeError::InvalidResponse);
                }
                self.event(LiveEvent::ToolCall {
                    id: id.into(),
                    name: name.into(),
                    args: call.get("args").cloned().unwrap_or(json!({})),
                })?;
            }
        }
        if let Some(ids) = v
            .pointer("/toolCallCancellation/ids")
            .and_then(Value::as_array)
        {
            let ids = ids
                .iter()
                .map(|id| {
                    id.as_str()
                        .filter(|s| s.len() <= 256)
                        .map(String::from)
                        .ok_or(ForgeError::InvalidResponse)
                })
                .collect::<Result<Vec<_>, _>>()?;
            self.event(LiveEvent::ToolCancelled { ids })?;
        }
        Ok(())
    }
}

pub fn encode_base64(bytes: &[u8]) -> String {
    const ABC: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut result = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let n = ((chunk[0] as u32) << 16)
            | ((chunk.get(1).copied().unwrap_or(0) as u32) << 8)
            | chunk.get(2).copied().unwrap_or(0) as u32;
        result.push(ABC[(n >> 18) as usize] as char);
        result.push(ABC[((n >> 12) & 63) as usize] as char);
        result.push(if chunk.len() > 1 {
            ABC[((n >> 6) & 63) as usize] as char
        } else {
            '='
        });
        result.push(if chunk.len() > 2 {
            ABC[(n & 63) as usize] as char
        } else {
            '='
        });
    }
    result
}
pub fn decode_base64(text: &str, limit: usize) -> Result<Vec<u8>, ForgeError> {
    if text.len() % 4 != 0 || text.len() / 4 * 3 > limit.saturating_add(2) {
        return Err(ForgeError::InvalidResponse);
    }
    let mut bytes = Vec::new();
    for (index, chunk) in text.as_bytes().chunks(4).enumerate() {
        let mut n = 0u32;
        let mut padding = 0;
        for (i, &b) in chunk.iter().enumerate() {
            let value = match b {
                b'A'..=b'Z' => b - b'A',
                b'a'..=b'z' => b - b'a' + 26,
                b'0'..=b'9' => b - b'0' + 52,
                b'+' => 62,
                b'/' => 63,
                b'=' if i >= 2 && index + 1 == text.len() / 4 => {
                    padding += 1;
                    0
                }
                _ => return Err(ForgeError::InvalidResponse),
            };
            if padding > 0 && b != b'=' {
                return Err(ForgeError::InvalidResponse);
            }
            n = (n << 6) | value as u32;
        }
        if padding > 2 || (padding == 2 && n & 0xffff != 0) || (padding == 1 && n & 0xff != 0) {
            return Err(ForgeError::InvalidResponse);
        }
        bytes.push((n >> 16) as u8);
        if padding < 2 {
            bytes.push((n >> 8) as u8);
        }
        if padding == 0 {
            bytes.push(n as u8);
        }
    }
    if bytes.len() > limit {
        return Err(ForgeError::InvalidResponse);
    }
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn session() -> LiveSession {
        let mut s = LiveSession::gemini(1, "test-live", "hello", Vec::new()).unwrap();
        s.take_outbound();
        s.poll_event();
        s.ingest(1, r#"{"setupComplete":{}}"#).unwrap();
        s.poll_event();
        s
    }
    #[test]
    fn setup_and_pcm_wire_format() {
        let mut s = LiveSession::gemini(1, "test-live", "hello", Vec::new()).unwrap();
        let wire: Value = serde_json::from_str(&s.take_outbound().unwrap()).unwrap();
        assert_eq!(wire["setup"]["model"], "models/test-live");
        assert_eq!(
            wire["setup"]["generationConfig"]["responseModalities"][0],
            "AUDIO"
        );
        assert_eq!(s.send_audio(1, &[0, 0]), Err(ForgeError::Busy));
        s.ingest(1, r#"{"setupComplete":{}}"#).unwrap();
        s.send_audio(1, &[0, 128, 255, 127]).unwrap();
        let wire: Value = serde_json::from_str(&s.take_outbound().unwrap()).unwrap();
        assert_eq!(wire["realtimeInput"]["audio"]["data"], "AID/fw==");
    }
    #[test]
    fn interruption_purges_queued_audio_and_advances_generation() {
        let mut s = session();
        s.ingest(1, r#"{"serverContent":{"modelTurn":{"parts":[{"inlineData":{"mimeType":"audio/pcm;rate=24000","data":"AAA="}}]}}}"#).unwrap();
        s.ingest(1, r#"{"serverContent":{"interrupted":true}}"#)
            .unwrap();
        assert_eq!(s.generation_id, 2);
        while let Some(event) = s.poll_event() {
            assert!(!matches!(event, LiveEvent::Audio { .. }));
        }
        s.stop(1).unwrap();
        assert_eq!(
            s.ingest(1, r#"{"setupComplete":{}}"#),
            Err(ForgeError::InvalidRequest)
        );
        assert!(s.take_outbound().is_none());
    }
    #[test]
    fn transcript_tools_and_mute() {
        let mut s = session();
        s.ingest(1,r#"{"serverContent":{"inputTranscription":{"text":"open app","finished":true},"outputTranscription":{"text":"Opening"}},"toolCall":{"functionCalls":[{"id":"c1","name":"open_app","args":{"id":"notes"}}]}}"#).unwrap();
        assert!(matches!(
            s.poll_event(),
            Some(LiveEvent::InputTranscript { finished: true, .. })
        ));
        assert!(matches!(
            s.poll_event(),
            Some(LiveEvent::OutputTranscript { .. })
        ));
        assert!(matches!(s.poll_event(), Some(LiveEvent::ToolCall { .. })));
        s.send_tool_result(1, "c1", "open_app", json!({"ok":true}))
            .unwrap();
        let wire: Value = serde_json::from_str(&s.take_outbound().unwrap()).unwrap();
        assert_eq!(wire["toolResponse"]["functionResponses"][0]["id"], "c1");
        s.set_muted(1, true).unwrap();
        assert_eq!(s.send_audio(1, &[0, 0]), Err(ForgeError::Busy));
        assert_eq!(s.state, LiveState::Muted);
        assert!(s.take_outbound().unwrap().contains("audioStreamEnd"));
    }
    #[test]
    fn malformed_and_backpressure_are_transactional() {
        let mut s = session();
        assert_eq!(
            s.ingest(2, r#"{"setupComplete":{}}"#),
            Err(ForgeError::InvalidRequest)
        );
        assert!(s.ingest(1,r#"{"serverContent":{"interrupted":true,"modelTurn":{"parts":[{"inlineData":{"mimeType":"audio/wav","data":"AAA="}}]}}}"#).is_err());
        assert_eq!(s.generation_id, 1);
        assert!(s.poll_event().is_none());
        for _ in 0..QUEUE_LIMIT {
            s.send_audio(1, &[0, 0]).unwrap();
        }
        assert_eq!(s.send_audio(1, &[0, 0]), Err(ForgeError::Busy));
    }
    #[test]
    fn history_waits_for_setup_and_muting_discards_pending_capture() {
        let mut s = LiveSession::gemini(1, "test-live", "", Vec::new()).unwrap();
        s.seed_conversation(
            1,
            alloc::vec![json!({"role":"user","parts":[{"text":"prior"}]})],
        )
        .unwrap();
        assert!(s.take_outbound().unwrap().contains("setup"));
        assert!(s.take_outbound().is_none());
        s.ingest(1, r#"{"setupComplete":{}}"#).unwrap();
        assert!(s.take_outbound().unwrap().contains("clientContent"));
        s.send_audio(1, &[0, 0]).unwrap();
        s.set_muted(1, true).unwrap();
        assert!(s.take_outbound().unwrap().contains("audioStreamEnd"));
        assert!(s.take_outbound().is_none());
    }
    #[test]
    fn complete_cancel_and_provider_error_lifecycle() {
        let mut s = session();
        s.ingest(
            1,
            r#"{"serverContent":{"turnComplete":true},"toolCallCancellation":{"ids":["c1"]}}"#,
        )
        .unwrap();
        assert_eq!(s.generation_id, 2);
        assert_eq!(s.poll_event(), Some(LiveEvent::TurnComplete));
        assert_eq!(s.poll_event(), Some(LiveEvent::State(LiveState::Listening)));
        assert_eq!(
            s.poll_event(),
            Some(LiveEvent::ToolCancelled {
                ids: alloc::vec!["c1".into()]
            })
        );
        s.send_audio(1, &[0, 0]).unwrap();
        s.ingest(1, r#"{"error":{"message":"Session expired"}}"#)
            .unwrap();
        assert_eq!(s.state, LiveState::Error);
        assert!(s.take_outbound().is_none());
        assert!(s.send_audio(1, &[0, 0]).is_err());
    }
    #[test]
    fn base64_roundtrips_and_rejects_invalid_padding() {
        for n in 0..128 {
            let bytes: Vec<_> = (0..n).map(|i| i as u8).collect();
            assert_eq!(decode_base64(&encode_base64(&bytes), 128).unwrap(), bytes);
        }
        for text in ["A===", "AA=A", "AB==", "AAF=", "!!!!", "AAA"] {
            assert!(decode_base64(text, 128).is_err(), "{text}");
        }
        assert!(decode_base64("AAAA", 2).is_err());
    }
}
