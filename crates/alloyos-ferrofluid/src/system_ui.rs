//! Ferrofluid-owned Slint platform, single System UI window, and software scanout.
//!
//! All APIs must run on the same execution context, outside interrupt handlers.
//! The bare-metal Slint runtime uses `unsafe-single-threaded` storage. Keep this
//! owner local to the UI loop; do not put it in a global mutex or mark it Send.

use alloc::{
    boxed::Box, collections::VecDeque, format, rc::Rc, string::String, string::ToString, vec,
    vec::Vec,
};
use alloyos_keyboard::{LiquidDisplacement, SoftKey};
use alloyos_mcp::protocol::{AppTile, TileAction};
use alloyos_proto::{DirtyRect, InputEvent, InputEventKind};
use core::{
    cell::{Cell, RefCell},
    time::Duration,
};
use slint::platform::software_renderer::{MinimalSoftwareWindow, RepaintBufferType};
use slint::platform::{Key, Platform, PointerEventButton, WindowAdapter, WindowEvent};
use slint::{ComponentHandle, LogicalPosition, ModelRc, PhysicalSize, SharedString, VecModel};

slint::include_modules!();

struct FerrofluidPlatform {
    window: Rc<MinimalSoftwareWindow>,
    now: Rc<Cell<Duration>>,
    claimed: Cell<bool>,
}

impl Platform for FerrofluidPlatform {
    fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, slint::PlatformError> {
        if self.claimed.replace(true) {
            return Err(slint::PlatformError::Other(
                "Ferrofluid owns one System UI window".into(),
            ));
        }
        Ok(self.window.clone())
    }

    fn duration_since_start(&self) -> Duration {
        self.now.get()
    }
}

/// Interleaved conversational item rendered in the continuous chat scroll view.
pub use alloyos_mcp::display::ChatItem;

/// Single owner of the system window. Drivers supply time/input and consume
/// opaque ARGB8888 pixels; they never own or invoke Slint components.
pub struct FerrofluidSystemUi {
    window: Rc<MinimalSoftwareWindow>,
    ui: SystemUi,
    now: Rc<Cell<Duration>>,
    submissions: Rc<RefCell<VecDeque<String>>>,
    model_cycles: Rc<Cell<usize>>,
    voice_toggles: Rc<Cell<usize>>,
    voice_mutes: Rc<Cell<usize>>,
    output_mute_toggles: Rc<Cell<usize>>,
    tile_discovery_requested: Rc<Cell<bool>>,
    remote_install_decisions: Rc<RefCell<VecDeque<(String, bool)>>>,
    client_profile_decisions: Rc<RefCell<VecDeque<(String, Option<alloyos_mcp::account::ClientProfile>)>>>,
    tiles: Rc<RefCell<Vec<AppTile>>>,
    chat_items: Rc<RefCell<Vec<ChatItem>>>,
    available_tiles: Rc<RefCell<Vec<AppTile>>>,
    dashboard: Rc<RefCell<crate::DashboardState>>,
    tile_events: Rc<RefCell<VecDeque<crate::TileEvent>>>,
    image_cache: Rc<RefCell<crate::tile_images::ImageCache>>,
    webview_reloads: Rc<Cell<usize>>,
    webview_url: Rc<RefCell<String>>,
    rgb: Vec<slint::Rgb8Pixel>,
    argb: Vec<u32>,
    size: PhysicalSize,
    scale: f32,
    keys: EvdevKeyboard,
    liquid: LiquidDisplacement,
    boot_liquid: crate::boot_liquid::BootLiquid,
    sidebar_liquid: LiquidDisplacement,
    sidebar_blur: crate::sidebar::BackdropBlur,
    sidebar_drag: Option<crate::sidebar::SidebarDrag>,
    backdrop_dirty: Cell<bool>,
    previous_sidebar_active: bool,
    previous_webview_expanded: bool,
    previous_keyboard_reveal: f32,
    previous_chat_offset: f32,
    top_blur: Vec<slint::Rgb8Pixel>,
    previous_blur_top: Option<u32>,
}

impl FerrofluidSystemUi {
    /// Installs the process-wide Slint platform once, creates and shows System UI.
    pub fn new(width: u32, height: u32, scale: f32) -> Result<Self, slint::PlatformError> {
        // Embedded glyphs are compiled at the current display's native 3x density.
        if width == 0 || height == 0 || scale != alloyos_proto::display::PHONE_SCALE as f32 {
            return Err(slint::PlatformError::Other(
                "Invalid System UI geometry".into(),
            ));
        }
        let window = MinimalSoftwareWindow::new(RepaintBufferType::ReusedBuffer);
        let now = Rc::new(Cell::new(Duration::ZERO));
        slint::platform::set_platform(Box::new(FerrofluidPlatform {
            window: window.clone(),
            now: now.clone(),
            claimed: Cell::new(false),
        }))
        .map_err(|_| slint::PlatformError::Other("Slint platform already installed".into()))?;
        let ui = SystemUi::new()?;
        let image_cache = Rc::new(RefCell::new(crate::tile_images::ImageCache::default()));
        let images = image_cache.clone();
        ui.global::<TileImages>().on_resolve(move |src, _| images.borrow().get(&src));
        ui.on_tile_is_pinned(|id, tiles| {
            use slint::Model;
            tiles.iter().any(|tile| tile.tile_id == id)
        });
        let weak = ui.as_weak();
        ui.on_finish_unpin(move |id| {
            use slint::Model;
            if let Some(ui) = weak.upgrade() {
                if ui.get_dashboard_pins().iter().any(|tile| tile.tile_id == id) {
                    return;
                }
                let remaining: Vec<_> = ui.get_dashboard_tiles().iter()
                    .filter(|tile| tile.tile_id != id).collect();
                ui.set_dashboard_tiles(ModelRc::new(VecModel::from(remaining)));
            }
        });
        let size = PhysicalSize::new(width, height);
        window.dispatch_event(WindowEvent::ScaleFactorChanged {
            scale_factor: scale,
        });
        window.set_size(size);
        let submissions = Rc::new(RefCell::new(VecDeque::new()));
        let queue = submissions.clone();
        let weak = ui.as_weak();
        ui.on_submit(move |text| {
            if weak.upgrade().is_some_and(|ui| ui.get_reply_pending())
                || text.trim().is_empty()
                || queue.borrow().len() >= 16
            {
                return;
            }
            queue.borrow_mut().push_back(String::from(text.as_str()));
            if let Some(ui) = weak.upgrade() {
                ui.set_draft(SharedString::default());
            }
        });
        ui.global::<MarkdownLayout>().on_has_formatting(|text| {
            crate::markdown::parse(text.as_str()).iter().zip(text.as_str().lines()).any(|(block, line)| {
                block.heading > 0 || block.code || block.quote || block.strong || block.text != line || crate::markdown::inline(&block.text).iter().any(|(_, strong)| *strong)
            })
        });
        ui.global::<MarkdownLayout>().on_layout_words(layout_words);
        ui.global::<MarkdownLayout>().on_refresh_words(|source, previous, available| {
            use slint::Model;
            let words = source.iter().enumerate().map(|(index, mut word)| {
                if let Some(old) = previous.row_data(index) {
                    if old.text == word.text && old.strong == word.strong {
                        word.width = old.width;
                        word.height = old.height;
                    }
                }
                word
            }).collect::<Vec<_>>();
            layout_words(ModelRc::new(VecModel::from(words)), -1, 0.0, 0.0, available)
        });
        ui.set_formatted_transcript(markdown_text(ui.get_transcript().as_str()));
        let model_cycles = Rc::new(Cell::new(0usize));
        let cycles = model_cycles.clone();
        let weak = ui.as_weak();
        ui.on_cycle_model(move || {
            if weak.upgrade().is_some_and(|ui| !ui.get_reply_pending()) {
                cycles.set(cycles.get().saturating_add(1));
            }
        });
        let voice_toggles = Rc::new(Cell::new(0usize));
        let toggles = voice_toggles.clone();
        ui.on_voice_toggle(move || { toggles.set(toggles.get().saturating_add(1).min(16)); });
        let output_mute_toggles = Rc::new(Cell::new(0usize));
        let toggles = output_mute_toggles.clone();
        ui.on_output_mute_toggle(move || toggles.set(toggles.get().saturating_add(1).min(16)));
        let voice_mutes = Rc::new(Cell::new(0usize));
        let mutes = voice_mutes.clone();
        ui.on_voice_mute(move || { mutes.set(mutes.get().saturating_add(1).min(16)); });
        let soft_window = window.clone();
        ui.on_soft_key(move |value| {
            let text = SoftKey::from(value).into_slint_text();
            soft_window.dispatch_event(WindowEvent::KeyPressed { text: text.clone() });
            soft_window.dispatch_event(WindowEvent::KeyReleased { text });
        });
        let tiles = Rc::new(RefCell::new(Vec::<AppTile>::new()));
        let chat_items = Rc::new(RefCell::new(Vec::<ChatItem>::new()));
        let available_tiles = Rc::new(RefCell::new(Vec::<AppTile>::new()));
        let dashboard = Rc::new(RefCell::new(crate::DashboardState::default()));
        let tile_events = Rc::new(RefCell::new(VecDeque::new()));
        let tile_discovery_requested = Rc::new(Cell::new(false));
        let requested = tile_discovery_requested.clone();
        ui.on_discover_tiles(move || requested.set(true));
        let client_profile_decisions = Rc::new(RefCell::new(VecDeque::new()));
        let decisions = client_profile_decisions.clone();
        let weak = ui.as_weak();
        ui.on_client_profile_submit(move |issuer, client, scopes, port, secret, offline, reprompt| {
            let Some(ui) = weak.upgrade() else { return; };
            if ui.get_client_profile_id().is_empty() || !decisions.borrow().is_empty() || ui.get_client_profile_busy() { return; }
            let Ok(port) = port.trim().parse::<u16>() else {
                ui.set_client_profile_error("Enter the registered loopback port (0–65535).".into());
                return;
            };
            let profile = alloyos_mcp::account::ClientProfile {
                endpoint: ui.get_client_profile_endpoint().as_str().into(),
                issuer: issuer.trim().into(), client_id: client.trim().into(),
                allowed_scopes: scopes.split_whitespace().map(String::from).collect(),
                loopback_port: port,
                client_secret: if secret.is_empty() { None } else { Some(secret.as_str().into()) },
                authorization_options: if offline {
                    alloyos_mcp::account::AuthorizationOptions::GoogleOffline{reprompt_consent:reprompt}
                } else {alloyos_mcp::account::AuthorizationOptions::Standard},
            };
            if profile.validate().is_err() {
                ui.set_client_profile_error("Check the HTTPS issuer, client ID, scopes and secret limits.".into());
                return;
            }
            decisions.borrow_mut().push_back((ui.get_client_profile_id().as_str().into(), Some(profile)));
            ui.set_client_profile_busy(true);
        });
        let decisions = client_profile_decisions.clone();
        let weak = ui.as_weak();
        ui.on_client_profile_cancel(move || {
            let Some(ui) = weak.upgrade() else { return; };
            if !ui.get_client_profile_id().is_empty() && decisions.borrow().is_empty() {
                decisions.borrow_mut().push_back((ui.get_client_profile_id().as_str().into(), None));
            }
        });
        let remote_install_decisions = Rc::new(RefCell::new(VecDeque::new()));
        let decisions = remote_install_decisions.clone();
        let weak = ui.as_weak();
        ui.on_remote_install_decision(move |id, approved| {
            if let Some(ui) = weak.upgrade() {
                if !id.is_empty() && id == ui.get_pending_install_id() && decisions.borrow().is_empty() {
                    decisions.borrow_mut().push_back((String::from(id.as_str()), approved));
                }
            }
        });
        let state = dashboard.clone();
        let available = available_tiles.clone();
        let weak = ui.as_weak();
        ui.on_pin_tile(move |id, pin| {
            if !id.starts_with("web.")
                && !available
                    .borrow()
                    .iter()
                    .any(|tile| tile.tile_id == id.as_str())
            {
                return;
            }
            if pin {
                state.borrow_mut().pin(id.as_str());
            } else {
                state.borrow_mut().unpin(id.as_str());
            }
            if let Some(ui) = weak.upgrade() {
                update_dashboard(&ui, &state.borrow().tiles(&available.borrow()), &available.borrow());
            }
        });
        let state = dashboard.clone();
        let available = available_tiles.clone();
        let weak = ui.as_weak();
        ui.on_move_pin(move |id| {
            state.borrow_mut().move_to(id.as_str(), 0);
            if let Some(ui) = weak.upgrade() {
                update_dashboard(&ui, &state.borrow().tiles(&available.borrow()), &available.borrow());
            }
        });
        let events = tile_events.clone();
        let chat = tiles.clone();
        let items_chat = chat_items.clone();
        let available = available_tiles.clone();
        let dash = dashboard.clone();
        let weak = ui.as_weak();
        ui.on_refresh_tile(move |id| {
            if weak.upgrade().is_some_and(|ui| ui.get_reply_pending()) {
                return;
            }
            let tile_opt = items_chat
                .borrow()
                .iter()
                .find_map(|item| match item {
                    ChatItem::Tile(t) if t.tile_id == id.as_str() => Some(t.clone()),
                    _ => None,
                })
                .or_else(|| {
                    chat.borrow()
                        .iter()
                        .find(|tile| tile.tile_id == id.as_str())
                        .cloned()
                })
                .or_else(|| {
                    if dash.borrow().pinned_ids().iter().any(|p| p == id.as_str()) {
                        available
                            .borrow()
                            .iter()
                            .find(|tile| tile.tile_id == id.as_str())
                            .cloned()
                    } else {
                        None
                    }
                });
            if let Some(tile) = tile_opt {
                if let Some(uri) = &tile.refresh_resource_uri {
                    if events.borrow().len() < 16 {
                        events.borrow_mut().push_back(crate::TileEvent::Refresh {
                            tile_id: tile.tile_id.clone(),
                            resource_uri: uri.clone(),
                        });
                    }
                }
            }
        });
        // Typed events resolve action IDs against the current descriptor.
        let events = tile_events.clone();
        let chat = tiles.clone();
        let items_chat = chat_items.clone();
        let available = available_tiles.clone();
        let dash = dashboard.clone();
        let weak = ui.as_weak();
        ui.on_tile_action(move |id, action_id| {
            if weak.upgrade().is_some_and(|ui| ui.get_reply_pending()) {
                return;
            }
            let tile_opt = items_chat
                .borrow()
                .iter()
                .find_map(|item| match item {
                    ChatItem::Tile(t) if t.tile_id == id.as_str() => Some(t.clone()),
                    _ => None,
                })
                .or_else(|| {
                    chat.borrow()
                        .iter()
                        .find(|tile| tile.tile_id == id.as_str())
                        .cloned()
                })
                .or_else(|| {
                    if dash.borrow().pinned_ids().iter().any(|p| p == id.as_str()) {
                        available.borrow().iter().find(|tile| tile.tile_id == id.as_str()).cloned()
                    } else {
                        None
                    }
                });
            if let Some(tile) = tile_opt {
                if let Some(action) = tile
                    .actions
                    .iter()
                    .find(|action| action.id == action_id.as_str())
                {
                    if events.borrow().len() < 16 {
                        events.borrow_mut().push_back(crate::TileEvent::Action {
                            tile_id: tile.tile_id.clone(),
                            action: action.clone(),
                        });
                    }
                }
            }
        });
        let webview_reloads = Rc::new(Cell::new(0usize));
        let reloads = webview_reloads.clone();
        ui.on_webview_reload(move || {
            reloads.set(reloads.get().saturating_add(1).min(16));
        });
        let weak = ui.as_weak();
        ui.on_webview_set_expanded(move |expanded| {
            if let Some(ui) = weak.upgrade() {
                ui.set_webview_expanded(expanded);
            }
        });
        let webview_url = Rc::new(RefCell::new(String::new()));
        let weak = ui.as_weak();
        let dash = dashboard.clone();
        let current_url = webview_url.clone();
        ui.on_webview_pin(move |pin| {
            if let Some(ui) = weak.upgrade() {
                ui.set_webview_pinned(pin);
                let url = current_url.borrow().clone();
                let pin_id = if url.is_empty() {
                    String::from("web.card")
                } else {
                    alloc::format!("web.{}", url)
                };
                if pin {
                    ui.set_webview_pin_departing(false);
                    dash.borrow_mut().pin(&pin_id);
                } else {
                    ui.set_webview_pin_departing(true);
                    dash.borrow_mut().unpin(&pin_id);
                    dash.borrow_mut().unpin("web.card");
                }
            }
        });
        let weak = ui.as_weak();
        ui.on_finish_webview_unpin(move || {
            if let Some(ui) = weak.upgrade() {
                if !ui.get_webview_pinned() {
                    ui.set_webview_pin_departing(false);
                }
            }
        });
        ui.show()?;
        window.dispatch_event(WindowEvent::WindowActiveChanged(true));
        Ok(Self {
            window,
            ui,
            now,
            submissions,
            model_cycles,
            voice_toggles,
            voice_mutes,
            output_mute_toggles,
            tile_discovery_requested,
            remote_install_decisions,
            client_profile_decisions,
            tiles,
            chat_items,
            available_tiles,
            dashboard,
            tile_events,
            image_cache,
            webview_reloads,
            webview_url,
            rgb: vec![slint::Rgb8Pixel::default(); width as usize * height as usize],
            argb: vec![0xff000000; width as usize * height as usize],
            size,
            scale,
            keys: EvdevKeyboard::default(),
            liquid: LiquidDisplacement::new(width, height, scale),
            boot_liquid: crate::boot_liquid::BootLiquid::new(size, scale),
            sidebar_liquid: LiquidDisplacement::from_left(width, height, scale),
            sidebar_blur: crate::sidebar::BackdropBlur::new(size, scale),
            sidebar_drag: None,
            backdrop_dirty: Cell::new(true),
            previous_sidebar_active: false,
            previous_webview_expanded: false,
            previous_keyboard_reveal: 0.0,
            previous_chat_offset: 0.0,
            previous_blur_top: None,
            top_blur: vec![slint::Rgb8Pixel::default(); width as usize * (192.0 * scale) as usize],
        })
    }

