use alloc::string::{String, ToString};
use alloc::vec::Vec;
use metalos_proto::{DirtyRect, InputEvent};

use crate::auth::{AgentUiToken, CallerPrincipal, FerrofluidError, SecurityGate};
use crate::framebuffer::{rect_union, FerrofluidFramebuffer};
use crate::ipc::{FerrofluidIpcChannel, FerrofluidIpcCommand, InputMultiplexer};
use crate::theme::BG_OBSIDIAN;
use crate::widgets::{AiConsoleView, FerrofluidWidget};

/// Window surface descriptor managed by `ferrofluidd`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FerrofluidSurface {
    pub id: u64,
    pub title: String,
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

/// Primary `ferrofluidd` GUI display server & compositor implementing Single-Window
/// AI Boot Mode and the Agent-Exclusive UI Security Gate.
pub struct FerrofluidServer {
    pub framebuffer: FerrofluidFramebuffer,
    pub input_mux: InputMultiplexer,
    pub ipc_channel: FerrofluidIpcChannel,
    security_gate: SecurityGate,
    pub console: AiConsoleView,
    pub widgets: Vec<FerrofluidWidget>,
    pub surfaces: Vec<FerrofluidSurface>,
    next_surface_id: u64,
}

impl FerrofluidServer {
    /// Initializes `FerrofluidServer` directly from the boot liquid-mercury drip
    /// border into the single centered/fullscreen **Ferrofluid AI Console** window,
    /// rendering the initial frame into the back buffer and tracking dirty rectangles.
    pub fn new(width: u32, height: u32) -> Self {
        let token = AgentUiToken::system_default();
        let security_gate = SecurityGate::new(token);
        let framebuffer = FerrofluidFramebuffer::new(width, height);
        let input_mux = InputMultiplexer::new();
        let ipc_channel = FerrofluidIpcChannel::new(64, token);
        let console = AiConsoleView::new("Gemini");

        let mut surfaces = Vec::new();
        let pad_x = (width / 32).min(8);
        let pad_y = (height / 32).min(8);
        let win_w = width.saturating_sub(pad_x * 2).max(1);
        let win_h = height.saturating_sub(pad_y * 2).max(1);

        surfaces.push(FerrofluidSurface {
            id: 1,
            title: "Ferrofluid AI Console".to_string(),
            x: pad_x,
            y: pad_y,
            width: win_w,
            height: win_h,
        });

        let mut server = Self {
            framebuffer,
            input_mux,
            ipc_channel,
            security_gate,
            console,
            widgets: Vec::new(),
            surfaces,
            next_surface_id: 2,
        };

        server.composite_internal();
        server
    }

    /// Returns the capability token assigned to the privileged AI Agent runtime.
    pub const fn agent_token(&self) -> AgentUiToken {
        self.security_gate.expected_token()
    }

    /// Returns the `CallerPrincipal::AiAgent` matching this server's token.
    pub const fn agent_principal(&self) -> CallerPrincipal {
        self.security_gate.expected_token().principal()
    }

    /// Verifies that the caller is the authenticated AI agent.
    pub fn authorize(
        &self,
        principal: &CallerPrincipal,
        token: Option<&AgentUiToken>,
    ) -> Result<(), FerrofluidError> {
        self.security_gate.authorize(principal, token)
    }

    /// Attempts to create a UI surface. Strictly rejects `DeveloperMcpApp` and
    /// `Unauthenticated` callers with `FerrofluidError::UiCreationForbiddenForApp`.
    pub fn create_surface(
        &mut self,
        principal: &CallerPrincipal,
        token: Option<&AgentUiToken>,
        title: &str,
        width: u32,
        height: u32,
    ) -> Result<u64, FerrofluidError> {
        self.authorize(principal, token)?;
        if width == 0 || height == 0 || width > self.framebuffer.width || height > self.framebuffer.height {
            return Err(FerrofluidError::InvalidDimensions);
        }

        let id = self.next_surface_id;
        self.next_surface_id += 1;
        let x = (self.framebuffer.width - width) / 2;
        let y = (self.framebuffer.height - height) / 2;

        self.surfaces.push(FerrofluidSurface {
            id,
            title: title.to_string(),
            x,
            y,
            width,
            height,
        });

        self.composite_internal();
        Ok(id)
    }

