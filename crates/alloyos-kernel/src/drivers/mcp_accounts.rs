//! Cooperative optional browser assistant. OAuth state, token exchange and
//! credential custody remain inside the OS; callback queries are private IPC.
use super::{
    mcp_https::{REQUEST_PENDING, REQUEST_WRITING},
    rng,
    serial::SERIAL1,
};
use alloc::{collections::VecDeque, format, string::String, vec::Vec};
use alloyos_mcp::{
    account::AccountError,
    browser::{BrowserOperation, BrowserOutcome, BrowserRequest, BrowserResponse, MAX_BROWSER_FRAME_BYTES},
};
use alloyos_welder::oauth::BrowserAssistant;
use core::{sync::atomic::{AtomicU64, Ordering}, task::Poll};

static NEXT_CANCEL: AtomicU64 = AtomicU64::new(1 << 63);
static CLEANUP: spin::Mutex<Option<Cleanup>> = spin::Mutex::new(None);
#[derive(Default)]
struct Cleanup {
    queue: VecDeque<BrowserRequest>,
    active: Option<(HostBrowserAssistant, u64)>,
}
fn queue_cancel(mut request: BrowserRequest) {
    request.operation = BrowserOperation::Cancel;
    request.request_id = NEXT_CANCEL.fetch_add(1, Ordering::Relaxed);
    let mut cleanup = CLEANUP.lock();
    let state = cleanup.get_or_insert_with(Cleanup::default);
    if state.queue.iter().any(|old| old.app_id == request.app_id && old.endpoint == request.endpoint && old.attempt == request.attempt) { return; }
    // The helper accepts at most four active flows. Bound abandoned cleanup work
    // independently; its autonomous lease expiry is the fallback if unavailable.
    if state.queue.len() < 8 { state.queue.push_back(request); }
}
/// Drive best-effort helper cleanup after the OAuth caller has stopped polling.
/// Use the cooperative serial pump; each attempt has a five-second deadline.
pub fn poll_browser_cleanup() {
    poll_browser_cleanup_at(rng::monotonic_time_ns());
}
fn poll_browser_cleanup_at(now: u64) {
    let mut cleanup = CLEANUP.lock();
    let Some(state) = cleanup.as_mut() else { return; };
    if let Some((assistant, started)) = &mut state.active {
        if now.saturating_sub(*started) >= 5_000_000_000 {
            assistant.cancel();
            state.active = None;
        } else if assistant.poll().is_ready() {
            state.active = None;
        }
        return;
    }
    if let Some(request) = state.queue.pop_front() {
        let mut assistant = HostBrowserAssistant::default();
        if assistant.start(&request).is_pending() {
            state.active = Some((assistant, now));
        }
    }
}