    /// Apply provider/session status without transferring microphone ownership to UI.
    pub fn set_voice_state(&self, state: alloyos_mcp::display::VoiceState, caption: &str, error: &str) {
        use alloyos_mcp::display::VoiceState;
        self.ui.set_voice_state(match state {
            VoiceState::Off => 0, VoiceState::Connecting => 1, VoiceState::Listening => 2,
            VoiceState::Responding => 3, VoiceState::Muted => 4, VoiceState::Error => 5,
        });
        self.ui.set_voice_caption(caption.into());
        self.ui.set_voice_error(error.into());
        self.backdrop_dirty.set(true);
    }
    /// Compact pipeline diagnostics; independent of user/assistant captions.
    pub fn set_voice_diagnostics(&self, diagnostics: &str) {
        self.ui.set_voice_diagnostics(diagnostics.into());
    }

    /// Smoothed by the view; level is microphone RMS normalized to 0..1000.
    pub fn set_voice_level(&self, level: u16) {
        self.ui.set_voice_level(level.min(1000) as f32 / 1000.0);
    }

    pub fn set_output_muted(&self, muted: bool) { self.ui.set_output_muted(muted); }

    pub fn take_output_mute_toggle(&self) -> bool {
        let count = self.output_mute_toggles.get();
        self.output_mute_toggles.set(count.saturating_sub(1));
        count > 0
    }

    pub fn take_voice_toggle(&self) -> bool {
        let count = self.voice_toggles.get();
        self.voice_toggles.set(count.saturating_sub(1));
        count > 0
    }
    pub fn take_voice_mute(&self) -> bool {
        let count = self.voice_mutes.get();
        self.voice_mutes.set(count.saturating_sub(1));
        count > 0
    }
    pub fn set_reduced_motion(&self, reduced: bool) { self.ui.set_reduced_motion(reduced); }
    pub fn open_keyboard(&self) { self.ui.invoke_open_keyboard(); }

    pub fn set_booting(&self, booting: bool) {
        self.backdrop_dirty.set(true);
        self.ui.set_booting(booting);
    }

    pub fn set_reply_pending(&self, pending: bool) {
        self.backdrop_dirty.set(true);
        if pending {
            self.ui.set_sidebar_open(false);
        }
        self.ui.set_reply_pending(pending);
    }
    /// Register all available semantic app descriptors for dashboard resolution.
    pub fn set_available_tiles(&self, tiles: &[AppTile]) {
        *self.available_tiles.borrow_mut() = tiles.to_vec();
        update_dashboard(
            &self.ui,
            &self.dashboard.borrow().tiles(&self.available_tiles.borrow()),
            &self.available_tiles.borrow(),
        );
    }

    pub fn take_tile_discovery_request(&self) -> bool {
        self.tile_discovery_requested.replace(false)
    }
    pub fn set_client_profile_setup(&self, pending: Option<&alloyos_mcp::account::PendingClientProfileSetup>, error: &str) {
        let id = pending.map(|p|p.id.as_str()).unwrap_or("");
        if self.ui.get_client_profile_id().as_str() != id {
            self.client_profile_decisions.borrow_mut().clear();
            self.ui.set_client_profile_busy(false);
            self.ui.invoke_release_editor();
        }
        self.ui.set_client_profile_name(pending.map(|p|p.name.as_str()).unwrap_or("").into());
        self.ui.set_client_profile_endpoint(pending.map(|p|p.endpoint.as_str()).unwrap_or("").into());
        self.ui.set_client_profile_error(error.into());
        if !error.is_empty() { self.ui.set_client_profile_busy(false); }
        self.ui.set_client_profile_id(id.into());
        if pending.is_some() { self.ui.set_sidebar_open(false); }
    }
    pub fn take_client_profile_decision(&self) -> Option<(String, Option<alloyos_mcp::account::ClientProfile>)> {
        self.client_profile_decisions.borrow_mut().pop_front()
    }
    pub fn set_pending_remote_install(&self, pending: Option<&alloyos_mcp::install::PendingRemoteInstall>) {
        let pending = pending.filter(|p|p.is_valid());
        let id = pending.map(|p|p.id.as_str()).unwrap_or("");
        if self.ui.get_pending_install_id().as_str() != id {
            self.remote_install_decisions.borrow_mut().clear();
            if pending.is_some() { self.ui.invoke_release_editor(); }
        }
        self.ui.set_pending_install_name(pending.map(|p|p.name.as_str()).unwrap_or("").into());
        self.ui.set_pending_install_removal(pending.is_some_and(|p|p.action == alloyos_mcp::install::PackageChangeAction::Remove));
        self.ui.set_pending_install_endpoint(pending.map(|p|p.endpoint.as_str()).unwrap_or("").into());
        self.ui.set_pending_install_source(pending.map(|p|p.source.as_str()).unwrap_or("").into());
        self.ui.set_pending_install_publisher(pending.map(|p|p.publisher.as_str()).unwrap_or("").into());
        self.ui.set_pending_install_id(id.into());
        if pending.is_some() { self.ui.set_sidebar_open(false); }
    }
    pub fn take_remote_install_decision(&self) -> Option<(String, bool)> {
        self.remote_install_decisions.borrow_mut().pop_front()
    }

    pub fn set_tile_discovery_status(&self, loaded: bool, error: &str) {
        self.ui.set_tile_catalog_loaded(loaded);
        self.ui.set_tile_catalog_error(error.into());
    }

    pub fn set_image_assets(&self, assets: &[alloyos_mcp::tile_images::ImageAsset]) {
        if self.image_cache.borrow_mut().update(assets) {
            let images = self.ui.global::<TileImages>();
            images.set_revision(images.get_revision().wrapping_add(1));
            self.backdrop_dirty.set(true);
        }
    }

    /// Synchronize the Ferrofluid-owned WebView surface and trusted origin chrome.
    pub fn set_webview(&self, webview: Option<&crate::webview::WebViewController>) {
        self.backdrop_dirty.set(true);
        let Some(webview) = webview else {
            self.ui.set_webview_visible(false);
            self.ui.set_webview_expanded(false);
            self.ui.set_webview_editable_focused(false);
            self.ui.set_webview_frame(slint::Image::default());
            self.ui.set_webview_pinned(false);
            self.ui.set_webview_pin_departing(false);
            let prev_url = self.webview_url.borrow().clone();
            if !prev_url.is_empty() {
                self.dashboard.borrow_mut().unpin(&format!("web.{}", prev_url));
            }
            self.dashboard.borrow_mut().unpin("web.card");
            *self.webview_url.borrow_mut() = String::new();
            self.ui.set_webview_url("".into());
            return;
        };
        let url = webview.requested_url().or_else(|| webview.committed_url()).unwrap_or("");
        *self.webview_url.borrow_mut() = url.to_string();
        self.ui.set_webview_url(url.into());
        let origin = webview
            .trusted_origin()
            .map(|o| o.as_str())
            .unwrap_or("Unverified origin");
        let title = if webview.untrusted_title().is_empty() {
            "Untitled page"
        } else {
            webview.untrusted_title()
        };
        let status = if webview.is_crashed() {
            "Renderer crashed"
        } else {
            match webview.loading_state() {
                crate::webview::LoadingState::Idle => "Verified origin",
                crate::webview::LoadingState::Loading => "Loading…",
                crate::webview::LoadingState::Failed => "Load failed",
            }
        };
        self.ui.set_webview_origin(origin.into());
        self.ui.set_webview_title(title.into());
        self.ui.set_webview_status(status.into());
        self.ui.set_webview_crashed(webview.is_crashed());
        self.ui.set_webview_editable_focused(webview.soft_keyboard_visible());
        let frame_image = webview
            .committed_frame()
            .map(webview_frame_image)
            .unwrap_or_default();
        self.ui.set_webview_frame(frame_image);
        let pin_id = if url.is_empty() {
            "web.card".to_string()
        } else {
            format!("web.{}", url)
        };
        if self.dashboard.borrow().pinned_ids().iter().any(|id| id == &pin_id || id == "web.card" || (!url.is_empty() && id == url)) {
            self.ui.set_webview_pinned(true);
        }
        self.ui.set_webview_visible(true);
    }

    pub fn set_webview_pinned(&self, pinned: bool) {
        if self.ui.get_webview_pinned() == pinned {
            return;
        }
        self.backdrop_dirty.set(true);
        self.ui.set_webview_pinned(pinned);
        let url = self.webview_url.borrow().clone();
        let pin_id = if url.is_empty() {
            "web.card".to_string()
        } else {
            format!("web.{}", url)
        };
        if pinned {
            self.ui.set_webview_pin_departing(false);
            self.dashboard.borrow_mut().pin(&pin_id);
        } else {
            self.dashboard.borrow_mut().unpin(&pin_id);
            self.dashboard.borrow_mut().unpin("web.card");
        }
    }

    pub fn webview_pinned(&self) -> bool {
        self.ui.get_webview_visible() && self.ui.get_webview_pinned()
    }

    pub fn set_sidebar_open(&self, open: bool) {
        self.backdrop_dirty.set(true);
        self.ui.set_sidebar_open(open);
    }

    pub fn sidebar_open(&self) -> bool {
        self.ui.get_sidebar_open()
    }

    pub fn set_webview_expanded(&self, expanded: bool) {
        self.backdrop_dirty.set(true);
        if expanded {
            self.ui.set_sidebar_open(false);
            self.ui.invoke_release_editor();
        }
        self.ui.set_webview_expanded(expanded);
    }

    pub fn webview_expanded(&self) -> bool {
        self.ui.get_webview_visible() && self.ui.get_webview_expanded()
    }

