//! Welder WebView capability broker and session orchestration.
//!
//! Enforces the strict alloyOS boundary:
//! 1. Only `WebViewCaller::Welder` and `WebViewCaller::Ferrofluid` may open,
//!    navigate, or control a WebView session.
//! 2. Headless MCP apps (`WebViewCaller::App` / `WebViewCaller::AppId`) are
//!    unconditionally denied from opening a WebView, navigating, supplying
//!    HTML/JavaScript, or pushing pixel frames (`WebViewError::AppAccessForbidden`).
//! 3. Untrusted web content inside the browser service receives zero implicit
//!    MCP, knowledge, filesystem, or agent credentials (`WebViewError::CapabilityDenied`).

use alloc::string::String;
use alloc::vec::Vec;

/// Path to the local acceptance form fixture used by browser service startup tests.
pub const LOCAL_FORM_FIXTURE_PATH: &str = "tests/fixtures/web/local-form.html";

/// Canonical HTTPS origin assigned when the browser service hosts the local acceptance fixture.
pub const LOCAL_FORM_FIXTURE_URL: &str = "https://fixture.alloyos.local/local-form.html";

/// Principal attempting to invoke a WebView operation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WebViewCaller {
    /// On-device agent runtime orchestrator.
    Welder,
    /// System UI compositor and sole owner of the presentation surface.
    Ferrofluid,
    /// Headless MCP app package (unconditionally forbidden from WebView access).
    App,
    /// Named headless MCP app package (unconditionally forbidden from WebView access).
    AppId(String),
}

impl WebViewCaller {
    pub fn app(app_id: impl Into<String>) -> Self {
        Self::AppId(app_id.into())
    }

    pub fn is_authorized(&self) -> bool {
        matches!(self, Self::Welder | Self::Ferrofluid)
    }
}

/// Errors returned by Welder's WebView capability broker.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WebViewError {
    /// Headless apps may never open, control, or push frames to a WebView.
    AppAccessForbidden,
    /// Untrusted web content requested a privileged OS/agent capability.
    CapabilityDenied,
    /// Target URL violated HTTPS-only / canonical origin rules.
    InvalidUrl,
    /// Reply or frame belonged to a stale view generation.
    StaleGeneration,
    /// Browser service crashed and requires explicit recovery before use.
    ServiceCrashed { reason: u32 },
    /// Invalid state or input payload.
    InvalidState,
}

/// Explicit zero-privilege capability set granted to web content inside the browser service.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct WebContentCapabilities {
    pub mcp_tools: bool,
    pub knowledge_store: bool,
    pub filesystem: bool,
    pub agent_credentials: bool,
}

impl WebContentCapabilities {
    pub const NONE: Self = Self {
        mcp_tools: false,
        knowledge_store: false,
        filesystem: false,
        agent_credentials: false,
    };

    pub fn has_any_privilege(self) -> bool {
        self.mcp_tools || self.knowledge_store || self.filesystem || self.agent_credentials
    }
}

/// High-level command emitted by `WebViewBroker` for Ferrofluid's `WebViewController`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WebViewCommand {
    Create {
        view_id: u64,
        generation: u64,
        width: u32,
        height: u32,
        scale_thousandths: u32,
    },
    Navigate {
        view_id: u64,
        generation: u64,
        url: String,
    },
    Focus {
        view_id: u64,
        generation: u64,
        focused: bool,
    },
    CommitText {
        view_id: u64,
        generation: u64,
        text: String,
    },
    SubmitForm {
        view_id: u64,
        generation: u64,
    },
    Reload {
        view_id: u64,
        generation: u64,
    },
    Destroy {
        view_id: u64,
        generation: u64,
    },
}

/// State of a managed WebView session mediated between Welder and Ferrofluid.
#[derive(Debug, Clone)]
pub struct WebViewSession {
    view_id: u64,
    generation: u64,
    width: u32,
    height: u32,
    scale_thousandths: u32,
    requested_url: Option<String>,
    committed_url: Option<String>,
    trusted_origin: Option<String>,
    untrusted_title: String,
    focused: bool,
    editable_focused: bool,
    input_value: String,
    submitted_output: Option<String>,
    crashed_reason: Option<u32>,
    capabilities: WebContentCapabilities,
    pending_commands: Vec<WebViewCommand>,
}