#[derive(Default)]
pub struct HostBrowserAssistant {
    waiting: Option<BrowserRequest>,
    transfer: Option<Transfer>,
    active: Option<BrowserRequest>,
}
struct Transfer {
    request: BrowserRequest,
    prefix: String,
    frame: String,
    sent: usize,
    line: Vec<u8>,
    started: u64,
}
impl HostBrowserAssistant {
    fn release(&mut self) {
        if self.transfer.take().is_some() {
            REQUEST_PENDING.store(false, Ordering::Relaxed);
            REQUEST_WRITING.store(false, Ordering::Relaxed);
        }
    }
}
impl Drop for HostBrowserAssistant {
    fn drop(&mut self) {
        self.cancel();
    }
}
impl BrowserAssistant for HostBrowserAssistant {
    fn start(&mut self, request: &BrowserRequest) -> Poll<Result<BrowserResponse, AccountError>> {
        if self.transfer.is_some() || self.waiting.is_some() {
            return Poll::Ready(Err(AccountError::Busy));
        }
        if request.validate().is_err() {
            return Poll::Ready(Err(AccountError::InvalidRequest));
        }
        if !matches!(request.operation, BrowserOperation::Cancel) {
            let mut binding = request.clone();
            // Retain the flow binding, never an authorization URL or callback.
            binding.operation = BrowserOperation::Cancel;
            self.active = Some(binding);
        }
        if REQUEST_PENDING
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Relaxed)
            .is_err()
        {
            self.waiting = Some(request.clone());
            return Poll::Pending;
        }
        let raw = match serde_json::to_string(request) {
            Ok(raw) if raw.len() <= MAX_BROWSER_FRAME_BYTES => raw,
            _ => {
                REQUEST_PENDING.store(false, Ordering::Relaxed);
                return Poll::Ready(Err(AccountError::InvalidRequest));
            }
        };
        self.transfer = Some(Transfer {
            request: request.clone(),
            prefix: format!("BROWSER1\t{}\t", request.request_id),
            frame: format!(
                "BROWSER1\t{}\t{}\n",
                request.request_id,
                alloyos_forge::protocol::encode_hex(&raw)
            ),
            sent: 0,
            line: Vec::new(),
            started: rng::monotonic_time_ns(),
        });
        Poll::Pending
    }
    fn poll(&mut self) -> Poll<Result<BrowserResponse, AccountError>> {
        if let Some(request) = self.waiting.take() {
            return self.start(&request);
        }
        let Some(transfer) = &mut self.transfer else {
            return Poll::Ready(Err(AccountError::InvalidRequest));
        };
        let now = rng::monotonic_time_ns();
        if now.saturating_sub(transfer.started) >= 90_000_000_000 {
            self.cancel();
            return Poll::Ready(Err(AccountError::TransportFailed));
        }
        let mut result = None;
        if !super::forge::rpc_output_pending() || REQUEST_WRITING.load(Ordering::Relaxed) {
            let mut uart = SERIAL1.lock();
            for _ in 0..4096 {
                if rng::monotonic_time_ns().saturating_sub(now) >= 1_000_000 {
                    break;
                }
                if transfer.sent == transfer.frame.len() {
                    break;
                }
                if !uart.try_send(transfer.frame.as_bytes()[transfer.sent]) {
                    break;
                }
                transfer.sent += 1;
                REQUEST_WRITING.store(transfer.sent < transfer.frame.len(), Ordering::Relaxed);
            }
            if transfer.sent == transfer.frame.len() {
                for _ in 0..4096 {
                    if rng::monotonic_time_ns().saturating_sub(now) >= 1_000_000 {
                        break;
                    }
                    let Some(byte) = uart.try_receive() else {
                        break;
                    };
                    if byte != b'\n' {
                        if transfer.line.len() >= MAX_BROWSER_FRAME_BYTES * 2 + 80 {
                            result = Some(Err(AccountError::InvalidBrokerResponse));
                            break;
                        }
                        transfer.line.push(byte);
                        continue;
                    }
                    let line = match String::from_utf8(core::mem::take(&mut transfer.line)) {
                        Ok(line) => line,
                        Err(_) => {
                            result = Some(Err(AccountError::InvalidBrokerResponse));
                            break;
                        }
                    };
                    if line.starts_with("FORGE1\t") || line.starts_with("VOICE1\t") {
                        #[cfg(all(target_arch = "aarch64", target_os = "none"))]
                        super::forge_https::defer_rpc(line);
                        continue;
                    }
                    let Some(raw) = line
                        .strip_prefix(&transfer.prefix)
                        .and_then(|line| line.strip_prefix("ok\t"))
                    else {
                        continue;
                    };
                    result = Some(
                        alloyos_forge::protocol::decode_hex(raw)
                            .map_err(|_| AccountError::InvalidBrokerResponse)
                            .and_then(|raw| {
                                if raw.len() <= MAX_BROWSER_FRAME_BYTES {
                                    serde_json::from_str::<BrowserResponse>(&raw)
                                        .map_err(|_| AccountError::InvalidBrokerResponse)
                                } else {
                                    Err(AccountError::InvalidBrokerResponse)
                                }
                            })
                            .and_then(|response| {
                                response
                                    .validate_for(&transfer.request)
                                    .map_err(|_| AccountError::InvalidBrokerResponse)?;
                                Ok(response)
                            }),
                    );
                    break;
                }
            }
        }
        if let Some(result) = result {
            if matches!(&result, Ok(response) if matches!(response.outcome, BrowserOutcome::Callback { .. } | BrowserOutcome::Cancelled)) {
                self.active = None;
            }
            self.release();
            Poll::Ready(result)
        } else {
            Poll::Pending
        }
    }
    fn cancel(&mut self) {
        self.waiting = None;
        if let Some(transfer) = &self.transfer {
            if transfer.sent > 0 && transfer.sent < transfer.frame.len() {
                SERIAL1.lock().send(b'\n');
            }
        }
        self.release();
        if let Some(request) = self.active.take() { queue_cancel(request); }
    }
}