    pub fn take_webview_reload(&self) -> bool {
        let count = self.webview_reloads.get();
        self.webview_reloads.set(count.saturating_sub(1));
        count > 0
    }
    /// Render semantic app descriptors in the chat thread.
    pub fn set_tiles(&self, tiles: &[AppTile]) {
        self.backdrop_dirty.set(true);
        *self.tiles.borrow_mut() = tiles.to_vec();
        {
            let mut available = self.available_tiles.borrow_mut();
            for tile in tiles {
                if let Some(existing) = available.iter_mut().find(|t| t.tile_id == tile.tile_id) {
                    *existing = tile.clone();
                } else {
                    available.push(tile.clone());
                }
            }
        }
        self.ui.set_tiles(tile_views(tiles));
        update_dashboard(
            &self.ui,
            &self.dashboard.borrow().tiles(&self.available_tiles.borrow()),
            &self.available_tiles.borrow(),
        );
    }
    pub fn set_chat_items(&self, items: &[ChatItem]) {
        // Periodic pinned-tile refreshes resubmit the current conversation.
        // Replacing an identical model triggers Slint's changed handler and
        // resets the reading position, even though no message arrived.
        if self.chat_items.borrow().as_slice() == items {
            return;
        }
        self.backdrop_dirty.set(true);
        *self.chat_items.borrow_mut() = items.to_vec();
        {
            let mut available = self.available_tiles.borrow_mut();
            for item in items {
                if let ChatItem::Tile(tile) = item {
                    if let Some(existing) = available.iter_mut().find(|t| t.tile_id == tile.tile_id) {
                        *existing = tile.clone();
                    } else {
                        available.push(tile.clone());
                    }
                }
            }
        }
        self.ui.set_chat_items(chat_item_views(items));
        update_dashboard(
            &self.ui,
            &self.dashboard.borrow().tiles(&self.available_tiles.borrow()),
            &self.available_tiles.borrow(),
        );
    }
    pub fn chat_items(&self) -> Vec<ChatItem> {
        self.chat_items.borrow().clone()
    }
    pub fn update_streaming_chat_item(&self, partial_reply: &str) {
        self.backdrop_dirty.set(true);
        let mut items = self.chat_items.borrow_mut();
        if let Some(ChatItem::Text(last_text)) = items.last_mut() {
            if last_text.starts_with("Welder:") {
                *last_text = alloc::format!("Welder: {}", partial_reply);
            } else {
                items.push(ChatItem::Text(alloc::format!("Welder: {}", partial_reply)));
            }
        } else {
            items.push(ChatItem::Text(alloc::format!("Welder: {}", partial_reply)));
        }
        self.ui.set_chat_items(chat_item_views(&items));
    }
    pub fn dashboard_state(&self) -> crate::DashboardState {
        self.dashboard.borrow().clone()
    }
    pub fn available_tiles(&self) -> Vec<AppTile> {
        self.available_tiles.borrow().clone()
    }
    pub fn chat_tiles(&self) -> Vec<AppTile> {
        self.tiles.borrow().clone()
    }
    pub fn restore_dashboard(&self, ids: &[String]) {
        self.dashboard.borrow_mut().restore(ids);
        let url = self.webview_url.borrow().clone();
        let pin_id = if url.is_empty() {
            "web.card".to_string()
        } else {
            format!("web.{}", url)
        };
        let webview_pinned = ids.iter().any(|id| {
            id == &pin_id
                || id == "web.card"
                || (!url.is_empty() && (id == &url || id.strip_prefix("web.") == Some(&url)))
        });
        if self.ui.get_webview_pinned() != webview_pinned {
            self.backdrop_dirty.set(true);
            self.ui.set_webview_pinned(webview_pinned);
            if webview_pinned {
                self.ui.set_webview_pin_departing(false);
            }
        }
        update_dashboard(
            &self.ui,
            &self.dashboard.borrow().tiles(&self.available_tiles.borrow()),
            &self.available_tiles.borrow(),
        );
    }
    pub fn take_tile_event(&self) -> Option<crate::TileEvent> {
        self.tile_events.borrow_mut().pop_front()
    }
    pub fn take_tile_action(&self) -> Option<TileAction> {
        let mut events = self.tile_events.borrow_mut();
        let index = events
            .iter()
            .position(|event| matches!(event, crate::TileEvent::Action { .. }))?;
        match events.remove(index)? {
            crate::TileEvent::Action { action, .. } => Some(action),
            _ => None,
        }
    }
    pub fn set_transcript(&self, text: &str) {
        self.backdrop_dirty.set(true);
        self.ui.set_formatted_transcript(markdown_text(text));
        self.ui.set_transcript(text.into());
    }
    pub fn transcript(&self) -> String {
        String::from(self.ui.get_transcript().as_str())
    }
    pub fn chat_scroll_offset(&self) -> f32 {
        self.ui.get_scroll_offset()
    }
    pub fn pinned_scroll_offset(&self) -> f32 {
        self.ui.get_pinned_scroll_offset()
    }
    pub fn pinned_scroll_maximum(&self) -> f32 {
        self.ui.get_pinned_scroll_maximum()
    }
    pub fn chat_scroll_maximum(&self) -> f32 {
        self.ui.get_scroll_maximum()
    }
    pub fn chat_content_bottom(&self) -> f32 {
        self.ui.get_chat_content_bottom()
    }
    pub fn chat_viewport_height(&self) -> f32 {
        self.ui.get_chat_viewport_height()
    }
    pub fn update_streaming_reply(&self, base_transcript: &str, partial_reply: &str) {
        self.backdrop_dirty.set(true);
        let mut combined = String::with_capacity(base_transcript.len() + 10 + partial_reply.len());
        combined.push_str(base_transcript);
        combined.push_str("\n\nWelder: ");
        combined.push_str(partial_reply);
        self.set_transcript(&combined);
    }
    pub fn set_active_model(&self, text: &str) {
        self.backdrop_dirty.set(true);
        self.ui.set_active_model(text.into());
    }
    pub fn set_model_selection_status(&self, text: &str) {
        self.backdrop_dirty.set(true);
        self.ui.set_model_selection_status(text.into());
    }
    pub fn take_model_cycle(&self) -> bool {
        let count = self.model_cycles.get();
        self.model_cycles.set(count.saturating_sub(1));
        count > 0
    }
    pub fn take_submission(&self) -> Option<String> {
        self.submissions.borrow_mut().pop_front()
    }
    pub fn pixels(&self) -> &[u32] {
        &self.argb
    }
    pub fn size(&self) -> PhysicalSize {
        self.size
    }

    /// Input coordinates are physical scanout pixels; Slint receives logical points.
    pub fn handle_input(&mut self, event: InputEvent) {
        // Capture the physical edge before mapping touches through the liquid
        // field: finger travel must not change as the scene bends beneath it.
        let px = event.x as f32 / self.scale;
        let py = event.y as f32 / self.scale;
        let reveal = self.ui.get_sidebar_reveal();
        let primary = matches!(event.code, 0x110 | 0x14a);
        if !self.ui.get_booting() {
            if event.kind == InputEventKind::PointerButtonDown && primary
                && (px <= 18.0 && reveal < 1.0
                    || reveal >= 1.0 && (px - reveal).abs() <= 14.0)
            {
                self.ui.invoke_release_editor();
                self.sidebar_drag = Some(crate::sidebar::SidebarDrag::new(px, py, reveal));
                self.ui.set_sidebar_drag_reveal(reveal);
                self.ui.set_sidebar_dragging(true);
                return;
            }
            if event.kind == InputEventKind::PointerMove {
                if let Some(drag) = &mut self.sidebar_drag {
                    let reveal = drag.update(px, py, self.ui.get_sidebar_width());
                    self.ui.set_sidebar_drag_reveal(reveal);
                    return;
                }
            }
            if event.kind == InputEventKind::PointerButtonUp && primary {
                if let Some(drag) = self.sidebar_drag.take() {
                    let open = drag.open_on_release(reveal, self.ui.get_sidebar_width());
                    self.ui.set_sidebar_open(open);
                    self.ui.set_sidebar_dragging(false);
                    return;
                }
            }
            if event.kind == InputEventKind::KeyDown && event.code == 1 && reveal > 0.0 {
                self.ui.set_sidebar_open(false);
                return;
            }
        }
        let (x, y) = self.sidebar_liquid.source(event.x as f32, event.y as f32);
        // Inverse-map touches through the same field used for presentation.
        let (x, y) = self.liquid.source(x, y);
        let position = LogicalPosition::new(x / self.scale, y / self.scale);
        // Release composer focus without consuming the touch, so chat scrolling
        // and tile actions still receive their normal pointer events.
        let chat_top = self.ui.get_chat_top();
        if event.kind == InputEventKind::PointerButtonDown && primary
            && !self.ui.get_booting() && reveal < 1.0
            && self.ui.get_keyboard_visible()
            && position.x >= 0.0 && position.x < self.size.width as f32 / self.scale
            && position.y >= chat_top
            && position.y < chat_top + self.ui.get_chat_viewport_height()
        {
            self.ui.invoke_release_editor();
        }
        let translated = match event.kind {
            InputEventKind::PointerMove => Some(WindowEvent::PointerMoved { position }),
            InputEventKind::PointerButtonDown | InputEventKind::PointerButtonUp => {
                let button = match event.code {
                    0x110 | 0x14a => PointerEventButton::Left, // BTN_LEFT / BTN_TOUCH
                    0x111 => PointerEventButton::Right,
                    0x112 => PointerEventButton::Middle,
                    _ => return,
                };
                Some(if event.kind == InputEventKind::PointerButtonDown {
                    WindowEvent::PointerPressed { position, button }
                } else {
                    WindowEvent::PointerReleased { position, button }
                })
            }
            InputEventKind::KeyDown | InputEventKind::KeyUp => {
                let down = event.kind == InputEventKind::KeyDown;
                let repeated = down && self.keys.pressed.contains_key(&event.code);
                self.keys.translate(event.code, down).map(|text| {
                    if repeated {
                        WindowEvent::KeyPressRepeated { text }
                    } else if down {
                        WindowEvent::KeyPressed { text }
                    } else {
                        WindowEvent::KeyReleased { text }
                    }
                })
            }
        };
        if let Some(event) = translated {
            self.window.dispatch_event(event);
        }
    }

    /// Advances local timers, boot turbulence and requested Slint repaints.
    /// Returns physical damage bounds for VirtIO-GPU transfer/flush.
    pub fn render(&mut self, elapsed: Duration) -> Option<DirtyRect> {
        self.now.set(elapsed.max(self.now.get()));
        slint::platform::update_timers_and_animations();
        let booting = self.ui.get_booting();
        if self.ui.get_reply_pending() || (1..=3).contains(&self.ui.get_voice_state()) {
            self.ui.set_loading_time_ms(self.now.get().as_millis() as i32);
        }
        let boot_damage = if booting {
            self.boot_liquid.advance(self.now.get());
            Some(self.boot_liquid.bounds())
        } else {
            None
        };
        let sidebar_active = !booting && self.ui.get_sidebar_reveal() > 0.0;
        let webview_expanded = !booting && self.ui.get_webview_visible() && self.ui.get_webview_expanded();
        let webview_mode_changed = webview_expanded != self.previous_webview_expanded;
        let keyboard_reveal = self.ui.get_keyboard_reveal();
        let chat_offset = self.ui.get_scroll_offset();
        if sidebar_active && (!self.previous_sidebar_active
            || webview_mode_changed
            || keyboard_reveal != self.previous_keyboard_reveal
            || chat_offset != self.previous_chat_offset || self.ui.get_reply_pending()) {
            self.backdrop_dirty.set(true);
        }
        let capture_backdrop = sidebar_active && self.backdrop_dirty.get();
        let sidebar_damage = self.sidebar_liquid.advance(
            self.now.get(), self.ui.get_sidebar_reveal(), booting,
        );
        let liquid_damage = self.liquid.advance(
            self.now.get(),
            self.ui.get_keyboard_reveal(),
            booting || sidebar_active || self.ui.get_reduced_motion(),
        );
        if liquid_damage.is_some() || sidebar_damage.is_some() || boot_damage.is_some() || webview_mode_changed {
            self.window.request_redraw();
        }
        let mut damage = None;
        self.window.draw_if_needed(|renderer| {
            if capture_backdrop {
                // Use one retained buffer for both passes. Hiding the overlay
                // damages its old bounds and restores the actual scene beneath
                // it; the blur never samples a previous composited frame.
                self.ui.set_sidebar_layer_visible(false);
                renderer.render(&mut self.rgb, self.size.width as usize);
                self.ui.set_sidebar_backdrop(self.sidebar_blur.capture(&self.rgb));
                self.ui.set_sidebar_layer_visible(true);
                self.backdrop_dirty.set(false);
            }
            let region = renderer.render(&mut self.rgb, self.size.width as usize);
            if booting {
                self.boot_liquid.prepare_drops(&self.rgb);
            }
            let origin = region.bounding_box_origin();
            let size = region.bounding_box_size();
            if (size.width == 0 || size.height == 0) && boot_damage.is_none()
                && liquid_damage.is_none() && sidebar_damage.is_none() && !webview_mode_changed {
                return;
            }
            let mut rect = if webview_mode_changed {
                DirtyRect::new(0, 0, self.size.width, self.size.height)
            } else {
                DirtyRect::new(origin.x as u32, origin.y as u32, size.width, size.height)
            };
            if let Some(boot) = boot_damage {
                if size.width == 0 || size.height == 0 {
                    rect = boot;
                } else {
                    let right = (rect.x + rect.width).max(boot.x + boot.width);
                    let bottom = (rect.y + rect.height).max(boot.y + boot.height);
                    rect.x = rect.x.min(boot.x);
                    rect.y = rect.y.min(boot.y);
                    rect.width = right - rect.x;
                    rect.height = bottom - rect.y;
                }
            }
            if let Some((top, bottom)) = liquid_damage {
                let end = (rect.y + rect.height).max(bottom);
                rect.y = rect.y.min(top);
                rect.height = end - rect.y;
                rect.x = 0;
                rect.width = self.size.width;
            }
            if let Some((left, right)) = sidebar_damage {
                let end = (rect.x + rect.width).max(right);
                rect.x = rect.x.min(left);
                rect.width = end - rect.x;
                rect.y = 0;
                rect.height = self.size.height;
            }
            // Rendering resolves layout and may update the scroll position.
            let chat_offset = self.ui.get_scroll_offset();
            let chat_top = (self.ui.get_chat_top() * self.scale) as u32;
            let blur_height = (40.0 * self.scale) as u32;
            let blur_bottom = (chat_top + blur_height).min(self.size.height);
            let blur_active = !booting && !sidebar_active && !webview_expanded && chat_offset > 0.0;
            if blur_active {
                if let Some(previous) = self.previous_blur_top {
                    if previous != chat_top || chat_offset != self.previous_chat_offset {
                        let bottom = (rect.y + rect.height)
                            .max((previous + blur_height).min(self.size.height))
                            .max(blur_bottom);
                        rect.y = rect.y.min(previous).min(chat_top);
                        rect.height = bottom - rect.y;
                        rect.x = 0;
                        rect.width = self.size.width;
                    }
                }
                self.previous_blur_top = Some(chat_top);
            } else {
                if let Some(previous) = self.previous_blur_top {
                    let bottom = (rect.y + rect.height)
                        .max((previous + blur_height).min(self.size.height));
                    rect.y = rect.y.min(previous);
                    rect.height = bottom - rect.y;
                    rect.x = 0;
                    rect.width = self.size.width;
                }
                self.previous_blur_top = None;
            }
            let blur_dirty = blur_active && rect.y < blur_bottom && rect.y + rect.height > chat_top;
            if blur_dirty {
                // The blur samples neighboring pixels, so damage anywhere in
                // its band invalidates the entire band, including its fringe.
                let bottom = (rect.y + rect.height).max(blur_bottom);
                rect.y = rect.y.min(chat_top);
                rect.height = bottom - rect.y;
                rect.x = 0;
                rect.width = self.size.width;
            }
            let bottom_top = (self.ui.get_bottom_fade_top().max(0.0) * self.scale) as u32;
            let bottom_height = ((self.ui.get_bottom_fade_height() * self.scale) as u32)
                .min(self.size.height.saturating_sub(bottom_top));
            let bottom_dirty = !booting && !sidebar_active && !webview_expanded && bottom_height > 0
                && rect.y < bottom_top + bottom_height && rect.y + rect.height > bottom_top;
            if bottom_dirty {
                let end = (rect.y + rect.height).max(bottom_top + bottom_height);
                rect.y = rect.y.min(bottom_top);
                rect.height = end - rect.y;
                rect.x = 0;
                rect.width = self.size.width;
            }
            for y in rect.y..rect.y + rect.height {
                for x in rect.x..rect.x + rect.width {
                    let i = y as usize * self.size.width as usize + x as usize;
                    let p = if booting {
                        self.boot_liquid.pixel(&self.rgb, x, y)
                    } else if sidebar_active || sidebar_damage.is_some() {
                        self.sidebar_liquid.pixel(&self.rgb, x, y)
                    } else {
                        self.liquid.pixel(&self.rgb, x, y)
                    };
                    self.argb[i] = 0xff000000 | (p.r as u32) << 16 | (p.g as u32) << 8 | p.b as u32;
                }
            }
            if blur_dirty {
                blur_chat_top(
                    &self.rgb,
                    &mut self.top_blur,
                    &mut self.argb,
                    self.size.width as usize,
                    chat_top as usize,
                    (blur_bottom - chat_top) as usize,
                    (6.0 * self.scale) as usize,
                    false,
                );
            }
            if bottom_dirty {
                blur_chat_top(
                    &self.rgb, &mut self.top_blur, &mut self.argb,
                    self.size.width as usize, bottom_top as usize, bottom_height as usize,
                    (6.0 * self.scale) as usize, true,
                );
            }
            damage = Some(rect);
        });
        if capture_backdrop && damage.is_some() {
            // Overlay visibility and backdrop setters requested redraws which
            // the second pass has already rendered. Avoid an idle redraw loop.
            self.window.draw_if_needed(|_| {});
        }
        self.previous_sidebar_active = sidebar_active;
        self.previous_webview_expanded = webview_expanded;
        self.previous_keyboard_reveal = keyboard_reveal;
        self.previous_chat_offset = self.ui.get_scroll_offset();
        damage
    }
}

