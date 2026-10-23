//! WebKit page container uniting DOM, JavaScriptCore runtime, and software rendering.

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use super::dom::{parse_svg, HtmlDocument, SvgGraphic};
use super::js_runtime::PageJsRuntime;
use super::renderer::{HitTarget, PageRenderer};
use super::PointerPhase;

const DEFAULT_LOCAL_FORM_HTML: &str =
    include_str!("../../../../tests/fixtures/web/local-form.html");
const DEFAULT_MARK_SVG: &str =
    include_str!("../../../../tests/fixtures/web/mark.svg");

/// Result of a user pointer interaction on the page.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PageInteraction {
    None,
    InputFocused(String),
    FormSubmitted(String),
}

/// A live WebKit page instance.
pub struct WebPage {
    pub url: String,
    pub doc: HtmlDocument,
    pub js: PageJsRuntime,
    pub renderer: PageRenderer,
    pub svg_assets: Vec<(String, SvgGraphic)>,
    pub focused_input_id: Option<String>,
    pub scroll_y: i32,
}

/// Fetches the raw HTML markup of a remote web page over the network.
#[cfg(any(test, not(target_os = "none")))]
pub fn fetch_page_html(url: &str) -> Result<String, String> {
    use std::process::Command;

    if !url.starts_with("http://") && !url.starts_with("https://") {
        return Err(format!("Unsupported URL scheme in '{}'", url));
    }

    let output = Command::new("curl")
        .args([
            "-s",
            "-L",
            "--compressed",
            "--max-time",
            "10",
            "-A",
            "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko)",
            url,
        ])
        .output()
        .map_err(|e| format!("Failed to execute network fetch: {}", e))?;

    if !output.status.success() {
        return Err(format!("Network fetch exited with status {}", output.status));
    }

    let html = String::from_utf8(output.stdout)
        .or_else(|e| String::from_utf8(e.into_bytes()))
        .map_err(|e| format!("Response contains invalid UTF-8: {}", e))?;

    if html.trim().is_empty() {
        return Err(String::from("Server returned empty response"));
    }

    Ok(html)
}

#[cfg(target_os = "none")]
pub fn fetch_page_html(url: &str) -> Result<String, String> {
    #[cfg(feature = "executable")]
    {
        use alloc::format;
        let rest = url
            .strip_prefix("https://")
            .or_else(|| url.strip_prefix("http://"))
            .ok_or_else(|| String::from("Unsupported scheme"))?;
        let (host_port, path) = match rest.find('/') {
            Some(idx) => (&rest[..idx], &rest[idx..]),
            None => (rest, "/"),
        };
        let host = host_port.split(':').next().unwrap_or(host_port);

        let addr = alloyos_proto::SocketAddrV4::new([10, 0, 2, 2], 80);
        let fd = liballoy::net::tcp_socket()
            .map_err(|e| format!("Failed to create socket: {e}"))?;

        if liballoy::net::tcp_connect(fd, &addr).is_err() {
            let _ = unsafe { liballoy::syscall::syscall1(alloyos_proto::SyscallNo::Close as u64, fd as u64) };
            return Err(format!("Network connection to {} failed", host));
        }

        let req = format!(
            "GET {} HTTP/1.1\r\nHost: {}\r\nUser-Agent: Mozilla/5.0 alloyOS-WebKit/1.0\r\nConnection: close\r\n\r\n",
            path, host
        );
        let _ = liballoy::net::tcp_send(fd, req.as_bytes());

        let mut buf = alloc::vec![0u8; 4096];
        let mut response = alloc::vec::Vec::new();
        while let Ok(n) = liballoy::net::tcp_recv(fd, &mut buf) {
            if n == 0 {
                break;
            }
            response.extend_from_slice(&buf[..n]);
            if response.len() > 1024 * 1024 {
                break;
            }
        }
        let _ = unsafe { liballoy::syscall::syscall1(alloyos_proto::SyscallNo::Close as u64, fd as u64) };

        if response.is_empty() {
            return Err(format!("No data received from {}", host));
        }

        let body = if let Some(pos) = response.windows(4).position(|w| w == b"\r\n\r\n") {
            &response[pos + 4..]
        } else {
            &response[..]
        };

        String::from_utf8(body.to_vec())
            .map_err(|_| String::from("Invalid UTF-8 in response"))
    }
    #[cfg(not(feature = "executable"))]
    {
        Err(String::from("Network stack unavailable in this build"))
    }
}

