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
#[derive(Default, Clone, PartialEq, Serialize, Deserialize)]
pub struct State {
    #[serde(default)]
    pub voice_state: VoiceState,
    /// Smoothed microphone RMS envelope, normalized to 0..=1000.
    #[serde(default)]
    pub voice_level: u16,
    #[serde(default)]
    pub voice_caption: String,
    #[serde(default)]
    pub voice_error: String,
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
