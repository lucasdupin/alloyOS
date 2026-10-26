//! Ferrofluid-owned WebView controller, WebKit page engine, and bounded BGRA8 frame assembler.
//!
//! Only Ferrofluid holds the browser controller endpoint and presentation surface;
//! Welder may request high-level navigation through Ferrofluid, while apps never
//! receive a WebView handle, navigation capability, or pixel-buffer access.

pub mod page;

pub use page::{PageInteraction, WebPage};

use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;
use alloyos_proto::web::{
    trusted_origin, Error, FrameInfo, FrameTransfer, Opcode, Packet, TrustedOrigin, View,
    MAX_DIMENSION, MAX_FRAME_BYTES,
};
use alloyos_proto::DirtyRect;

/// Pointer / touch interaction phase encoded for `Opcode::Pointer`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum PointerPhase {
    Move = 0,
    Down = 1,
    Up = 2,
    Cancel = 3,
}

/// Bitmask constants for `Opcode::Key` modifier byte.
pub mod key_modifiers {
    pub const SHIFT: u8 = 1 << 0;
    pub const CONTROL: u8 = 1 << 1;
    pub const ALT: u8 = 1 << 2;
    pub const META: u8 = 1 << 3;
}

/// Browser page loading state reported via `Opcode::Loading`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum LoadingState {
    Idle = 0,
    Loading = 1,
    Failed = 2,
}

impl LoadingState {
    fn from_byte(byte: u8) -> Result<Self, Error> {
        match byte {
            0 => Ok(Self::Idle),
            1 => Ok(Self::Loading),
            2 => Ok(Self::Failed),
            _ => Err(Error::InvalidPayload),
        }
    }
}

/// Focus and editable state reported by the browser engine via `Opcode::Editing`.
/// Drives alloyOS soft-keyboard visibility in Ferrofluid.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct EditingState {
    pub focused: bool,
    pub editable: bool,
}

impl EditingState {
    pub fn soft_keyboard_visible(self) -> bool {
        self.focused && self.editable
    }
}

/// Completed, validated top-down BGRA8 frame owned by Ferrofluid.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommittedFrame {
    pub view: View,
    pub sequence: u32,
    pub width: u32,
    pub height: u32,
    pub stride: u32,
    pub damage: DirtyRect,
    pub pixels: Vec<u8>,
}

/// Bounded staging assembler for `BeginFrame` -> contiguous `FrameChunk`s -> `EndFrame`.
/// Validates dimensions, stride, total byte size (`<= MAX_FRAME_BYTES`), damage clipping,
/// view generation, sequence matching, and contiguous chunk offsets before publishing.
#[derive(Debug)]
pub struct FrameAssembler {
    info: FrameInfo,
    transfer: FrameTransfer,
    view: View,
    sequence: u32,
    staging: Vec<u8>,
}

impl FrameAssembler {
    pub fn begin(packet: &Packet<'_>, current_view: View) -> Result<Self, Error> {
        let transfer = FrameTransfer::begin(packet, current_view)?;
        let info = FrameInfo::decode(packet.payload)?;
        let total_bytes = info.byte_len()? as usize;
        Ok(Self {
            info,
            transfer,
            view: packet.view,
            sequence: packet.sequence,
            staging: vec![0u8; total_bytes],
        })
    }

    pub fn chunk(&mut self, packet: &Packet<'_>) -> Result<u32, Error> {
        let (offset, data) = self.transfer.chunk(packet)?;
        let start = offset as usize;
        let end = start + data.len();
        if end > self.staging.len() {
            return Err(Error::InvalidPayload);
        }
        self.staging[start..end].copy_from_slice(data);
        Ok((end as u32) - offset)
    }

    pub fn finish(self, packet: &Packet<'_>) -> Result<CommittedFrame, Error> {
        self.transfer.finish(packet)?;
        Ok(CommittedFrame {
            view: self.view,
            sequence: self.sequence,
            width: self.info.width,
            height: self.info.height,
            stride: self.info.stride,
            damage: self.info.damage,
            pixels: self.staging,
        })
    }
}