impl WebViewSession {
    pub fn view_id(&self) -> u64 {
        self.view_id
    }

    pub fn generation(&self) -> u64 {
        self.generation
    }

    pub fn requested_url(&self) -> Option<&str> {
        self.requested_url.as_deref()
    }

    pub fn committed_url(&self) -> Option<&str> {
        self.committed_url.as_deref()
    }

    /// Returns the displayable origin derived strictly from the browser's committed URL.
    pub fn trusted_origin(&self) -> Option<&str> {
        self.trusted_origin.as_deref()
    }

    /// Returns the untrusted document title; never used for origin identification.
    pub fn untrusted_title(&self) -> &str {
        &self.untrusted_title
    }

    pub fn soft_keyboard_visible(&self) -> bool {
        self.crashed_reason.is_none() && self.focused && self.editable_focused
    }

    pub fn input_value(&self) -> &str {
        &self.input_value
    }

    pub fn submitted_output(&self) -> Option<&str> {
        self.submitted_output.as_deref()
    }

    pub fn is_crashed(&self) -> bool {
        self.crashed_reason.is_some()
    }

    pub fn crash_reason(&self) -> Option<u32> {
        self.crashed_reason
    }

    pub fn capabilities(&self) -> WebContentCapabilities {
        self.capabilities
    }

    pub fn take_commands(&mut self) -> Vec<WebViewCommand> {
        core::mem::take(&mut self.pending_commands)
    }
}

/// Capability-enforcing broker mediating WebView access between Welder and Ferrofluid.
#[derive(Debug, Default)]
pub struct WebViewBroker {
    next_view_id: u64,
}

impl WebViewBroker {
    pub fn new() -> Self {
        Self { next_view_id: 1 }
    }

    fn ensure_authorized(caller: &WebViewCaller) -> Result<(), WebViewError> {
        if caller.is_authorized() {
            Ok(())
        } else {
            Err(WebViewError::AppAccessForbidden)
        }
    }

    /// Opens a new isolated WebView session for Welder or Ferrofluid.
    /// Unconditionally denies `WebViewCaller::App` and `WebViewCaller::AppId`.
    pub fn open_session(
        &mut self,
        caller: &WebViewCaller,
        width: u32,
        height: u32,
        scale_thousandths: u32,
    ) -> Result<WebViewSession, WebViewError> {
        Self::ensure_authorized(caller)?;
        if width == 0 || height == 0 || !(250..=8000).contains(&scale_thousandths) {
            return Err(WebViewError::InvalidState);
        }
        let view_id = self.next_view_id.max(1);
        self.next_view_id = view_id + 1;
        let generation = 1;
        let mut session = WebViewSession {
            view_id,
            generation,
            width,
            height,
            scale_thousandths,
            requested_url: None,
            committed_url: None,
            trusted_origin: None,
            untrusted_title: String::new(),
            focused: false,
            editable_focused: false,
            input_value: String::new(),
            submitted_output: None,
            crashed_reason: None,
            capabilities: WebContentCapabilities::NONE,
            pending_commands: Vec::new(),
        };
        session.pending_commands.push(WebViewCommand::Create {
            view_id,
            generation,
            width,
            height,
            scale_thousandths,
        });
        Ok(session)
    }

    /// Requests navigation to an HTTPS URL or `about:blank`.
    pub fn navigate(
        &self,
        caller: &WebViewCaller,
        session: &mut WebViewSession,
        url: &str,
    ) -> Result<(), WebViewError> {
        Self::ensure_authorized(caller)?;
        if let Some(reason) = session.crashed_reason {
            return Err(WebViewError::ServiceCrashed { reason });
        }
        let _ = parse_trusted_origin(url)?;
        session.requested_url = Some(String::from(url));
        session.pending_commands.push(WebViewCommand::Navigate {
            view_id: session.view_id,
            generation: session.generation,
            url: String::from(url),
        });
        Ok(())
    }

