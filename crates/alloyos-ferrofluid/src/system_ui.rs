//! Ferrofluid-owned Slint platform, single System UI window, and software scanout.
//!
//! All APIs must run on the same execution context, outside interrupt handlers.
//! The bare-metal Slint runtime uses `unsafe-single-threaded` storage. Keep this
//! owner local to the UI loop; do not put it in a global mutex or mark it Send.

use alloc::{boxed::Box, collections::VecDeque, rc::Rc, string::String, vec, vec::Vec};
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
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChatItem {
    Text(String),
    Tile(AppTile),
}

/// Single owner of the system window. Drivers supply time/input and consume
/// opaque ARGB8888 pixels; they never own or invoke Slint components.
pub struct FerrofluidSystemUi {
    window: Rc<MinimalSoftwareWindow>,
    ui: SystemUi,
    now: Rc<Cell<Duration>>,
    submissions: Rc<RefCell<VecDeque<String>>>,
    model_cycles: Rc<Cell<usize>>,
    tiles: Rc<RefCell<Vec<AppTile>>>,
    chat_items: Rc<RefCell<Vec<ChatItem>>>,
    available_tiles: Rc<RefCell<Vec<AppTile>>>,
    dashboard: Rc<RefCell<crate::DashboardState>>,
    tile_events: Rc<RefCell<VecDeque<crate::TileEvent>>>,
    rgb: Vec<slint::Rgb8Pixel>,
    argb: Vec<u32>,
    top_blur: Vec<slint::Rgb8Pixel>,
    previous_blur_top: Option<u32>,
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
    previous_keyboard_reveal: f32,
    previous_chat_offset: f32,
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
        let model_cycles = Rc::new(Cell::new(0usize));
        let cycles = model_cycles.clone();
        let weak = ui.as_weak();
        ui.on_cycle_model(move || {
            if weak.upgrade().is_some_and(|ui| !ui.get_reply_pending()) {
                cycles.set(cycles.get().saturating_add(1));
            }
        });
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
        let state = dashboard.clone();
        let available = available_tiles.clone();
        let weak = ui.as_weak();
        ui.on_pin_tile(move |id, pin| {
            if !available
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
                update_dashboard(&ui, &state.borrow().tiles(&available.borrow()));
            }
        });
        let state = dashboard.clone();
        let available = available_tiles.clone();
        let weak = ui.as_weak();
        ui.on_move_pin(move |id| {
            state.borrow_mut().move_to(id.as_str(), 0);
            if let Some(ui) = weak.upgrade() {
                update_dashboard(&ui, &state.borrow().tiles(&available.borrow()));
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
        ui.show()?;
        window.dispatch_event(WindowEvent::WindowActiveChanged(true));
        Ok(Self {
            window,
            ui,
            now,
            submissions,
            model_cycles,
            tiles,
            chat_items,
            available_tiles,
            dashboard,
            tile_events,
            rgb: vec![slint::Rgb8Pixel::default(); width as usize * height as usize],
            previous_blur_top: None,
            top_blur: vec![slint::Rgb8Pixel::default(); width as usize * (40.0 * scale) as usize],
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
            previous_keyboard_reveal: 0.0,
            previous_chat_offset: 0.0,
        })
    }

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
        );
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
        );
    }
    pub fn set_chat_items(&self, items: &[ChatItem]) {
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
        update_dashboard(
            &self.ui,
            &self.dashboard.borrow().tiles(&self.available_tiles.borrow()),
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
        self.ui.set_transcript(text.into());
    }
    pub fn transcript(&self) -> String {
        String::from(self.ui.get_transcript().as_str())
    }
    pub fn chat_scroll_offset(&self) -> f32 {
        self.ui.get_scroll_offset()
    }
    pub fn chat_scroll_maximum(&self) -> f32 {
        self.ui.get_scroll_maximum()
    }
    pub fn update_streaming_reply(&self, base_transcript: &str, partial_reply: &str) {
        self.backdrop_dirty.set(true);
        let mut combined = String::with_capacity(base_transcript.len() + 10 + partial_reply.len());
        combined.push_str(base_transcript);
        combined.push_str("\n\nWelder: ");
        combined.push_str(partial_reply);
        self.ui.set_transcript(combined.into());
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
        let boot_damage = if booting {
            self.boot_liquid.advance(self.now.get());
            Some(self.boot_liquid.bounds())
        } else {
            None
        };
        let sidebar_active = !booting && self.ui.get_sidebar_reveal() > 0.0;
        let keyboard_reveal = self.ui.get_keyboard_reveal();
        let chat_offset = self.ui.get_scroll_offset();
        if sidebar_active && (!self.previous_sidebar_active
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
            booting || sidebar_active,
        );
        if liquid_damage.is_some() || sidebar_damage.is_some() || boot_damage.is_some() {
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
                && liquid_damage.is_none() && sidebar_damage.is_none() {
                return;
            }
            let mut rect =
                DirtyRect::new(origin.x as u32, origin.y as u32, size.width, size.height);
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
            let chat_top = (self.ui.get_chat_top() * self.scale) as u32;
            let blur_height = (40.0 * self.scale) as u32;
            let blur_bottom = (chat_top + blur_height).min(self.size.height);
            if !booting {
                if let Some(previous) = self.previous_blur_top {
                    if previous != chat_top {
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
                self.previous_blur_top = None;
            }
            let blur_dirty = !booting && !sidebar_active && rect.y < blur_bottom && rect.y + rect.height > chat_top;
            if blur_dirty {
                // The blur samples neighboring pixels, so damage anywhere in
                // its band invalidates the entire band, including its fringe.
                let bottom = (rect.y + rect.height).max(blur_bottom);
                rect.y = rect.y.min(chat_top);
                rect.height = bottom - rect.y;
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
        self.previous_keyboard_reveal = keyboard_reveal;
        self.previous_chat_offset = chat_offset;
        damage
    }
}

fn update_dashboard(ui: &SystemUi, tiles: &[AppTile]) {
    ui.set_dashboard_tiles(tile_views(tiles));
}

fn tile_to_view(tile: &AppTile) -> TileView {
    TileView {
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

fn chat_item_views(items: &[ChatItem]) -> ModelRc<ChatItemView> {
    ModelRc::new(VecModel::from(
        items
            .iter()
            .map(|item| match item {
                ChatItem::Text(text) => ChatItemView {
                    is_tile: false,
                    text: text.as_str().into(),
                    tile: TileView::default(),
                },
                ChatItem::Tile(tile) => ChatItemView {
                    is_tile: true,
                    text: "".into(),
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
            let p = rgb[i];
            let strength = ((height - 1 - y) * 256 / height) as u32;
            let n = (end - start) as u32;
            let channel = |original: u8, total: u32, background: u32| {
                let blurred = (original as u32 * (256 - strength) + total / n * strength) / 256;
                // Fade into the window background at the very top.
                (blurred * (256 - strength) + background * strength) / 256
            };
            argb[i] = 0xff000000
                | channel(p.r, sum[0], 10) << 16
                | channel(p.g, sum[1], 10) << 8
                | channel(p.b, sum[2], 12);
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

    // Only one test owns Slint's process-wide, single-context platform.
    fn assert_long_composer_scrolling(ui: &mut FerrofluidSystemUi) {
        for kind in [InputEventKind::PointerButtonDown, InputEventKind::PointerButtonUp] {
            ui.handle_input(InputEvent { kind, code: 0x110, x: 300, y: 2500 });
        }
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
                x: 300,
                y: 2500,
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
                x: 300,
                y: 2500,
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
                x: 300,
                y: 2500,
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
            items: Vec::new(),
            actions: vec![refresh.clone()],
        }]);
        ui.ui.invoke_pin_tile("fixture.now".into(), true);
        assert_eq!(
            ui.dashboard_state().pinned_ids(),
            &[String::from("fixture.now")]
        );
        assert_eq!(ui.ui.get_dashboard_tiles().row_count(), 1);
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
