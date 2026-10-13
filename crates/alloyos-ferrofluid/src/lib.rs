#![cfg_attr(not(test), no_std)]

#[cfg(test)]
extern crate std;

extern crate alloc;

pub mod auth;
pub mod dashboard;
pub use dashboard::{DashboardState, TileEvent};
pub mod system_ui;

pub use auth::{AgentUiToken, CallerPrincipal, FerrofluidError, SecurityGate};
pub use system_ui::{ChatItem, FerrofluidSystemUi};
mod boot_liquid;

mod sidebar;