/// Ferrofluid-owned controller for a single isolated browser `View`.
pub struct WebViewController {
    view: View,
    next_sequence: u32,
    width: u32,
    height: u32,
    scale_thousandths: u32,
    created: bool,
    focused: bool,
    requested_url: Option<String>,
    committed_url: Option<String>,
    trusted_origin: Option<TrustedOrigin>,
    untrusted_title: String,
    loading: LoadingState,
    editing: EditingState,
    crash_reason: Option<u32>,
    in_flight_frame: Option<FrameAssembler>,
    committed_frame: Option<CommittedFrame>,
    dirty_regions: Vec<DirtyRect>,
    page: Option<WebPage>,
}

impl core::fmt::Debug for WebViewController {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("WebViewController")
            .field("view", &self.view)
            .field("width", &self.width)
            .field("height", &self.height)
            .field("created", &self.created)
            .field("requested_url", &self.requested_url)
            .field("committed_url", &self.committed_url)
            .field("trusted_origin", &self.trusted_origin)
            .field("untrusted_title", &self.untrusted_title)
            .field("loading", &self.loading)
            .field("editing", &self.editing)
            .field("crash_reason", &self.crash_reason)
            .finish()
    }
}

impl WebViewController {
    pub fn new(
        view_id: u64,
        width: u32,
        height: u32,
        scale_thousandths: u32,
    ) -> Result<Self, Error> {
        if view_id == 0 {
            return Err(Error::InvalidIdentity);
        }
        validate_viewport(width, height, scale_thousandths)?;
        Ok(Self {
            view: View {
                id: view_id,
                generation: 1,
            },
            next_sequence: 1,
            width,
            height,
            scale_thousandths,
            created: false,
            focused: false,
            requested_url: None,
            committed_url: None,
            trusted_origin: None,
            untrusted_title: String::new(),
            loading: LoadingState::Idle,
            editing: EditingState::default(),
            crash_reason: None,
            in_flight_frame: None,
            committed_frame: None,
            dirty_regions: Vec::new(),
            page: None,
        })
    }

    pub fn view(&self) -> View {
        self.view
    }

    pub fn dimensions(&self) -> (u32, u32, u32) {
        (self.width, self.height, self.scale_thousandths)
    }

    pub fn is_created(&self) -> bool {
        self.created
    }

    pub fn is_focused(&self) -> bool {
        self.focused
    }

    pub fn is_crashed(&self) -> bool {
        self.crash_reason.is_some()
    }

    pub fn crash_reason(&self) -> Option<u32> {
        self.crash_reason
    }

    pub fn loading_state(&self) -> LoadingState {
        self.loading
    }

    pub fn editing_state(&self) -> EditingState {
        self.editing
    }

    pub fn soft_keyboard_visible(&self) -> bool {
        !self.is_crashed() && self.focused && self.editing.soft_keyboard_visible()
    }

    pub fn requested_url(&self) -> Option<&str> {
        self.requested_url.as_deref()
    }

    pub fn committed_url(&self) -> Option<&str> {
        self.committed_url.as_deref()
    }

    pub fn trusted_origin(&self) -> Option<&TrustedOrigin> {
        self.trusted_origin.as_ref()
    }

    pub fn untrusted_title(&self) -> &str {
        &self.untrusted_title
    }

    pub fn committed_frame(&self) -> Option<&CommittedFrame> {
        self.committed_frame.as_ref()
    }

    pub fn take_dirty_regions(&mut self) -> Vec<DirtyRect> {
        core::mem::take(&mut self.dirty_regions)
    }

    pub fn page(&self) -> Option<&WebPage> {
        self.page.as_ref()
    }

    pub fn page_mut(&mut self) -> Option<&mut WebPage> {
        self.page.as_mut()
    }

