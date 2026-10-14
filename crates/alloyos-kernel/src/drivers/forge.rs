//! Boot-owned Forge service and bounded UART bridge. Models and API routing remain owned by Forge.
use super::{fw_cfg::BootAuth, serial::SERIAL1};
use alloc::{
    boxed::Box,
    format,
    collections::VecDeque,
    rc::Rc,
    string::{String, ToString},
    vec::Vec,
};
use core::{cell::RefCell, task::Poll};
use alloyos_forge::{Forge, DEFAULT_MODEL, MAX_FRAME_BYTES};

#[cfg(not(all(target_arch = "aarch64", target_os = "none")))]
#[path = "forge_https.rs"]
pub mod forge_https;

pub const MODEL_ADDRESS: usize = 0x8000_0000;
pub const MAX_MODEL_BYTES: u64 = 14 * 1024 * 1024 * 1024;
pub fn rpc_output_pending() -> bool {
    #[cfg(all(target_arch = "aarch64", target_os = "none"))]
    { super::forge_https::RPC_OUTPUT_PENDING.load(core::sync::atomic::Ordering::Relaxed) }
    #[cfg(not(all(target_arch = "aarch64", target_os = "none")))]
    { false }
}
#[cfg(any(
    test,
    all(target_arch = "aarch64", target_os = "none", feature = "forge")
))]
const IMAGE_HEADER_BYTES: usize = 4096;

#[cfg(all(target_arch = "aarch64", target_os = "none", feature = "forge"))]
struct BootModel {
    disk: super::model_disk::ModelDisk,
    offset: usize,
    total: usize,
    weights: usize,
}

/// Owns resident weights/backend across requests and retains ephemeral boot auth.
pub struct ForgeService {
    shared: Rc<RefCell<Forge>>,
    incoming: Vec<u8>,
    discarding: bool,
    _auth: Option<BootAuth>,
    remote_pending: Option<(u64, u64)>,
    outgoing: VecDeque<u8>,
    console_requests: VecDeque<(u64, String)>,
    ui_frames: u64,
    ui_input_events: u64,
    boot_error: Option<&'static str>,
    #[cfg(all(target_arch = "aarch64", target_os = "none", feature = "forge"))]
    boot_model: Option<BootModel>,
}

impl ForgeService {
    pub fn new() -> Self {
        let mut service = Self {
            shared: Rc::new(RefCell::new(Forge::new(MAX_MODEL_BYTES))),
            incoming: Vec::new(),
            discarding: false,
            _auth: None,
            remote_pending: None,
            outgoing: VecDeque::new(),
            console_requests: VecDeque::new(),
            ui_frames: 0,
            ui_input_events: 0,
            boot_error: None,
            #[cfg(all(target_arch = "aarch64", target_os = "none", feature = "forge"))]
            boot_model: None,
        };
        service.configure_clouds();
        #[cfg(all(target_arch = "aarch64", target_os = "none", any(not(feature = "debug-shell"), feature = "forge")))]
        service.shared.borrow_mut().transform_backends(super::forge_worker::wrap);
        service
    }

    fn configure_clouds(&mut self) {
        use alloyos_forge::{cloud::ApiProvider, CloudModel};
        #[cfg(all(target_arch = "aarch64", target_os = "none"))]
        {
            self._auth = BootAuth::read();
        }
        for provider in [
            ApiProvider::Claude,
            ApiProvider::Gemini,
            ApiProvider::OpenAi,
        ] {
            let model = CloudModel::new(
                provider.default_model(),
                provider.name(),
                provider.default_model(),
                provider.key_name(),
            )
            .expect("valid built-in cloud model");
            #[cfg(all(target_arch = "aarch64", target_os = "none"))]
            if let Some(key) = self
                ._auth
                .as_ref()
                .and_then(|auth| auth.get(provider.key_name()))
            {
                let result = self.shared.borrow_mut().connect_api(
                    provider,
                    provider.default_model(),
                    key,
                    Box::new(super::forge_https::HostHttps::default()),
                );
                if result.is_ok() {
                    continue;
                }
            }
            let _ = self.shared.borrow_mut().register_cloud(&model);
        }
    }

