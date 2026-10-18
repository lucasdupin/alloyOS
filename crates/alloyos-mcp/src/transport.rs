//! HTTP capability supplied by the host. Without a host transport only explicitly
//! registered test fixtures can be used; missing fixtures are never a real HTTP 404.
use crate::{McpError, McpValue};
use alloc::{
    boxed::Box,
    collections::BTreeMap,
    format,
    rc::Rc,
    string::{String, ToString},
    vec::Vec,
};
use core::cell::RefCell;
use core::task::Poll;

pub const MAX_HTTP_BODY_BYTES: usize = 1024 * 1024;
pub const MAX_BEARER_BYTES: usize = 4000;
pub const MAX_HTTP_HEADERS: usize = 16;
pub const MAX_HTTP_HEADER_BYTES: usize = 8192;

pub fn valid_headers(headers: &[(String, String)]) -> bool {
    headers.len() <= MAX_HTTP_HEADERS
        && headers.iter().map(|(n,v)| n.len() + v.len()).sum::<usize>() <= MAX_HTTP_HEADER_BYTES
        && headers.iter().all(|(name,value)| !name.is_empty() && name.len() <= 128
            && name.bytes().all(|b| b.is_ascii_alphanumeric() || b"!#$%&'*+-.^_`|~".contains(&b))
            && value.len() <= 4096 && !value.bytes().any(|b| b.is_ascii_control()))
        && headers.iter().enumerate().all(|(i,(name,_))|
            !headers[..i].iter().any(|(other,_)| other.eq_ignore_ascii_case(name)))
}

/// Production remote MCP requires TLS and a DNS authority; DNS results are
/// separately validated and pinned by the host relay before connecting.
pub fn remote_mcp_authority(url: &str) -> Option<(&str, u16)> {
    if url.len() > 8192 || url.bytes().any(|b| b.is_ascii_control() || b.is_ascii_whitespace())
        || url.contains(['@', '#', '\\', '{', '}']) { return None; }
    let authority = url.strip_prefix("https://")?.split(['/', '?']).next()?;
    let (host, port) = match authority.split_once(':') {
        Some((host,port)) => (host,port.parse::<u16>().ok().filter(|p| *p != 0)?),
        None => (authority,443),
    };
    let lower = host.to_ascii_lowercase();
    if host.len() > 253 || !host.contains('.') || host.ends_with('.')
        || host.bytes().all(|b| b.is_ascii_digit() || b == b'.')
        || ["localhost","local","localdomain","internal","lan","home"].iter()
            .any(|s| lower == *s || lower.ends_with(&format!(".{s}")))
        || !host.split('.').all(|label| !label.is_empty() && label.len() <= 63
            && !label.starts_with('-') && !label.ends_with('-')
            && label.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')) { return None; }
    Some((host,port))
}

pub fn allowed_remote_mcp(request: &HttpRequest) -> bool {
    if request.method != "POST" || remote_mcp_authority(&request.url).is_none()
        || request.body.len() > MAX_HTTP_BODY_BYTES || !valid_headers(&request.headers) { return false; }
    if !request.headers.iter().all(|(name,_)| ["content-type","accept","mcp-protocol-version","mcp-session-id","mcp-method","mcp-name","x-alloyos-app-id","x-alloyos-credential-ref","x-alloyos-trusted-bearer"]
        .iter().any(|allowed| name.eq_ignore_ascii_case(allowed)) || name.to_ascii_lowercase().starts_with("mcp-param-")) { return false; }
    let Ok(value) = McpValue::from_json(&request.body) else { return false; };
    let Some(map) = value.as_map() else { return false; };
    map.get("jsonrpc").and_then(McpValue::as_str) == Some("2.0")
        && matches!(map.get("method").and_then(McpValue::as_str),
            Some("initialize" | "notifications/initialized" | "tools/list" | "tools/call" | "resources/read" | "prompts/get"))
}

/// Fixed public endpoints accepted by the QEMU relay. Search queries must be
/// percent-encoded: no credentials, alternate hosts, or extra URL parameters.
pub fn allowed_public_get(url: &str) -> bool {
    if url.len() > 8192 || url.bytes().any(|byte| byte.is_ascii_control()) { return false; }
    if url.starts_with("https://registry.modelcontextprotocol.io/v0.1/servers?") { return true; }
    let Some(query) = url.strip_prefix("https://html.duckduckgo.com/html/?q=") else { return false; };
    !query.is_empty() && query.bytes().all(|byte| byte.is_ascii_alphanumeric() || b"%-_.!~*'()".contains(&byte))
}

pub fn allowed_gemini_url(url: &str) -> bool {
    if url.len() > 8192 || url.bytes().any(|byte| byte.is_ascii_control()) { return false; }
    url.strip_prefix("https://generativelanguage.googleapis.com/v1beta/models/")
        .and_then(|s| s.strip_suffix(":generateContent"))
        .is_some_and(|id| {
            !id.is_empty()
                && id.len() <= 128
                && id.bytes().all(|b| b.is_ascii_alphanumeric() || b"-_.".contains(&b))
        })
}

pub fn allowed_public_post(url: &str) -> bool {
    allowed_gemini_url(url)
}

pub fn allowed_public_request(request: &HttpRequest) -> bool {
    match request.method {
        "GET" => request.headers.is_empty() && request.body.is_empty() && allowed_public_get(&request.url),
        "POST" => allowed_public_post(&request.url),
        _ => false,
    }
}

#[derive(Clone, serde::Serialize)]
pub struct HttpRequest {
    pub method: &'static str,
    pub url: String,
    pub headers: Vec<(String, String)>,
    pub body: String,
}

#[derive(serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HttpResponse {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: String,
}

