//! Cooperative public-registry HTTP relay. No network I/O blocks a UI frame.
use alloc::{format, string::String, vec::Vec};
use core::{sync::atomic::{AtomicBool, Ordering}, task::Poll};
use alloyos_mcp::transport::{HttpRequest, HttpResponse, HttpTransport};
use super::{rng, serial::SERIAL1};

pub static REQUEST_PENDING: AtomicBool = AtomicBool::new(false);
pub static REQUEST_WRITING: AtomicBool = AtomicBool::new(false);
struct Transfer { prefix: String, frame: String, sent: usize, line: Vec<u8>, started: u64 }
#[derive(Default)]
pub struct HostRegistryHttp { pending: Option<Transfer>, next: u64 }
impl Drop for HostRegistryHttp {
    fn drop(&mut self) { if self.pending.is_some() { REQUEST_PENDING.store(false, Ordering::Relaxed); } }
}
impl HttpTransport for HostRegistryHttp {
    fn cancel_request(&mut self) {
        // Preserve framing if cancellation lands midway through a UART line.
        // A single delimiter lets the host reject the abandoned request.
        if let Some(transfer) = self.pending.take() {
            if transfer.sent > 0 && transfer.sent < transfer.frame.len() {
                SERIAL1.lock().send(b'\n');
            }
        }
        REQUEST_PENDING.store(false, Ordering::Relaxed);
        REQUEST_WRITING.store(false, Ordering::Relaxed);
    }
    fn request(&mut self, _: &HttpRequest) -> Result<HttpResponse, String> {
        Err("registry HTTP requires cooperative tool polling".into())
    }
    fn start_request(&mut self, request: &HttpRequest) -> Poll<Result<HttpResponse, String>> {
        if self.pending.is_some() { return Poll::Ready(Err("registry request already pending".into())); }
        if request.method != "GET" || !request.url.starts_with("https://registry.modelcontextprotocol.io/v0.1/servers?") {
            return Poll::Ready(Err("only public MCP Registry search is wired to this transport".into()));
        }
        self.next = self.next.saturating_add(1);
        let json = match serde_json::to_string(request) { Ok(json) => json, Err(_) => return Poll::Ready(Err("invalid registry HTTP request".into())) };
        let frame = format!("MCPHTTP1\t{}\tget\t{}\n", self.next, alloyos_forge::protocol::encode_hex(&json));
        self.pending = Some(Transfer { prefix: format!("MCPHTTP1\t{}\t", self.next), frame, sent: 0, line: Vec::new(), started: rng::monotonic_time_ns() });
        REQUEST_PENDING.store(true, Ordering::Relaxed);
        crate::serial_println!("[mcp-search] stage=request_started");
        Poll::Pending
    }
    fn poll_request(&mut self) -> Poll<Result<HttpResponse, String>> {
        let Some(transfer) = &mut self.pending else { return Poll::Ready(Err("no registry request pending".into())); };
        let now = rng::monotonic_time_ns();
        let mut result = None;
        if now.saturating_sub(transfer.started) > 90_000_000_000 { result = Some(Err("registry HTTP timeout".into())); }
        if result.is_none() && (!super::forge::rpc_output_pending() || REQUEST_WRITING.load(Ordering::Relaxed)) {
            let mut uart = SERIAL1.lock();
            for _ in 0..4096 {
                if rng::monotonic_time_ns().saturating_sub(now) >= 1_000_000 { break; }
                if transfer.sent == transfer.frame.len() { break; }
                if !uart.try_send(transfer.frame.as_bytes()[transfer.sent]) { break; }
                transfer.sent += 1;
                REQUEST_WRITING.store(transfer.sent < transfer.frame.len(), Ordering::Relaxed);
            }
            if transfer.sent == transfer.frame.len() {
                for _ in 0..4096 {
                    if rng::monotonic_time_ns().saturating_sub(now) >= 1_000_000 { break; }
                    let Some(byte) = uart.try_receive() else { break; };
                    if byte != b'\n' {
                        if transfer.line.len() >= alloyos_forge::cloud::HTTP_FRAME_LIMIT { result = Some(Err("registry response frame too large".into())); break; }
                        transfer.line.push(byte); continue;
                    }
                    let line = match String::from_utf8(core::mem::take(&mut transfer.line)) { Ok(line) => line, Err(_) => { result = Some(Err("invalid registry response encoding".into())); break; } };
                    if line.starts_with("FORGE1\t") || line.starts_with("VOICE1\t") {
                        #[cfg(all(target_arch="aarch64",target_os="none"))]
                        super::forge_https::defer_rpc(line);
                        continue;
                    }
                    let Some(payload) = line.strip_prefix(&transfer.prefix) else { continue; };
                    let fields: Vec<_> = payload.split('\t').collect();
                    result = Some(match fields.as_slice() {
                        ["ok", body] => alloyos_forge::protocol::decode_hex(body).map_err(|_| "invalid registry frame hex".into()).and_then(|body| serde_json::from_str(&body).map_err(|_| "invalid registry HTTP envelope".into())),
                        ["error", _] => Err("public registry HTTPS request failed".into()),
                        _ => Err("invalid registry response frame".into()),
                    });
                    break;
                }
            }
        }
        if let Some(result) = result {
            self.pending = None;
            REQUEST_PENDING.store(false, Ordering::Relaxed);
            REQUEST_WRITING.store(false, Ordering::Relaxed);
            crate::serial_println!("[mcp-search] stage=response_received success={}", result.is_ok());
            Poll::Ready(result)
        } else { Poll::Pending }
    }
}