    fn encode_packet(&mut self, opcode: Opcode, payload: &[u8]) -> Result<Vec<u8>, Error> {
        let seq = self.next_sequence;
        self.next_sequence = self.next_sequence.checked_add(1).unwrap_or(1);
        let pkt = Packet {
            opcode,
            view: self.view,
            sequence: seq,
            payload,
        };
        pkt.validate()?;
        let mut buf = [0u8; alloyos_proto::web::MAX_PACKET_BYTES];
        let len = pkt.encode(&mut buf)?;
        Ok(buf[..len].to_vec())
    }

    pub fn encode_create(&mut self) -> Result<Vec<u8>, Error> {
        validate_viewport(self.width, self.height, self.scale_thousandths)?;
        let mut payload = [0u8; 12];
        payload[0..4].copy_from_slice(&self.width.to_le_bytes());
        payload[4..8].copy_from_slice(&self.height.to_le_bytes());
        payload[8..12].copy_from_slice(&self.scale_thousandths.to_le_bytes());
        self.encode_packet(Opcode::Create, &payload)
    }

    pub fn encode_resize(
        &mut self,
        width: u32,
        height: u32,
        scale_thousandths: u32,
    ) -> Result<Vec<u8>, Error> {
        validate_viewport(width, height, scale_thousandths)?;
        self.width = width;
        self.height = height;
        self.scale_thousandths = scale_thousandths;
        let mut payload = [0u8; 12];
        payload[0..4].copy_from_slice(&width.to_le_bytes());
        payload[4..8].copy_from_slice(&height.to_le_bytes());
        payload[8..12].copy_from_slice(&scale_thousandths.to_le_bytes());
        self.encode_packet(Opcode::Resize, &payload)
    }

    pub fn encode_navigate(&mut self, url: &str) -> Result<Vec<u8>, Error> {
        self.requested_url = Some(String::from(url));
        let bytes = self.encode_packet(Opcode::Navigate, url.as_bytes())?;
        self.load_url(url);
        Ok(bytes)
    }

    pub fn encode_pointer(
        &mut self,
        phase: PointerPhase,
        x: i32,
        y: i32,
    ) -> Result<Vec<u8>, Error> {
        let mut payload = [0u8; 9];
        payload[0] = phase as u8;
        payload[1..5].copy_from_slice(&x.to_le_bytes());
        payload[5..9].copy_from_slice(&y.to_le_bytes());

        // Process pointer in the local WebKit page engine
        self.interact_pointer(phase, x, y);

        self.encode_packet(Opcode::Pointer, &payload)
    }

    pub fn encode_scroll(&mut self, dx: i32, dy: i32) -> Result<Vec<u8>, Error> {
        let mut payload = [0u8; 8];
        payload[0..4].copy_from_slice(&dx.to_le_bytes());
        payload[4..8].copy_from_slice(&dy.to_le_bytes());

        if let Some(page) = &mut self.page {
            page.interact_scroll(dy);
            self.refresh_rendered_frame();
        }

        self.encode_packet(Opcode::Scroll, &payload)
    }

    pub fn encode_key(&mut self, code: u16, pressed: bool, modifiers: u8) -> Result<Vec<u8>, Error> {
        let mut payload = [0u8; 4];
        payload[0..2].copy_from_slice(&code.to_le_bytes());
        payload[2] = if pressed { 1 } else { 0 };
        payload[3] = modifiers;
        self.encode_packet(Opcode::Key, &payload)
    }

    pub fn encode_commit_text(&mut self, text: &str) -> Result<Vec<u8>, Error> {
        self.interact_commit_text(text);
        self.encode_packet(Opcode::CommitText, text.as_bytes())
    }

    pub fn encode_composition(&mut self, preedit: &str) -> Result<Vec<u8>, Error> {
        self.encode_packet(Opcode::Composition, preedit.as_bytes())
    }

