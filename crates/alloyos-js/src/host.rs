use alloc::collections::BTreeMap;
use alloc::{boxed::Box,rc::Rc};
use core::cell::RefCell;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use alloyos_proto::{InputEvent, InputEventKind};

use crate::value::JsValue;

/// Operating system events emitted by kernel/subsystems and dispatched by `scriptd`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OsEvent {
    /// A new file was created in CobaltFS/VFS.
    FsCreated { path: String },
    /// An existing file was modified in CobaltFS/VFS.
    FsModified { path: String },
    /// A file was deleted from CobaltFS/VFS.
    FsDeleted { path: String },
    /// The network interface came online.
    NetInterfaceUp,
    /// A network packet/payload was received on a local port.
    NetPacketReceived { port: u16, payload: String },
    /// The system completed boot initialization.
    SystemBoot,
    /// Free physical memory dropped below threshold.
    LowMemory { free_bytes: u64 },
    /// No user input activity detected for the idle threshold.
    UserIdle,
}

impl OsEvent {
    /// Converts this `OsEvent` into a JavaScript `EVENT` object for script execution.
    pub fn to_js_object(&self) -> JsValue {
        let mut map = BTreeMap::new();
        match self {
            OsEvent::FsCreated { path } => {
                map.insert("type".to_string(), JsValue::String("FsCreated".to_string()));
                map.insert("path".to_string(), JsValue::String(path.clone()));
            }
            OsEvent::FsModified { path } => {
                map.insert(
                    "type".to_string(),
                    JsValue::String("FsModified".to_string()),
                );
                map.insert("path".to_string(), JsValue::String(path.clone()));
            }
            OsEvent::FsDeleted { path } => {
                map.insert("type".to_string(), JsValue::String("FsDeleted".to_string()));
                map.insert("path".to_string(), JsValue::String(path.clone()));
            }
            OsEvent::NetInterfaceUp => {
                map.insert(
                    "type".to_string(),
                    JsValue::String("NetInterfaceUp".to_string()),
                );
            }
            OsEvent::NetPacketReceived { port, payload } => {
                map.insert(
                    "type".to_string(),
                    JsValue::String("NetPacketReceived".to_string()),
                );
                map.insert("port".to_string(), JsValue::Number(*port as i64));
                map.insert("payload".to_string(), JsValue::String(payload.clone()));
            }
            OsEvent::SystemBoot => {
                map.insert(
                    "type".to_string(),
                    JsValue::String("SystemBoot".to_string()),
                );
            }
            OsEvent::LowMemory { free_bytes } => {
                map.insert("type".to_string(), JsValue::String("LowMemory".to_string()));
                map.insert(
                    "freeBytes".to_string(),
                    JsValue::Number(*free_bytes as i64),
                );
            }
            OsEvent::UserIdle => {
                map.insert("type".to_string(), JsValue::String("UserIdle".to_string()));
            }
        }
        JsValue::Object(map)
    }

    /// Returns `true` if the given hardware `InputEvent` represents active user interaction
    /// that resets the `OsEvent::UserIdle` timer.
    pub const fn is_user_activity_input(ev: &InputEvent) -> bool {
        matches!(
            ev.kind,
            InputEventKind::KeyDown
                | InputEventKind::KeyUp
                | InputEventKind::PointerMove
                | InputEventKind::PointerButtonDown
                | InputEventKind::PointerButtonUp
        )
    }

    pub fn file_created(path: impl Into<String>) -> Self {
        Self::FsCreated { path: path.into() }
    }

    pub fn file_modified(path: impl Into<String>) -> Self {
        Self::FsModified { path: path.into() }
    }

    pub fn file_deleted(path: impl Into<String>) -> Self {
        Self::FsDeleted { path: path.into() }
    }

    pub const fn network_online() -> Self {
        Self::NetInterfaceUp
    }

    pub fn network_request(port: u16, payload: impl Into<String>) -> Self {
        Self::NetPacketReceived {
            port,
            payload: payload.into(),
        }
    }

