use alloc::collections::BTreeMap;
use alloc::{boxed::Box,rc::Rc};
use core::cell::RefCell;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use metalos_proto::{InputEvent, InputEventKind};

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
}

/// Snapshot of system resource metrics returned by `MetalOS.sys.status()`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SystemStatus {
    pub uptime_ms: u64,
    pub mem_used_bytes: u64,
    pub mem_free_bytes: u64,
    pub active_tasks: u32,
}

impl Default for SystemStatus {
    fn default() -> Self {
        Self {
            uptime_ms: 1_000,
            mem_used_bytes: 16 * 1024 * 1024,
            mem_free_bytes: 112 * 1024 * 1024,
            active_tasks: 4,
        }
    }
}

/// Host capability trait exposed to JavaScript isolates under the `MetalOS` namespace.
///
/// Notice that shell execution capabilities (`sh`, `exec`, `spawn`) are intentionally
/// absent by design in metalOS.
pub trait MetalOsHost {
    /// Local inference capability; hosts must opt in explicitly.
    fn forge(&mut self, _prompt: &str, _model: Option<&str>, _max_tokens: u32) -> Result<String, String> {
        Err("Forge capability unavailable".to_string())
    }

    /// Reads a UTF-8 file from CobaltFS/VFS (`MetalOS.fs.read(path)`).
    fn fs_read(&mut self, path: &str) -> Option<String>;

    /// Writes a UTF-8 file to CobaltFS/VFS (`MetalOS.fs.write(path, content)`).
    fn fs_write(&mut self, path: &str, content: &str) -> Result<(), String>;

    /// Deletes a file from CobaltFS/VFS (`MetalOS.fs.delete(path)`).
    fn fs_delete(&mut self, path: &str) -> Result<bool, String>;

    /// Lists file names or paths under `dir_path` (`MetalOS.fs.list(path)`).
    fn fs_list(&mut self, dir_path: &str) -> Vec<String>;

    /// Performs an HTTP GET request via the network stack (`MetalOS.net.fetch(url)`).
    fn net_fetch(&mut self, url: &str) -> Result<String, String>;

    /// Queries system uptime and memory statistics (`MetalOS.sys.status()`).
    fn sys_status(&self) -> SystemStatus;

    /// Queues a notification to the Ferrofluid UI (`MetalOS.ui.notify(message)`).
    fn ui_notify(&mut self, message: &str);

    /// Appends a message to the script execution log (`MetalOS.log(message)`).
    fn log(&mut self, message: &str);

    /// Drains any `OsEvent`s generated by host mutations (such as `FsCreated` / `FsModified`).
    fn drain_emitted_events(&mut self) -> Vec<OsEvent>;
}

/// In-memory virtual host environment implementing `MetalOsHost` for testing and local sandboxing.
pub type ForgeHandler = dyn FnMut(&str, Option<&str>, u32) -> Result<String, String>;
#[derive(Clone, Default)]
pub struct VirtualHostEnv {
    pub files: BTreeMap<String, String>,
    pub http_routes: BTreeMap<String, String>,
    pub status: SystemStatus,
    pub notifications: Vec<String>,
    pub logs: Vec<String>,
    pub emitted_events: Vec<OsEvent>,
    forge_handler: Option<Rc<RefCell<Box<ForgeHandler>>>>,
}

impl core::fmt::Debug for VirtualHostEnv {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result { f.debug_struct("VirtualHostEnv").field("status", &self.status).field("forge_enabled", &self.forge_handler.is_some()).finish_non_exhaustive() }
}
impl VirtualHostEnv {
    pub fn set_forge_handler(&mut self, handler: Box<ForgeHandler>) {
        self.forge_handler = Some(Rc::new(RefCell::new(handler)));
    }
    /// Creates a new `VirtualHostEnv` with default system status.
    pub fn new() -> Self {
        Self {
            files: BTreeMap::new(),
            http_routes: BTreeMap::new(),
            status: SystemStatus::default(),
            notifications: Vec::new(),
            logs: Vec::new(),
            emitted_events: Vec::new(),
            forge_handler: None,
        }
    }

    /// Registers a mock HTTP response for `MetalOS.net.fetch(url)`.
    pub fn register_http_response(&mut self, url: &str, body: &str) {
        self.http_routes.insert(url.to_string(), body.to_string());
    }

    /// Normalizes a filesystem path so leading slashes are consistent.
    fn normalize_path(path: &str) -> String {
        let trimmed = path.trim();
        if trimmed.is_empty() {
            "/".to_string()
        } else if trimmed.starts_with('/') {
            trimmed.to_string()
        } else {
            let mut s = String::from("/");
            s.push_str(trimmed);
            s
        }
    }
}

impl MetalOsHost for VirtualHostEnv {
    fn forge(&mut self, prompt: &str, model: Option<&str>, max_tokens: u32) -> Result<String, String> {
        match &self.forge_handler { Some(handler) => (handler.borrow_mut())(prompt, model, max_tokens), None => Err("Forge capability unavailable".to_string()) }
    }
    fn fs_read(&mut self, path: &str) -> Option<String> {
        let norm = Self::normalize_path(path);
        self.files.get(&norm).cloned()
    }

    fn fs_write(&mut self, path: &str, content: &str) -> Result<(), String> {
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
        let norm = Self::normalize_path(path);
        let removed = self.files.remove(&norm).is_some();
        if removed {
            self.emitted_events.push(OsEvent::FsDeleted { path: norm });
        }
        Ok(removed)
    }

    fn fs_list(&mut self, dir_path: &str) -> Vec<String> {
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
        self.notifications.push(message.to_string());
    }

    fn log(&mut self, message: &str) {
        self.logs.push(message.to_string());
    }

    fn drain_emitted_events(&mut self) -> Vec<OsEvent> {
        core::mem::take(&mut self.emitted_events)
    }
}
