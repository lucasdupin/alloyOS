//! Cooperative optional browser assistant. OAuth state, token exchange and
//! credential custody remain inside the OS; callback queries are private IPC.
use super::{
    mcp_https::{REQUEST_PENDING, REQUEST_WRITING},
    rng,
    serial::SERIAL1,
};
use alloc::{format, string::String, vec::Vec};
use alloyos_mcp::{
    account::AccountError,
    browser::{BrowserRequest, BrowserResponse, MAX_BROWSER_FRAME_BYTES},
};
use alloyos_welder::oauth::BrowserAssistant;
use core::{sync::atomic::Ordering, task::Poll};

#[derive(Default)]
pub struct HostBrowserAssistant {
    waiting: Option<BrowserRequest>,
    transfer: Option<Transfer>,
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
}