    pub const fn system_boot() -> Self {
        Self::SystemBoot
    }
}

/// Snapshot of system resource metrics returned by `AlloyOS.sys.status()`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SystemStatus {
    pub uptime_ms: u64,
    /// UTC Unix epoch milliseconds, or None when no wall-clock source is available.
    pub wall_clock_ms: Option<u64>,
    pub mem_used_bytes: u64,
    pub mem_free_bytes: u64,
    pub active_tasks: u32,
}

impl Default for SystemStatus {
    fn default() -> Self {
        Self {
            uptime_ms: 1_000,
            wall_clock_ms: None,
            mem_used_bytes: 16 * 1024 * 1024,
            mem_free_bytes: 112 * 1024 * 1024,
            active_tasks: 4,
        }
    }
}

/// Fine-grained capability request checked before invoking `AlloyOS.*` host bindings.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CapabilityRequest<'a> {
    FsRead(&'a str),
    FsWrite(&'a str),
    NetFetch(&'a str),
    SysStatus,
    UiNotify,
    ForgeGenerate,
    /// Access to `AlloyOS.scheduler.*` (`ScriptScheduler`), granted through declared capabilities.
    SchedulerAccess,
}

/// Declarative capability grant for a JavaScript isolate or scheduled script.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ScriptCapability {
    FsRead { prefix: String },
    FsWrite { prefix: String },
    NetFetch { domain: String },
    SysStatus,
    UiNotify,
    ForgeGenerate,
    /// Capability allowing a manifest-authorized application to access `AlloyOS.scheduler.*`.
    SchedulerAccess,
}

/// Capability policy enforced on `AlloyOS.*` host bindings.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScriptCapabilities {
    pub fs_read_prefixes: Vec<String>,
    pub fs_write_prefixes: Vec<String>,
    pub net_domains: Vec<String>,
    pub allow_sys_status: bool,
    pub allow_ui_notify: bool,
    pub allow_forge: bool,
    pub allow_scheduler: bool,
}

impl Default for ScriptCapabilities {
    fn default() -> Self {
        Self::default_automation()
    }
}

impl ScriptCapabilities {
    /// Standard capability set for model-authored local automations (`ForgeGenerate` and `SchedulerAccess` are opt-in).
    pub fn default_automation() -> Self {
        Self {
            fs_read_prefixes: alloc::vec!["/".to_string()],
            fs_write_prefixes: alloc::vec!["/".to_string()],
            net_domains: alloc::vec!["*".to_string()],
            allow_sys_status: true,
            allow_ui_notify: true,
            allow_forge: false,
            allow_scheduler: false,
        }
    }

    /// Empty capability set denying all host operations until explicitly granted.
    pub fn none() -> Self {
        Self {
            fs_read_prefixes: Vec::new(),
            fs_write_prefixes: Vec::new(),
            net_domains: Vec::new(),
            allow_sys_status: false,
            allow_ui_notify: false,
            allow_forge: false,
            allow_scheduler: false,
        }
    }

    /// Allow-list capability policy for a headless MCP app (`app_id`).
    ///
    /// Write access is strictly allow-listed to `/var/<app_id>` . Read access is scoped to
    /// `/var/<app_id>` and `/apps/<app_id>`.
    pub fn for_mcp_app(app_id: &str) -> Self {
        let clean = app_id.trim().trim_matches('/');
        let var_root = alloc::format!("/var/{}", clean);
        let app_root = alloc::format!("/apps/{}", clean);
        let fs_read_prefixes = alloc::vec![var_root.clone(), app_root, "/proc/app-packages.json".to_string()];
        let fs_write_prefixes = alloc::vec![var_root];
        let allow_scheduler = false;
        let allow_sys_status = true;

        Self {
            fs_read_prefixes,
            fs_write_prefixes,
            net_domains: alloc::vec!["*".to_string()],
            allow_sys_status,
            allow_ui_notify: false,
            allow_forge: false,
            allow_scheduler,
        }
    }