    /// Called only by the boot CPU in a Forge-enabled, >=16 GiB QEMU guest.
    #[cfg(all(target_arch = "aarch64", target_os = "none", feature = "forge"))]
    pub fn from_boot() -> Self {
        let mut service = Self::new();
        if let Some(mut disk) = super::model_disk::ModelDisk::probe() {
            // The launcher reserves this RAM; first read only the 4 KiB header.
            if unsafe { disk.start_read(0, MODEL_ADDRESS, IMAGE_HEADER_BYTES) }.is_ok() {
                crate::serial_println!("[forge] Model disk read started after splash.");
                service.boot_model = Some(BootModel {
                    disk,
                    offset: 0,
                    total: IMAGE_HEADER_BYTES,
                    weights: 0,
                });
            } else {
                service.boot_error = Some("Could not read the local model disk.");
            }
        } else {
            service.boot_error = Some("Local model disk is missing.");
        }
        service
    }

    pub fn is_boot_loading(&self) -> bool {
        #[cfg(all(target_arch = "aarch64", target_os = "none", feature = "forge"))]
        {
            self.boot_model.is_some()
        }
        #[cfg(not(all(target_arch = "aarch64", target_os = "none", feature = "forge")))]
        {
            false
        }
    }

    pub fn take_boot_error(&mut self) -> Option<&'static str> {
        self.boot_error.take()
    }

    #[cfg(all(target_arch = "aarch64", target_os = "none", feature = "forge"))]
    fn poll_boot_model(&mut self) {
        let Some(mut loading) = self.boot_model.take() else {
            return;
        };
        match loading.disk.poll() {
            Poll::Pending => {
                self.boot_model = Some(loading);
                return;
            }
            Poll::Ready(Err(())) => {
                self.boot_error = Some("Could not load the local model from disk.");
                crate::serial_println!("[forge] Model disk load failed.");
                return;
            }
            Poll::Ready(Ok(())) => {}
        }
        if loading.offset == 0 {
            let header = unsafe {
                core::slice::from_raw_parts(MODEL_ADDRESS as *const u8, IMAGE_HEADER_BYTES)
            };
            let Some((id, weights)) = model_header(header) else {
                self.boot_error = Some("Local model image has an invalid header.");
                return;
            };
            if id != DEFAULT_MODEL {
                self.boot_error = Some("Local model image has an unsupported model ID.");
                return;
            }
            let Some(total) = model_image_size(weights, loading.disk.sectors) else {
                self.boot_error = Some("Local model image is truncated or too large.");
                return;
            };
            loading.weights = weights;
            loading.total = total;
            loading.offset = IMAGE_HEADER_BYTES;
        } else {
            loading.offset += model_read_chunk(loading.total, loading.offset).unwrap().2;
        }
        if loading.offset == loading.total {
            crate::serial_println!(
                "[forge] Model disk transfer complete ({} bytes, {} UI frames).",
                loading.total,
                self.ui_frames
            );
            let weights = unsafe {
                core::slice::from_raw_parts(
                    (MODEL_ADDRESS + IMAGE_HEADER_BYTES) as *const u8,
                    loading.weights,
                )
            };
            self.load_model(DEFAULT_MODEL, weights);
            if self
                .shared
                .borrow()
                .models()
                .iter()
                .any(|model| model.id == DEFAULT_MODEL && model.state == "ready")
            {
                crate::serial_println!("[forge] Local model ready.");
            } else {
                self.boot_error =
                    Some("Could not initialize the local model. Cloud models remain available.");
                crate::serial_println!("[forge] Local model initialization failed.");
            }
            crate::serial_println!("[forge] Model initialization complete.");
            return;
        }
        let (sector, destination, length) = model_read_chunk(loading.total, loading.offset).unwrap();
        if unsafe { loading.disk.start_read(sector, destination, length) }
        .is_err()
        {
            self.boot_error = Some("Could not read the local model disk.");
            return;
        }
        self.boot_model = Some(loading);
    }

    pub fn load_model(&mut self, id: &str, weights: &'static [u8]) {
        let mut forge = self.shared.borrow_mut();
        if id != DEFAULT_MODEL && forge.register(id).is_err() {
            return;
        }
        match alloyos_forge::gemma2::Gemma2::load(id, weights) {
            Ok(model) => {
                let backend: Box<dyn alloyos_forge::InferenceBackend> = Box::new(model);
                #[cfg(all(target_arch = "aarch64", target_os = "none", any(not(feature = "debug-shell"), feature = "forge")))]
                let backend = super::forge_worker::wrap(id, backend);
                if forge.attach(id, backend).is_ok() {
                    let _ = forge.set_default(id);
                }
            }
            Err(error) => {
                crate::serial_println!("[forge] Local model load error: {}", error.code());
                let _ = forge.mark_load_error(id, &error);
            }
        }
    }

    pub fn shared(&self) -> Rc<RefCell<Forge>> {
        self.shared.clone()
    }

    /// Feed one byte. Production ignores non-protocol serial chatter and never
    /// logs request/prompt bytes; bounded overflow is discarded through newline.
    pub fn feed(&mut self, byte: u8) -> Option<String> {
        if byte != b'\n' {
            if !self.discarding {
                if self.incoming.len() >= MAX_FRAME_BYTES {
                    self.incoming.clear();
                    self.discarding = true;
                } else {
                    self.incoming.push(byte);
                }
            }
            return None;
        }
        if self.discarding {
            self.discarding = false;
            return None;
        }
        let bytes = core::mem::take(&mut self.incoming);
        let line = core::str::from_utf8(&bytes).ok()?;
        if !line.starts_with("FORGE1\t") {
            return None;
        }
        Some(match alloyos_forge::protocol::Request::parse(line) {
            Ok(request) => {
                if let alloyos_forge::protocol::Command::Chat(input) = &request.command {
                    if self.console_requests.len() >= 1 { return Some(alloyos_forge::protocol::response(request.id, Err(alloyos_forge::ForgeError::Busy))); }
                    self.console_requests.push_back((request.id, input.clone()));
                    return None;
                }
                if let alloyos_forge::protocol::Command::Generate(input) = &request.command {
                    match self.shared.borrow_mut().start_generation(input) {
                        Ok(job) => {
                            self.remote_pending = Some((request.id, job));
                            return None;
                        }
                        Err(error) => alloyos_forge::protocol::response(request.id, Err(error)),
                    }
                } else if request.command == alloyos_forge::protocol::Command::Status {
                    // Frame-loop diagnostics let the real QEMU test verify progress during waits.
                    let json = self.shared.borrow_mut().dispatch(request);
                    let fields: Vec<&str> = json.trim_end().split('\t').collect();
                    if fields.len() == 4 {
                        if let Ok(payload) = alloyos_forge::protocol::decode_hex(fields[3]) {
                            if let Ok(mut status) =
                                serde_json::from_str::<serde_json::Value>(&payload)
                            {
                                status["ui_frames"] = serde_json::json!(self.ui_frames);
                                status["ui_input_events"] = serde_json::json!(self.ui_input_events);
                                status["inference_timing"] = serde_json::json!(super::forge_worker::timing());
                                status["generation_pending"] = serde_json::json!(self
                                    .shared
                                    .borrow()
                                    .has_pending_generation());
                                status["model_loading"] = serde_json::json!(self.is_boot_loading());
                                return Some(alloyos_forge::protocol::response(
                                    fields[1].parse().unwrap_or(0),
                                    Ok(status.to_string()),
                                ));
                            }
                        }
                    }
                    json
                } else {
                    self.shared.borrow_mut().dispatch(request)
                }
            }
            Err(error) => {
                let id = line
                    .split('\t')
                    .nth(1)
                    .and_then(|s| s.parse().ok())
                    .unwrap_or(0);
                alloyos_forge::protocol::response(id, Err(error))
            }
        })
    }

    pub fn note_ui_input(&mut self) {
        self.ui_input_events = self.ui_input_events.saturating_add(1);
    }
    pub fn take_console_request(&mut self) -> Option<(u64, String)> { self.console_requests.pop_front() }
    pub fn finish_console_request(&mut self, id: u64, result: Result<String, String>) {
        let frame = match result {
            Ok(output) => alloyos_forge::protocol::response(id, Ok(output)),
            Err(error) => format!("FORGE1\t{id}\terror\tchat_failed\t{}\n", alloyos_forge::protocol::encode_hex(&error)),
        };
        self.outgoing.extend(frame.bytes());
    }
    pub fn note_ui_frame(&mut self) {
        self.ui_frames = self.ui_frames.saturating_add(1);
    }
    pub fn poll(&mut self) {
        #[cfg(all(target_arch = "aarch64", target_os = "none", feature = "forge"))]
        self.poll_boot_model();
        #[cfg(all(target_arch = "aarch64", target_os = "none"))]
        super::forge_https::RPC_OUTPUT_PENDING.store(
            !self.outgoing.is_empty(),
            core::sync::atomic::Ordering::Relaxed,
        );
        if let Some((rpc, job)) = self.remote_pending {
            let result = self.shared.borrow_mut().poll_generation(job);
            if let Poll::Ready(result) = result {
                self.remote_pending = None;
                let response = alloyos_forge::protocol::response(rpc, result.map(|g| g.text));
                self.outgoing.extend(response.bytes());
            }
        }
        let tick_start = super::rng::monotonic_time_ns();
        // HTTPS owns incoming UART bytes while an API generation is pending.
        if !self.shared.borrow().pending_uses_transport()
            && !super::mcp_https::REQUEST_PENDING.load(core::sync::atomic::Ordering::Relaxed) {
            #[cfg(all(target_arch = "aarch64", target_os = "none"))]
            if let Some(line) = super::forge_https::take_deferred_rpc() {
                for byte in line.bytes().chain(core::iter::once(b'\n')) {
                    if let Some(reply) = self.feed(byte) {
                        self.outgoing.extend(reply.bytes());
                    }
                }
            }
            for _ in 0..1024 {
                if super::rng::monotonic_time_ns().saturating_sub(tick_start) >= 1_000_000
                    || self.shared.borrow().pending_uses_transport()
                {
                    break;
                }
                let Some(byte) = SERIAL1.lock().try_receive() else {
                    break;
                };
                if let Some(response) = self.feed(byte) {
                    self.outgoing.extend(response.bytes());
                }
            }
        }
        let mut uart = SERIAL1.lock();
        for _ in 0..1024 {
            if super::rng::monotonic_time_ns().saturating_sub(tick_start) >= 1_000_000 {
                break;
            }
            let Some(&byte) = self.outgoing.front() else {
                break;
            };
            if !uart.try_send(byte) {
                break;
            }
            self.outgoing.pop_front();
        }
        #[cfg(all(target_arch = "aarch64", target_os = "none"))]
        super::forge_https::RPC_OUTPUT_PENDING.store(
            !self.outgoing.is_empty(),
            core::sync::atomic::Ordering::Relaxed,
        );
    }

    pub fn run(mut self) -> ! {
        loop {
            self.poll();
            core::hint::spin_loop();
        }
    }
}