/// Hosts own network I/O. The OS owns credentials; development TLS relays see
/// trusted bearer bytes transiently and must never log or persist them. Bound requests and avoid
/// blocking the kernel UI; the synchronous tool interface does not supply a driver.
pub trait HttpTransport {
    fn request(&mut self, request: &HttpRequest) -> Result<HttpResponse, String>;
    fn start_request(&mut self, request: &HttpRequest) -> Poll<Result<HttpResponse, String>> {
        Poll::Ready(self.request(request))
    }
    fn poll_request(&mut self) -> Poll<Result<HttpResponse, String>> {
        Poll::Ready(Err("no pending HTTP request".into()))
    }
    fn cancel_request(&mut self) {}
}

pub struct HttpClient {
    routes: Rc<RefCell<BTreeMap<String, String>>>,
    transport: Option<Box<dyn HttpTransport>>,
}

impl HttpClient {
    /// Status-preserving API for protocol/authentication state machines. Unlike
    /// get/post, a401 is a bounded response rather than a lost auth challenge.
    pub fn start_request(&mut self, request: &HttpRequest) -> Poll<Result<HttpResponse, McpError>> {
        if request.body.len() > MAX_HTTP_BODY_BYTES || !valid_headers(&request.headers) {
            return Poll::Ready(Err(McpError::invalid_params("HTTP request exceeds bounds")));
        }
        match &mut self.transport {
            Some(transport) => transport.start_request(request).map(Self::bounded_response),
            None => Poll::Ready(self.request(request)),
        }
    }
    pub fn poll_request(&mut self) -> Poll<Result<HttpResponse, McpError>> {
        match &mut self.transport {
            Some(transport) => transport.poll_request().map(Self::bounded_response),
            None => Poll::Ready(Err(McpError::internal_error("no pending HTTP transport"))),
        }
    }
    pub fn cancel_request(&mut self) { self.cancel_get(); }
    fn bounded_response(response: Result<HttpResponse, String>) -> Result<HttpResponse, McpError> {
        let response = response.map_err(McpError::internal_error)?;
        if response.body.len() > MAX_HTTP_BODY_BYTES || !valid_headers(&response.headers) {
            return Err(McpError::internal_error("HTTP response exceeds bounds"));
        }
        Ok(response)
    }
    pub fn new(routes: Rc<RefCell<BTreeMap<String, String>>>) -> Self {
        Self {
            routes,
            transport: None,
        }
    }
    pub fn set_transport(&mut self, transport: Box<dyn HttpTransport>) {
        self.transport = Some(transport);
    }
    pub fn request(&mut self, request: &HttpRequest) -> Result<HttpResponse, McpError> {
        if request.body.len() > MAX_HTTP_BODY_BYTES || !valid_headers(&request.headers) {
            return Err(McpError::invalid_params("HTTP request body exceeds limit"));
        }
        let response = if let Some(transport) = &mut self.transport {
            transport
                .request(request)
                .map_err(McpError::internal_error)?
        } else {
            let routes = self.routes.borrow();
            let key = if request.method == "POST" {
                let rpc = McpValue::from_json(&request.body);
                let method = rpc
                    .as_ref()
                    .ok()
                    .and_then(McpValue::as_map)
                    .and_then(|map| map.get("method"))
                    .and_then(McpValue::as_str);
                if let Some(method) = method {
                    format!("POST {} {}", request.url, method)
                } else {
                    format!("POST {}", request.url)
                }
            } else {
                request.url.clone()
            };
            let body = routes.get(&key).or_else(|| routes.get(&request.url)).cloned().ok_or_else(|| {
                McpError::internal_error(
                    "HTTP transport unavailable and no matching test fixture registered",
                )
            })?;
            HttpResponse {
                status: if body.is_empty() { 202 } else { 200 },
                headers: Vec::new(),
                body,
            }
        };
        if response.body.len() > MAX_HTTP_BODY_BYTES || !valid_headers(&response.headers) {
            return Err(McpError::internal_error("HTTP response body exceeds limit"));
        }
        if !(200..300).contains(&response.status) {
            return Err(McpError::internal_error(format!(
                "HTTP request failed with status {}",
                response.status
            )));
        }
        Ok(response)
    }
    pub fn get(&mut self, url: &str) -> Result<String, McpError> {
        self.request(&HttpRequest {
            method: "GET",
            url: url.to_string(),
            headers: Vec::new(),
            body: String::new(),
        })
        .map(|response| response.body)
    }
    pub fn start_get(&mut self, url: &str) -> Poll<Result<HttpResponse, McpError>> {
        let request = HttpRequest { method: "GET", url: url.into(), headers: Vec::new(), body: String::new() };
        match &mut self.transport {
            Some(transport) => transport.start_request(&request).map(Self::check_response),
            None => Poll::Ready(self.request(&request)),
        }
    }
    pub fn poll_get(&mut self) -> Poll<Result<HttpResponse, McpError>> {
        match &mut self.transport {
            Some(transport) => transport.poll_request().map(Self::check_response),
            None => Poll::Ready(Err(McpError::internal_error("no pending HTTP transport"))),
        }
    }
    pub fn cancel_get(&mut self) {
        if let Some(transport) = &mut self.transport { transport.cancel_request(); }
    }
    pub fn post(&mut self, url: &str, body: &str, headers: &[(String, String)]) -> Result<HttpResponse, McpError> {
        self.request(&HttpRequest {
            method: "POST",
            url: url.to_string(),
            headers: headers.to_vec(),
            body: body.to_string(),
        })
    }
    pub fn start_post(&mut self, url: &str, body: &str, headers: &[(String, String)]) -> Poll<Result<HttpResponse, McpError>> {
        if body.len() > MAX_HTTP_BODY_BYTES {
            return Poll::Ready(Err(McpError::invalid_params("HTTP request body exceeds limit")));
        }
        let request = HttpRequest {
            method: "POST",
            url: url.into(),
            headers: headers.to_vec(),
            body: body.into(),
        };
        match &mut self.transport {
            Some(transport) => transport.start_request(&request).map(Self::check_response),
            None => Poll::Ready(self.request(&request)),
        }
    }
    pub fn poll_post(&mut self) -> Poll<Result<HttpResponse, McpError>> {
        match &mut self.transport {
            Some(transport) => transport.poll_request().map(Self::check_response),
            None => Poll::Ready(Err(McpError::internal_error("no pending HTTP transport"))),
        }
    }
    pub fn cancel_post(&mut self) {
        if let Some(transport) = &mut self.transport { transport.cancel_request(); }
    }
    fn check_response(response: Result<HttpResponse, String>) -> Result<HttpResponse, McpError> {
        let response = response.map_err(McpError::internal_error)?;
        if response.body.len() > MAX_HTTP_BODY_BYTES || !valid_headers(&response.headers) { return Err(McpError::internal_error("HTTP response body/headers exceed limit")); }
        if !(200..300).contains(&response.status) { return Err(McpError::internal_error(format!("HTTP request failed with status {}", response.status))); }
        Ok(response)
    }
}

/// OS account-network requests only. The dedicated kernel adapter supplies the
/// private marker; model/JS networking cannot select this route.
pub fn allowed_oauth_http(request:&HttpRequest)->bool {
    remote_mcp_authority(&request.url).is_some() && valid_headers(&request.headers)
        && request.body.len()<=64*1024
        && request.headers.iter().any(|(n,v)|n.eq_ignore_ascii_case("x-alloyos-oauth")&&v=="1")
        && request.headers.iter().all(|(n,_)|["content-type","accept","x-alloyos-oauth"].iter().any(|a|n.eq_ignore_ascii_case(a)))
        && match request.method {
            "GET"=>request.body.is_empty(),
            "POST"=>request.headers.iter().any(|(n,v)|n.eq_ignore_ascii_case("content-type")&&v=="application/x-www-form-urlencoded"),
            _=>false,
        }
}
