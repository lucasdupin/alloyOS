//! Synchronous, bounded QEMU HTTPS capability. API semantics remain in Forge.
use super::{rng, serial::SERIAL1};
use alloc::{format, vec::Vec};
use core::sync::atomic::{AtomicU64, Ordering};
use metalos_forge::{
    cloud::{HttpRequest, HttpResponse, HttpsClient, HTTP_FRAME_LIMIT},
    ForgeError,
};
static NEXT: AtomicU64 = AtomicU64::new(1);
pub struct HostHttps;
impl HttpsClient for HostHttps {
    fn post(&mut self, request: &HttpRequest) -> Result<HttpResponse, ForgeError> {
        let id = NEXT.fetch_add(1, Ordering::Relaxed);
        let json = serde_json::to_string(request).map_err(|_| ForgeError::InvalidRequest)?;
        let mut frame = format!(
            "FORGEHTTP1\t{id}\tpost\t{}\n",
            metalos_forge::protocol::encode_hex(&json)
        );
        if frame.len() > HTTP_FRAME_LIMIT {
            return Err(ForgeError::InvalidRequest);
        }
        {
            let mut uart = SERIAL1.lock();
            for byte in frame.bytes() {
                uart.send(byte);
            }
        }
        unsafe {
            frame.as_bytes_mut().fill(0);
        }
        let start = rng::monotonic_time_ns();
        let prefix = format!("FORGEHTTP1\t{id}\t");
        let mut line = Vec::new();
        loop {
            if rng::monotonic_time_ns().saturating_sub(start) > 55_000_000_000 {
                return Err(ForgeError::TransportUnavailable);
            }
            let byte = SERIAL1.lock().try_receive();
            let Some(byte) = byte else {
                core::hint::spin_loop();
                continue;
            };
            if byte != b'\n' {
                if line.len() >= HTTP_FRAME_LIMIT {
                    return Err(ForgeError::InvalidResponse);
                }
                line.push(byte);
                continue;
            }
            let text = core::str::from_utf8(&line).map_err(|_| ForgeError::InvalidResponse)?;
            if let Some(rest) = text.strip_prefix(&prefix) {
                let fields: Vec<&str> = rest.trim_end_matches('\r').split('\t').collect();
                return match fields.as_slice() {
                    ["ok", payload] => {
                        let decoded = metalos_forge::protocol::decode_hex(payload)?;
                        serde_json::from_str::<HttpResponse>(&decoded)
                            .map_err(|_| ForgeError::InvalidResponse)
                    }
                    ["error", _] => Err(ForgeError::TransportUnavailable),
                    _ => Err(ForgeError::InvalidResponse),
                };
            }
            line.clear();
        }
    }
}
