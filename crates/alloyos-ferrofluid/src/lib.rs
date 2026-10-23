#![cfg_attr(target_os = "none", no_std)]

#[cfg(not(target_os = "none"))]
extern crate std;

extern crate alloc;

pub mod auth;
pub mod dashboard;
pub use dashboard::{DashboardState, TileEvent};
pub mod system_ui;
pub mod webview;

pub use auth::{AgentUiToken, CallerPrincipal, FerrofluidError, SecurityGate};
pub use system_ui::{ChatItem, FerrofluidSystemUi};
mod boot_liquid;

mod sidebar;

mod markdown;
mod tile_images;
