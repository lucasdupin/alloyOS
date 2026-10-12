#![cfg_attr(not(test), no_std)]

#[cfg(test)]
extern crate std;

extern crate alloc;

pub mod auth;
pub mod compositor;
pub mod font;
pub mod framebuffer;
pub mod ipc;
pub mod theme;
pub mod widgets;

pub use auth::{AgentUiToken, CallerPrincipal, MercuryError, SecurityGate};
pub use compositor::{MercuryServer, MercurySurface};
pub use font::{
    draw_char, draw_char_styled, draw_cursor, draw_markdown, draw_str, draw_str_styled,
    glyph_bitmap, CHAR_ADVANCE_X, CHAR_LINE_HEIGHT,
};
pub use framebuffer::MercuryFramebuffer;
pub use ipc::{InputMultiplexer, MercuryIpcChannel, MercuryIpcCommand};
pub use theme::{
    blend_argb, liquid_mercury_border_color, ACCENT_LIQUID_SILVER, BG_OBSIDIAN, BORDER_CHROME,
    BORDER_SILVER_DIM, SURFACE_CARBON, TEXT_MERCURY_BRIGHT,
};
pub use widgets::{AiConsoleView, MercuryWidget, MODEL_BADGE_BAR};