fn webview_frame_image(frame: &crate::webview::CommittedFrame) -> slint::Image {
    let mut buffer = slint::SharedPixelBuffer::<slint::Rgba8Pixel>::new(frame.width, frame.height);
    let dst = buffer.make_mut_slice();
    let row_bytes = (frame.width as usize) * 4;
    for y in 0..(frame.height as usize) {
        let row_start = y * (frame.stride as usize);
        let row = &frame.pixels[row_start..row_start + row_bytes];
        let dst_row = &mut dst[y * (frame.width as usize)..(y + 1) * (frame.width as usize)];
        for (px, bgra) in dst_row.iter_mut().zip(row.chunks_exact(4)) {
            *px = slint::Rgba8Pixel {
                r: bgra[2],
                g: bgra[1],
                b: bgra[0],
                a: bgra[3],
            };
        }
    }
    slint::Image::from_rgba8(buffer)
}

fn update_dashboard(ui: &SystemUi, tiles: &[AppTile], available: &[AppTile]) {
    // Discovery is independent of chat: a filesystem-provided descriptor is
    // immediately available even when Welder has never embedded it in a reply.
    use slint::Model;
    let unpinned: Vec<_> = available.iter().filter(|candidate|
        !tiles.iter().any(|pin| pin.tile_id == candidate.tile_id)).collect();
    let current = ui.get_available_tiles();
    let unchanged = current.row_count() == unpinned.len() && unpinned.iter().enumerate().all(|(index, tile)|
        current.row_data(index).is_some_and(|row| row.tile_id == tile.tile_id.as_str() && row.title == tile.title.as_str()));
    if !unchanged {
        // Discovery never copies item/action models or image references. Keep
        // the model stable during periodic refreshes that only change content.
        ui.set_available_tiles(ModelRc::new(VecModel::from(unpinned.iter().map(|tile| AvailableTileView {
            tile_id: tile.tile_id.as_str().into(), title: tile.title.as_str().into(),
        }).collect::<Vec<_>>())));
    }
    let pins = tile_views(tiles);
    ui.set_dashboard_pins(pins.clone());
    if !ui.get_sidebar_open() || ui.get_reduced_motion() {
        ui.set_dashboard_tiles(pins);
        return;
    }
    let mut displayed: Vec<_> = pins.iter().collect();
    for (index, old) in ui.get_dashboard_tiles().iter().enumerate() {
        if !pins.iter().any(|tile| tile.tile_id == old.tile_id) {
            displayed.insert(index.min(displayed.len()), old);
        }
    }
    ui.set_dashboard_tiles(ModelRc::new(VecModel::from(displayed)));

}

fn tile_to_view(tile: &AppTile) -> TileView {
    TileView {
        image_source: tile.image.as_ref().filter(|image| image.valid_for(&tile.tile_id)).map(|image| image.src.as_str()).unwrap_or("").into(),
        image_alt: tile.image.as_ref().map(|image| image.alt.as_str()).unwrap_or("").into(),
        tile_id: tile.tile_id.as_str().into(),
        title: tile.title.as_str().into(),
        subtitle: tile.subtitle.as_str().into(),
        refreshable: tile.refresh_resource_uri.is_some(),
        items: ModelRc::new(VecModel::from(
            tile.items
                .iter()
                .map(|item| TileItemView {
                    id: item.id.as_str().into(),
                    title: item.title.as_str().into(),
                    subtitle: item.subtitle.as_str().into(),
                    badge: item.badge.as_str().into(),
                    status: item.status.as_str().into(),
                })
                .collect::<Vec<_>>(),
        )),
        actions: ModelRc::new(VecModel::from(
            tile.actions
                .iter()
                .map(|action| TileActionView {
                    id: action.id.as_str().into(),
                    label: action.label.as_str().into(),
                })
                .collect::<Vec<_>>(),
        )),
    }
}

fn tile_views(tiles: &[AppTile]) -> ModelRc<TileView> {
    ModelRc::new(VecModel::from(
        tiles.iter().map(tile_to_view).collect::<Vec<_>>(),
    ))
}

fn layout_words(words: ModelRc<MarkdownWordView>, measured: i32, width: f32, height: f32, available: f32) -> ModelRc<MarkdownWordView> {
    use slint::Model;
    if let Some(word) = usize::try_from(measured).ok().and_then(|i| words.row_data(i)) {
        if word.width == width && word.height == height {
            return words;
        }
    }
    let mut words: Vec<_> = words.iter().collect();
    if let Some(word) = usize::try_from(measured).ok().and_then(|i| words.get_mut(i)) {
        word.width = width;
        word.height = height;
    }
    let (mut x, mut y, mut line_height) = (0.0f32, 0.0f32, 0.0f32);
    for word in &mut words {
        if x > 0.0 && x + word.width > available {
            x = 0.0;
            y += line_height;
            line_height = 0.0;
        }
        word.x = x;
        word.y = y;
        x += word.width;
        line_height = line_height.max(word.height);
    }
    ModelRc::new(VecModel::from(words))
}

fn markdown_text(text: &str) -> ModelRc<MarkdownBlockView> {
    ModelRc::new(VecModel::from(crate::markdown::parse(text).into_iter().map(|block| {
        let spans = if block.code { Vec::new() } else { crate::markdown::inline(&block.text) };
        let inline_bold = spans.iter().any(|(_, strong)| *strong);
        // Invisible InlineMarkdown items still instantiate their repeaters.
        // Plain/list/code blocks must not create per-word Text + measure items.
        let words = if inline_bold {
            ModelRc::new(VecModel::from(spans.into_iter().map(|(text, strong)| MarkdownWordView {
                text: text.into(), strong: strong || block.strong, ..Default::default()
            }).collect::<Vec<_>>()))
        } else { ModelRc::default() };
        MarkdownBlockView {
            text: block.text.as_str().into(), heading: block.heading as i32,
            code: block.code, quote: block.quote, strong: block.strong,
            inline_bold, words,
        }
    }).collect::<Vec<_>>()))
}

fn chat_item_views(items: &[ChatItem]) -> ModelRc<ChatItemView> {
    ModelRc::new(VecModel::from(
        items
            .iter()
            .map(|item| match item {
                ChatItem::Text(text) => {
                    let body = ["You:", "Welder:", "Alloy:"]
                        .iter()
                        .find_map(|prefix| text.strip_prefix(prefix))
                        .map(str::trim_start)
                        .unwrap_or(text);
                    ChatItemView {
                        is_tile: false,
                        is_user: text.starts_with("You:"),
                        text: body.into(),
                        blocks: markdown_text(body),
                        tile: TileView::default(),
                    }
                },
                ChatItem::Tile(tile) => ChatItemView {
                    is_tile: true,
                    is_user: false,
                    text: "".into(),
                    blocks: ModelRc::default(),
                    tile: tile_to_view(tile),
                },
            })
            .collect::<Vec<_>>(),
    ))
}

/// Two sliding box passes keep the scroll-edge blur linear in band size.
/// Always sample pristine Slint pixels so repeated frames cannot accumulate blur.
fn blur_chat_top(
    rgb: &[slint::Rgb8Pixel],
    scratch: &mut [slint::Rgb8Pixel],
    argb: &mut [u32],
    width: usize,
    top: usize,
    height: usize,
    radius: usize,
    bottom: bool,
) {
    if height == 0 {
        return;
    }
    for y in 0..height {
        let row = &rgb[(top + y) * width..(top + y + 1) * width];
        let mut sum = [0u32; 3];
        let mut left = 0;
        let mut right = 0;
        for x in 0..width {
            let end = (x + radius + 1).min(width);
            while right < end {
                let p = row[right];
                sum[0] += p.r as u32;
                sum[1] += p.g as u32;
                sum[2] += p.b as u32;
                right += 1;
            }
            while left < x.saturating_sub(radius) {
                let p = row[left];
                sum[0] -= p.r as u32;
                sum[1] -= p.g as u32;
                sum[2] -= p.b as u32;
                left += 1;
            }
            let n = (right - left) as u32;
            scratch[y * width + x] = slint::Rgb8Pixel {
                r: (sum[0] / n) as u8,
                g: (sum[1] / n) as u8,
                b: (sum[2] / n) as u8,
            };
        }
    }
    for x in 0..width {
        let mut sum = [0u32; 3];
        let mut start = 0;
        let mut end = 0;
        for y in 0..height {
            while end < (y + radius + 1).min(height) {
                let p = scratch[end * width + x];
                sum[0] += p.r as u32;
                sum[1] += p.g as u32;
                sum[2] += p.b as u32;
                end += 1;
            }
            while start < y.saturating_sub(radius) {
                let p = scratch[start * width + x];
                sum[0] -= p.r as u32;
                sum[1] -= p.g as u32;
                sum[2] -= p.b as u32;
                start += 1;
            }
            let i = (top + y) * width + x;
            // Floating controls occupy the central bar. Their Slint scrim is
            // already behind them; keep their foreground pixels crisp.
            if bottom && x.abs_diff(width / 2) < width * 3 / 8 {
                continue;
            }
            let p = rgb[i];
            let distance = if bottom { y } else { height - 1 - y };
            let strength = (distance * 256 / height) as u32;
            let n = (end - start) as u32;
            let channel = |original: u8, total: u32, background: u32| {
                let blurred = (original as u32 * (256 - strength) + total / n * strength) / 256;
                // Fade into the window background at the very top.
                (blurred * (256 - strength) + background * strength) / 256
            };
            argb[i] = 0xff000000
                | channel(p.r, sum[0], if bottom { 11 } else { 10 }) << 16
                | channel(p.g, sum[1], if bottom { 13 } else { 10 }) << 8
                | channel(p.b, sum[2], if bottom { 16 } else { 12 });
        }
    }
}

/// Linux evdev codes are not ASCII: KEY_D (32) must not become a space.
#[derive(Default)]
struct EvdevKeyboard {
    shifts: u8,
    caps: bool,
    pressed: alloc::collections::BTreeMap<u16, SharedString>,
}