    /// Replaces the synthesized generative UI widget list and re-renders.
    /// Rejects `DeveloperMcpApp` and `Unauthenticated` with `FerrofluidError::UiCreationForbiddenForApp`.
    pub fn submit_widgets(
        &mut self,
        principal: &CallerPrincipal,
        token: Option<&AgentUiToken>,
        widgets: Vec<FerrofluidWidget>,
    ) -> Result<DirtyRect, FerrofluidError> {
        self.authorize(principal, token)?;
        self.widgets = widgets;
        Ok(self.composite_internal())
    }

    /// Mutates or appends a single generative UI widget and re-renders.
    /// Rejects `DeveloperMcpApp` and `Unauthenticated` with `FerrofluidError::UiCreationForbiddenForApp`.
    pub fn mutate_widget(
        &mut self,
        principal: &CallerPrincipal,
        token: Option<&AgentUiToken>,
        index: usize,
        widget: FerrofluidWidget,
    ) -> Result<DirtyRect, FerrofluidError> {
        self.authorize(principal, token)?;
        if index < self.widgets.len() {
            self.widgets[index] = widget;
        } else {
            self.widgets.push(widget);
        }
        Ok(self.composite_internal())
    }

    /// Renders a single generative UI widget directly into `(x, y, width, height)`
    /// after validating the Agent-Exclusive UI Security Gate.
    pub fn render_widget(
        &mut self,
        principal: &CallerPrincipal,
        token: Option<&AgentUiToken>,
        widget: &FerrofluidWidget,
        x: u32,
        y: u32,
        width: u32,
        height: u32,
    ) -> Result<DirtyRect, FerrofluidError> {
        self.authorize(principal, token)?;
        Ok(widget.render(&mut self.framebuffer, x, y, width, height))
    }

    /// Appends streaming chat tokens from the AI agent into the Ferrofluid AI Console.
    /// Rejects `DeveloperMcpApp` and `Unauthenticated` with `FerrofluidError::UiCreationForbiddenForApp`.
    pub fn stream_chat_token(
        &mut self,
        principal: &CallerPrincipal,
        token: Option<&AgentUiToken>,
        chunk: &str,
    ) -> Result<DirtyRect, FerrofluidError> {
        self.authorize(principal, token)?;
        self.console.push_streaming_token(chunk);
        Ok(self.composite_internal())
    }

    /// Submits a command over the shared-memory IPC channel and immediately
    /// processes all pending IPC commands.
    pub fn submit_ipc_command(
        &mut self,
        principal: &CallerPrincipal,
        token: Option<&AgentUiToken>,
        command: FerrofluidIpcCommand,
    ) -> Result<DirtyRect, FerrofluidError> {
        self.ipc_channel.send(principal, token, command)?;
        Ok(self.process_ipc())
    }

    /// Processes all authenticated commands waiting in `self.ipc_channel`.
    pub fn process_ipc(&mut self) -> DirtyRect {
        let mut dirty = DirtyRect::new(0, 0, 0, 0);
        let mut mutated = false;
        while let Some(cmd) = self.ipc_channel.recv() {
            mutated = true;
            match cmd {
                FerrofluidIpcCommand::CreateSurface {
                    title,
                    width,
                    height,
                } => {
                    let id = self.next_surface_id;
                    self.next_surface_id += 1;
                    let w = width.min(self.framebuffer.width).max(1);
                    let h = height.min(self.framebuffer.height).max(1);
                    self.surfaces.push(FerrofluidSurface {
                        id,
                        title,
                        x: (self.framebuffer.width - w) / 2,
                        y: (self.framebuffer.height - h) / 2,
                        width: w,
                        height: h,
                    });
                }
                FerrofluidIpcCommand::SubmitWidgets { widgets } => {
                    self.widgets = widgets;
                }
                FerrofluidIpcCommand::UpsertWidget { index, widget } => {
                    if index < self.widgets.len() {
                        self.widgets[index] = widget;
                    } else {
                        self.widgets.push(widget);
                    }
                }
                FerrofluidIpcCommand::StreamChatToken { token } => {
                    self.console.push_streaming_token(&token);
                }
                FerrofluidIpcCommand::FinishStreamingTurn => {
                    self.console.finish_streaming_turn();
                }
                FerrofluidIpcCommand::SetActiveModel { model } => {
                    self.console.active_model = model;
                }
            }
        }
        if mutated {
            dirty = self.composite_internal();
        }
        dirty
    }