    /// Constructs a `ScriptCapabilities` policy from a slice of `ScriptCapability` grants.
    pub fn from_list(caps: &[ScriptCapability]) -> Self {
        let mut out = Self::none();
        for cap in caps {
            match cap {
                ScriptCapability::FsRead { prefix } => {
                    out.fs_read_prefixes.push(Self::normalize_path(prefix));
                }
                ScriptCapability::FsWrite { prefix } => {
                    out.fs_write_prefixes.push(Self::normalize_path(prefix));
                }
                ScriptCapability::NetFetch { domain } => {
                    out.net_domains.push(domain.trim().to_ascii_lowercase());
                }
                ScriptCapability::SysStatus => out.allow_sys_status = true,
                ScriptCapability::UiNotify => out.allow_ui_notify = true,
                ScriptCapability::ForgeGenerate => out.allow_forge = true,
                ScriptCapability::SchedulerAccess => out.allow_scheduler = true,
            }
        }
        out
    }

    pub fn with_fs_read(mut self, prefix: impl Into<String>) -> Self {
        self.fs_read_prefixes
            .push(Self::normalize_path(&prefix.into()));
        self
    }

    pub fn with_fs_write(mut self, prefix: impl Into<String>) -> Self {
        self.fs_write_prefixes
            .push(Self::normalize_path(&prefix.into()));
        self
    }

    pub fn with_net_domain(mut self, domain: impl Into<String>) -> Self {
        self.net_domains
            .push(domain.into().trim().to_ascii_lowercase());
        self
    }

    pub fn with_sys_status(mut self, allow: bool) -> Self {
        self.allow_sys_status = allow;
        self
    }

    pub fn with_ui_notify(mut self, allow: bool) -> Self {
        self.allow_ui_notify = allow;
        self
    }

    pub fn with_forge(mut self, allow: bool) -> Self {
        self.allow_forge = allow;
        self
    }

    pub fn with_scheduler(mut self, allow: bool) -> Self {
        self.allow_scheduler = allow;
        self
    }

    /// Canonicalizes a path (resolving `.` and `..`) so path traversal cannot escape a prefix.
    pub fn normalize_path(path: &str) -> String {
        let trimmed = path.trim();
        if trimmed.is_empty() || trimmed == "/" {
            return "/".to_string();
        }
        let mut parts: Vec<&str> = Vec::new();
        for seg in trimmed.split('/') {
            match seg {
                "" | "." => {}
                ".." => {
                    parts.pop();
                }
                other => parts.push(other),
            }
        }
        if parts.is_empty() {
            "/".to_string()
        } else {
            let mut out = String::new();
            for p in parts {
                out.push('/');
                out.push_str(p);
            }
            out
        }
    }

    fn path_matches_prefix(prefix: &str, target_path: &str) -> bool {
        let norm_prefix = Self::normalize_path(prefix);
        let norm_target = Self::normalize_path(target_path);
        if norm_prefix == "/" {
            return norm_target.starts_with('/');
        }
        if norm_target == norm_prefix {
            return true;
        }
        let mut with_slash = norm_prefix;
        with_slash.push('/');
        norm_target.starts_with(&with_slash)
    }

    fn extract_url_domain(url: &str) -> Option<String> {
        let trimmed = url.trim();
        let after_scheme = if let Some(rest) = trimmed.strip_prefix("https://") {
            rest
        } else if let Some(rest) = trimmed.strip_prefix("http://") {
            rest
        } else {
            trimmed
        };
        let authority = after_scheme
            .split(&['/', '?', '#'][..])
            .next()
            .unwrap_or("");
        let host = authority.split(':').next().unwrap_or("").trim();
        if host.is_empty() {
            None
        } else {
            Some(host.to_ascii_lowercase())
        }
    }

    fn domain_matches(allowed: &str, host: &str) -> bool {
        let allowed = allowed.trim().to_ascii_lowercase();
        if allowed == "*" {
            return !host.is_empty();
        }
        if let Some(suffix) = allowed.strip_prefix("*.") {
            return host == suffix || host.ends_with(&alloc::format!(".{}", suffix));
        }
        host == allowed
    }

