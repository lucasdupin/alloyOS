//! Boot-owned Forge service and bounded UART bridge. Models and API routing remain owned by Forge.
use super::{fw_cfg::BootAuth, serial::SERIAL1};
use alloc::{boxed::Box, rc::Rc, string::String, vec::Vec};
use core::cell::RefCell;
use metalos_forge::{Forge, DEFAULT_MODEL, MAX_FRAME_BYTES};

pub const MODEL_ADDRESS: usize = 0x8000_0000;
pub const MAX_MODEL_BYTES: u64 = 14 * 1024 * 1024 * 1024;
#[cfg(any(
    test,
    all(target_arch = "aarch64", target_os = "none", feature = "forge")
))]
const IMAGE_HEADER_BYTES: usize = 4096;

/// Owns resident weights/backend across requests and retains ephemeral boot auth.
pub struct ForgeService {
    shared: Rc<RefCell<Forge>>,
    incoming: Vec<u8>,
    discarding: bool,
    _auth: Option<BootAuth>,
}

impl ForgeService {
    pub fn new() -> Self {
        let mut service = Self {
            shared: Rc::new(RefCell::new(Forge::new(MAX_MODEL_BYTES))),
            incoming: Vec::new(),
            discarding: false,
            _auth: None,
        };
        service.configure_clouds();
        service
    }

    fn configure_clouds(&mut self) {
        use metalos_forge::{cloud::ApiProvider, CloudModel};
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
                    Box::new(super::forge_https::HostHttps),
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
        // SAFETY: Welder's Forge launch reserves the model region in guest RAM
        // and the allocator only uses addresses strictly below MODEL_ADDRESS.
        let header =
            unsafe { core::slice::from_raw_parts(MODEL_ADDRESS as *const u8, IMAGE_HEADER_BYTES) };
        if let Some((id, length)) = model_header(header) {
            let weights = unsafe {
                core::slice::from_raw_parts(
                    (MODEL_ADDRESS + IMAGE_HEADER_BYTES) as *const u8,
                    length,
                )
            };
            service.load_model(id, weights);
        }
        service
    }

    pub fn load_model(&mut self, id: &str, weights: &'static [u8]) {
        let mut forge = self.shared.borrow_mut();
        if id != DEFAULT_MODEL && forge.register(id).is_err() {
            return;
        }
        match metalos_forge::gemma2::Gemma2::load(id, weights) {
            Ok(model) => {
                if forge.attach(id, Box::new(model)).is_ok() {
                    let _ = forge.set_default(id);
                }
            }
            Err(error) => {
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
        Some(match metalos_forge::protocol::Request::parse(line) {
            Ok(request) => self.shared.borrow_mut().dispatch(request),
            Err(error) => {
                let id = line
                    .split('\t')
                    .nth(1)
                    .and_then(|s| s.parse().ok())
                    .unwrap_or(0);
                metalos_forge::protocol::response(id, Err(error))
            }
        })
    }

    pub fn poll(&mut self) {
        for _ in 0..1024 {
            let Some(byte) = SERIAL1.lock().try_receive() else {
                break;
            };
            if let Some(response) = self.feed(byte) {
                let mut uart = SERIAL1.lock();
                for byte in response.bytes() {
                    uart.send(byte);
                }
            }
        }
    }

    pub fn run(mut self) -> ! {
        loop {
            self.poll();
            core::hint::spin_loop();
        }
    }
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
    fn model_region_bounds_are_checked() {
        let mut header = [0; IMAGE_HEADER_BYTES];
        header[..8].copy_from_slice(b"FGMODEL1");
        header[8..16].copy_from_slice(&1024u64.to_le_bytes());
        header[48..61].copy_from_slice(DEFAULT_MODEL.as_bytes());
        assert_eq!(model_header(&header), Some((DEFAULT_MODEL, 1024)));
        header[8..16].copy_from_slice(&(MAX_MODEL_BYTES + 1).to_le_bytes());
        assert!(model_header(&header).is_none());
    }
}
