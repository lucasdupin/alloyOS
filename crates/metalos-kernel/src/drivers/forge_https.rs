//! Cooperative QEMU HTTPS capability. Each poll does bounded UART work and never
//! waits for a response or holds the UI thread while the host performs HTTPS.
use super::{rng, serial::SERIAL1};
use alloc::{collections::VecDeque, format, string::String, vec::Vec};
use core::{
    sync::atomic::{AtomicBool, AtomicU64, Ordering},
    task::Poll,
};
use metalos_forge::{
    cloud::{HttpRequest, HttpResponse, HttpsClient, HTTP_FRAME_LIMIT},
    ForgeError,
};
pub static RPC_OUTPUT_PENDING: AtomicBool = AtomicBool::new(false);
static NEXT: AtomicU64 = AtomicU64::new(1);
static DEFERRED_RPC: crate::sync::IrqMutex<VecDeque<String>> = crate::sync::IrqMutex::new(VecDeque::new());
const BYTE_BUDGET: usize = 4096;
const TIME_BUDGET_NS: u64 = 1_000_000;
struct Transfer {
    prefix: String,
    frame: String,
    sent: usize,
    line: Vec<u8>,
    started: u64,
}
impl Drop for Transfer {
    fn drop(&mut self) {
        unsafe {
            self.frame.as_bytes_mut().fill(0);
        }
    }
}
#[derive(Default)]
pub struct HostHttps {
    pending: Option<Transfer>,
}
pub fn take_deferred_rpc() -> Option<String> {
    DEFERRED_RPC.lock().pop_front()
}
impl HttpsClient for HostHttps {
    fn post(&mut self, request: &HttpRequest) -> Result<HttpResponse, ForgeError> {
        // Compatibility for callers outside the UI's cooperative request path.
        match self.start_post(request) {
            Poll::Ready(result) => return result,
            Poll::Pending => {}
        }
        loop {
            match self.poll_post() {
                Poll::Ready(result) => return result,
                Poll::Pending => core::hint::spin_loop(),
            }
        }
    }
    fn start_post(&mut self, request: &HttpRequest) -> Poll<Result<HttpResponse, ForgeError>> {
        if self.pending.is_some() {
            return Poll::Ready(Err(ForgeError::Busy));
        }
        let id = NEXT.fetch_add(1, Ordering::Relaxed);
        let mut json = match serde_json::to_string(request) {
            Ok(json) => json,
            Err(_) => return Poll::Ready(Err(ForgeError::InvalidRequest)),
        };
        let frame = format!(
            "FORGEHTTP1\t{id}\tpost\t{}\n",
            metalos_forge::protocol::encode_hex(&json)
        );
        unsafe {
            json.as_bytes_mut().fill(0);
        }
        if frame.len() > HTTP_FRAME_LIMIT {
            return Poll::Ready(Err(ForgeError::InvalidRequest));
        }
        self.pending = Some(Transfer {
            prefix: format!("FORGEHTTP1\t{id}\t"),
            frame,
            sent: 0,
            line: Vec::new(),
            started: rng::monotonic_time_ns(),
        });
        Poll::Pending
    }
    fn poll_post(&mut self) -> Poll<Result<HttpResponse, ForgeError>> {
        let Some(transfer) = self.pending.as_mut() else {
            return Poll::Ready(Err(ForgeError::InvalidRequest));
        };
        let now = rng::monotonic_time_ns();
        if now.saturating_sub(transfer.started) > 55_000_000_000 {
            self.pending = None;
            return Poll::Ready(Err(ForgeError::TransportUnavailable));
        }
        // Finish any preceding RPC frame before beginning an HTTPS frame.
        if RPC_OUTPUT_PENDING.load(Ordering::Relaxed) {
            return Poll::Pending;
        }
        let mut uart = SERIAL1.lock();
        for _ in 0..BYTE_BUDGET {
            if transfer.sent == transfer.frame.len()
                || rng::monotonic_time_ns().saturating_sub(now) >= TIME_BUDGET_NS
            {
                break;
            }
            if !uart.try_send(transfer.frame.as_bytes()[transfer.sent]) {
                break;
            }
            transfer.sent += 1;
        }
        let mut completed = None;
        for _ in 0..BYTE_BUDGET {
            if rng::monotonic_time_ns().saturating_sub(now) >= TIME_BUDGET_NS {
                break;
            }
            let Some(byte) = uart.try_receive() else {
                break;
            };
            if byte != b'\n' {
                if transfer.line.len() >= HTTP_FRAME_LIMIT {
                    completed = Some(Err(ForgeError::InvalidResponse));
                    break;
                }
                transfer.line.push(byte);
                continue;
            }
            let line = core::mem::take(&mut transfer.line);
            let Ok(text) = core::str::from_utf8(&line) else {
                completed = Some(Err(ForgeError::InvalidResponse));
                break;
            };
            if let Some(rest) = text.strip_prefix(&transfer.prefix) {
                let fields: Vec<&str> = rest.trim_end_matches('\r').split('\t').collect();
                completed = Some(match fields.as_slice() {
                    ["ok", payload] => {
                        metalos_forge::protocol::decode_hex(payload).and_then(|decoded| {
                            serde_json::from_str::<HttpResponse>(&decoded)
                                .map_err(|_| ForgeError::InvalidResponse)
                        })
                    }
                    ["error", _] => Err(ForgeError::TransportUnavailable),
                    _ => Err(ForgeError::InvalidResponse),
                });
                break;
            }
            if text.starts_with("FORGE1\t") && text.len() <= metalos_forge::MAX_FRAME_BYTES {
                let mut deferred = DEFERRED_RPC.lock();
                if deferred.len() < 8 {
                    deferred.push_back(String::from(text));
                }
            }
        }
        drop(uart);
        if let Some(result) = completed {
            self.pending = None;
            Poll::Ready(result)
        } else {
            Poll::Pending
        }
    }
}
