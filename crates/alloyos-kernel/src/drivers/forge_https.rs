//! Guest-native authenticated HTTPS (`GuestHttps` over VirtIO-Net + TCP/IP + DNS + TLS 1.3)
//! and cooperative QEMU UART fallback (`HostHttps`). Each poll does bounded work and never
//! blocks the UI thread while performing TLS 1.3 handshake or streaming SSE chunks.
use crate::drivers::{rng, serial::SERIAL1};
use crate::net::{with_net_stack, DnsResolver};
use alloc::{collections::VecDeque, format, string::String, vec::Vec};
use core::{
    sync::atomic::{AtomicBool, AtomicU64, Ordering},
    task::Poll,
};
use liballoy::{
    net::{
        HandshakeProgress, HttpRequestBuilder, ParsedHttpResponse, RootCaStore, Tls13Session,
        Tls13State,
    },
    tls::TlsError,
};
use alloyos_forge::{
    cloud::{HttpRequest, HttpResponse, HttpsClient, HTTP_BODY_LIMIT, HTTP_FRAME_LIMIT},
    ForgeError,
};
use alloyos_proto::{SocketAddrV4, AF_INET, SOCK_STREAM};

pub static RPC_OUTPUT_PENDING: AtomicBool = AtomicBool::new(false);
static NEXT: AtomicU64 = AtomicU64::new(1);
static DEFERRED_RPC: crate::sync::IrqMutex<VecDeque<String>> =
    crate::sync::IrqMutex::new(VecDeque::new());
const BYTE_BUDGET: usize = 4096;
const TIME_BUDGET_NS: u64 = 1_000_000;

fn parse_https_url(url: &str) -> Result<(&str, u16, &str), ForgeError> {
    let rest = url
        .strip_prefix("https://")
        .ok_or(ForgeError::InvalidRequest)?;
    if rest.is_empty() || rest.bytes().any(|b| b.is_ascii_control() || b == b' ') {
        return Err(ForgeError::InvalidRequest);
    }
    let (host_port, path) = match rest.find('/') {
        Some(idx) => (&rest[..idx], &rest[idx..]),
        None => (rest, "/"),
    };
    let (host, port) = match host_port.split_once(':') {
        Some((h, p)) => (
            h,
            p.parse::<u16>().map_err(|_| ForgeError::InvalidRequest)?,
        ),
        None => (host_port, 443),
    };
    if host.is_empty() {
        return Err(ForgeError::InvalidRequest);
    }
    Ok((host, port, path))
}

fn map_tls_error(err: TlsError) -> ForgeError {
    match err {
        TlsError::UntrustedCertificateAuthority
        | TlsError::CertificateExpiredOrInvalid
        | TlsError::HostnameMismatch
        | TlsError::InvalidCertificateSignature
        | TlsError::InvalidCertificateVerify
        | TlsError::InvalidFinishedVerifyData => ForgeError::AuthenticationFailed,
        TlsError::BadRecordMac
        | TlsError::HandshakeMalformed
        | TlsError::UnsupportedVersion
        | TlsError::UnsupportedCipherSuite
        | TlsError::MissingKeyShare
        | TlsError::InvalidKeyShare
        | TlsError::UnexpectedMessage
        | TlsError::AlertReceived(_) => ForgeError::InvalidResponse,
    }
}

struct GuestTransfer {
    sock_fd: usize,
    tls: Tls13Session,
    pending_http_plain: Vec<u8>,
    rx_encrypted: Vec<u8>,
    rx_decrypted_http: Vec<u8>,
    partial_body: String,
    started: u64,
}

impl Drop for GuestTransfer {
    fn drop(&mut self) {
        self.pending_http_plain.fill(0);
        let fd = self.sock_fd;
        with_net_stack(|net| {
            let _ = net.close_socket(fd);
        });
    }
}

/// Guest-native authenticated HTTPS client executing DNS resolution, TCP socket connection,
/// TLS 1.3 X25519 ECDHE handshake, X.509v3 certificate chain & hostname verification, and
/// ChaCha20-Poly1305 encrypted record transport directly over the kernel network stack.
pub struct GuestHttps {
    ca_store: RootCaStore,
    pending: Option<GuestTransfer>,
}

impl Default for GuestHttps {
    fn default() -> Self {
        Self::new()
    }
}

impl GuestHttps {
    pub fn new() -> Self {
        Self {
            ca_store: RootCaStore::default_bundle(),
            pending: None,
        }
    }