    pub fn encode_focus(&mut self, focused: bool) -> Result<Vec<u8>, Error> {
        self.focused = focused;
        let payload = [if focused { 1 } else { 0 }];
        let encoded = self.encode_packet(Opcode::Focus, &payload)?;
        if !focused {
            self.editing.focused = false;
            self.editing.editable = false;
            if let Some(page) = &mut self.page {
                page.focused_input_id = None;
            }
            self.refresh_rendered_frame();
        }
        Ok(encoded)
    }

    pub fn encode_back(&mut self) -> Result<Vec<u8>, Error> {
        self.encode_packet(Opcode::Back, &[])
    }

    pub fn encode_forward(&mut self) -> Result<Vec<u8>, Error> {
        self.encode_packet(Opcode::Forward, &[])
    }

    pub fn encode_reload(&mut self) -> Result<Vec<u8>, Error> {
        if let Some(url) = self.committed_url.clone().or_else(|| self.requested_url.clone()) {
            self.load_url(&url);
        }
        self.encode_packet(Opcode::Reload, &[])
    }

    pub fn encode_stop(&mut self) -> Result<Vec<u8>, Error> {
        self.encode_packet(Opcode::Stop, &[])
    }

    pub fn encode_destroy(&mut self) -> Result<Vec<u8>, Error> {
        let encoded = self.encode_packet(Opcode::Destroy, &[])?;
        self.bump_generation_and_reset();
        Ok(encoded)
    }

    /// Loads a URL into the WebKit page engine and updates the committed frame.
    pub fn load_url(&mut self, url: &str) {
        let mut page = WebPage::load_url(url);
        let title = String::from(page.title());
        let pixels = page.render(self.width, self.height);
        self.untrusted_title = title;
        self.page = Some(page);

        self.committed_frame = Some(CommittedFrame {
            view: self.view,
            sequence: self.next_sequence,
            width: self.width,
            height: self.height,
            stride: self.width * 4,
            damage: DirtyRect::new(0, 0, self.width, self.height),
            pixels,
        });
        self.dirty_regions
            .push(DirtyRect::new(0, 0, self.width, self.height));
    }

    /// Loads raw HTML markup into the WebKit page engine and updates the committed frame.
    pub fn load_html(&mut self, html: &str, url: &str) {
        let mut page = WebPage::load_html(html, url);
        let title = String::from(page.title());
        let pixels = page.render(self.width, self.height);
        self.untrusted_title = title;
        self.page = Some(page);

        self.committed_frame = Some(CommittedFrame {
            view: self.view,
            sequence: self.next_sequence,
            width: self.width,
            height: self.height,
            stride: self.width * 4,
            damage: DirtyRect::new(0, 0, self.width, self.height),
            pixels,
        });
        self.dirty_regions
            .push(DirtyRect::new(0, 0, self.width, self.height));
    }

    /// Handles pointer interaction inside the page engine.
    pub fn interact_pointer(&mut self, phase: PointerPhase, x: i32, y: i32) {
        if let Some(page) = &mut self.page {
            let interaction = page.interact_pointer(phase, x, y);
            match interaction {
                PageInteraction::InputFocused(_) => {
                    self.focused = true;
                    self.editing.focused = true;
                    self.editing.editable = true;
                    self.refresh_rendered_frame();
                }
                PageInteraction::FormSubmitted(_) => {
                    self.editing.focused = false;
                    self.editing.editable = false;
                    self.refresh_rendered_frame();
                }
                PageInteraction::None => {}
            }
        }
    }

    /// Commits text into the active focused input field.
    pub fn interact_commit_text(&mut self, text: &str) {
        if let Some(page) = &mut self.page {
            page.interact_commit_text(text);
            self.refresh_rendered_frame();
        }
    }

    /// Submits the active form and invokes registered JavaScript submit listeners.
    pub fn interact_submit(&mut self) -> Result<String, String> {
        if let Some(page) = &mut self.page {
            let res = page.submit_active_form()?;
            self.editing.focused = false;
            self.editing.editable = false;
            self.refresh_rendered_frame();
            Ok(res)
        } else {
            Err(String::from("No page loaded"))
        }
    }