impl EvdevKeyboard {
    fn translate(&mut self, code: u16, down: bool) -> Option<SharedString> {
        if code == 42 || code == 54 {
            let bit = if code == 42 { 1 } else { 2 };
            if down {
                self.shifts |= bit;
            } else {
                self.shifts &= !bit;
            }
        }
        if code == 58 && down && !self.pressed.contains_key(&code) {
            self.caps = !self.caps;
        }
        if !down {
            return self.pressed.remove(&code).filter(|text| !text.is_empty());
        }
        if let Some(text) = self.pressed.get(&code) {
            return Some(text.clone());
        }
        let shift = self.shifts != 0;
        let special = match code {
            1 => Some(Key::Escape),
            14 => Some(Key::Backspace),
            15 => Some(Key::Tab),
            28 => Some(Key::Return),
            29 => Some(Key::Control),
            97 => Some(Key::ControlR),
            42 => Some(Key::Shift),
            54 => Some(Key::ShiftR),
            56 => Some(Key::Alt),
            100 => Some(Key::AltGr),
            102 => Some(Key::Home),
            103 => Some(Key::UpArrow),
            104 => Some(Key::PageUp),
            105 => Some(Key::LeftArrow),
            106 => Some(Key::RightArrow),
            107 => Some(Key::End),
            108 => Some(Key::DownArrow),
            109 => Some(Key::PageDown),
            111 => Some(Key::Delete),
            125 => Some(Key::Meta),
            126 => Some(Key::MetaR),
            _ => None,
        };
        let text: SharedString = if let Some(key) = special {
            key.into()
        } else {
            let letter = match code {
                16..=25 => Some(b"qwertyuiop"[(code - 16) as usize]),
                30..=38 => Some(b"asdfghjkl"[(code - 30) as usize]),
                44..=50 => Some(b"zxcvbnm"[(code - 44) as usize]),
                _ => None,
            };
            let ch = if let Some(letter) = letter {
                (if shift ^ self.caps {
                    letter.to_ascii_uppercase()
                } else {
                    letter
                }) as char
            } else {
                match code {
                    2..=11 => {
                        (if shift { b"!@#$%^&*()" } else { b"1234567890" })[(code - 2) as usize]
                            as char
                    }
                    12 => {
                        if shift {
                            '_'
                        } else {
                            '-'
                        }
                    }
                    13 => {
                        if shift {
                            '+'
                        } else {
                            '='
                        }
                    }
                    26 => {
                        if shift {
                            '{'
                        } else {
                            '['
                        }
                    }
                    27 => {
                        if shift {
                            '}'
                        } else {
                            ']'
                        }
                    }
                    39 => {
                        if shift {
                            ':'
                        } else {
                            ';'
                        }
                    }
                    40 => {
                        if shift {
                            '"'
                        } else {
                            '\''
                        }
                    }
                    41 => {
                        if shift {
                            '~'
                        } else {
                            '`'
                        }
                    }
                    43 => {
                        if shift {
                            '|'
                        } else {
                            '\\'
                        }
                    }
                    51 => {
                        if shift {
                            '<'
                        } else {
                            ','
                        }
                    }
                    52 => {
                        if shift {
                            '>'
                        } else {
                            '.'
                        }
                    }
                    53 => {
                        if shift {
                            '?'
                        } else {
                            '/'
                        }
                    }
                    57 => ' ',
                    // Keep CapsLock tracked across repeats even though it emits no text.
                    58 => {
                        self.pressed.insert(code, SharedString::default());
                        return None;
                    }
                    _ => return None,
                }
            };
            ch.into()
        };
        self.pressed.insert(code, text.clone());
        Some(text)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use slint::Model;

    #[test]
    fn only_inline_bold_blocks_allocate_word_measurements() {
        let blocks = markdown_text("Plain text\n- A list item\n```\n**literal code**\n```\nA **bold** word");
        for i in 0..blocks.row_count() - 1 {
            assert_eq!(blocks.row_data(i).unwrap().words.row_count(), 0);
        }
        assert!(blocks.row_data(blocks.row_count() - 1).unwrap().words.row_count() > 0);
        let words = ModelRc::new(VecModel::from(vec![MarkdownWordView {
            text: "test".into(), width: 10.0, height: 20.0, ..Default::default()
        }]));
        assert!(layout_words(words.clone(), 0, 10.0, 20.0, 100.0) == words,
            "identical measurements must preserve the existing model");
    }

    // Only one test owns Slint's process-wide, single-context platform.
    fn assert_dynamic_glyph_coverage(ui: &mut FerrofluidSystemUi) {
        ui.set_transcript("X");
        for _ in 0..4 { ui.render(Duration::ZERO); }
        let baseline = ui.pixels().to_vec();
        for glyph in ['"', '“', '”', '‘', '’', '—', '–', '…', 'é', 'ã', 'ç', '€', '`'] {
            ui.set_transcript(&alloc::format!("X{glyph}"));
            for _ in 0..4 { ui.render(Duration::ZERO); }
            assert_ne!(ui.pixels(), baseline.as_slice(), "dynamic glyph {glyph:?} must paint pixels");
        }
        ui.set_transcript("Straight quotes: \"hello\"\nSmart quotes: “hello” ‘hello’\nApostrophes: don't, don’t\nAccents: São Paulo, café, ação\nPunctuation: — – … € £");
        for _ in 0..4 { ui.render(Duration::ZERO); }
        if let Ok(path) = std::env::var("ALLOYOS_GLYPH_SCREENSHOT") {
            let file = std::fs::File::create(path).unwrap();
            let mut encoder = png::Encoder::new(file, ui.size.width, ui.size.height);
            encoder.set_color(png::ColorType::Rgb);
            encoder.set_depth(png::BitDepth::Eight);
            let bytes: Vec<u8> = ui.pixels().iter().flat_map(|p|[(p>>16) as u8,(p>>8) as u8,*p as u8]).collect();
            encoder.write_header().unwrap().write_image_data(&bytes).unwrap();
        }
    }

    fn assert_long_composer_scrolling(ui: &mut FerrofluidSystemUi) {
        // This fixture tests caret scrolling. Button opening is covered by the
        // standalone integration test, outside this fixture's voice animations.
        ui.open_keyboard();
        assert!(ui.ui.get_keyboard_visible(), "composer fixture opening must focus the editor");
        let text = "This is a long chat message that should keep the newest characters visible while typing. ".repeat(4);
        for ch in text.chars() { ui.ui.invoke_soft_key(ch.to_string().into()); }
        ui.render(Duration::ZERO);
        assert_eq!(ui.ui.get_draft().as_str(), text);
        assert!(ui.ui.get_composer_scroll_offset() > 0.0, "long text must scroll to follow the caret");
        for (key, at_start) in [(Key::LeftArrow, true), (Key::RightArrow, false)] {
            for _ in text.chars() {
                ui.ui.window().dispatch_event(WindowEvent::KeyPressed { text: key.into() });
                ui.ui.window().dispatch_event(WindowEvent::KeyReleased { text: key.into() });
            }
            ui.render(Duration::ZERO);
            if at_start { assert_eq!(ui.ui.get_composer_scroll_offset(), 0.0); }
            else { assert!(ui.ui.get_composer_scroll_offset() > 0.0); }
        }
        for _ in text.chars() { ui.ui.invoke_soft_key("backspace".into()); }
        ui.render(Duration::ZERO);
        assert!(ui.ui.get_draft().is_empty());
        assert_eq!(ui.ui.get_composer_scroll_offset(), 0.0, "empty input returns to the start");
        ui.ui.invoke_release_editor();
        ui.render(Duration::ZERO);
    }

    fn assert_mixed_chat_vertical_scrolling(ui: &mut FerrofluidSystemUi) {
        use alloyos_mcp::protocol::TileItem;

        let mut history = Vec::new();
        for index in 0..12 {
            history.push(ChatItem::Text(alloc::format!(
                "You: Plan section {index}.\nFirst detail.\nSecond detail.\nThird detail."
            )));
            history.push(ChatItem::Tile(AppTile {
                tile_id: alloc::format!("scroll-fixture.{index}"),
                title: alloc::format!("Plan tile {index}"),
                subtitle: "Details embedded in the conversation".into(),
                refresh_resource_uri: None,
                image: None,
                items: vec![TileItem {
                    id: "detail".into(),
                    title: alloc::format!("Task {index}"),
                    subtitle: "Leave time for a break.".into(),
                    badge: String::new(),
                    status: "Ready".into(),
                }],
                actions: Vec::new(),
            }));
            history.push(ChatItem::Text(alloc::format!(
                "Welder: Section {index} is ready.\nReview the tile above before continuing."
            )));
        }
        ui.ui.invoke_release_editor();
        ui.ui.set_sidebar_open(false);
        // Cards must contribute their full height to the scroll range even
        // when there are no text rows supplying layout constraints.
        let cards: Vec<_> = history.iter().filter(|item| matches!(item, ChatItem::Tile(_))).cloned().collect();
        ui.set_chat_items(&cards);
        for frame in 0..=60 {
            ui.render(Duration::from_millis(2000 + frame * 16));
        }
        let cards_maximum = ui.chat_scroll_maximum();
        let mut doubled_cards = cards.clone();
        doubled_cards.extend(cards);
        ui.set_chat_items(&doubled_cards);
        for frame in 0..=60 {
            ui.render(Duration::from_millis(3000 + frame * 16));
        }
        assert!(ui.chat_scroll_maximum() - cards_maximum > 12.0 * 150.0,
            "each additional card must contribute its rendered height to the scroll range");
        let short_cards_maximum = ui.chat_scroll_maximum();
        for card in &mut doubled_cards {
            if let ChatItem::Tile(tile) = card {
                tile.subtitle = "A long card detail that must remain reachable. ".repeat(30);
            }
        }
        ui.set_chat_items(&doubled_cards);
        for frame in 0..=60 {
            ui.render(Duration::from_millis(4000 + frame * 16));
        }
        assert!(ui.chat_scroll_maximum() - short_cards_maximum > 24.0 * 400.0,
            "growing card content must grow the scroll range by its full height");
        // A single generated tile can be taller than the entire viewport.
        // Its repeated rows must remain in the scroll extent rather than clip.
        let mut tall_tile = match &doubled_cards[0] {
            ChatItem::Tile(tile) => tile.clone(),
            _ => unreachable!(),
        };
        tall_tile.subtitle.clear();
        tall_tile.items = vec![tall_tile.items[0].clone(); 40];
        ui.set_chat_items(&[ChatItem::Tile(tall_tile.clone())]);
        for frame in 0..=10 { ui.render(Duration::from_millis(4980 + frame)); }
        assert!(ui.chat_scroll_maximum() > 40.0 * 70.0 - ui.chat_viewport_height(),
            "a tall generated tile must expose every item through scrolling");
        // User Knowledge can remain taller than the viewport while its editor
        // is open. Keyboard reveal must reduce only the viewport, never the
        // card's content extent or the ability to reach its final actions.
        // The floating-control fade reserves 80px of scrollable clearance.
        // That clearance disappears when the keyboard and composer take over.
        let tall_content_height = ui.chat_scroll_maximum() + ui.chat_viewport_height() - 80.0;
        let full_viewport_height = ui.chat_viewport_height();
        ui.open_keyboard();
        for frame in 0..=60 { ui.render(Duration::from_millis(5000 + frame * 16)); }
        assert!(ui.chat_viewport_height() < full_viewport_height - 100.0,
            "keyboard must actually occupy space in the regression fixture");
        assert!((ui.chat_scroll_maximum() + ui.chat_viewport_height() - tall_content_height).abs() < 2.0,
            "keyboard reveal must preserve every row of an oversized card");
        assert!((ui.chat_scroll_offset() - ui.chat_scroll_maximum()).abs() < 1.0,
            "the oversized card's final actions remain reachable above the keyboard");
        ui.ui.invoke_release_editor();
        for frame in 0..=60 { ui.render(Duration::from_millis(6000 + frame * 16)); }
        ui.set_tiles(&[tall_tile.clone()]);
        ui.restore_dashboard(&[tall_tile.tile_id.clone()]);
        ui.ui.set_sidebar_open(true);
        for frame in 0..=10 { ui.render(Duration::from_millis(7000 + frame)); }
        assert!(ui.ui.get_pinned_content_height() > 40.0 * 70.0,
            "pinned cards must reserve the full height of a tall generated tile");
        ui.ui.set_sidebar_open(false);
        ui.restore_dashboard(&[]);
        ui.set_chat_items(&history);
        for frame in 0..=60 {
            ui.render(Duration::from_millis(7100 + frame * 16));
        }
        assert!(ui.chat_scroll_maximum() > 1000.0, "mixed content must overflow the viewport");
        assert!((ui.chat_scroll_offset() - ui.chat_scroll_maximum()).abs() < 1.0,
            "new mixed chat content must reveal the latest message after layout");
        let initial_offset = ui.chat_scroll_offset();
        let initial_pixels = ui.pixels().to_vec();
        let first_drag_down = initial_offset > ui.chat_scroll_maximum() * 0.5;

        let mut now_ms = 8060;
        let mut drag = |ui: &mut FerrofluidSystemUi, start_y: i32, positions: &[i32]| {
            ui.handle_input(InputEvent { kind: InputEventKind::PointerButtonDown, code: 0x110, x: 500, y: start_y });
            for &y in positions {
                ui.handle_input(InputEvent { kind: InputEventKind::PointerMove, code: 0x110, x: 500, y });
                now_ms += 16;
                ui.render(Duration::from_millis(now_ms));
            }
            ui.handle_input(InputEvent { kind: InputEventKind::PointerButtonUp, code: 0x110, x: 500, y: *positions.last().unwrap() });
            for _ in 0..90 {
                now_ms += 16;
                ui.render(Duration::from_millis(now_ms));
            }
        };
        if first_drag_down {
            drag(ui, 700, &[750, 850, 1050, 1250, 1450]);
        } else {
            drag(ui, 1450, &[1400, 1300, 1100, 900, 700]);
        }
        let reading_offset = ui.chat_scroll_offset();
        let movement = reading_offset - initial_offset;
        assert!(if first_drag_down { movement < -100.0 } else { movement > 100.0 },
            "a vertical drag across mixed text and tiles must move the history in the intended direction");
        assert!(ui.pixels() != initial_pixels.as_slice(), "scrolling must repaint the visible history");
        assert!(reading_offset >= 0.0 && reading_offset <= ui.chat_scroll_maximum());

        ui.set_chat_items(&history);
        // The drag helper has advanced the owner clock; a fixed-time render
        // still processes any model update without introducing more inertia.
        ui.render(ui.now.get());
        assert!((ui.chat_scroll_offset() - reading_offset).abs() < 1.0,
            "refreshing mixed content must preserve the reading position");

        if first_drag_down {
            drag(ui, 1450, &[1400, 1300, 1100, 900, 700]);
        } else {
            drag(ui, 700, &[750, 850, 1050, 1250, 1450]);
        }
        let reverse_movement = ui.chat_scroll_offset() - reading_offset;
        assert!(if first_drag_down { reverse_movement > 50.0 } else { reverse_movement < -50.0 },
            "the reverse drag must move through the history in the opposite direction");
        assert!(ui.chat_scroll_offset() <= ui.chat_scroll_maximum());
        assert_eq!(ui.chat_items(), history, "scrolling must preserve text/tile ordering and content");
        history.push(ChatItem::Text("You: Show the latest message.".into()));
        ui.set_chat_items(&history);
        ui.render(Duration::from_millis(now_ms + 16));
        assert!((ui.chat_scroll_offset() - ui.chat_scroll_maximum()).abs() < 1.0,
            "appended message must be visible after reading older history");
        ui.set_reply_pending(true);
        ui.render(Duration::from_millis(now_ms + 32));
        assert!((ui.chat_scroll_offset() - ui.chat_scroll_maximum()).abs() < 1.0,
            "loading indicator must remain visible as content grows");
        ui.set_reply_pending(false);
        ui.render(Duration::from_millis(now_ms + 48));
        assert!(ui.take_tile_event().is_none(), "scrolling must not dispatch tile actions");
    }

    fn launcher_fixture_for_reorder(template: &AppTile, id: &str, title: &str) -> AppTile {
        let mut tile = template.clone();
        tile.tile_id = id.into();
        tile.title = title.into();
        tile.subtitle.clear();
        tile.refresh_resource_uri = None;
        tile.items.clear();
        tile.actions.clear();
        tile
    }

    #[test]
    fn system_ui_renders_and_accepts_local_input() {
        use alloyos_proto::display::{PHONE_HEIGHT, PHONE_SCALE, PHONE_WIDTH};
        let mut ui =
            FerrofluidSystemUi::new(PHONE_WIDTH, PHONE_HEIGHT, PHONE_SCALE as f32).unwrap();
        ui.set_booting(true);
        ui.render(Duration::ZERO).unwrap();
        assert_eq!(ui.pixels()[0], 0xff000000);
        assert!(ui.pixels().contains(&0xffffffff));
        for (i, &pixel) in ui.pixels().iter().enumerate() {
            if pixel != 0xff000000 {
                let x = i as u32 % PHONE_WIDTH;
                let y = i as u32 / PHONE_WIDTH;
                assert!(x > PHONE_WIDTH / 3 && x < PHONE_WIDTH * 2 / 3);
                assert!(y > PHONE_HEIGHT / 2 - 100 && y < PHONE_HEIGHT / 2 + 100);
            }
        }
        let initial = ui.pixels().to_vec();
        let boot_damage = ui.render(Duration::from_millis(16)).unwrap();
        assert!(boot_damage.width < PHONE_WIDTH);
        assert!(boot_damage.height < PHONE_HEIGHT);
        assert_ne!(ui.pixels(), initial.as_slice(), "boot turbulence advances");
        assert_eq!(ui.pixels()[0], 0xff000000);
        let animated = ui.pixels().to_vec();
        ui.render(Duration::from_millis(16));
        assert_eq!(
            ui.pixels(),
            animated.as_slice(),
            "fixed-time sampling must not drift"
        );
        ui.set_booting(false);
        let damage = ui.render(Duration::ZERO).unwrap();
        assert!(damage.width > 0 && damage.height > 0);
        assert_eq!(ui.pixels().len(), (PHONE_WIDTH * PHONE_HEIGHT) as usize);
        assert!(ui.pixels().iter().all(|pixel| pixel >> 24 == 255));
        assert!(ui.pixels().iter().any(|pixel| *pixel != ui.pixels()[0]));
        assert!(ui.previous_blur_top.is_none(), "chat starts without edge blur");
        let original_transcript = ui.transcript();
        assert_dynamic_glyph_coverage(&mut ui);
        let example = "**Explore my capabilities:** I can tell you about the system and help with your tasks.";
        ui.set_transcript(example);
        for frame in 0..4 { ui.render(Duration::from_millis(frame * 16)); }
        let bold_pixels = ui.pixels().to_vec();
        if let Ok(path) = std::env::var("ALLOYOS_MARKDOWN_SCREENSHOT") {
            let file = std::fs::File::create(path).unwrap();
            let mut encoder = png::Encoder::new(file, ui.size.width, ui.size.height);
            encoder.set_color(png::ColorType::Rgba);
            encoder.set_depth(png::BitDepth::Eight);
            let bytes: Vec<u8> = bold_pixels.iter().flat_map(|p| [(p >> 16) as u8, (p >> 8) as u8, *p as u8, 255]).collect();
            encoder.write_header().unwrap().write_image_data(&bytes).unwrap();
        }
        ui.set_transcript("Explore my capabilities: I can tell you about the system and help with your tasks.");
        for frame in 0..4 { ui.render(Duration::from_millis(frame * 16)); }
        assert_ne!(ui.pixels(), bold_pixels.as_slice(), "inline bold must affect rendered pixels");
        ui.set_transcript(&example.repeat(30));
        for frame in 0..4 { ui.render(Duration::from_millis(frame * 16)); }
        assert!(ui.chat_scroll_maximum() > 0.0, "inline bold paragraphs must wrap and grow the transcript");
        ui.set_transcript(&original_transcript);
        // Voice intent starts only on an explicit mic tap; typing emits mute.
        assert!(!ui.take_voice_toggle());
        for kind in [InputEventKind::PointerButtonDown, InputEventKind::PointerButtonUp] {
            ui.handle_input(InputEvent { kind, code: 0x110, x: 459, y: 2454 });
        }
        assert!(ui.take_voice_toggle());
        assert!(!ui.take_voice_toggle());
        ui.set_reduced_motion(true);
        ui.set_voice_state(alloyos_mcp::display::VoiceState::Listening, "Live caption", "");
        assert_eq!(ui.ui.get_voice_caption().as_str(), "Live caption");
        ui.set_reduced_motion(true);
        ui.set_voice_level(0);
        ui.render(Duration::ZERO);
        assert!(!ui.take_output_mute_toggle());
        for kind in [InputEventKind::PointerButtonDown, InputEventKind::PointerButtonUp] {
            ui.handle_input(InputEvent { kind, code: 0x110, x: 831, y: 2454 });
        }
        assert!(ui.take_output_mute_toggle());
        assert!(!ui.take_output_mute_toggle());
        assert!(!ui.take_voice_toggle(), "speaker control must not toggle capture");
        ui.set_output_muted(true);
        assert!(ui.ui.get_output_muted());
        ui.render(Duration::ZERO);
        let quiet = ui.pixels().to_vec();
        ui.set_voice_state(alloyos_mcp::display::VoiceState::Listening, "Spoken words in the conversation", "");
        ui.render(Duration::ZERO);
        let split = PHONE_WIDTH as usize * (PHONE_HEIGHT as usize / 2);
        assert_ne!(&ui.pixels()[..split], &quiet[..split], "live speech must render in chat");
        let toolbar_start = PHONE_WIDTH as usize * (PHONE_HEIGHT as usize - 300);
        // The edge wave is decorative and composited independently. Compare
        // the status/controls, not the wave's antialiased clipping fringe.
        let toolbar_end = PHONE_WIDTH as usize * (PHONE_HEIGHT as usize - 70);
        assert!(&ui.pixels()[toolbar_start..toolbar_end] == &quiet[toolbar_start..toolbar_end],
            "speech must not replace the mic status");
        ui.set_voice_state(alloyos_mcp::display::VoiceState::Listening, "Live caption", "");
        ui.render(Duration::ZERO);
        ui.set_voice_diagnostics("Mic 24 · sent 20 · transcript 0");
        ui.render(Duration::ZERO);
        assert!(&ui.pixels()[..toolbar_end] == &quiet[..toolbar_end], "voice diagnostics must stay hidden");
        assert_eq!(ui.ui.get_voice_caption().as_str(), "Live caption");
        ui.set_voice_diagnostics("");
        ui.render(Duration::ZERO);
        // Captions and mic sit above this strip; only edge-bleeding waves can fill it.
        let assert_wave_visible = |pixels: &[u32]| {
            let width = PHONE_WIDTH as usize;
            let strip = &pixels[(PHONE_HEIGHT as usize - 8) * width..];
            assert!(strip.iter().filter(|pixel| **pixel != pixels[0]).count() > width * 4,
                "silent listening must retain a visible wave at the display edge");
        };
        assert_wave_visible(ui.pixels()); // Reduced motion at zero volume.

        ui.set_voice_level(1000);
        ui.render(Duration::ZERO);
        assert!(ui.pixels() != quiet.as_slice(), "microphone volume must change vector wave geometry");
        assert_eq!(ui.ui.get_voice_level(), 1.0);
        ui.set_voice_level(u16::MAX);
        assert_eq!(ui.ui.get_voice_level(), 1.0, "volume is clamped");
        // The merged mic remains centered and can still stop the session.
        for kind in [InputEventKind::PointerButtonDown, InputEventKind::PointerButtonUp] {
            ui.handle_input(InputEvent { kind, code: 0x110, x: 603, y: 2454 });
        }
        assert!(ui.take_voice_toggle());
        ui.set_reduced_motion(false);
        ui.open_keyboard();
        ui.render(Duration::ZERO);
        assert!(ui.take_voice_mute(), "opening keyboard must mute capture");
        ui.set_reduced_motion(true);
        ui.render(Duration::ZERO);
        assert_eq!(ui.ui.get_keyboard_reveal(), 213.0, "reduced motion opens without sliding");
        ui.ui.set_draft("preserved draft".into());
        ui.ui.invoke_release_editor();
        ui.render(Duration::ZERO);
        assert_eq!(ui.ui.get_draft().as_str(), "preserved draft");
        assert_eq!(ui.ui.get_keyboard_reveal(), 0.0, "reduced motion dismisses immediately");
        assert!(!ui.take_voice_toggle(), "dismissal must not resume capture");
        ui.ui.set_draft("".into());
        ui.set_voice_state(alloyos_mcp::display::VoiceState::Off, "", "");
        ui.render(Duration::ZERO);
        ui.set_reduced_motion(false);
        assert_long_composer_scrolling(&mut ui);
        // Tapping the model name directly requests a model change.
        for kind in [InputEventKind::PointerButtonDown, InputEventKind::PointerButtonUp] {
            ui.handle_input(InputEvent { kind, code: 0x110, x: 1050, y: 110 });
        }
        assert!(ui.take_model_cycle());
        assert!(!ui.take_model_cycle());
        for model in [
            "gemma-2-2b-it",
            "claude-sonnet-4-6",
            "gemini-3.8-flash",
            "gpt-4.1",
        ] {
            ui.set_active_model(model);
            ui.render(Duration::from_millis(16));
            for kind in [
                InputEventKind::PointerButtonDown,
                InputEventKind::PointerButtonUp,
            ] {
                ui.handle_input(InputEvent {
                    kind,
                    code: 0x110,
                    x: 1050,
                    y: 110,
                });
            }
            assert!(
                ui.take_model_cycle(),
                "tap must reach model selector for {model}"
            );
        }
        ui.set_active_model("Gemini");
        ui.set_reply_pending(true);
        ui.ui.invoke_cycle_model();
        assert!(
            !ui.take_model_cycle(),
            "pending replies prevent model changes"
        );
        ui.set_reply_pending(false);
        // Click the composer through physical-to-logical coordinate conversion.
        for kind in [
            InputEventKind::PointerButtonDown,
            InputEventKind::PointerButtonUp,
        ] {
            ui.handle_input(InputEvent {
                kind,
                code: 0x110,
                x: 771,
                y: 2454,
            });
        }
        for kind in [InputEventKind::KeyDown, InputEventKind::KeyUp] {
            ui.handle_input(InputEvent {
                kind,
                code: 32,
                x: 0,
                y: 0,
            });
        }
        assert_eq!(
            ui.ui.get_draft().as_str(),
            "d",
            "evdev KEY_D is not ASCII space"
        );
        ui.set_reply_pending(true);
        ui.render(Duration::from_millis(16));
        ui.handle_input(InputEvent {
            kind: InputEventKind::KeyDown,
            code: 28,
            x: 0,
            y: 0,
        });
        assert!(
            ui.take_submission().is_none(),
            "pending reply prevents duplicate sends"
        );
        assert_eq!(ui.ui.get_draft().as_str(), "d", "draft remains editable");
        ui.set_reply_pending(false);
        ui.handle_input(InputEvent {
            kind: InputEventKind::KeyDown,
            code: 28,
            x: 0,
            y: 0,
        });
        assert_eq!(ui.take_submission().as_deref(), Some("d"));
        assert_eq!(ui.ui.get_draft().as_str(), "");
        assert!(ui.take_submission().is_none());
        assert!(
            ui.ui.get_keyboard_visible(),
            "focused editor requests soft keys"
        );
        ui.render(Duration::from_millis(32));
        ui.render(Duration::from_millis(560));
        assert_eq!(ui.ui.get_keyboard_reveal(), 213.0);
        // Tap Q through the real soft-key hit area; focus must stay in the editor.
        for kind in [
            InputEventKind::PointerButtonDown,
            InputEventKind::PointerButtonUp,
        ] {
            ui.handle_input(InputEvent {
                kind,
                code: 0x110,
                x: 75,
                y: 2073,
            });
        }
        assert_eq!(ui.ui.get_draft().as_str(), "q");
        ui.ui.invoke_soft_key("backspace".into());
        assert_eq!(ui.ui.get_draft().as_str(), "");
        ui.ui.invoke_soft_key("é".into());
        ui.ui.invoke_soft_key("return".into());
        assert_eq!(ui.take_submission().as_deref(), Some("é"));
        // A chat tap dismisses the keyboard without clearing an unsent draft.
        ui.ui.set_draft("Keep this draft".into());
        for kind in [InputEventKind::PointerButtonDown, InputEventKind::PointerButtonUp] {
            ui.handle_input(InputEvent { kind, code: 0x14a, x: 400, y: 1000 });
        }
        assert!(!ui.ui.get_keyboard_visible(), "chat tap releases editor focus");
        assert_eq!(ui.ui.get_draft().as_str(), "Keep this draft");
        ui.ui.set_draft("".into());
        // Hide removes focus, allowing a subsequent composer tap to reopen it.
        for kind in [
            InputEventKind::PointerButtonDown,
            InputEventKind::PointerButtonUp,
        ] {
            ui.handle_input(InputEvent {
                kind,
                code: 0x110,
                x: 75,
                y: 2530,
            });
        }
        assert!(!ui.ui.get_keyboard_visible());
        ui.render(Duration::from_millis(576));
        ui.render(Duration::from_millis(1120));
        assert_eq!(ui.ui.get_keyboard_reveal(), 0.0);
        ui.set_transcript(&"Scrollable transcript line\n".repeat(80));
        for kind in [
            InputEventKind::PointerButtonDown,
            InputEventKind::PointerButtonUp,
        ] {
            ui.handle_input(InputEvent {
                kind,
                code: 0x110,
                x: 771,
                y: 2454,
            });
        }
        ui.render(Duration::from_millis(1136));
        ui.render(Duration::from_millis(1680));
        assert!(ui.ui.get_keyboard_visible(), "composer reopens keyboard");
        assert_eq!(ui.ui.get_keyboard_reveal(), 213.0);
        ui.handle_input(InputEvent {
            kind: InputEventKind::PointerButtonDown,
            code: 0x110,
            x: 400,
            y: 1000,
        });
        // Render during the drag so scroll physics and keyboard reveal animate
        // together. Batching every move before rendering missed a binding cycle.
        for (frame, y) in [990, 980, 970, 960, 950, 900, 750, 600, 450]
            .into_iter()
            .enumerate()
        {
            ui.handle_input(InputEvent {
                kind: InputEventKind::PointerMove,
                code: 0,
                x: 400,
                y,
            });
            ui.render(Duration::from_millis(1680 + (frame as u64 + 1) * 16));
        }
        ui.handle_input(InputEvent {
            kind: InputEventKind::PointerButtonUp,
            code: 0x110,
            x: 400,
            y: 450,
        });
        ui.render(Duration::from_millis(1840));
        ui.render(Duration::from_millis(2384));
        assert!(ui.ui.get_scroll_offset() > 0.0);
        assert!(ui.previous_blur_top.is_some(), "scrolled chat blurs at the header");
        assert_eq!(
            ui.ui.get_keyboard_reveal(),
            0.0,
            "scrolling fully hides keyboard"
        );
        assert!(
            !ui.ui.get_keyboard_visible(),
            "scrolling removes editor focus"
        );
        for kind in [
            InputEventKind::PointerButtonDown,
            InputEventKind::PointerButtonUp,
        ] {
            ui.handle_input(InputEvent {
                kind,
                code: 0x110,
                x: 771,
                y: 2454,
            });
        }
        ui.render(Duration::from_millis(2400));
        ui.render(Duration::from_millis(2944));
        assert_eq!(
            ui.ui.get_keyboard_reveal(),
            213.0,
            "composer reopens after scrolling"
        );
        let key_y = 2073;
        for kind in [
            InputEventKind::PointerButtonDown,
            InputEventKind::PointerButtonUp,
        ] {
            ui.handle_input(InputEvent {
                kind,
                code: 0x110,
                x: 75,
                y: key_y,
            });
        }
        assert_eq!(
            ui.ui.get_draft().as_str(),
            "q",
            "soft keys work after a flick"
        );
        ui.render(Duration::from_millis(2960));
        let base = "Welcome to alloyOS.\n\nYou: Stream test";
        ui.set_reply_pending(true);
        ui.set_transcript(base);
        assert!(ui.render(Duration::from_millis(2976)).is_some());
        assert_eq!(ui.transcript(), base);
        ui.update_streaming_reply(base, "Hello");
        assert!(ui.render(Duration::from_millis(2992)).is_some());
        assert_eq!(
            ui.transcript(),
            "Welcome to alloyOS.\n\nYou: Stream test\n\nWelder: Hello"
        );
        ui.update_streaming_reply(base, "Hello alloyOS!");
        ui.set_reply_pending(false);
        assert!(ui.render(Duration::from_millis(3008)).is_some());
        assert_eq!(
            ui.transcript(),
            "Welcome to alloyOS.\n\nYou: Stream test\n\nWelder: Hello alloyOS!"
        );
        let refresh = TileAction::new("refresh", "Refresh", "fixture.now", Default::default());
        ui.set_tiles(&[AppTile {
            tile_id: "fixture.now".into(),
            title: "Clock".into(),
            subtitle: String::new(),
            refresh_resource_uri: None,
            image: None,
            items: Vec::new(),
            actions: vec![refresh.clone()],
        }]);
        ui.ui.invoke_pin_tile("fixture.now".into(), true);
        assert_eq!(
            ui.dashboard_state().pinned_ids(),
            &[String::from("fixture.now")]
        );
        assert_eq!(ui.ui.get_dashboard_tiles().row_count(), 1);
        assert!(ui.ui.invoke_tile_is_pinned("fixture.now".into(), ui.ui.get_dashboard_pins()));
        ui.ui.invoke_pin_tile("fixture.now".into(), false);
        assert!(ui.dashboard_state().pinned_ids().is_empty());
        assert!(!ui.ui.invoke_tile_is_pinned("fixture.now".into(), ui.ui.get_dashboard_pins()));
        ui.ui.invoke_pin_tile("fixture.now".into(), true);
        assert!(!ui.ui.get_sidebar_open(), "pins start collapsed");
        ui.ui.set_sidebar_open(true);
        ui.set_reply_pending(true);
        assert!(!ui.ui.get_sidebar_open(), "a conversation reply collapses pins");
        ui.set_reply_pending(false);
        ui.ui.invoke_pin_tile("missing.tile".into(), true);
        assert_eq!(ui.dashboard_state().pinned_ids().len(), 1);
        ui.ui
            .invoke_tile_action("fixture.now".into(), "refresh".into());
        assert_eq!(
            ui.take_tile_event(),
            Some(crate::TileEvent::Action {
                tile_id: "fixture.now".into(),
                action: refresh.clone()
            })
        );
        ui.ui
            .invoke_tile_action("fixture.now".into(), "refresh".into());
        assert_eq!(ui.take_tile_action(), Some(refresh));
        let mut clock = ui.tiles.borrow()[0].clone();
        clock.refresh_resource_uri = Some("mcp://fixture/now".into());
        let mut second = clock.clone();
        second.tile_id = "fixture.board".into();
        let mut third = clock.clone();
        third.tile_id = "notes.summary".into();
        ui.set_tiles(&[clock.clone(), second.clone(), third.clone()]);
        ui.restore_dashboard(&[
            clock.tile_id.clone(),
            second.tile_id.clone(),
            third.tile_id.clone(),
        ]);
        assert_eq!(
            ui.ui.get_dashboard_tiles().row_count(),
            3,
            "drawer keeps every pin in its persisted order"
        );
        assert_eq!(ui.ui.get_dashboard_tiles().row_data(0).unwrap().tile_id.as_str(), "fixture.now");
        third.actions.push(TileAction::new(
            "edit",
            "Edit",
            "notes.edit",
            Default::default(),
        ));
        ui.set_tiles(&[clock.clone(), second.clone(), third]);
        assert_eq!(ui.ui.get_dashboard_tiles().row_data(2).unwrap().actions.row_count(), 2);
        ui.ui.invoke_refresh_tile("fixture.now".into());
        assert_eq!(
            ui.take_tile_event(),
            Some(crate::TileEvent::Refresh {
                tile_id: "fixture.now".into(),
                resource_uri: "mcp://fixture/now".into()
            })
        );
        ui.set_reply_pending(true);
        ui.ui.invoke_refresh_tile("fixture.now".into());
        assert!(ui.take_tile_event().is_none(), "pending replies disable resource refresh");
        ui.set_reply_pending(false);
        ui.ui.invoke_pin_tile("fixture.board".into(), false);
        assert_eq!(ui.dashboard_state().pinned_ids().len(), 2);
        ui.ui.invoke_move_pin("notes.summary".into());
        assert_eq!(ui.dashboard_state().pinned_ids()[0], "notes.summary");
        ui.ui
            .invoke_tile_action("fixture.now".into(), "unknown".into());
        assert!(ui.take_tile_action().is_none());
        ui.set_reply_pending(true);
        ui.ui
            .invoke_tile_action("fixture.now".into(), "refresh".into());
        assert!(
            ui.take_tile_action().is_none(),
            "pending replies disable tile actions"
        );
        ui.set_reply_pending(false);
        ui.set_tiles(&[]);
        ui.ui
            .invoke_tile_action("fixture.now".into(), "refresh".into());
        assert!(ui.take_tile_action().is_some(), "pinned tiles retain their actions outside chat");
        ui.ui.invoke_pin_tile("fixture.now".into(), false);
        ui.ui.invoke_tile_action("fixture.now".into(), "refresh".into());
        assert!(ui.take_tile_action().is_none(), "unavailable unpinned tiles cannot dispatch actions");

        // Interleaved ChatItem stream preserves order: message, widget, then subsequent message below widget
        let clock_tile = AppTile {
            tile_id: "fixture.now".into(),
            title: "Clock".into(),
            subtitle: "System uptime".into(),
            refresh_resource_uri: Some("mcp://fixture/now".into()),
            image: None,
            items: vec![],
            actions: vec![TileAction::new("refresh", "Refresh", "fixture.now", alloc::collections::BTreeMap::new())],
        };
        let items = vec![
            ChatItem::Text("You: what time is it?".into()),
            ChatItem::Text("Welder: It's 10:34".into()),
            ChatItem::Tile(clock_tile.clone()),
            ChatItem::Text("You: hello".into()),
            ChatItem::Text("Welder: Hello! How can I help you?".into()),
        ];
        ui.set_chat_items(&items);
        assert_eq!(ui.chat_items().len(), 5);
        assert_eq!(ui.chat_items()[0], ChatItem::Text("You: what time is it?".into()));
        assert_eq!(ui.chat_items()[1], ChatItem::Text("Welder: It's 10:34".into()));
        assert_eq!(ui.chat_items()[2], ChatItem::Tile(clock_tile));
        assert_eq!(ui.chat_items()[3], ChatItem::Text("You: hello".into()));
        assert_eq!(ui.chat_items()[4], ChatItem::Text("Welder: Hello! How can I help you?".into()));

        // Embedded tile actions in chat_items dispatch properly
        ui.ui.invoke_tile_action("fixture.now".into(), "refresh".into());
        assert_eq!(
            ui.take_tile_action(),
            Some(TileAction::new("refresh", "Refresh", "fixture.now", alloc::collections::BTreeMap::new()))
        );
        assert_mixed_chat_vertical_scrolling(&mut ui);
        ui.ui.invoke_release_editor();
        ui.set_reduced_motion(true);
        ui.set_voice_state(alloyos_mcp::display::VoiceState::Listening, "", "");
        ui.set_voice_level(0);
        ui.render(ui.now.get());
        assert_wave_visible(ui.pixels());
        ui.set_reduced_motion(false);
        let start = ui.now.get();
        for phase in 0..32 {
            ui.render(start + Duration::from_millis(phase * 150));
            assert_wave_visible(ui.pixels());
        }
        // A 100ms microphone update must interpolate across display frames.
        // Wave phase must advance every frame, even within one 300ms interval.
        let start = ui.now.get();
        let initial_phase = ui.ui.get_wave_phase();
        ui.set_voice_level(300);
        ui.render(start);
        let mut previous = ui.ui.get_wave_level();
        for frame in 1..=6 {
            ui.render(start + Duration::from_millis(frame * 16));
            let level = ui.ui.get_wave_level();
            assert!(level > previous && level < 0.9,
                "microphone changes must interpolate rather than snap: {previous} -> {level}");
            let phase_delta = ui.ui.get_wave_phase() - initial_phase;
            assert!((phase_delta - frame as f32 * 16.0 / 300.0).abs() < 0.001,
                "wave phase must advance smoothly each frame");
            previous = level;
        }
        // Unpin state changes immediately while the drawer retains an exit row.
        ui.set_voice_state(alloyos_mcp::display::VoiceState::Off, "", "");
        ui.restore_dashboard(&["fixture.now".into()]);
        ui.ui.set_sidebar_open(true);
        let start = ui.now.get();
        ui.render(start + Duration::from_millis(300));
        let start = ui.now.get();
        ui.ui.invoke_pin_tile("fixture.now".into(), false);
        assert!(ui.dashboard_state().pinned_ids().is_empty());
        assert_eq!(ui.ui.get_dashboard_pins().row_count(), 0);
        assert_eq!(ui.ui.get_dashboard_tiles().row_count(), 1);
        ui.render(start);
        let before_exit = ui.pixels().to_vec();
        ui.render(start + Duration::from_millis(100));
        assert_ne!(ui.pixels(), before_exit.as_slice(), "unpin visibly slides out");
        assert_eq!(ui.ui.get_dashboard_tiles().row_count(), 1);
        ui.render(start + Duration::from_millis(320));
        assert_eq!(ui.ui.get_dashboard_tiles().row_count(), 1);
        ui.render(start + Duration::from_millis(460));
        assert_eq!(ui.ui.get_dashboard_tiles().row_count(), 0);
        ui.ui.invoke_pin_tile("fixture.now".into(), true);
        ui.render(ui.now.get());
        ui.ui.invoke_pin_tile("fixture.now".into(), false);
        ui.ui.invoke_pin_tile("fixture.now".into(), true);
        ui.render(ui.now.get() + Duration::from_millis(460));
        assert_eq!(ui.ui.get_dashboard_tiles().row_count(), 1, "re-pin cancels removal");
        ui.set_reduced_motion(true);
        ui.ui.invoke_pin_tile("fixture.now".into(), false);
        assert_eq!(ui.ui.get_dashboard_tiles().row_count(), 0);

        // Reordering is reachable through the pinned card's actual native
        // control, without changing membership or opening discovery.
        let reorder_first = launcher_fixture_for_reorder(&clock, "reorder.first", "First");
        let reorder_second = launcher_fixture_for_reorder(&clock, "reorder.second", "Second");
        ui.set_tiles(&[reorder_first.clone(), reorder_second.clone()]);
        ui.restore_dashboard(&[reorder_first.tile_id.clone(), reorder_second.tile_id.clone()]);
        ui.ui.set_sidebar_open(true);
        ui.render(ui.now.get() + Duration::from_millis(16));
        if let Ok(path) = std::env::var("ALLOYOS_REORDER_SCREENSHOT") {
            let file = std::fs::File::create(path).unwrap();
            let mut encoder = png::Encoder::new(file, ui.size.width, ui.size.height);
            encoder.set_color(png::ColorType::Rgba);
            encoder.set_depth(png::BitDepth::Eight);
            let bytes: Vec<u8> = ui.pixels().iter().flat_map(|p| [(p >> 16) as u8, (p >> 8) as u8, *p as u8, 255]).collect();
            encoder.write_header().unwrap().write_image_data(&bytes).unwrap();
        }

        let initial_order = ui.dashboard_state().pinned_ids().to_vec();
        let initial_revision = ui.dashboard_state().revision();
        for kind in [InputEventKind::PointerButtonDown, InputEventKind::PointerButtonUp] {
            ui.handle_input(InputEvent { kind, code: 0x110, x: 735, y: 390 });
        }
        assert_eq!(ui.dashboard_state().revision(), initial_revision, "top row cannot move");
        ui.ui.set_reply_pending(true);
        for kind in [InputEventKind::PointerButtonDown, InputEventKind::PointerButtonUp] {
            ui.handle_input(InputEvent { kind, code: 0x110, x: 735, y: 660 });
        }
        assert_eq!(ui.dashboard_state().pinned_ids(), initial_order, "pending reply disables reorder");
        ui.ui.set_reply_pending(false);
        for kind in [InputEventKind::PointerButtonDown, InputEventKind::PointerButtonUp] {
            ui.handle_input(InputEvent { kind, code: 0x110, x: 735, y: 660 });
        }
        ui.render(ui.now.get() + Duration::from_millis(16));
        let reordered = ui.dashboard_state().pinned_ids().to_vec();
        assert_eq!(reordered, [String::from("reorder.second"), String::from("reorder.first")]);
        assert!(ui.dashboard_state().revision() > initial_revision, "reorder reaches the existing persistence publisher");
        ui.restore_dashboard(&initial_order);
        ui.restore_dashboard(&reordered);
        assert_eq!(ui.ui.get_dashboard_tiles().row_data(0).unwrap().tile_id.as_str(), "reorder.second");
        assert!(!ui.ui.get_available_tiles_expanded());
        assert!(!ui.take_tile_discovery_request(), "ordering never loads the app catalog");

        // Discovery works before an app has ever appeared in chat. Pinning its
        // actual exposed descriptor makes its actions available in the drawer.
        let launcher_tile = AppTile {
            tile_id: "discovery.board".into(),
            title: "Project board".into(),
            subtitle: "Track the next steps".into(),
            refresh_resource_uri: Some("mcp://discovery/board".into()),
            image: None,
            items: Vec::new(),
            actions: vec![TileAction::new("list", "Show tasks", "discovery.list", Default::default())],
        };
        ui.set_chat_items(&[]);
        ui.set_tiles(&[]);
        ui.restore_dashboard(&[]);
        ui.set_available_tiles(&[]);
        ui.set_tile_discovery_status(false, "");
        assert!(ui.chat_items().is_empty());
        assert_eq!(ui.ui.get_available_tiles().row_count(), 0);
        ui.ui.set_sidebar_open(true);
        ui.render(ui.now.get() + Duration::from_millis(16));
        assert!(!ui.ui.get_available_tiles_expanded(), "discovery starts collapsed");
        assert!(!ui.take_tile_discovery_request(), "opening the drawer never loads the catalog");
        for kind in [InputEventKind::PointerButtonDown, InputEventKind::PointerButtonUp] {
            ui.handle_input(InputEvent { kind, code: 0x110, x: 900, y: 710 });
        }
        assert!(ui.dashboard_state().pinned_ids().is_empty(), "collapsed discovery has no pin target");
        for kind in [InputEventKind::PointerButtonDown, InputEventKind::PointerButtonUp] {
            ui.handle_input(InputEvent { kind, code: 0x110, x: 450, y: 500 });
        }
        ui.render(ui.now.get() + Duration::from_millis(16));
        assert!(ui.ui.get_available_tiles_expanded(), "disclosure opens by pointer");
        assert!(ui.take_tile_discovery_request(), "expansion requests catalog metadata");
        assert!(!ui.take_tile_discovery_request(), "discovery requests coalesce and drain");
        // Simulate the host's asynchronous discovery reply.
        ui.set_available_tiles(&[launcher_tile.clone()]);
        ui.set_tile_discovery_status(true, "");
        ui.render(ui.now.get() + Duration::from_millis(16));
        assert_eq!(ui.ui.get_available_tiles().row_count(), 1);
        assert_eq!(ui.ui.get_available_tiles().row_data(0).unwrap().title.as_str(), "Project board");
        if let Ok(path) = std::env::var("ALLOYOS_DISCOVERY_SCREENSHOT") {
            let file = std::fs::File::create(path).unwrap();
            let mut encoder = png::Encoder::new(file, ui.size.width, ui.size.height);
            encoder.set_color(png::ColorType::Rgba);
            encoder.set_depth(png::BitDepth::Eight);
            let bytes: Vec<u8> = ui.pixels().iter().flat_map(|p| [(p >> 16) as u8, (p >> 8) as u8, *p as u8, 255]).collect();
            encoder.write_header().unwrap().write_image_data(&bytes).unwrap();
        }
        // The add control is a real pointer target in the discovery row, rather
        // than relying on chat to expose the pin action.
        for kind in [InputEventKind::PointerButtonDown, InputEventKind::PointerButtonUp] {
            ui.handle_input(InputEvent { kind, code: 0x110, x: 900, y: 660 });
        }
        ui.render(ui.now.get() + Duration::from_millis(16));
        assert_eq!(ui.dashboard_state().pinned_ids(), &[String::from("discovery.board")]);
        assert_eq!(ui.ui.get_available_tiles().row_count(), 0);
        ui.ui.invoke_tile_action("discovery.board".into(), "list".into());
        assert_eq!(ui.take_tile_event(), Some(crate::TileEvent::Action {
            tile_id: "discovery.board".into(), action: launcher_tile.actions[0].clone(),
        }));
        ui.ui.invoke_pin_tile("discovery.board".into(), false);
        assert_eq!(ui.ui.get_available_tiles().row_count(), 1);
        ui.ui.set_available_tiles_expanded(false);
        ui.ui.set_sidebar_open(false);
        ui.render(ui.now.get() + Duration::from_millis(16));
        ui.ui.set_sidebar_open(true);
        ui.render(ui.now.get() + Duration::from_millis(16));
        assert!(!ui.ui.get_available_tiles_expanded(), "reopening the drawer preserves collapsed discovery");
        ui.set_available_tiles(&[]);
        assert_eq!(ui.ui.get_available_tiles().row_count(), 0, "removed packages leave discovery");

        // Count actual Slint model reads to check virtualization at runtime.
        // A normal repeated layout requests every row; ListView must request
        // only rows around its bounded viewport, even with a large catalog.
        struct CountedCatalog {
            rows: Vec<AvailableTileView>,
            reads: RefCell<alloc::collections::BTreeSet<usize>>,
        }
        impl slint::Model for CountedCatalog {
            type Data = AvailableTileView;
            fn row_count(&self) -> usize { self.rows.len() }
            fn row_data(&self, row: usize) -> Option<Self::Data> {
                self.reads.borrow_mut().insert(row);
                self.rows.get(row).cloned()
            }
            fn model_tracker(&self) -> &dyn slint::ModelTracker { &() }
        }
        let catalog = Rc::new(CountedCatalog {
            rows: (0..128).map(|i| AvailableTileView {
                tile_id: alloc::format!("catalog.tile-{i}").into(),
                title: alloc::format!("Catalog tile {i}").into(),
            }).collect(),
            reads: RefCell::new(Default::default()),
        });
        ui.ui.set_available_tiles(ModelRc::from(catalog.clone()));
        ui.ui.set_available_tiles_expanded(false);
        ui.render(ui.now.get() + Duration::from_millis(16));
        assert!(catalog.reads.borrow().is_empty(), "collapsed list reads no rows");
        ui.ui.set_available_tiles_expanded(true);
        ui.render(ui.now.get() + Duration::from_millis(16));
        let initial_reads = catalog.reads.borrow().len();
        assert!(initial_reads > 0 && initial_reads <= 8, "bounded initial realization: {initial_reads}");
        catalog.reads.borrow_mut().clear();
        ui.window.dispatch_event(WindowEvent::PointerScrolled {
            position: LogicalPosition::new(150.0, 230.0), delta_x: 0.0, delta_y: -100_000.0,
        });
        ui.render(ui.now.get() + Duration::from_millis(16));
        let bottom_reads = catalog.reads.borrow();
        assert!(bottom_reads.contains(&127), "scrolling reaches the final catalog row");
        assert!(bottom_reads.len() <= 12, "scrolling realizes a bounded viewport: {}", bottom_reads.len());
        drop(bottom_reads);
        ui.ui.set_sidebar_open(false);
        ui.render(ui.now.get() + Duration::from_millis(16));
        assert!(!ui.ui.get_available_tiles_expanded(), "closing drawer collapses discovery");
        catalog.reads.borrow_mut().clear();
        ui.ui.set_sidebar_open(true);
        ui.render(ui.now.get() + Duration::from_millis(16));
        assert!(catalog.reads.borrow().is_empty(), "reopening collapsed discovery reads no rows");

        // Only native user controls can approve the immutable package proposal.
        let mut review = alloyos_mcp::install::PendingRemoteInstall {
            id: "install-1".into(), app_id: "mail".into(), name: "Mail service".into(),
            endpoint: "https://mail.example/mcp".into(), source: "Direct endpoint".into(),
            publisher: "Not verified".into(), action: alloyos_mcp::install::PackageChangeAction::Install,
        };
        ui.set_pending_remote_install(Some(&review));
        ui.render(ui.now.get() + Duration::from_millis(16));
        assert_eq!(ui.ui.get_pending_install_endpoint().as_str(), review.endpoint);
        assert!(ui.take_remote_install_decision().is_none(), "displaying review does not approve it");
        ui.ui.invoke_remote_install_decision("stale-id".into(), true);
        assert!(ui.take_remote_install_decision().is_none(), "a stale proposal cannot be approved");
        if let Ok(path) = std::env::var("ALLOYOS_INSTALL_SCREENSHOT") {
            let file = std::fs::File::create(path).unwrap();
            let mut encoder = png::Encoder::new(file, ui.size.width, ui.size.height);
            encoder.set_color(png::ColorType::Rgba);
            encoder.set_depth(png::BitDepth::Eight);
            let bytes: Vec<u8> = ui.pixels().iter().flat_map(|p| [(p >> 16) as u8, (p >> 8) as u8, *p as u8, 255]).collect();
            encoder.write_header().unwrap().write_image_data(&bytes).unwrap();
        }
        for kind in [InputEventKind::PointerButtonDown, InputEventKind::PointerButtonUp] {
            ui.handle_input(InputEvent { kind, code: 0x110, x: 850, y: 1945 });
        }
        assert_eq!(ui.take_remote_install_decision(), Some(("install-1".into(), true)));
        for kind in [InputEventKind::PointerButtonDown, InputEventKind::PointerButtonUp] {
            ui.handle_input(InputEvent { kind, code: 0x110, x: 850, y: 1945 });
        }
        assert!(ui.take_remote_install_decision().is_none(), "double approval is disabled");
        ui.set_pending_remote_install(None);
        review.id = "install-2".into();
        review.action = alloyos_mcp::install::PackageChangeAction::Remove;
        review.endpoint.clear();
        review.source = "Installed alloyOS JavaScript SDK package".into();
        ui.set_pending_remote_install(Some(&review));
        ui.render(ui.now.get() + Duration::from_millis(16));
        assert!(ui.ui.get_pending_install_removal());
        for kind in [InputEventKind::PointerButtonDown, InputEventKind::PointerButtonUp] {
            ui.handle_input(InputEvent { kind, code: 0x110, x: 350, y: 1945 });
        }
        assert_eq!(ui.take_remote_install_decision(), Some(("install-2".into(), false)));
        ui.set_pending_remote_install(None);

        let draft_before_profile = ui.ui.get_draft();
        let profile_setup = alloyos_mcp::account::PendingClientProfileSetup {
            id: "profile-1".into(), app_id: "mail".into(), name: "Mail service".into(),
            endpoint: "https://mail.example/mcp".into(),
        };
        ui.set_client_profile_setup(Some(&profile_setup), "");
        ui.render(ui.now.get() + Duration::from_millis(16));
        assert!(ui.take_client_profile_decision().is_none());
        if let Ok(path) = std::env::var("ALLOYOS_PROFILE_SCREENSHOT") {
            let file = std::fs::File::create(path).unwrap();
            let mut encoder = png::Encoder::new(file, ui.size.width, ui.size.height);
            encoder.set_color(png::ColorType::Rgba); encoder.set_depth(png::BitDepth::Eight);
            let bytes: Vec<u8> = ui.pixels().iter().flat_map(|p| [(p >> 16) as u8, (p >> 8) as u8, *p as u8, 255]).collect();
            encoder.write_header().unwrap().write_image_data(&bytes).unwrap();
        }
        for (y, value) in [(940, "https://issuer.example"), (1230, "native-client"),
            (1520, "mail.read"), (1810, "8381"), (2095, "synthetic-client-secret")] {
            for kind in [InputEventKind::PointerButtonDown, InputEventKind::PointerButtonUp] {
                ui.handle_input(InputEvent { kind, code: 0x110, x: 230, y });
            }
            for ch in value.chars() { ui.ui.invoke_soft_key(ch.to_string().into()); }
            ui.ui.invoke_release_editor();
            ui.render(ui.now.get() + Duration::from_millis(16));
        }
        assert_eq!(ui.ui.get_draft(), draft_before_profile, "registration typing never reaches chat");
        assert!(ui.take_client_profile_decision().is_none(), "typing is not approval");
        for kind in [InputEventKind::PointerButtonDown, InputEventKind::PointerButtonUp] {
            ui.handle_input(InputEvent { kind, code: 0x110, x: 920, y: 2490 });
        }
        ui.render(ui.now.get() + Duration::from_millis(16));
        assert!(ui.take_client_profile_decision().is_none(), "Review does not save");
        if let Ok(path) = std::env::var("ALLOYOS_PROFILE_REVIEW_SCREENSHOT") {
            let file = std::fs::File::create(path).unwrap();
            let mut encoder = png::Encoder::new(file, ui.size.width, ui.size.height);
            encoder.set_color(png::ColorType::Rgba); encoder.set_depth(png::BitDepth::Eight);
            let bytes: Vec<u8> = ui.pixels().iter().flat_map(|p| [(p >> 16) as u8, (p >> 8) as u8, *p as u8, 255]).collect();
            encoder.write_header().unwrap().write_image_data(&bytes).unwrap();
        }

        for kind in [InputEventKind::PointerButtonDown, InputEventKind::PointerButtonUp] {
            ui.handle_input(InputEvent { kind, code: 0x110, x: 980, y: 2490 });
        }
        let (id, saved) = ui.take_client_profile_decision().expect("native Save must submit");
        assert_eq!(id, "profile-1");
        let saved = saved.unwrap();
        assert_eq!(saved.endpoint, profile_setup.endpoint);
        assert_eq!(saved.issuer, "https://issuer.example");
        assert_eq!(saved.client_id, "native-client");
        assert_eq!(saved.allowed_scopes, vec!["mail.read"]);
        assert_eq!(saved.loopback_port, 8381);
        assert_eq!(saved.client_secret.as_deref(), Some("synthetic-client-secret"));
        assert!(!alloc::format!("{saved:?}").contains("synthetic-client-secret"));
        ui.set_client_profile_setup(Some(&profile_setup), "Synthetic storage failure");
        ui.render(ui.now.get() + Duration::from_millis(16));
        for kind in [InputEventKind::PointerButtonDown, InputEventKind::PointerButtonUp] {
            ui.handle_input(InputEvent { kind, code: 0x110, x: 980, y: 2490 });
        }
        ui.render(ui.now.get() + Duration::from_millis(16));
        assert!(ui.take_client_profile_decision().is_none(), "failed Save requires another explicit Review, with the cleared secret explained");
        let next_profile = alloyos_mcp::account::PendingClientProfileSetup { id: "profile-2".into(), ..profile_setup.clone() };
        ui.set_client_profile_setup(Some(&next_profile), "");
        ui.render(ui.now.get() + Duration::from_millis(16));
        for kind in [InputEventKind::PointerButtonDown, InputEventKind::PointerButtonUp] {
            ui.handle_input(InputEvent { kind, code: 0x110, x: 980, y: 2490 });
        }
        assert!(ui.take_client_profile_decision().is_none(), "replacing a review clears all previous fields and secret");
        for kind in [InputEventKind::PointerButtonDown, InputEventKind::PointerButtonUp] {
            ui.handle_input(InputEvent { kind, code: 0x110, x: 350, y: 2490 });
        }
        assert!(matches!(ui.take_client_profile_decision(), Some((id, None)) if id == "profile-2"), "Cancel submits no fields for the current review");
        let google_profile = alloyos_mcp::account::PendingClientProfileSetup { id:"google-profile".into(), ..profile_setup.clone() };
        ui.set_client_profile_setup(Some(&google_profile), "");
        ui.render(ui.now.get() + Duration::from_millis(16));
        for kind in [InputEventKind::PointerButtonDown, InputEventKind::PointerButtonUp] {
            ui.handle_input(InputEvent {kind,code:0x110,x:230,y:940});
        }
        for ch in "https://accounts.google.com".chars() {ui.ui.invoke_soft_key(ch.to_string().into());}
        ui.ui.invoke_release_editor();
        ui.render(ui.now.get() + Duration::from_millis(16));
        if let Ok(path)=std::env::var("ALLOYOS_GOOGLE_PROFILE_SCREENSHOT") {
            let file=std::fs::File::create(path).unwrap();
            let mut encoder=png::Encoder::new(file,ui.size.width,ui.size.height);
            encoder.set_color(png::ColorType::Rgba);encoder.set_depth(png::BitDepth::Eight);
            let bytes:Vec<u8>=ui.pixels().iter().flat_map(|p|[(p>>16)as u8,(p>>8)as u8,*p as u8,255]).collect();
            encoder.write_header().unwrap().write_image_data(&bytes).unwrap();
        }
        ui.ui.invoke_client_profile_submit("https://issuer.example".into(), "native-client".into(), "mail.read".into(), "0".into(), "".into(), true, true);
        assert!(ui.take_client_profile_decision().is_none(), "provider option cannot apply to another issuer");
        ui.ui.invoke_client_profile_submit("https://accounts.google.com".into(), "native-client".into(), "mail.read".into(), "0".into(), "".into(), true, true);
        let (_,google)=ui.take_client_profile_decision().unwrap();
        assert!(matches!(google.unwrap().authorization_options,alloyos_mcp::account::AuthorizationOptions::GoogleOffline{reprompt_consent:true}));
        ui.set_client_profile_setup(None, "");

        // Key releases retain the original character even if Shift changes.
        let mut keys = EvdevKeyboard::default();
        keys.translate(42, true);
        assert_eq!(keys.translate(32, true).unwrap().as_str(), "D");
        keys.translate(42, false);
        assert_eq!(keys.translate(32, false).unwrap().as_str(), "D");
        assert!(
            FerrofluidSystemUi::new(PHONE_WIDTH, PHONE_HEIGHT, PHONE_SCALE as f32).is_err(),
            "second platform is rejected"
        );
    }
}
