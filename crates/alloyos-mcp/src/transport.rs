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

#[derive(serde::Serialize)]
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

/// Hosts own network I/O and credentials. They must bound requests and avoid
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
        if request.body.len() > MAX_HTTP_BODY_BYTES {
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
        if response.body.len() > MAX_HTTP_BODY_BYTES {
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
        if response.body.len() > MAX_HTTP_BODY_BYTES { return Err(McpError::internal_error("HTTP response body exceeds limit")); }
        if !(200..300).contains(&response.status) { return Err(McpError::internal_error(format!("HTTP request failed with status {}", response.status))); }
        Ok(response)
    }
}