    /// Validates whether `req` is permitted by this `ScriptCapabilities` policy.
    pub fn check(&self, req: &CapabilityRequest<'_>) -> Result<(), String> {
        match req {
            CapabilityRequest::FsRead(path) => {
                if path.contains("..") {
                    // Reject raw parent-traversal attempts if they escape configured prefixes
                    let norm = Self::normalize_path(path);
                    if !self
                        .fs_read_prefixes
                        .iter()
                        .any(|p| Self::path_matches_prefix(p, &norm))
                    {
                        return Err(alloc::format!(
                            "CapabilityDenied: FsRead not permitted for path '{}'",
                            path
                        ));
                    }
                }
                if self
                    .fs_read_prefixes
                    .iter()
                    .any(|p| Self::path_matches_prefix(p, path))
                {
                    Ok(())
                } else {
                    Err(alloc::format!(
                        "CapabilityDenied: FsRead not permitted for path '{}'",
                        path
                    ))
                }
            }
            CapabilityRequest::FsWrite(path) => {
                if self
                    .fs_write_prefixes
                    .iter()
                    .any(|p| Self::path_matches_prefix(p, path))
                {
                    Ok(())
                } else {
                    Err(alloc::format!(
                        "CapabilityDenied: FsWrite not permitted for path '{}'",
                        path
                    ))
                }
            }
            CapabilityRequest::NetFetch(url) => {
                let Some(host) = Self::extract_url_domain(url) else {
                    return Err(alloc::format!(
                        "CapabilityDenied: Invalid URL '{}' for NetFetch",
                        url
                    ));
                };
                if self
                    .net_domains
                    .iter()
                    .any(|d| Self::domain_matches(d, &host))
                {
                    Ok(())
                } else {
                    Err(alloc::format!(
                        "CapabilityDenied: NetFetch not permitted for domain '{}'",
                        host
                    ))
                }
            }
            CapabilityRequest::SysStatus => {
                if self.allow_sys_status {
                    Ok(())
                } else {
                    Err("CapabilityDenied: SysStatus capability not granted".to_string())
                }
            }
            CapabilityRequest::UiNotify => {
                if self.allow_ui_notify {
                    Ok(())
                } else {
                    Err("CapabilityDenied: UiNotify capability not granted".to_string())
                }
            }
            CapabilityRequest::ForgeGenerate => {
                if self.allow_forge {
                    Ok(())
                } else {
                    Err("CapabilityDenied: ForgeGenerate capability not granted".to_string())
                }
            }
            CapabilityRequest::SchedulerAccess => {
                if self.allow_scheduler {
                    Ok(())
                } else {
                    Err(
                        "CapabilityDenied: SchedulerAccess capability is restricted exclusively to the 'tasks' app"
                            .to_string(),
                    )
                }
            }
        }
    }
}

/// Host capability trait exposed to JavaScript isolates under the `AlloyOS` namespace.
///
/// Notice that shell execution capabilities (`sh`, `exec`, `spawn`) are intentionally
/// absent by design in alloyOS.
pub trait AlloyOsHost {
    /// Checks whether the current isolate/script is authorized to perform `req`.
    fn check_capability(&self, _req: &CapabilityRequest<'_>) -> Result<(), String> {
        Ok(())
    }

    /// Local inference capability; hosts must opt in explicitly.
    fn forge(&mut self, _prompt: &str, _model: Option<&str>, _max_tokens: u32) -> Result<String, String> {
        Err("Forge capability unavailable".to_string())
    }

    /// Reads a UTF-8 file from CobaltFS/VFS (`AlloyOS.fs.read(path)`).
    fn fs_read(&mut self, path: &str) -> Option<String>;

    /// Writes a UTF-8 file to CobaltFS/VFS (`AlloyOS.fs.write(path, content)`).
    fn fs_write(&mut self, path: &str, content: &str) -> Result<(), String>;

    /// Deletes a file from CobaltFS/VFS (`AlloyOS.fs.delete(path)`).
    fn fs_delete(&mut self, path: &str) -> Result<bool, String>;