    /// Enqueues a hardware `InputEvent` and dispatches it to the active Ferrofluid AI Console.
    pub fn handle_input(&mut self, event: InputEvent) -> DirtyRect {
        self.input_mux.push_event(event);
        self.input_mux.dispatch_to_console(&mut self.console);
        self.composite_internal()
    }

    /// Flips the back buffer to the front buffer for all damaged regions,
    /// increments the vsync counter, and returns the presented `DirtyRect` list.
    pub fn present(&mut self) -> Vec<DirtyRect> {
        self.framebuffer.present()
    }

    /// Internal compositor pass rendering the boot drip perimeter border,
    /// the centered/fullscreen Ferrofluid AI Console window, and any synthesized
    /// generative UI widgets into the back buffer.
    pub fn composite_internal(&mut self) -> DirtyRect {
        let fb_w = self.framebuffer.width;
        let fb_h = self.framebuffer.height;
        if fb_w == 0 || fb_h == 0 {
            return DirtyRect::new(0, 0, 0, 0);
        }

        let mut total_dirty = DirtyRect::new(0, 0, 0, 0);
        if let Some(r) = self.framebuffer.fill_rect(0, 0, fb_w, fb_h, BG_OBSIDIAN) {
            total_dirty = rect_union(total_dirty, r);
        }

        // Outer boot liquid-mercury drip border around the entire screen
        if let Some(r) = self.framebuffer.draw_liquid_mercury_border(0, 0, fb_w, fb_h) {
            total_dirty = rect_union(total_dirty, r);
        }

        let console_surface = self.surfaces.first().cloned().unwrap_or(FerrofluidSurface {
            id: 1,
            title: "Ferrofluid AI Console".to_string(),
            x: 0,
            y: 0,
            width: fb_w,
            height: fb_h,
        });

        if self.widgets.is_empty() {
            let r = self.console.render(
                &mut self.framebuffer,
                console_surface.x,
                console_surface.y,
                console_surface.width,
                console_surface.height,
            );
            total_dirty = rect_union(total_dirty, r);
        } else {
            // Keep phone content in one column beneath the AI console.
            let console_h = (console_surface.height * 3) / 5;
            let r_console = self.console.render(
                &mut self.framebuffer,
                console_surface.x,
                console_surface.y,
                console_surface.width,
                console_h,
            );
            total_dirty = rect_union(total_dirty, r_console);

            let deck_y = console_surface.y + console_h + 4;
            let deck_h = console_surface.height.saturating_sub(console_h + 4);
            let count = self.widgets.len() as u32;
            let portrait = fb_h > fb_w;
            let slot_w = console_surface.width / count.max(1);
            let slot_h = deck_h / count.max(1);

            for (i, widget) in self.widgets.iter().enumerate() {
                let (wx, wy, ww, wh) = if portrait {
                    (
                        console_surface.x,
                        deck_y + (i as u32) * slot_h,
                        console_surface.width,
                        slot_h.saturating_sub(4),
                    )
                } else {
                    (
                        console_surface.x + (i as u32) * slot_w,
                        deck_y,
                        slot_w.saturating_sub(2),
                        deck_h,
                    )
                };
                if ww < 32 || wh < 32 {
                    continue;
                }
                let r_widget = widget.render(&mut self.framebuffer, wx, wy, ww, wh);
                total_dirty = rect_union(total_dirty, r_widget);
            }
        }

        total_dirty
    }
}