impl WebPage {
    /// Loads a page by URL.
    pub fn load_url(url: &str) -> Self {
        let is_fixture = url.contains("local-form.html")
            || url.ends_with("local-form")
            || url.is_empty();

        let html = if is_fixture {
            String::from(DEFAULT_LOCAL_FORM_HTML)
        } else {
            match fetch_page_html(url) {
                Ok(content) => content,
                Err(err) => {
                    let host = url
                        .strip_prefix("https://")
                        .or_else(|| url.strip_prefix("http://"))
                        .unwrap_or(url)
                        .split('/')
                        .next()
                        .unwrap_or(url);

                    format!(
                        r#"<!doctype html>
<html>
<head>
<title>Unable to connect</title>
<style>
  body {{ font-family: sans-serif; margin: 24px; background: #101418; color: #fff; }}
  h1 {{ color: #ff6b6b; }}
  p {{ color: #a5adb7; }}
</style>
</head>
<body>
  <h1>Unable to connect</h1>
  <p>alloyOS WebKit can't establish a network connection to {host}.</p>
  <p role="status">Error: {err}</p>
</body>
</html>"#,
                        host = host,
                        err = err
                    )
                }
            }
        };

        Self::load_html(&html, url)
    }

    /// Loads and initializes a page from HTML source.
    pub fn load_html(html: &str, url: &str) -> Self {
        let mut doc = HtmlDocument::parse(html);
        let mut js = PageJsRuntime::new();
        let mut svg_assets = Vec::new();

        // Default mark.svg asset
        let mark_svg = parse_svg(DEFAULT_MARK_SVG);
        svg_assets.push((String::from("mark.svg"), mark_svg));

        // Scan doc for elements with IDs to populate the JS DOM proxy
        fn register_ids(nodes: &[super::dom::DomNode], js: &mut PageJsRuntime) {
            for node in nodes {
                if let super::dom::DomNode::Element(el) = node {
                    if let Some(id) = &el.id {
                        js.register_element(id, &el.tag, &el.value, &el.text_content);
                    }
                    register_ids(&el.children, js);
                }
            }
        }
        register_ids(&doc.root_children, &mut js);

        // Add page scripts to JS runtime
        for script in &doc.inline_scripts {
            js.add_script(script);
        }

        // Execute scripts using JavaScriptCore
        let _ = js.execute_page_scripts();

        // Sync back dataset and element properties
        for (k, v) in js.dataset() {
            doc.set_dataset_attr(k, v);
        }

        fn sync_back(nodes: &mut [super::dom::DomNode], js: &PageJsRuntime) {
            for node in nodes {
                if let super::dom::DomNode::Element(el) = node {
                    if let Some(id) = &el.id {
                        if let Some(val) = js.get_element_value(id) {
                            el.value = String::from(val);
                        }
                        if let Some(txt) = js.get_element_text(id) {
                            if !txt.is_empty() {
                                el.text_content = String::from(txt);
                            }
                        }
                    }
                    sync_back(&mut el.children, js);
                }
            }
        }
        sync_back(&mut doc.root_children, &js);

        Self {
            url: String::from(url),
            doc,
            js,
            renderer: PageRenderer::new(),
            svg_assets,
            focused_input_id: None,
            scroll_y: 0,
        }
    }

    /// Renders the page into BGRA8 pixels.
    pub fn render(&mut self, width: u32, height: u32) -> Vec<u8> {
        self.renderer.render(
            &self.doc,
            width,
            height,
            self.scroll_y,
            self.focused_input_id.as_deref(),
            &self.svg_assets,
        )
    }

    /// Handles pointer/touch interaction (phase, x, y).
    pub fn interact_pointer(&mut self, phase: PointerPhase, x: i32, y: i32) -> PageInteraction {
        if phase != PointerPhase::Down && phase != PointerPhase::Up {
            return PageInteraction::None;
        }

        if let Some(target) = self.renderer.hit_test(x, y) {
            match target {
                HitTarget::Input(id) => {
                    self.focused_input_id = Some(id.clone());
                    PageInteraction::InputFocused(id)
                }
                HitTarget::Button(id) => {
                    let _ = self.submit_active_form();
                    self.focused_input_id = None;
                    PageInteraction::FormSubmitted(id)
                }
            }
        } else {
            PageInteraction::None
        }
    }

    /// Commits text into the currently focused editable element.
    pub fn interact_commit_text(&mut self, text: &str) {
        if let Some(id) = &self.focused_input_id {
            let current = self.js.get_element_value(id).unwrap_or("");
            let updated = format!("{}{}", current, text);
            self.js.set_element_value(id, &updated);
            if let Some(el) = self.doc.find_element_by_id_mut(id) {
                el.value = updated;
            }
        }
    }

    /// Submits the active form and invokes registered JavaScript submit listeners.
    pub fn submit_active_form(&mut self) -> Result<String, String> {
        let form_id = self.find_first_form_id().unwrap_or_else(|| String::from("form"));

        self.js.dispatch_event(&form_id, "submit")?;

        // Sync updated DOM elements from JS back to HtmlDocument
        fn sync_back(nodes: &mut [super::dom::DomNode], js: &PageJsRuntime) {
            for node in nodes {
                if let super::dom::DomNode::Element(el) = node {
                    if let Some(id) = &el.id {
                        if let Some(val) = js.get_element_value(id) {
                            el.value = String::from(val);
                        }
                        if let Some(txt) = js.get_element_text(id) {
                            el.text_content = String::from(txt);
                        }
                    }
                    sync_back(&mut el.children, js);
                }
            }
        }
        sync_back(&mut self.doc.root_children, &self.js);

        if let Some(res) = self.js.get_element_text("result") {
            return Ok(String::from(res));
        }

        Ok(String::new())
    }

    /// Finds the ID of the first form in the document.
    pub fn find_first_form_id(&self) -> Option<String> {
        fn search(nodes: &[super::dom::DomNode]) -> Option<String> {
            for node in nodes {
                if let super::dom::DomNode::Element(el) = node {
                    if el.tag == "form" {
                        return Some(el.id.clone().unwrap_or_else(|| String::from("form")));
                    }
                    if let Some(id) = search(&el.children) {
                        return Some(id);
                    }
                }
            }
            None
        }
        search(&self.doc.root_children)
    }

    /// Adjusts viewport scroll offset.
    pub fn interact_scroll(&mut self, delta_y: i32) {
        self.scroll_y = (self.scroll_y + delta_y).max(0);
    }

    /// Evaluates arbitrary JavaScript in the page context.
    pub fn eval_js(&mut self, code: &str) -> Result<String, String> {
        self.js.eval(code)
    }

    pub fn title(&self) -> &str {
        if !self.doc.title.is_empty() {
            &self.doc.title
        } else {
            &self.url
        }
    }

    pub fn dataset_attr(&self, key: &str) -> Option<&str> {
        self.doc.dataset_attr(key)
    }

    pub fn element_text(&self, id: &str) -> Option<&str> {
        self.doc
            .find_element_by_id(id)
            .map(|el| el.text_content.as_str())
    }

    pub fn element_value(&self, id: &str) -> Option<&str> {
        self.doc.find_element_by_id(id).map(|el| el.value.as_str())
    }
}