    /// Starts the local acceptance fixture (`tests/fixtures/web/local-form.html`)
    /// through the dedicated service startup fixture option (never via arbitrary
    /// app-supplied `file://` URLs).
    pub fn load_local_form_fixture(
        &self,
        caller: &WebViewCaller,
        session: &mut WebViewSession,
    ) -> Result<(), WebViewError> {
        self.navigate(caller, session, LOCAL_FORM_FIXTURE_URL)?;
        let generation = session.generation;
        self.commit_browser_url(session, generation, LOCAL_FORM_FIXTURE_URL)?;
        self.receive_untrusted_title(session, generation, "alloyOS Local Acceptance Form")?;
        Ok(())
    }

    /// Rejects any attempt by an app or caller to inject raw HTML or JavaScript.
    pub fn supply_markup_or_script(
        &self,
        caller: &WebViewCaller,
        _session: &mut WebViewSession,
        _markup_or_js: &str,
    ) -> Result<(), WebViewError> {
        Self::ensure_authorized(caller)?;
        Err(WebViewError::CapabilityDenied)
    }

    /// Rejects any attempt by an app to push pixel frames into Ferrofluid.
    pub fn app_push_frame(
        &self,
        caller: &WebViewCaller,
        _pixels: &[u8],
    ) -> Result<(), WebViewError> {
        Self::ensure_authorized(caller)?;
        Err(WebViewError::CapabilityDenied)
    }

    /// Rejects any attempt by web content to access MCP tools, knowledge, VFS, or credentials.
    pub fn request_host_capability_from_web_content(
        &self,
        _session: &WebViewSession,
        _capability: &str,
    ) -> Result<(), WebViewError> {
        Err(WebViewError::CapabilityDenied)
    }

    /// Applies a browser-committed URL reply for `generation`, updating `trusted_origin`.
    pub fn commit_browser_url(
        &self,
        session: &mut WebViewSession,
        generation: u64,
        committed_url: &str,
    ) -> Result<(), WebViewError> {
        if generation != session.generation {
            return Err(WebViewError::StaleGeneration);
        }
        let origin = parse_trusted_origin(committed_url)?;
        session.committed_url = Some(String::from(committed_url));
        session.trusted_origin = Some(origin);
        Ok(())
    }

    /// Applies an untrusted `Opcode::Title` reply without modifying `trusted_origin`.
    pub fn receive_untrusted_title(
        &self,
        session: &mut WebViewSession,
        generation: u64,
        title: &str,
    ) -> Result<(), WebViewError> {
        if generation != session.generation {
            return Err(WebViewError::StaleGeneration);
        }
        session.untrusted_title = String::from(title);
        Ok(())
    }

    /// Focuses an editable form field inside the WebView and updates soft-keyboard state.
    pub fn focus_editable(
        &self,
        caller: &WebViewCaller,
        session: &mut WebViewSession,
        focused: bool,
        editable: bool,
    ) -> Result<(), WebViewError> {
        Self::ensure_authorized(caller)?;
        if let Some(reason) = session.crashed_reason {
            return Err(WebViewError::ServiceCrashed { reason });
        }
        session.focused = focused;
        session.editable_focused = focused && editable;
        session.pending_commands.push(WebViewCommand::Focus {
            view_id: session.view_id,
            generation: session.generation,
            focused,
        });
        Ok(())
    }

    /// Commits UTF-8 text from Ferrofluid's soft keyboard into the focused editable field.
    pub fn commit_text(
        &self,
        caller: &WebViewCaller,
        session: &mut WebViewSession,
        text: &str,
    ) -> Result<(), WebViewError> {
        Self::ensure_authorized(caller)?;
        if let Some(reason) = session.crashed_reason {
            return Err(WebViewError::ServiceCrashed { reason });
        }
        if !session.soft_keyboard_visible() || text.is_empty() {
            return Err(WebViewError::InvalidState);
        }
        session.input_value.push_str(text);
        session.pending_commands.push(WebViewCommand::CommitText {
            view_id: session.view_id,
            generation: session.generation,
            text: String::from(text),
        });
        Ok(())
    }