    /// Evaluates arbitrary JavaScript in the page context.
    pub fn eval_js(&mut self, code: &str) -> Result<String, String> {
        if let Some(page) = &mut self.page {
            page.eval_js(code)
        } else {
            Err(String::from("No page loaded"))
        }
    }

    pub fn refresh_rendered_frame(&mut self) {
        if let Some(page) = &mut self.page {
            let pixels = page.render(self.width, self.height);
            self.committed_frame = Some(CommittedFrame {
                view: self.view,
                sequence: self.next_sequence,
                width: self.width,
                height: self.height,
                stride: self.width * 4,
                damage: DirtyRect::new(0, 0, self.width, self.height),
                pixels,
            });
            self.dirty_regions
                .push(DirtyRect::new(0, 0, self.width, self.height));
        }
    }

    /// Instantiates and frames a WebView for a verified URL with the real WebKit page engine.
    pub fn for_url(
        view_id: u64,
        url: &str,
        title: &str,
    ) -> Result<Self, Error> {
        let width = 576;
        let height = 380;
        let mut ctrl = Self::new(view_id, width, height, 2000)?;
        ctrl.created = true;
        let origin = trusted_origin(url)?;
        ctrl.requested_url = Some(String::from(url));
        ctrl.committed_url = Some(String::from(url));
        ctrl.trusted_origin = Some(origin);
        ctrl.untrusted_title = String::from(title);
        ctrl.loading = LoadingState::Idle;

        // Load the page using the WebKit page engine
        ctrl.load_url(url);

        if ctrl.untrusted_title.is_empty() && !title.is_empty() {
            ctrl.untrusted_title = String::from(title);
        }

        Ok(ctrl)
    }

    pub fn restart_service(&mut self) -> View {
        self.bump_generation_and_reset();
        self.view
    }

    pub fn recover_and_reload(&mut self) -> Result<Vec<Vec<u8>>, Error> {
        if self.is_crashed() {
            self.crash_reason = None;
        }
        let reload_target = self
            .committed_url
            .clone()
            .or_else(|| self.requested_url.clone());
        let mut packets = Vec::with_capacity(2);
        packets.push(self.encode_create()?);
        if let Some(url) = reload_target {
            packets.push(self.encode_navigate(&url)?);
        }
        Ok(packets)
    }

    fn bump_generation_and_reset(&mut self) {
        self.view.generation = self.view.generation.checked_add(1).unwrap_or(1);
        self.created = false;
        self.in_flight_frame = None;
        self.editing = EditingState::default();
        self.loading = LoadingState::Idle;
    }

    pub fn handle_reply_bytes(&mut self, bytes: &[u8]) -> Result<(), Error> {
        let packet = Packet::decode(bytes)?;
        self.handle_reply(&packet)
    }