#[derive(Default)]
pub struct KernelOAuthCrypto;
impl alloyos_welder::oauth::OAuthCrypto for KernelOAuthCrypto {
    fn random(&mut self, output: &mut [u8]) -> Result<(), AccountError> {
        rng::try_fill_secure_random(output).map_err(|_| AccountError::HostUnavailable)
    }
    fn sha256(&self, input: &[u8]) -> [u8; 32] { liballoy::net::sha256(input) }
    fn unix_seconds(&self) -> Option<u64> { super::rtc::unix_time_ms().map(|ms| ms / 1000) }
}

#[cfg(test)]
mod crypto_tests {
    use alloyos_welder::oauth::{base64url, OAuthCrypto};
    #[test]
    fn rfc7636_pkce_uses_kernel_sha256() {
        let crypto = super::KernelOAuthCrypto;
        let verifier = b"dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk";
        assert_eq!(base64url(&crypto.sha256(verifier)),
            "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM");
    }
    #[test]
    fn canceled_completed_flow_queues_bound_cleanup_and_never_blocks_for_transport() {
        use super::*;
        *CLEANUP.lock() = None;
        REQUEST_PENDING.store(false, Ordering::Relaxed);
        let request = BrowserRequest {
            request_id: 42, app_id: "mail".into(), endpoint: "https://mail.example/mcp".into(),
            attempt: "synthetic-attempt-0123456789".into(),
            operation: BrowserOperation::Open { resource: None, authorization_url: "https://auth.example/?state=private-state".into() },
        };
        let mut browser = HostBrowserAssistant::default();
        assert!(browser.start(&request).is_pending());
        // A completed Open response releases the transport, but must preserve
        // enough binding to cancel its host listener later, between requests.
        browser.release();
        browser.cancel();
        browser.cancel();
        {
            let cleanup = CLEANUP.lock();
            let state = cleanup.as_ref().unwrap();
            assert_eq!(state.queue.len(), 1, "repeated cancellation queues no duplicate");
            let cancel = state.queue.front().unwrap();
            assert!(matches!(cancel.operation, BrowserOperation::Cancel));
            assert_eq!((&cancel.app_id, &cancel.endpoint, &cancel.attempt), (&request.app_id, &request.endpoint, &request.attempt));
            assert_ne!(cancel.request_id, request.request_id);
            assert!(!serde_json::to_string(cancel).unwrap().contains("private-state"));
        }
        // A busy transport queues cleanup rather than writing into another
        // request. The maintenance deadline also releases abandoned queue work.
        REQUEST_PENDING.store(true, Ordering::Relaxed);
        poll_browser_cleanup_at(1);
        {
            let cleanup = CLEANUP.lock();
            let assistant = &cleanup.as_ref().unwrap().active.as_ref().unwrap().0;
            assert!(assistant.waiting.is_some());
            assert!(assistant.transfer.is_none());
        }
        poll_browser_cleanup_at(5_000_000_001);
        assert!(CLEANUP.lock().as_ref().unwrap().active.is_none());
        assert!(REQUEST_PENDING.load(Ordering::Relaxed), "cleanup never releases another transport's ownership");
        REQUEST_PENDING.store(false, Ordering::Relaxed);
        queue_cancel(request);
        poll_browser_cleanup_at(6_000_000_000);
        {
            let cleanup = CLEANUP.lock();
            let transfer = cleanup.as_ref().unwrap().active.as_ref().unwrap().0.transfer.as_ref().unwrap();
            let hex = transfer.frame.trim_end().rsplit('\t').next().unwrap();
            let raw = alloyos_forge::protocol::decode_hex(hex).unwrap();
            let cancel: BrowserRequest = serde_json::from_str(&raw).unwrap();
            assert!(matches!(cancel.operation, BrowserOperation::Cancel));
            cancel.validate().unwrap();
        }
        poll_browser_cleanup_at(11_000_000_000);
        assert!(!REQUEST_PENDING.load(Ordering::Relaxed));
        *CLEANUP.lock() = None;
    }
}