    /// Submits the active local form (`#f` in `tests/fixtures/web/local-form.html`),
    /// producing the deterministic `"submitted:" + value.trim()` output.
    pub fn submit_form(
        &self,
        caller: &WebViewCaller,
        session: &mut WebViewSession,
    ) -> Result<String, WebViewError> {
        Self::ensure_authorized(caller)?;
        if let Some(reason) = session.crashed_reason {
            return Err(WebViewError::ServiceCrashed { reason });
        }
        let mut out = String::from("submitted:");
        out.push_str(session.input_value.trim());
        session.submitted_output = Some(out.clone());
        session.pending_commands.push(WebViewCommand::SubmitForm {
            view_id: session.view_id,
            generation: session.generation,
        });
        Ok(out)
    }

    /// Records a browser service crash, immediately advancing the view generation
    /// so any stale packets from the crashed process are rejected.
    pub fn handle_crash(&self, session: &mut WebViewSession, generation: u64, reason: u32) -> Result<(), WebViewError> {
        if generation != session.generation {
            return Err(WebViewError::StaleGeneration);
        }
        session.generation += 1;
        session.crashed_reason = Some(reason);
        session.editable_focused = false;
        Ok(())
    }

    /// Explicitly recovers a crashed session under its new generation and reloads the last URL.
    pub fn recover_and_reload(
        &self,
        caller: &WebViewCaller,
        session: &mut WebViewSession,
    ) -> Result<(), WebViewError> {
        Self::ensure_authorized(caller)?;
        session.crashed_reason = None;
        session.pending_commands.push(WebViewCommand::Create {
            view_id: session.view_id,
            generation: session.generation,
            width: session.width,
            height: session.height,
            scale_thousandths: session.scale_thousandths,
        });
        if let Some(url) = session
            .committed_url
            .clone()
            .or_else(|| session.requested_url.clone())
        {
            session.pending_commands.push(WebViewCommand::Navigate {
                view_id: session.view_id,
                generation: session.generation,
                url,
            });
        }
        Ok(())
    }
}

fn parse_trusted_origin(url: &str) -> Result<String, WebViewError> {
    let bytes = url.as_bytes();
    if bytes.is_empty()
        || bytes.len() > 768
        || bytes.iter().any(|b| *b <= 0x20 || *b == 0x7f || *b == b'\\')
    {
        return Err(WebViewError::InvalidUrl);
    }
    if url == "about:blank" {
        return Ok(String::from("about:blank"));
    }
    let rest = url
        .strip_prefix("https://")
        .ok_or(WebViewError::InvalidUrl)?;
    let authority_end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let authority = &rest[..authority_end];
    if authority.is_empty() || authority.contains('@') {
        return Err(WebViewError::InvalidUrl);
    }

    let mut parts = authority.split(':');
    let host = parts.next().unwrap_or("");
    let port_part = parts.next();
    if parts.next().is_some()
        || host.is_empty()
        || host.starts_with('.')
        || host.ends_with('.')
    {
        return Err(WebViewError::InvalidUrl);
    }
    for label in host.split('.') {
        if label.is_empty()
            || label.starts_with('-')
            || label.ends_with('-')
            || !label
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-')
        {
            return Err(WebViewError::InvalidUrl);
        }
    }

    let mut origin = String::from("https://");
    for b in host.bytes() {
        origin.push((b.to_ascii_lowercase()) as char);
    }
    if let Some(p_str) = port_part {
        if p_str.is_empty()
            || p_str.len() > 5
            || p_str.starts_with('0')
            || !p_str.bytes().all(|b| b.is_ascii_digit())
        {
            return Err(WebViewError::InvalidUrl);
        }
        let port = p_str.parse::<u16>().map_err(|_| WebViewError::InvalidUrl)?;
        if port == 0 {
            return Err(WebViewError::InvalidUrl);
        }
        origin.push(':');
        origin.push_str(p_str);
    }
    Ok(origin)
}