    pub fn with_ca_store(ca_store: RootCaStore) -> Self {
        Self {
            ca_store,
            pending: None,
        }
    }
}

impl HttpsClient for GuestHttps {
    fn post(&mut self, request: &HttpRequest) -> Result<HttpResponse, ForgeError> {
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
        let (host, port, path) = match parse_https_url(&request.url) {
            Ok(v) => v,
            Err(e) => return Poll::Ready(Err(e)),
        };
        if request.body.len() > HTTP_BODY_LIMIT {
            return Poll::Ready(Err(ForgeError::InvalidRequest));
        }
        for (k, v) in &request.headers {
            if k.bytes().any(|b| b.is_ascii_control()) || v.bytes().any(|b| b.is_ascii_control()) {
                return Poll::Ready(Err(ForgeError::InvalidRequest));
            }
        }

        let ip = DnsResolver::resolve_hostname(host);
        let mut builder = HttpRequestBuilder::new("POST", host, path);
        for (k, v) in &request.headers {
            if !k.eq_ignore_ascii_case("content-type") {
                builder = builder.header(k, v);
            }
        }
        builder = builder.json_body(&request.body);
        let http_plain = builder.build_bytes();

        let mut tls = Tls13Session::with_ca_store(host, self.ca_store.clone());
        let client_hello = tls.build_client_hello_record();

        let sock_res = with_net_stack(|net| -> Result<usize, ForgeError> {
            let fd = net
                .create_socket(AF_INET, SOCK_STREAM)
                .map_err(|_| ForgeError::TransportUnavailable)?;
            if net.connect_socket(fd, SocketAddrV4 { ip, port }).is_err() {
                let _ = net.close_socket(fd);
                return Err(ForgeError::TransportUnavailable);
            }
            if net.send_socket(fd, &client_hello).is_err() {
                let _ = net.close_socket(fd);
                return Err(ForgeError::TransportUnavailable);
            }
            Ok(fd)
        });

        let sock_fd = match sock_res {
            Ok(fd) => fd,
            Err(err) => return Poll::Ready(Err(err)),
        };

        self.pending = Some(GuestTransfer {
            sock_fd,
            tls,
            pending_http_plain: http_plain,
            rx_encrypted: Vec::new(),
            rx_decrypted_http: Vec::new(),
            partial_body: String::new(),
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

        let fd = transfer.sock_fd;
        let mut chunk = [0u8; 2048];
        let read_res = with_net_stack(|net| net.recv_socket(fd, &mut chunk));
        match read_res {
            Ok(n) if n > 0 => {
                if transfer.rx_encrypted.len() + n > HTTP_FRAME_LIMIT {
                    self.pending = None;
                    return Poll::Ready(Err(ForgeError::InvalidResponse));
                }
                transfer.rx_encrypted.extend_from_slice(&chunk[..n]);
            }
            Ok(_) => {}
            Err(_) => {
                self.pending = None;
                return Poll::Ready(Err(ForgeError::TransportUnavailable));
            }
        }

        // Phase 1: Complete TLS 1.3 handshake and authenticate server certificate chain + signature
        while !transfer.tls.is_handshake_complete() {
            if transfer.rx_encrypted.len() < 5 {
                return Poll::Pending;
            }
            let rec_len =
                u16::from_be_bytes([transfer.rx_encrypted[3], transfer.rx_encrypted[4]]) as usize;
            if transfer.rx_encrypted.len() < 5 + rec_len {
                return Poll::Pending;
            }
            let rec: Vec<u8> = transfer.rx_encrypted.drain(..5 + rec_len).collect();
            match transfer.tls.process_handshake_record(&rec) {
                Ok(HandshakeProgress::NeedMoreRecords) => {}
                Ok(HandshakeProgress::HandshakeComplete {
                    client_finished_record,
                }) => {
                    let encrypted_http = transfer
                        .tls
                        .seal_application_data(&transfer.pending_http_plain);
                    transfer.pending_http_plain.fill(0);
                    transfer.pending_http_plain.clear();
                    let send_ok = with_net_stack(|net| {
                        net.send_socket(fd, &client_finished_record).is_ok()
                            && net.send_socket(fd, &encrypted_http).is_ok()
                    });
                    if !send_ok {
                        self.pending = None;
                        return Poll::Ready(Err(ForgeError::TransportUnavailable));
                    }
                    return Poll::Pending;
                }
                Err(tls_err) => {
                    self.pending = None;
                    return Poll::Ready(Err(map_tls_error(tls_err)));
                }
            }
        }

        // Phase 2: Process encrypted server records until partial_body advances by one body chunk
        // or the stream closes, so SSE streams advance incrementally across polls.
        let prev_partial_len = transfer.partial_body.len();
        while transfer.rx_encrypted.len() >= 5 && transfer.tls.state != Tls13State::Closed {
            let rec_len =
                u16::from_be_bytes([transfer.rx_encrypted[3], transfer.rx_encrypted[4]]) as usize;
            if transfer.rx_encrypted.len() < 5 + rec_len {
                break;
            }
            let rec: Vec<u8> = transfer.rx_encrypted.drain(..5 + rec_len).collect();
            match transfer.tls.open_server_record(&rec) {
                Ok((ctype, plaintext)) => {
                    if ctype == liballoy::net::TLS_CONTENT_APPLICATION_DATA {
                        if transfer.rx_decrypted_http.len() + plaintext.len() > HTTP_FRAME_LIMIT {
                            self.pending = None;
                            return Poll::Ready(Err(ForgeError::InvalidResponse));
                        }
                        transfer.rx_decrypted_http.extend_from_slice(&plaintext);
                        if let Ok(parsed) = ParsedHttpResponse::parse(&transfer.rx_decrypted_http) {
                            if let Ok(text) = core::str::from_utf8(&parsed.body) {
                                transfer.partial_body = String::from(text);
                            }
                        }
                        if transfer.partial_body.len() > prev_partial_len {
                            // If only the 24-byte close_notify record remains, consume it now;
                            // otherwise yield so the caller observes this incremental SSE chunk.
                            if transfer.rx_encrypted.len() != 24 {
                                break;
                            }
                        }
                    }
                }
                Err(tls_err) => {
                    self.pending = None;
                    return Poll::Ready(Err(map_tls_error(tls_err)));
                }
            }
        }

        if transfer.tls.state == Tls13State::Closed {
            let raw_http = core::mem::take(&mut transfer.rx_decrypted_http);
            self.pending = None;
            let parsed =
                ParsedHttpResponse::parse(&raw_http).map_err(|_| ForgeError::InvalidResponse)?;
            if parsed.body.len() > HTTP_BODY_LIMIT {
                return Poll::Ready(Err(ForgeError::InvalidResponse));
            }
            let body =
                String::from_utf8(parsed.body).map_err(|_| ForgeError::InvalidResponse)?;
            return Poll::Ready(Ok(HttpResponse {
                status: parsed.status,
                body,
            }));
        }

        Poll::Pending
    }

    fn partial_response_body(&self) -> Option<&str> {
        let transfer = self.pending.as_ref()?;
        if transfer.partial_body.is_empty() {
            None
        } else {
            Some(&transfer.partial_body)
        }
    }
}

struct Transfer {
    prefix: String,
    frame: String,
    sent: usize,
    line: Vec<u8>,
    partial_body: String,
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
    guest: GuestHttps,
    using_guest: bool,
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
        if self.pending.is_some() || self.using_guest {
            return Poll::Ready(Err(ForgeError::Busy));
        }
        if let Ok((host, port, _)) = parse_https_url(&request.url) {
            let ip = DnsResolver::resolve_hostname(host);
            if with_net_stack(|net| net.has_tls_endpoint(ip, port)) {
                self.using_guest = true;
                let poll = self.guest.start_post(request);
                if poll.is_ready() {
                    self.using_guest = false;
                }
                return poll;
            }
        }
        let id = NEXT.fetch_add(1, Ordering::Relaxed);
        let mut json = match serde_json::to_string(request) {
            Ok(json) => json,
            Err(_) => return Poll::Ready(Err(ForgeError::InvalidRequest)),
        };
        let frame = format!(
            "FORGEHTTP1\t{id}\tpost\t{}\n",
            alloyos_forge::protocol::encode_hex(&json)
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
            partial_body: String::new(),
            started: rng::monotonic_time_ns(),
        });
        Poll::Pending
    }
    fn poll_post(&mut self) -> Poll<Result<HttpResponse, ForgeError>> {
        if self.using_guest {
            let poll = self.guest.poll_post();
            if poll.is_ready() {
                self.using_guest = false;
            }
            return poll;
        }
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
                match fields.as_slice() {
                    ["chunk", payload] => {
                        if let Ok(decoded) = alloyos_forge::protocol::decode_hex(payload) {
                            transfer.partial_body.push_str(&decoded);
                        }
                        break;
                    }
                    ["ok", payload] => {
                        completed = Some(
                            alloyos_forge::protocol::decode_hex(payload).and_then(|decoded| {
                                serde_json::from_str::<HttpResponse>(&decoded)
                                    .map_err(|_| ForgeError::InvalidResponse)
                            }),
                        );
                        break;
                    }
                    ["error", _] => {
                        completed = Some(Err(ForgeError::TransportUnavailable));
                        break;
                    }
                    _ => {
                        completed = Some(Err(ForgeError::InvalidResponse));
                        break;
                    }
                }
            }
            if text.starts_with("FORGE1\t") && text.len() <= alloyos_forge::MAX_FRAME_BYTES {
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
    fn partial_response_body(&self) -> Option<&str> {
        if self.using_guest {
            return self.guest.partial_response_body();
        }
        let transfer = self.pending.as_ref()?;
        if transfer.partial_body.is_empty() {
            None
        } else {
            Some(&transfer.partial_body)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::net::{TlsEndpoint, TEST_NET_LOCK};
    use alloc::{boxed::Box, vec};
    use liballoy::net::sha256;
    use liballoy::tls::{ed25519_public_key, Tls13ServerConfig, X509Certificate};
    use alloyos_forge::{cloud::ApiProvider, Forge, GenerationRequest};

    #[test]
    fn guest_https_authenticates_tls13_and_streams_sse_over_kernel_net_stack() {
        let _net_guard = TEST_NET_LOCK.lock();
        for provider in [
            ApiProvider::Claude,
            ApiProvider::Gemini,
            ApiProvider::OpenAi,
        ] {
            let (host, header_pair, chunks) = match provider {
                ApiProvider::Claude => (
                    "api.anthropic.com",
                    ("x-api-key", "guest-secret-key"),
                    vec![
                        b"event: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"delta\":{\"text\":\"Native \"}}\n\n".to_vec(),
                        b"event: message_delta\ndata: {\"type\":\"message_delta\",\"delta\":{\"text\":\"TLS 1.3\"},\"usage\":{\"output_tokens\":3}}\n\n".to_vec(),
                    ],
                ),
                ApiProvider::Gemini => (
                    "generativelanguage.googleapis.com",
                    ("x-goog-api-key", "guest-secret-key"),
                    vec![
                        b"data: {\"candidates\":[{\"content\":{\"parts\":[{\"text\":\"Native \"}]}}]}\n\n".to_vec(),
                        b"data: {\"candidates\":[{\"content\":{\"parts\":[{\"text\":\"TLS 1.3\"}]}}],\"usageMetadata\":{\"candidatesTokenCount\":3}}\n\n".to_vec(),
                    ],
                ),
                ApiProvider::OpenAi => (
                    "api.openai.com",
                    ("Authorization", "Bearer guest-secret-key"),
                    vec![
                        b"data: {\"type\":\"response.output_text.delta\",\"delta\":\"Native \"}\n\n".to_vec(),
                        b"data: {\"type\":\"response.output_text.delta\",\"delta\":\"TLS 1.3\",\"usage\":{\"output_tokens\":3}}\n\ndata: [DONE]\n\n".to_vec(),
                    ],
                ),
            };
            let ip = DnsResolver::resolve_hostname(host);
            with_net_stack(|net| {
                net.clear_tls_endpoints();
                net.register_tls_endpoint(
                    ip,
                    443,
                    TlsEndpoint {
                        server_config: Tls13ServerConfig::trusted_for_host(host),
                        status: 200,
                        content_type: String::from("text/event-stream"),
                        body_chunks: chunks,
                        require_header: Some((
                            String::from(header_pair.0),
                            String::from(header_pair.1),
                        )),
                    },
                );
            });

            let mut forge = Forge::new(0);
            forge
                .connect_api(
                    provider,
                    provider.default_model(),
                    "guest-secret-key",
                    Box::new(HostHttps::default()),
                )
                .unwrap();

            let req = GenerationRequest {
                model: Some(provider.default_model().into()),
                prompt: "Verify guest HTTPS".into(),
                max_tokens: 32,
            };
            let job = forge.start_generation(&req).unwrap();

            let mut saw_partial = false;
            let mut final_gen = None;
            for _ in 0..16 {
                match forge.poll_generation(job) {
                    Poll::Pending => {
                        if forge.partial_text(job) == Some("Native ") {
                            saw_partial = true;
                        }
                    }
                    Poll::Ready(res) => {
                        final_gen = Some(res.expect("authenticated TLS 1.3 generation"));
                        break;
                    }
                }
            }
            assert!(
                saw_partial,
                "expected incremental SSE partial_text during TLS 1.3 stream for {}",
                provider.name()
            );
            let done = final_gen.expect("completed generation");
            assert_eq!(done.text, "Native TLS 1.3");
            assert_eq!(done.generated_tokens, 3);
        }
        with_net_stack(|net| net.clear_tls_endpoints());
    }

    #[test]
    fn guest_https_rejects_untrusted_certs_wrong_host_bad_signatures_and_tampered_records() {
        let _net_guard = TEST_NET_LOCK.lock();
        let provider = ApiProvider::Claude;
        let host = "api.anthropic.com";
        let ip = DnsResolver::resolve_hostname(host);

        let mut bad_configs: Vec<(Tls13ServerConfig, ForgeError)> = Vec::new();

        // 1. Untrusted Root CA
        let mut untrusted_ca = Tls13ServerConfig::trusted_for_host(host);
        untrusted_ca.ca_name = String::from("Untrusted Evil Root");
        untrusted_ca.ca_private_seed = sha256(b"untrusted-evil-root");
        bad_configs.push((untrusted_ca, ForgeError::AuthenticationFailed));

        // 2. Wrong hostname in leaf certificate
        let wrong_host = Tls13ServerConfig::trusted_for_host("attacker.example.com");
        bad_configs.push((wrong_host, ForgeError::AuthenticationFailed));

        // 3. Forged leaf certificate signature
        let mut forged_cert = Tls13ServerConfig::trusted_for_host(host);
        let leaf_pub = ed25519_public_key(&forged_cert.leaf_private_seed);
        let ca_pub = ed25519_public_key(&forged_cert.ca_private_seed);
        let bad_leaf_der = X509Certificate::build_signed_der(
            host,
            &forged_cert.ca_name,
            &[host],
            &leaf_pub,
            false,
            true,
            &sha256(b"wrong-issuer-seed"),
        );
        let root_der = X509Certificate::build_signed_der(
            &forged_cert.ca_name,
            &forged_cert.ca_name,
            &[],
            &ca_pub,
            true,
            true,
            &forged_cert.ca_private_seed,
        );
        forged_cert.cert_chain_override = Some(vec![bad_leaf_der, root_der]);
        bad_configs.push((forged_cert, ForgeError::AuthenticationFailed));

        // 4. Bad CertificateVerify signature
        let mut bad_cv = Tls13ServerConfig::trusted_for_host(host);
        bad_cv.cert_verify_key_override = Some(sha256(b"impostor-leaf-key"));
        bad_configs.push((bad_cv, ForgeError::AuthenticationFailed));

        // 5. Tampered Server Finished MAC
        let mut bad_fin = Tls13ServerConfig::trusted_for_host(host);
        bad_fin.tamper_finished = true;
        bad_configs.push((bad_fin, ForgeError::AuthenticationFailed));

        // 6. Tampered encrypted application record
        let mut bad_record = Tls13ServerConfig::trusted_for_host(host);
        bad_record.tamper_app_record = true;
        bad_configs.push((bad_record, ForgeError::InvalidResponse));

        for (cfg, expected_err) in bad_configs {
            with_net_stack(|net| {
                net.clear_tls_endpoints();
                net.register_tls_endpoint(
                    ip,
                    443,
                    TlsEndpoint {
                        server_config: cfg,
                        status: 200,
                        content_type: String::from("application/json"),
                        body_chunks: vec![b"{\"type\":\"message\",\"content\":[{\"type\":\"text\",\"text\":\"ok\"}],\"usage\":{\"output_tokens\":1}}".to_vec()],
                        require_header: None,
                    },
                );
            });

            let mut forge = Forge::new(0);
            forge
                .connect_api(
                    provider,
                    provider.default_model(),
                    "guest-secret-key",
                    Box::new(GuestHttps::new()),
                )
                .unwrap();

            let err = forge
                .generate(&GenerationRequest {
                    model: Some(provider.default_model().into()),
                    prompt: "Must fail".into(),
                    max_tokens: 16,
                })
                .unwrap_err();
            assert_eq!(err, expected_err);
        }
        with_net_stack(|net| net.clear_tls_endpoints());
    }
}