    pub fn handle_reply(&mut self, packet: &Packet<'_>) -> Result<(), Error> {
        packet.validate()?;
        if !packet.opcode.is_reply() {
            return Err(Error::Opcode);
        }
        if packet.view != self.view {
            return Err(Error::StaleFrame);
        }

        match packet.opcode {
            Opcode::Created => {
                self.created = true;
                self.crash_reason = None;
                Ok(())
            }
            Opcode::Url => {
                let url =
                    core::str::from_utf8(packet.payload).map_err(|_| Error::InvalidPayload)?;
                let origin = trusted_origin(url)?;
                self.committed_url = Some(String::from(url));
                self.trusted_origin = Some(origin);
                Ok(())
            }
            Opcode::Title => {
                let title =
                    core::str::from_utf8(packet.payload).map_err(|_| Error::InvalidPayload)?;
                self.untrusted_title = String::from(title);
                Ok(())
            }
            Opcode::Loading => {
                self.loading = LoadingState::from_byte(packet.payload[0])?;
                Ok(())
            }
            Opcode::Editing => {
                let focused = packet.payload[0] == 1;
                let editable = packet.payload[1] == 1;
                self.editing = EditingState { focused, editable };
                Ok(())
            }
            Opcode::BeginFrame => {
                let assembler = FrameAssembler::begin(packet, self.view)?;
                self.in_flight_frame = Some(assembler);
                Ok(())
            }
            Opcode::FrameChunk => {
                let assembler = self
                    .in_flight_frame
                    .as_mut()
                    .ok_or(Error::IncompleteFrame)?;
                if let Err(err) = assembler.chunk(packet) {
                    self.in_flight_frame = None;
                    return Err(err);
                }
                Ok(())
            }
            Opcode::EndFrame => {
                let assembler = self.in_flight_frame.take().ok_or(Error::IncompleteFrame)?;
                let committed = assembler.finish(packet)?;
                self.dirty_regions.push(committed.damage);
                self.committed_frame = Some(committed);
                Ok(())
            }
            Opcode::Crashed => {
                let reason = u32::from_le_bytes(packet.payload[..4].try_into().unwrap());
                self.bump_generation_and_reset();
                self.crash_reason = Some(reason);
                self.loading = LoadingState::Failed;
                Ok(())
            }
            Opcode::Destroyed => {
                self.bump_generation_and_reset();
                Ok(())
            }
            _ => Err(Error::Opcode),
        }
    }
}

pub fn render_placeholder_surface(
    width: u32,
    height: u32,
    url: &str,
    _title: &str,
) -> Vec<u8> {
    let mut page = WebPage::load_url(url);
    page.render(width, height)
}

