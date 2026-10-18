//! Bounded display IPC schema. Authoritative state belongs to the host.
use crate::{AppTile, TileAction};
use alloc::{string::String, vec::Vec};
use serde::{Deserialize, Serialize};
pub const MAX_STATE: usize = 512 * 1024;
pub const DISPLAY_INFO: u64 = 130;
pub const DISPLAY_INPUT: u64 = 131;
pub const DISPLAY_PRESENT: u64 = 132;
pub const DISPLAY_STATE: u64 = 133;
pub const DISPLAY_EVENT: u64 = 134;
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ChatItem {
    Text(String),
    Tile(AppTile),
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum TileEvent {
    Action {
        tile_id: String,
        action: TileAction,
    },
    Refresh {
        tile_id: String,
        resource_uri: String,
    },
}
/// Structured setup question rendered as conversation, separate from app widgets.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SetupView {
    pub revision: u64,
    pub title: String,
    pub prompt: String,
    pub progress: String,
    pub field_label: String,
    pub field_value: String,
    pub field_prefix: String,
    pub field_placeholder: String,
    pub choices: Vec<SetupChoice>,
    pub rows: Vec<SetupRow>,
    pub can_back: bool,
    pub can_defer: bool,
    pub error: String,
}
impl SetupView {
    pub fn chat_text(&self) -> alloc::string::String {
        let mut text = self.prompt.clone();
        if !self.error.is_empty() {
            text.push_str("\n\n");
            text.push_str(&self.error);
        }
        text
    }
}
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SetupChoice { pub value: String, pub label: String }
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SetupRow { pub title: String, pub detail: String }
impl SetupRow {
    pub fn new(_id: impl Into<String>,title: impl Into<String>, detail: impl Into<String>,_badge: impl Into<String>,_status: impl Into<String>)->Self {
        Self {title:title.into(), detail:detail.into()}
    }
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SetupAnswer { pub revision: u64, pub value: String }
#[derive(Default, Clone, PartialEq, Serialize, Deserialize)]
pub struct State {
    #[serde(default)]
    pub pending_remote_install: Option<crate::install::PendingRemoteInstall>,
    #[serde(default)]
    pub output_muted: bool,
    #[serde(default)]
    pub voice_state: VoiceState,
    /// Smoothed microphone RMS envelope, normalized to 0..=1000.
    #[serde(default)]
    pub voice_level: u16,
    #[serde(default)]
    pub voice_diagnostics: String,
    #[serde(default)]
    pub voice_caption: String,
    #[serde(default)]
    pub voice_error: String,
    #[serde(default)]
    pub tile_catalog_loaded: bool,
    #[serde(default)]
    pub tile_catalog_error: String,
    pub fault_probe: bool,
    pub booting: bool,
    pub pending: bool,
    pub model: String,
    pub model_status: String,
    pub transcript: String,
    pub items: Vec<ChatItem>,
    pub available: Vec<AppTile>,
    pub tiles: Vec<AppTile>,
    pub pinned: Vec<String>,
    pub partial: String,
    pub revision: u64,
}
#[derive(Serialize, Deserialize)]
pub enum Event {
    RemoteInstallDecision { id: String, approved: bool },
    DiscoverTiles,
    OutputMuteToggle,
    VoiceToggle,
    VoiceMute,
    Submit(String),
    CycleModel,
    Dashboard(Vec<String>),
    Tile(TileEvent),
    Ready,
    Applied { revision: u64, digest: u64 },
}

#[derive(Default, Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum VoiceState {
    #[default]
    Off,
    Connecting,
    Listening,
    Responding,
    Muted,
    Error,
}

/// Stable digest of actual applied conversation and pin order for recovery ACKs.
pub fn state_digest(transcript: &str, pins: &[String]) -> u64 {
    let mut hash = 0xcbf29ce484222325u64;
    for byte in transcript.bytes().chain(core::iter::once(0)).chain(
        pins.iter()
            .flat_map(|p| p.bytes().chain(core::iter::once(0))),
    ) {
        hash = (hash ^ byte as u64).wrapping_mul(0x100000001b3);
    }
    hash
}