    /// Lists file names or paths under `dir_path` (`AlloyOS.fs.list(path)`).
    fn fs_list(&mut self, dir_path: &str) -> Vec<String>;

    /// Performs an HTTP GET request via the network stack (`AlloyOS.net.fetch(url)`).
    fn net_fetch(&mut self, url: &str) -> Result<String, String>;

    /// Queries system uptime and memory statistics (`AlloyOS.sys.status()`).
    fn sys_status(&self) -> SystemStatus;

    /// Queues a notification to the Ferrofluid UI (`AlloyOS.ui.notify(message)`).
    fn ui_notify(&mut self, message: &str);

    /// Appends a message to the script execution log (`AlloyOS.log(message)`).
    fn log(&mut self, message: &str);

    /// Drains any `OsEvent`s generated by host mutations (such as `FsCreated` / `FsModified`).
    fn drain_emitted_events(&mut self) -> Vec<OsEvent>;

    /// Lists all scheduled tasks (`AlloyOS.scheduler.list()`).
    fn scheduler_list(&mut self) -> Result<JsValue, String> {
        Err("Scheduler host binding is not available in this context".to_string())
    }

    /// Creates a new scheduled task (`AlloyOS.scheduler.create(id, path, trigger, source)`).
    fn scheduler_create(
        &mut self,
        _id: &str,
        _script_path: &str,
        _trigger: &str,
        _source: &str,
    ) -> Result<JsValue, String> {
        Err("Scheduler host binding is not available in this context".to_string())
    }

    /// Edits an existing scheduled task (`AlloyOS.scheduler.edit(id, path, trigger, source)`).
    fn scheduler_edit(
        &mut self,
        _id: &str,
        _script_path: &str,
        _trigger: &str,
        _source: &str,
    ) -> Result<JsValue, String> {
        Err("Scheduler host binding is not available in this context".to_string())
    }

    /// Deletes a scheduled task (`AlloyOS.scheduler.delete(id)`).
    fn scheduler_delete(&mut self, _id: &str) -> Result<JsValue, String> {
        Err("Scheduler host binding is not available in this context".to_string())
    }
}

/// Wrapper around any `AlloyOsHost` that enforces a specific `ScriptCapabilities` policy.
pub struct CapabilityScopedHost<'a, H: AlloyOsHost + ?Sized> {
    inner: &'a mut H,
    capabilities: ScriptCapabilities,
}

impl<'a, H: AlloyOsHost + ?Sized> CapabilityScopedHost<'a, H> {
    pub fn new(inner: &'a mut H, capabilities: ScriptCapabilities) -> Self {
        Self {
            inner,
            capabilities,
        }
    }

    pub fn capabilities(&self) -> &ScriptCapabilities {
        &self.capabilities
    }
}

impl<'a, H: AlloyOsHost + ?Sized> AlloyOsHost for CapabilityScopedHost<'a, H> {
    fn check_capability(&self, req: &CapabilityRequest<'_>) -> Result<(), String> {
        self.capabilities.check(req)
    }

    fn forge(&mut self, prompt: &str, model: Option<&str>, max_tokens: u32) -> Result<String, String> {
        self.check_capability(&CapabilityRequest::ForgeGenerate)?;
        self.inner.forge(prompt, model, max_tokens)
    }

    fn fs_read(&mut self, path: &str) -> Option<String> {
        if self.check_capability(&CapabilityRequest::FsRead(path)).is_err() {
            return None;
        }
        self.inner.fs_read(path)
    }

    fn fs_write(&mut self, path: &str, content: &str) -> Result<(), String> {
        self.check_capability(&CapabilityRequest::FsWrite(path))?;
        self.inner.fs_write(path, content)
    }

    fn fs_delete(&mut self, path: &str) -> Result<bool, String> {
        self.check_capability(&CapabilityRequest::FsWrite(path))?;
        self.inner.fs_delete(path)
    }

    fn fs_list(&mut self, dir_path: &str) -> Vec<String> {
        if self.check_capability(&CapabilityRequest::FsRead(dir_path)).is_err() {
            return Vec::new();
        }
        self.inner.fs_list(dir_path)
    }

