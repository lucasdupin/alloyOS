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
                let rpc = McpValue::from_json(&request.body)
                    .map_err(|_| McpError::invalid_params("invalid RPC request"))?;
                let method = rpc
                    .as_map()
                    .and_then(|map| map.get("method"))
                    .and_then(McpValue::as_str)
                    .unwrap_or("");
                format!("POST {} {}", request.url, method)
            } else {
                request.url.clone()
            };
            let body = routes.get(&key).cloned().ok_or_else(|| {
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
    fn check_response(response: Result<HttpResponse, String>) -> Result<HttpResponse, McpError> {
        let response = response.map_err(McpError::internal_error)?;
        if response.body.len() > MAX_HTTP_BODY_BYTES { return Err(McpError::internal_error("HTTP response body exceeds limit")); }
        if !(200..300).contains(&response.status) { return Err(McpError::internal_error(format!("HTTP request failed with status {}", response.status))); }
        Ok(response)
    }
}
