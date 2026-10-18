//! Cooperative public-search HTTP relay. No network I/O blocks a UI frame.
use alloc::{format, string::String, vec::Vec};
use core::{sync::atomic::{AtomicBool, AtomicU64, Ordering}, task::Poll};
use alloyos_mcp::transport::{HttpRequest, HttpResponse, HttpTransport};
use super::{rng, serial::SERIAL1};

static NEXT_REQUEST: AtomicU64 = AtomicU64::new(1);
pub static REQUEST_PENDING: AtomicBool = AtomicBool::new(false);
pub static REQUEST_WRITING: AtomicBool = AtomicBool::new(false);
struct Transfer { prefix: String, frame: String, sent: usize, line: Vec<u8>, started: u64 }
#[derive(Default)]
pub struct HostRegistryHttp { pending: Option<Transfer>, next: u64, waiting: Option<HttpRequest>, queued_at: u64 }
impl Drop for HostRegistryHttp {
    fn drop(&mut self) { if self.pending.is_some() { REQUEST_PENDING.store(false, Ordering::Relaxed); } }
}
impl HttpTransport for HostRegistryHttp {
    fn cancel_request(&mut self) {
        self.waiting = None;
        // Preserve framing if cancellation lands midway through a UART line.
        // A single delimiter lets the host reject the abandoned request.
        if let Some(transfer) = self.pending.take() {
            if transfer.sent > 0 && transfer.sent < transfer.frame.len() {
                SERIAL1.lock().send(b'\n');
            }
            if transfer.sent==transfer.frame.len() && transfer.frame.contains("\tmcp\t") {
                let frame=format!("MCPHTTP1\t{}\tcancel\n",self.next);
                let mut uart=SERIAL1.lock();
                for byte in frame.bytes() {uart.send(byte);}
            }
            REQUEST_PENDING.store(false, Ordering::Relaxed);
            REQUEST_WRITING.store(false, Ordering::Relaxed);
        }
    }
    fn request(&mut self, _: &HttpRequest) -> Result<HttpResponse, String> {
        Err("registry HTTP requires cooperative tool polling".into())
    }
    fn start_request(&mut self, request: &HttpRequest) -> Poll<Result<HttpResponse, String>> {
        if self.pending.is_some() || self.waiting.is_some() { return Poll::Ready(Err("registry request already pending".into())); }
        if REQUEST_PENDING.load(Ordering::Relaxed) {
            self.waiting = Some(request.clone());
            self.queued_at = rng::monotonic_time_ns();
            return Poll::Pending;
        }
        let action = if alloyos_mcp::transport::allowed_oauth_http(request) {"authhttp"} else if request.method == "POST" {
            if alloyos_mcp::transport::allowed_public_post(&request.url) { "post" }
            else if alloyos_mcp::transport::allowed_remote_mcp(request) { "mcp" }
            else { return Poll::Ready(Err("unsupported or invalid hosted MCP POST request".into())); }
        } else if request.method == "GET" {
            if !request.headers.is_empty() || !request.body.is_empty() || !alloyos_mcp::transport::allowed_public_get(&request.url) {
                return Poll::Ready(Err("only public MCP Registry and DuckDuckGo search are wired to this transport".into()));
            }
            "get"
        } else {
            return Poll::Ready(Err("unsupported HTTP method".into()));
        };
        self.next = NEXT_REQUEST.fetch_add(1, Ordering::Relaxed);
        let json = match serde_json::to_string(request) { Ok(json) => json, Err(_) => return Poll::Ready(Err("invalid registry HTTP request".into())) };
        let frame = format!("MCPHTTP1\t{}\t{}\t{}\n", self.next, action, alloyos_forge::protocol::encode_hex(&json));
        self.pending = Some(Transfer { prefix: format!("MCPHTTP1\t{}\t", self.next), frame, sent: 0, line: Vec::new(), started: rng::monotonic_time_ns() });
        REQUEST_PENDING.store(true, Ordering::Relaxed);
        crate::serial_println!("[mcp-search] stage=request_started");
        Poll::Pending
    }
    fn poll_request(&mut self) -> Poll<Result<HttpResponse, String>> {
        if self.waiting.is_some() {
            if rng::monotonic_time_ns().saturating_sub(self.queued_at)>90_000_000_000 {
                self.waiting=None;
                return Poll::Ready(Err("HTTP relay queue timeout".into()));
            }
            if REQUEST_PENDING.load(Ordering::Relaxed) { return Poll::Pending; }
            let request = self.waiting.take().unwrap();
            return self.start_request(&request);
        }
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

/// Opt-in live guest/host transport gate. Never enabled by ordinary builds or CI.
/// The host supplies an explicit endpoint and optional public read-only call;
/// no provider/model credentials or OAuth browser actions are involved.
#[cfg(feature="remote-mcp-selftest")]
pub fn remote_acceptance() {
    use alloc::{boxed::Box,collections::BTreeMap,rc::Rc,string::ToString};
    use core::cell::RefCell;
    use alloyos_mcp::{AppCapability,AppManifest,McpApp,McpValue,RemoteMcpApp,RemoteConnectionStatus};
    let result=(||->Result<(),String>{
        let config=super::fw_cfg::read_file("opt/alloyos/remote-probe",16384).ok_or("missing probe config")?;
        let config:serde_json::Value=serde_json::from_slice(&config).map_err(|_|"invalid probe config")?;
        let endpoint=config["endpoint"].as_str().ok_or("missing probe endpoint")?;
        if alloyos_mcp::transport::remote_mcp_authority(endpoint).is_none() {return Err("invalid probe endpoint".into());}
        let mut client=alloyos_mcp::transport::HttpClient::new(Rc::new(RefCell::new(BTreeMap::new())));
        client.set_transport(Box::new(HostRegistryHttp::default()));
        let mut app=RemoteMcpApp::new(AppManifest::new_remote("probe","Transport probe","1",
            alloc::vec![AppCapability::ExposeMcpTools,AppCapability::ScopedNetwork],endpoint,Some("streamable-http".into())),
            Vec::new(),Vec::new(),Vec::new(),Vec::new()).map_err(|_|"invalid probe manifest")?
            .with_http_client(Rc::new(RefCell::new(client)));
        let deadline=rng::monotonic_time_ns()+90_000_000_000;
        loop {
            if rng::monotonic_time_ns()>deadline {return Err("probe connect timeout".into());}
            match app.poll_connect() {
                Poll::Pending=>core::hint::spin_loop(),
                Poll::Ready(Ok(tools))=>{
                    crate::serial_println!("REMOTE-MCP: tools/list verified count={} protocol={}",tools.len(),app.negotiated_protocol_version().unwrap_or_default());break;
                },
                Poll::Ready(Err(_)) if config["expect_auth"]==true && matches!(app.connection_status(),RemoteConnectionStatus::AuthorizationRequired{..})=>{
                    crate::serial_println!("REMOTE-MCP: authorization challenge preserved PASS");return Ok(());
                }
                Poll::Ready(Err(_))=>return Err("remote connection did not become ready".into()),
            }
        }
        if config["expect_auth"]==true {return Err("expected authorization challenge was absent".into());}
        if let Some(tool)=config["tool"].as_str() {
            let args=McpValue::from_json(&config["arguments"].to_string()).map_err(|_|"invalid probe tool arguments")?;
            let args=args.as_map().ok_or("probe arguments must be object")?;
            let deadline=rng::monotonic_time_ns()+90_000_000_000;
            loop {
                if rng::monotonic_time_ns()>deadline {return Err("probe call timeout".into());}
                match app.poll_call_tool(tool,args) {
                    Poll::Pending=>core::hint::spin_loop(),
                    Poll::Ready(Ok(value))=>{
                        if value.as_map().and_then(|map|map.get("isError")).and_then(McpValue::as_bool)==Some(true) {return Err("remote tool reported error".into());}
                        crate::serial_println!("REMOTE-MCP: tools/call verified PASS");break;
                    }
                    Poll::Ready(Err(_))=>return Err("remote tool call failed".into()),
                }
            }
        }
        Ok(())
    })();
    match result {
        Ok(())=>{crate::serial_println!("REMOTE-MCP: production guest relay PASS");},
        Err(error)=>{crate::serial_println!("REMOTE-MCP: FAIL {}",error);},
    }
}

/// Trusted OS OAuth HTTP port. The marker is constructed here, never model input.
#[derive(Default)]
pub struct OAuthHttp { inner: HostRegistryHttp }
impl alloyos_welder::oauth::OAuthNetwork for OAuthHttp {
    fn start(&mut self,request:&HttpRequest)->Poll<Result<HttpResponse,alloyos_mcp::account::AccountError>> {
        let mut request=request.clone();
        if request.headers.iter().any(|(n,_)|n.to_ascii_lowercase().starts_with("x-alloyos-")) {
            return Poll::Ready(Err(alloyos_mcp::account::AccountError::InvalidRequest));
        }
        request.headers.push(("X-AlloyOS-OAuth".into(),"1".into()));
        if !alloyos_mcp::transport::allowed_oauth_http(&request) {return Poll::Ready(Err(alloyos_mcp::account::AccountError::InvalidRequest));}
        self.inner.start_request(&request).map(|result|result.map_err(|_|alloyos_mcp::account::AccountError::TransportFailed))
    }
    fn poll(&mut self)->Poll<Result<HttpResponse,alloyos_mcp::account::AccountError>> {
        self.inner.poll_request().map(|result|result.map_err(|_|alloyos_mcp::account::AccountError::TransportFailed))
    }
    fn cancel(&mut self) {self.inner.cancel_request();}
}