    fn net_fetch(&mut self, url: &str) -> Result<String, String> {
        self.check_capability(&CapabilityRequest::NetFetch(url))?;
        self.inner.net_fetch(url)
    }

    fn sys_status(&self) -> SystemStatus {
        if self.check_capability(&CapabilityRequest::SysStatus).is_err() {
            return SystemStatus {
                uptime_ms: 0,
                wall_clock_ms: None,
                mem_used_bytes: 0,
                mem_free_bytes: 0,
                active_tasks: 0,
            };
        }
        self.inner.sys_status()
    }

    fn ui_notify(&mut self, message: &str) {
        if self.check_capability(&CapabilityRequest::UiNotify).is_ok() {
            self.inner.ui_notify(message);
        }
    }

    fn log(&mut self, message: &str) {
        self.inner.log(message);
    }

    fn drain_emitted_events(&mut self) -> Vec<OsEvent> {
        self.inner.drain_emitted_events()
    }

    fn scheduler_list(&mut self) -> Result<JsValue, String> {
        self.check_capability(&CapabilityRequest::SchedulerAccess)?;
        self.inner.scheduler_list()
    }

    fn scheduler_create(
        &mut self,
        id: &str,
        script_path: &str,
        trigger: &str,
        source: &str,
    ) -> Result<JsValue, String> {
        self.check_capability(&CapabilityRequest::SchedulerAccess)?;
        self.inner.scheduler_create(id, script_path, trigger, source)
    }

    fn scheduler_edit(
        &mut self,
        id: &str,
        script_path: &str,
        trigger: &str,
        source: &str,
    ) -> Result<JsValue, String> {
        self.check_capability(&CapabilityRequest::SchedulerAccess)?;
        self.inner.scheduler_edit(id, script_path, trigger, source)
    }

    fn scheduler_delete(&mut self, id: &str) -> Result<JsValue, String> {
        self.check_capability(&CapabilityRequest::SchedulerAccess)?;
        self.inner.scheduler_delete(id)
    }
}

/// In-memory virtual host environment implementing `AlloyOsHost` for testing and local sandboxing.
pub type ForgeHandler = dyn FnMut(&str, Option<&str>, u32) -> Result<String, String>;
#[derive(Clone, Default)]
pub struct VirtualHostEnv {
    pub files: BTreeMap<String, String>,
    pub http_routes: BTreeMap<String, String>,
    pub status: SystemStatus,
    pub notifications: Vec<String>,
    pub logs: Vec<String>,
    pub emitted_events: Vec<OsEvent>,
    pub capabilities: ScriptCapabilities,
    forge_handler: Option<Rc<RefCell<Box<ForgeHandler>>>>,
    pub scheduler: Option<Rc<RefCell<crate::scheduler::ScriptScheduler>>>,
}

impl core::fmt::Debug for VirtualHostEnv {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("VirtualHostEnv")
            .field("status", &self.status)
            .field("capabilities", &self.capabilities)
            .field("forge_enabled", &self.forge_handler.is_some())
            .field("has_scheduler", &self.scheduler.is_some())
            .finish_non_exhaustive()
    }
}
impl VirtualHostEnv {
    pub fn set_forge_handler(&mut self, handler: Box<ForgeHandler>) {
        self.capabilities.allow_forge = true;
        self.forge_handler = Some(Rc::new(RefCell::new(handler)));
    }

    pub fn with_scheduler(mut self, scheduler: Rc<RefCell<crate::scheduler::ScriptScheduler>>) -> Self {
        self.scheduler = Some(scheduler);
        self
    }

    /// Creates a new `VirtualHostEnv` with default system status and automation capabilities.
    pub fn new() -> Self {
        Self {
            files: BTreeMap::new(),
            http_routes: BTreeMap::new(),
            status: SystemStatus::default(),
            notifications: Vec::new(),
            logs: Vec::new(),
            emitted_events: Vec::new(),
            capabilities: ScriptCapabilities::default_automation(),
            forge_handler: None,
            scheduler: None,
        }
    }