#[cfg(any(
    test,
    all(target_arch = "aarch64", target_os = "none", feature = "forge")
))]
fn model_image_size(weights: usize, disk_sectors: u64) -> Option<usize> {
    if weights < 24 { return None; }
    let total = weights.checked_add(IMAGE_HEADER_BYTES + 511)? & !511;
    if total as u64 > MAX_MODEL_BYTES || (total / 512) as u64 > disk_sectors {
        return None;
    }
    Some(total)
}

#[cfg(any(
    test,
    all(target_arch = "aarch64", target_os = "none", feature = "forge")
))]
fn model_read_chunk(total: usize, offset: usize) -> Option<(u64, usize, usize)> {
    if total % 512 != 0 || offset % 512 != 0 { return None; }
    let remaining = total.checked_sub(offset)?;
    if remaining == 0 { return None; }
    let length = remaining.min(16 * 1024 * 1024);
    let destination = MODEL_ADDRESS.checked_add(offset)?;
    destination.checked_add(length)?;
    Some(((offset / 512) as u64, destination, length))
}

/// Reject malformed bounds/model IDs before forming a huge borrowed slice.
#[cfg(any(
    test,
    all(target_arch = "aarch64", target_os = "none", feature = "forge")
))]
fn model_header(bytes: &[u8]) -> Option<(&str, usize)> {
    if bytes.len() < IMAGE_HEADER_BYTES || &bytes[..8] != b"FGMODEL1" {
        return None;
    }
    let size = u64::from_le_bytes(bytes[8..16].try_into().ok()?);
    if size < 24 || size > MAX_MODEL_BYTES {
        return None;
    }
    let id_bytes = &bytes[48..176];
    let end = id_bytes.iter().position(|b| *b == 0)?;
    let id = core::str::from_utf8(&id_bytes[..end]).ok()?;
    if id.is_empty()
        || !id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"-_.".contains(&b))
    {
        return None;
    }
    Some((id, usize::try_from(size).ok()?))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn serial_status_and_missing_weights_are_real_errors() {
        let mut service = ForgeService::new();
        let status = b"FORGE1\t7\tstatus\n"
            .iter()
            .filter_map(|b| service.feed(*b))
            .last()
            .unwrap();
        assert!(status.starts_with("FORGE1\t7\tok\t"));
        let result = b"FORGE1\t8\tgenerate\t-\t1\t6869\n"
            .iter()
            .filter_map(|b| service.feed(*b))
            .last()
            .unwrap();
        assert!(result.contains("\terror\tweights_missing\t"));
        let bad = b"FORGE1\t9\tgenerate\t-\t0\t6869\n"
            .iter()
            .filter_map(|b| service.feed(*b))
            .last()
            .unwrap();
        assert!(bad.starts_with("FORGE1\t9\terror\t"));
    }
    #[test]
    fn frames_are_bounded_and_recover_after_overflow() {
        let mut service = ForgeService::new();
        for _ in 0..=MAX_FRAME_BYTES {
            assert!(service.feed(b'x').is_none());
        }
        assert!(service.feed(b'\n').is_none());
        assert!(b"FORGE1\t1\tstatus\n"
            .iter()
            .any(|b| service.feed(*b).is_some()));
    }
    #[test]
    fn model_image_sizes_check_alignment_capacity_and_overflow() {
        assert_eq!(model_image_size(24, 9), Some(4608));
        assert_eq!(model_image_size(512, 9), Some(4608));
        assert_eq!(model_image_size(513, 10), Some(5120));
        assert_eq!(model_image_size(513, 9), None);
        assert_eq!(model_image_size(23, u64::MAX), None);
        assert_eq!(model_image_size(usize::MAX, u64::MAX), None);
        let maximum = MAX_MODEL_BYTES as usize;
        assert_eq!(model_image_size(maximum - IMAGE_HEADER_BYTES, u64::MAX), Some(maximum));
        assert_eq!(model_image_size(maximum - IMAGE_HEADER_BYTES + 1, u64::MAX), None);
        for boundary in [2usize << 30, 4usize << 30] {
            assert_eq!(model_image_size(boundary + 1, u64::MAX), Some(boundary + 4608));
        }
    }

    #[test]
    fn model_transfer_offsets_remain_contiguous_across_large_boundaries() {
        for weights in [24, (2usize << 30) + 512, (4usize << 30) + 512,
            MAX_MODEL_BYTES as usize - IMAGE_HEADER_BYTES] {
            let total = model_image_size(weights, u64::MAX).unwrap();
            let mut offset = IMAGE_HEADER_BYTES;
            while offset < total {
                let (sector, destination, length) = model_read_chunk(total, offset).unwrap();
                assert_eq!(sector * 512, offset as u64);
                assert_eq!(destination, MODEL_ADDRESS + offset);
                assert!(length > 0 && length <= 16 * 1024 * 1024);
                assert_eq!(length % 512, 0);
                offset += length;
                assert!(offset <= total);
            }
            assert_eq!(offset, total);
            assert_eq!(model_read_chunk(total, offset), None);
        }
        assert_eq!(model_read_chunk(512, 1024), None);
        assert_eq!(model_read_chunk(513, 0), None);
        assert_eq!(model_read_chunk(512, 1), None);
        let overflowing_address = usize::MAX & !511;
        assert_eq!(model_read_chunk(overflowing_address, overflowing_address - 512), None);
    }

    #[test]
    fn model_region_bounds_are_checked() {
        let mut header = [0; IMAGE_HEADER_BYTES];
        header[..8].copy_from_slice(b"FGMODEL1");
        header[8..16].copy_from_slice(&1024u64.to_le_bytes());
        header[48..61].copy_from_slice(DEFAULT_MODEL.as_bytes());
        assert_eq!(model_header(&header), Some((DEFAULT_MODEL, 1024)));
        for size in [(2u64 << 30) + 1, (4u64 << 30) + 1, MAX_MODEL_BYTES] {
            header[8..16].copy_from_slice(&size.to_le_bytes());
            assert_eq!(model_header(&header), Some((DEFAULT_MODEL, size as usize)));
        }
        header[8..16].copy_from_slice(&(MAX_MODEL_BYTES + 1).to_le_bytes());
        assert!(model_header(&header).is_none());
    }
}