fn validate_viewport(width: u32, height: u32, scale_thousandths: u32) -> Result<(), Error> {
    if width == 0
        || height == 0
        || width > MAX_DIMENSION
        || height > MAX_DIMENSION
        || (width as u64) * (height as u64) * 4 > MAX_FRAME_BYTES as u64
        || !(250..=8000).contains(&scale_thousandths)
    {
        return Err(Error::InvalidPayload);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_reply(view: View, sequence: u32, opcode: Opcode, payload: &[u8]) -> Vec<u8> {
        let pkt = Packet {
            opcode,
            view,
            sequence,
            payload,
        };
        let mut buf = [0u8; alloyos_proto::web::MAX_PACKET_BYTES];
        let len = pkt.encode(&mut buf).unwrap();
        buf[..len].to_vec()
    }

    fn begin_frame_payload(
        width: u32,
        height: u32,
        stride: u32,
        damage: (u32, u32, u32, u32),
    ) -> [u8; 28] {
        let mut meta = [0u8; 28];
        for (i, val) in [
            width, height, stride, damage.0, damage.1, damage.2, damage.3,
        ]
        .iter()
        .enumerate()
        {
            meta[i * 4..i * 4 + 4].copy_from_slice(&val.to_le_bytes());
        }
        meta
    }

    #[test]
    fn assembles_complete_frame_and_tracks_damage_regions() {
        let mut ctrl = WebViewController::new(1, 4, 2, 1000).unwrap();
        let view = ctrl.view();

        let create_bytes = ctrl.encode_create().unwrap();
        assert_eq!(
            Packet::decode(&create_bytes).unwrap().opcode,
            Opcode::Create
        );
        ctrl.handle_reply_bytes(&make_reply(view, 1, Opcode::Created, &[]))
            .unwrap();

        let meta = begin_frame_payload(4, 2, 16, (0, 0, 4, 2));
        ctrl.handle_reply_bytes(&make_reply(view, 10, Opcode::BeginFrame, &meta))
            .unwrap();

        let mut chunk1 = vec![0u8; 4 + 16];
        chunk1[0..4].copy_from_slice(&0u32.to_le_bytes());
        chunk1[4..].copy_from_slice(&[10u8; 16]);
        ctrl.handle_reply_bytes(&make_reply(view, 10, Opcode::FrameChunk, &chunk1))
            .unwrap();

        let mut chunk2 = vec![0u8; 4 + 16];
        chunk2[0..4].copy_from_slice(&16u32.to_le_bytes());
        chunk2[4..].copy_from_slice(&[20u8; 16]);
        ctrl.handle_reply_bytes(&make_reply(view, 10, Opcode::FrameChunk, &chunk2))
            .unwrap();

        ctrl.handle_reply_bytes(&make_reply(view, 10, Opcode::EndFrame, &[]))
            .unwrap();

        let frame = ctrl.committed_frame().expect("frame must be committed");
        assert_eq!(frame.width, 4);
        assert_eq!(frame.height, 2);
        assert_eq!(frame.stride, 16);
        assert_eq!(frame.pixels[..16], [10u8; 16]);
        assert_eq!(frame.pixels[16..], [20u8; 16]);

        let dirty = ctrl.take_dirty_regions();
        assert_eq!(dirty.len(), 1);
        assert_eq!(dirty[0], DirtyRect::new(0, 0, 4, 2));
    }

    #[test]
    fn for_url_renders_initial_web_surface() {
        let ctrl = WebViewController::for_url(1, "https://example.org", "Example").unwrap();
        assert_eq!(ctrl.committed_url(), Some("https://example.org"));
        assert_eq!(ctrl.trusted_origin().unwrap().as_str(), "https://example.org");
        let frame = ctrl.committed_frame().expect("must have committed frame");
        assert_eq!(frame.width, 576);
        assert_eq!(frame.height, 380);
    }

    #[test]
    fn local_form_interaction_flow_drives_soft_keyboard_and_input_packets() {
        let mut ctrl = WebViewController::new(2, 800, 600, 2000).unwrap();
        let view = ctrl.view();

        ctrl.encode_create().unwrap();
        ctrl.handle_reply_bytes(&make_reply(view, 1, Opcode::Created, &[]))
            .unwrap();

        ctrl.encode_navigate("https://local.alloyos.test/local-form.html")
            .unwrap();
        ctrl.handle_reply_bytes(&make_reply(
            view,
            2,
            Opcode::Url,
            b"https://local.alloyos.test/local-form.html",
        ))
        .unwrap();
        ctrl.handle_reply_bytes(&make_reply(
            view,
            3,
            Opcode::Title,
            b"alloyOS native web acceptance",
        ))
        .unwrap();

        assert!(!ctrl.soft_keyboard_visible());

        // Focusing field inside WebView shows soft keyboard
        ctrl.handle_reply_bytes(&make_reply(view, 4, Opcode::Editing, &[1, 1]))
            .unwrap();
        let focus_pkt = ctrl.encode_focus(true).unwrap();
        assert_eq!(Packet::decode(&focus_pkt).unwrap().opcode, Opcode::Focus);
        assert!(ctrl.soft_keyboard_visible());

        let commit_pkt = ctrl.encode_commit_text("alloy").unwrap();
        let decoded_commit = Packet::decode(&commit_pkt).unwrap();
        assert_eq!(decoded_commit.opcode, Opcode::CommitText);
        assert_eq!(decoded_commit.payload, b"alloy");

        let key_down = ctrl.encode_key(0x28, true, key_modifiers::SHIFT).unwrap();
        let key_up = ctrl.encode_key(0x28, false, 0).unwrap();
        assert_eq!(Packet::decode(&key_down).unwrap().opcode, Opcode::Key);
        assert_eq!(Packet::decode(&key_up).unwrap().opcode, Opcode::Key);

        ctrl.encode_focus(false).unwrap();
        assert!(!ctrl.soft_keyboard_visible());
    }

    #[test]
    fn rejects_stale_generations_and_recovers_after_crash() {
        let mut ctrl = WebViewController::new(3, 640, 480, 1000).unwrap();
        let gen1_view = ctrl.view();
        ctrl.encode_create().unwrap();
        ctrl.encode_navigate("https://portal.example.org/local-form")
            .unwrap();
        ctrl.handle_reply_bytes(&make_reply(
            gen1_view,
            1,
            Opcode::Url,
            b"https://portal.example.org/local-form",
        ))
        .unwrap();
        ctrl.handle_reply_bytes(&make_reply(gen1_view, 2, Opcode::Editing, &[1, 1]))
            .unwrap();
        ctrl.encode_focus(true).unwrap();
        assert!(ctrl.soft_keyboard_visible());

        let meta = begin_frame_payload(2, 1, 8, (0, 0, 2, 1));
        ctrl.handle_reply_bytes(&make_reply(gen1_view, 3, Opcode::BeginFrame, &meta))
            .unwrap();
        ctrl.handle_reply_bytes(&make_reply(
            gen1_view,
            4,
            Opcode::Crashed,
            &77u32.to_le_bytes(),
        ))
        .unwrap();

        assert!(ctrl.is_crashed());
        assert_eq!(ctrl.crash_reason(), Some(77));
        assert_eq!(ctrl.loading_state(), LoadingState::Failed);
        assert!(!ctrl.soft_keyboard_visible());
        assert_eq!(ctrl.view().generation, gen1_view.generation + 1);

        let mut stale_chunk = vec![0u8; 12];
        stale_chunk[4..].copy_from_slice(&[1, 2, 3, 4, 5, 6, 7, 8]);
        assert_eq!(
            ctrl.handle_reply_bytes(&make_reply(gen1_view, 3, Opcode::FrameChunk, &stale_chunk)),
            Err(Error::StaleFrame)
        );

        let recovery_packets = ctrl.recover_and_reload().unwrap();
        assert_eq!(recovery_packets.len(), 2);
        let create_pkt = Packet::decode(&recovery_packets[0]).unwrap();
        let nav_pkt = Packet::decode(&recovery_packets[1]).unwrap();
        assert_eq!(create_pkt.opcode, Opcode::Create);
        assert_eq!(create_pkt.view.generation, gen1_view.generation + 1);
        assert_eq!(nav_pkt.opcode, Opcode::Navigate);
        assert_eq!(nav_pkt.payload, b"https://portal.example.org/local-form");
        assert!(!ctrl.is_crashed());
    }

    #[test]
    fn prevents_origin_spoofing_from_untrusted_titles_or_uncommitted_requests() {
        let mut ctrl = WebViewController::new(9, 1024, 768, 2000).unwrap();
        let view = ctrl.view();

        ctrl.encode_navigate("https://trusted.example.org/welcome")
            .unwrap();
        assert_eq!(
            ctrl.requested_url(),
            Some("https://trusted.example.org/welcome")
        );
        assert!(ctrl.trusted_origin().is_none());
        assert!(ctrl.committed_url().is_none());

        ctrl.handle_reply_bytes(&make_reply(
            view,
            1,
            Opcode::Url,
            b"https://actual-host.example.net:8443/landing?from=trusted",
        ))
        .unwrap();
        assert_eq!(
            ctrl.trusted_origin().unwrap().as_str(),
            "https://actual-host.example.net:8443"
        );

        ctrl.handle_reply_bytes(&make_reply(
            view,
            2,
            Opcode::Title,
            b"https://trusted.example.org - Verified Lock",
        ))
        .unwrap();
        assert_eq!(
            ctrl.untrusted_title(),
            "https://trusted.example.org - Verified Lock"
        );
        assert_eq!(
            ctrl.trusted_origin().unwrap().as_str(),
            "https://actual-host.example.net:8443"
        );

        assert_eq!(
            ctrl.handle_reply_bytes(&make_reply(
                view,
                3,
                Opcode::Url,
                b"https://trusted.example.org@evil.example.com/phish",
            )),
            Err(Error::InvalidPayload)
        );
        assert_eq!(
            ctrl.trusted_origin().unwrap().as_str(),
            "https://actual-host.example.net:8443"
        );
    }
}