    /// Creates a `VirtualHostEnv` restricted to the specified `ScriptCapabilities`.
    pub fn with_capabilities(capabilities: ScriptCapabilities) -> Self {
        Self {
            capabilities,
            ..Self::new()
        }
    }

    /// Updates the capability policy enforced by this host environment.
    pub fn set_capabilities(&mut self, capabilities: ScriptCapabilities) {
        self.capabilities = capabilities;
    }

    /// Registers a mock HTTP response for `AlloyOS.net.fetch(url)`.
    pub fn register_http_response(&mut self, url: &str, body: &str) {
        self.http_routes.insert(url.to_string(), body.to_string());
    }

    /// Normalizes a filesystem path so leading slashes are consistent.
    pub fn normalize_path(path: &str) -> String {
        ScriptCapabilities::normalize_path(path)
    }
}

impl AlloyOsHost for VirtualHostEnv {
    fn check_capability(&self, req: &CapabilityRequest<'_>) -> Result<(), String> {
        self.capabilities.check(req)
    }

    fn forge(&mut self, prompt: &str, model: Option<&str>, max_tokens: u32) -> Result<String, String> {
        self.capabilities.check(&CapabilityRequest::ForgeGenerate)?;
        match &self.forge_handler {
            Some(handler) => (handler.borrow_mut())(prompt, model, max_tokens),
            None => Err("Forge capability unavailable".to_string()),
        }
    }

    fn fs_read(&mut self, path: &str) -> Option<String> {
        if self.capabilities.check(&CapabilityRequest::FsRead(path)).is_err() {
            return None;
        }
        let norm = Self::normalize_path(path);
        self.files.get(&norm).cloned()
    }

    fn fs_write(&mut self, path: &str, content: &str) -> Result<(), String> {
        self.capabilities.check(&CapabilityRequest::FsWrite(path))?;
        let norm = Self::normalize_path(path);
        let existed = self.files.insert(norm.clone(), content.to_string()).is_some();
        if existed {
            self.emitted_events.push(OsEvent::FsModified { path: norm });
        } else {
            self.emitted_events.push(OsEvent::FsCreated { path: norm });
        }
        Ok(())
    }

    fn fs_delete(&mut self, path: &str) -> Result<bool, String> {
        self.capabilities.check(&CapabilityRequest::FsWrite(path))?;
        let norm = Self::normalize_path(path);
        let removed = self.files.remove(&norm).is_some();
        if removed {
            self.emitted_events.push(OsEvent::FsDeleted { path: norm });
        }
        Ok(removed)
    }

    fn fs_list(&mut self, dir_path: &str) -> Vec<String> {
        if self
            .capabilities
            .check(&CapabilityRequest::FsRead(dir_path))
            .is_err()
        {
            return Vec::new();
        }
        let mut prefix = Self::normalize_path(dir_path);
        if !prefix.ends_with('/') {
            prefix.push('/');
        }
        let mut results = Vec::new();
        for key in self.files.keys() {
            if prefix == "/" {
                let rel = key.trim_start_matches('/');
                if !rel.is_empty() {
                    results.push(rel.to_string());
                }
            } else if let Some(stripped) = key.strip_prefix(&prefix) {
                if !stripped.is_empty() && !stripped.contains('/') {
                    results.push(stripped.to_string());
                }
            }
        }
        results
    }

    fn net_fetch(&mut self, url: &str) -> Result<String, String> {
        self.capabilities.check(&CapabilityRequest::NetFetch(url))?;
        if let Some(resp) = self.http_routes.get(url) {
            Ok(resp.clone())
        } else {
            Err(alloc::format!("HTTP 404 Not Found: {}", url))
        }
    }

    fn sys_status(&self) -> SystemStatus {
        self.status
    }

    fn ui_notify(&mut self, message: &str) {
        if self.capabilities.check(&CapabilityRequest::UiNotify).is_ok() {
            self.notifications.push(message.to_string());
        }
    }

