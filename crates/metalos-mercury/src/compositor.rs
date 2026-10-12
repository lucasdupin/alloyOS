use alloc::string::{String, ToString};
use alloc::vec::Vec;
use metalos_proto::{DirtyRect, InputEvent};

use crate::auth::{AgentUiToken, CallerPrincipal, MercuryError, SecurityGate};
use crate::framebuffer::{rect_union, MercuryFramebuffer};
use crate::ipc::{InputMultiplexer, MercuryIpcChannel, MercuryIpcCommand};
use crate::theme::BG_OBSIDIAN;
use crate::widgets::{AiConsoleView, MercuryWidget};

/// Window surface descriptor managed by `mercuryd`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MercurySurface {
    pub id: u64,
    pub title: String,
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

/// Primary `mercuryd` GUI display server & compositor implementing Single-Window
/// AI Boot Mode and the Agent-Exclusive UI Security Gate.
pub struct MercuryServer {
    pub framebuffer: MercuryFramebuffer,
    pub input_mux: InputMultiplexer,
    pub ipc_channel: MercuryIpcChannel,
    security_gate: SecurityGate,
    pub console: AiConsoleView,
    pub widgets: Vec<MercuryWidget>,
    pub surfaces: Vec<MercurySurface>,
    next_surface_id: u64,
}

impl MercuryServer {
    /// Initializes `MercuryServer` directly from the boot liquid-mercury drip
    /// border into the single centered/fullscreen **Mercury AI Console** window,
    /// rendering the initial frame into the back buffer and tracking dirty rectangles.
    pub fn new(width: u32, height: u32) -> Self {
        let token = AgentUiToken::system_default();
        let security_gate = SecurityGate::new(token);
        let framebuffer = MercuryFramebuffer::new(width, height);
        let input_mux = InputMultiplexer::new();
        let ipc_channel = MercuryIpcChannel::new(64, token);
        let console = AiConsoleView::new("Gemini");

        let mut surfaces = Vec::new();
        let pad_x = (width / 32).min(8);
        let pad_y = (height / 32).min(8);
        let win_w = width.saturating_sub(pad_x * 2).max(1);
        let win_h = height.saturating_sub(pad_y * 2).max(1);

        surfaces.push(MercurySurface {
            id: 1,
            title: "Mercury AI Console".to_string(),
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
    ) -> Result<(), MercuryError> {
        self.security_gate.authorize(principal, token)
    }

    /// Attempts to create a UI surface. Strictly rejects `DeveloperMcpApp` and
    /// `Unauthenticated` callers with `MercuryError::UiCreationForbiddenForApp`.
    pub fn create_surface(
        &mut self,
        principal: &CallerPrincipal,
        token: Option<&AgentUiToken>,
        title: &str,
        width: u32,
        height: u32,
    ) -> Result<u64, MercuryError> {
        self.authorize(principal, token)?;
        if width == 0 || height == 0 || width > self.framebuffer.width || height > self.framebuffer.height {
            return Err(MercuryError::InvalidDimensions);
        }

        let id = self.next_surface_id;
        self.next_surface_id += 1;
        let x = (self.framebuffer.width - width) / 2;
        let y = (self.framebuffer.height - height) / 2;

        self.surfaces.push(MercurySurface {
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
    /// Rejects `DeveloperMcpApp` and `Unauthenticated` with `MercuryError::UiCreationForbiddenForApp`.
    pub fn submit_widgets(
        &mut self,
        principal: &CallerPrincipal,
        token: Option<&AgentUiToken>,
        widgets: Vec<MercuryWidget>,
    ) -> Result<DirtyRect, MercuryError> {
        self.authorize(principal, token)?;
        self.widgets = widgets;
        Ok(self.composite_internal())
    }

    /// Mutates or appends a single generative UI widget and re-renders.
    /// Rejects `DeveloperMcpApp` and `Unauthenticated` with `MercuryError::UiCreationForbiddenForApp`.
    pub fn mutate_widget(
        &mut self,
        principal: &CallerPrincipal,
        token: Option<&AgentUiToken>,
        index: usize,
        widget: MercuryWidget,
    ) -> Result<DirtyRect, MercuryError> {
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
        widget: &MercuryWidget,
        x: u32,
        y: u32,
        width: u32,
        height: u32,
    ) -> Result<DirtyRect, MercuryError> {
        self.authorize(principal, token)?;
        Ok(widget.render(&mut self.framebuffer, x, y, width, height))
    }

    /// Appends streaming chat tokens from the AI agent into the Mercury AI Console.
    /// Rejects `DeveloperMcpApp` and `Unauthenticated` with `MercuryError::UiCreationForbiddenForApp`.
    pub fn stream_chat_token(
        &mut self,
        principal: &CallerPrincipal,
        token: Option<&AgentUiToken>,
        chunk: &str,
    ) -> Result<DirtyRect, MercuryError> {
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
        command: MercuryIpcCommand,
    ) -> Result<DirtyRect, MercuryError> {
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
                MercuryIpcCommand::CreateSurface {
                    title,
                    width,
                    height,
                } => {
                    let id = self.next_surface_id;
                    self.next_surface_id += 1;
                    let w = width.min(self.framebuffer.width).max(1);
                    let h = height.min(self.framebuffer.height).max(1);
                    self.surfaces.push(MercurySurface {
                        id,
                        title,
                        x: (self.framebuffer.width - w) / 2,
                        y: (self.framebuffer.height - h) / 2,
                        width: w,
                        height: h,
                    });
                }
                MercuryIpcCommand::SubmitWidgets { widgets } => {
                    self.widgets = widgets;
                }
                MercuryIpcCommand::UpsertWidget { index, widget } => {
                    if index < self.widgets.len() {
                        self.widgets[index] = widget;
                    } else {
                        self.widgets.push(widget);
                    }
                }
                MercuryIpcCommand::StreamChatToken { token } => {
                    self.console.push_streaming_token(&token);
                }
                MercuryIpcCommand::FinishStreamingTurn => {
                    self.console.finish_streaming_turn();
                }
                MercuryIpcCommand::SetActiveModel { model } => {
                    self.console.active_model = model;
                }
            }
        }
        if mutated {
            dirty = self.composite_internal();
        }
        dirty
    }

    /// Enqueues a hardware `InputEvent` and dispatches it to the active Mercury AI Console.
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
    /// the centered/fullscreen Mercury AI Console window, and any synthesized
    /// generative UI widgets into the back buffer.
    fn composite_internal(&mut self) -> DirtyRect {
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

        let console_surface = self.surfaces.first().cloned().unwrap_or(MercurySurface {
            id: 1,
            title: "Mercury AI Console".to_string(),
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
            // Split layout: top/left AI console + synthesized generative UI widgets deck
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
            let deck_h = console_surface
                .height
                .saturating_sub(console_h + 4)
                .max(24);
            let count = self.widgets.len() as u32;
            let slot_w = (console_surface.width / count.max(1)).max(32);

            for (i, widget) in self.widgets.iter().enumerate() {
                let wx = console_surface.x + (i as u32) * slot_w;
                let ww = slot_w.saturating_sub(2).max(16);
                let r_widget = widget.render(&mut self.framebuffer, wx, deck_y, ww, deck_h);
                total_dirty = rect_union(total_dirty, r_widget);
            }
        }

        total_dirty
    }
}