    fn log(&mut self, message: &str) {
        self.logs.push(message.to_string());
    }

    fn drain_emitted_events(&mut self) -> Vec<OsEvent> {
        core::mem::take(&mut self.emitted_events)
    }

    fn scheduler_list(&mut self) -> Result<JsValue, String> {
        self.capabilities.check(&CapabilityRequest::SchedulerAccess)?;
        if let Some(ref sched) = self.scheduler {
            let sched_ref = sched.borrow();
            let mut list = Vec::new();
            for s in sched_ref.schedules() {
                let mut obj = BTreeMap::new();
                obj.insert("id".to_string(), JsValue::String(s.id.clone()));
                obj.insert("title".to_string(), JsValue::String(s.id.clone()));
                obj.insert("script_path".to_string(), JsValue::String(s.script_path.clone()));
                obj.insert("trigger".to_string(), JsValue::String(s.trigger.serialize()));
                obj.insert("enabled".to_string(), JsValue::Bool(true));
                obj.insert("status".to_string(), JsValue::String("active".to_string()));
                obj.insert("run_count".to_string(), JsValue::Number(if s.once_fired { 1 } else { 0 }));
                obj.insert("last_result".to_string(), JsValue::Null);
                list.push(JsValue::Object(obj));
            }
            Ok(JsValue::Array(list))
        } else {
            Err("SchedulerAccess capability unavailable".to_string())
        }
    }

    fn scheduler_create(
        &mut self,
        id: &str,
        script_path: &str,
        trigger: &str,
        source: &str,
    ) -> Result<JsValue, String> {
        self.capabilities.check(&CapabilityRequest::SchedulerAccess)?;
        if let Some(ref sched) = self.scheduler {
            let parsed = crate::scheduler::ScheduleTrigger::deserialize(trigger)?;
            let mut sched_mut = sched.borrow_mut();
            if !source.is_empty() {
                sched_mut.register_script_source(script_path, source);
            }
            sched_mut.schedule_script(id, script_path, parsed);
            let mut obj = BTreeMap::new();
            obj.insert("id".to_string(), JsValue::String(id.to_string()));
            obj.insert("script_path".to_string(), JsValue::String(script_path.to_string()));
            obj.insert("trigger".to_string(), JsValue::String(trigger.to_string()));
            obj.insert("created".to_string(), JsValue::Bool(true));
            Ok(JsValue::Object(obj))
        } else {
            Err("SchedulerAccess capability unavailable".to_string())
        }
    }

    fn scheduler_edit(
        &mut self,
        id: &str,
        script_path: &str,
        trigger: &str,
        source: &str,
    ) -> Result<JsValue, String> {
        self.capabilities.check(&CapabilityRequest::SchedulerAccess)?;
        if let Some(ref sched) = self.scheduler {
            let mut sched_mut = sched.borrow_mut();
            if !script_path.is_empty() && !trigger.is_empty() {
                let parsed = crate::scheduler::ScheduleTrigger::deserialize(trigger)?;
                if !source.is_empty() {
                    sched_mut.register_script_source(script_path, source);
                }
                sched_mut.schedule_script(id, script_path, parsed);
            }
            let mut obj = BTreeMap::new();
            obj.insert("id".to_string(), JsValue::String(id.to_string()));
            obj.insert("updated".to_string(), JsValue::Bool(true));
            Ok(JsValue::Object(obj))
        } else {
            Err("SchedulerAccess capability unavailable".to_string())
        }
    }

    fn scheduler_delete(&mut self, id: &str) -> Result<JsValue, String> {
        self.capabilities.check(&CapabilityRequest::SchedulerAccess)?;
        if let Some(ref sched) = self.scheduler {
            let ok = sched.borrow_mut().unregister_script(id);
            let mut obj = BTreeMap::new();
            obj.insert("id".to_string(), JsValue::String(id.to_string()));
            obj.insert("deleted".to_string(), JsValue::Bool(ok));
            Ok(JsValue::Object(obj))
        } else {
            Err("SchedulerAccess capability unavailable".to_string())
        }
    }
}
