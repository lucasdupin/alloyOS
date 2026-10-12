use alloc::collections::{BTreeMap, BTreeSet};
use alloc::format;
use alloc::rc::Rc;
use alloc::string::{String, ToString};
use alloc::vec;
use alloc::vec::Vec;
use core::cell::RefCell;

use crate::ui_synth::{
    AgentUiSynthesizer, SynthesizedUiView, UiCallerOrigin, UiMetric, UiScheduleEntry, ViewKind,
};
use crate::{ToolCall as WelderToolCall, ToolExecutor, WelderError};
use metalos_forge::provider::{
    extract_json_string_field, json_escape, AiError, ToolCall, ToolParameter, ToolSpec,
};
use metalos_forge::{Forge, GenerationRequest};
use metalos_js::{
    json_stringify, parse_program, CalendarTime, CapabilityRequest, JsError, MetalOsHost,
    OsEvent, ScheduleTrigger, ScriptCapabilities, ScriptExecutionReport, ScriptScheduler,
    SystemStatus, SCHEDULES_CONF_PATH,
};

pub use metalos_forge::policy::{
    is_forbidden_shell_tool, validate_console_input_no_shell, SHELL_FREE_POLICY_MESSAGE,
};

/// Pluggable persistent VFS / CobaltFS backing store for `NativeToolRegistry`.
pub trait ToolVfsBackend: core::fmt::Debug {
    fn read_file(&mut self, path: &str) -> Option<String>;
    fn write_file(&mut self, path: &str, content: &str) -> Result<(), String>;
    fn delete_file(&mut self, path: &str) -> Result<bool, String>;
    fn list_dir(&mut self, dir_path: &str) -> Vec<String>;
}

/// Shared map-backed VFS store implementing `ToolVfsBackend` for simulated persistence across reboots.
#[derive(Debug, Clone, Default)]
pub struct SharedMapVfsBackend {
    pub files: BTreeMap<String, String>,
}

impl SharedMapVfsBackend {
    pub fn new() -> Self {
        Self {
            files: BTreeMap::new(),
        }
    }

    pub fn shared() -> Rc<RefCell<Self>> {
        Rc::new(RefCell::new(Self::new()))
    }
}

impl ToolVfsBackend for SharedMapVfsBackend {
    fn read_file(&mut self, path: &str) -> Option<String> {
        let norm = ScriptCapabilities::normalize_path(path);
        self.files.get(&norm).cloned()
    }

    fn write_file(&mut self, path: &str, content: &str) -> Result<(), String> {
        let norm = ScriptCapabilities::normalize_path(path);
        self.files.insert(norm, content.to_string());
        Ok(())
    }

    fn delete_file(&mut self, path: &str) -> Result<bool, String> {
        let norm = ScriptCapabilities::normalize_path(path);
        Ok(self.files.remove(&norm).is_some())
    }

    fn list_dir(&mut self, dir_path: &str) -> Vec<String> {
        let norm = ScriptCapabilities::normalize_path(dir_path);
        let prefix = if norm == "/" {
            "/".to_string()
        } else {
            format!("{}/", norm)
        };
        self.files
            .keys()
            .filter(|k| *k == &norm || k.starts_with(&prefix))
            .cloned()
            .collect()
    }
}

/// Output produced by executing a structured native OS tool.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolExecutionOutput {
    pub tool_name: String,
    pub output_json: String,
    pub synthesized_view: Option<SynthesizedUiView>,
}

/// Registry of safe, structured AI-first OS capabilities in metalOS.
///
/// The AI agent has zero access to `sh` or POSIX shell execution; all OS interaction goes through
/// strongly-typed tools for CobaltFS, system telemetry, local JavaScript automation, headless MCP
/// apps, and Ferrofluid UI synthesis.
#[derive(Clone)]
pub struct NativeToolRegistry {
    tools: Vec<ToolSpec>,
    vfs_files: BTreeMap<String, String>,
    vfs_backend: Option<Rc<RefCell<dyn ToolVfsBackend>>>,
    js_schedules: Vec<(String, String)>,
    scheduler: ScriptScheduler,
    default_capabilities: ScriptCapabilities,
    sys_status: SystemStatus,
    http_routes: BTreeMap<String, String>,
    notifications: Vec<String>,
    logs: Vec<String>,
    execution_reports: Vec<ScriptExecutionReport>,
    forge: Option<Rc<RefCell<Forge>>>,
    mcp_apps: BTreeMap<String, String>,
}

impl core::fmt::Debug for NativeToolRegistry {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("NativeToolRegistry")
            .field("tools_count", &self.tools.len())
            .field("vfs_files_count", &self.vfs_files.len())
            .field("has_vfs_backend", &self.vfs_backend.is_some())
            .field("js_schedules", &self.js_schedules)
            .field("sys_status", &self.sys_status)
            .finish()
    }
}

impl Default for NativeToolRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl NativeToolRegistry {
    pub fn new() -> Self {
        let mut vfs_files = BTreeMap::new();
        vfs_files.insert(
            "/etc/ai.conf".to_string(),
            "provider=gemini\nmodel=gemini-3.8-flash\n".to_string(),
        );
        vfs_files.insert(
            SCHEDULES_CONF_PATH.to_string(),
            "# metalOS /etc/schedules.conf\n".to_string(),
        );
        let heartbeat_src =
            "export function main() { return MetalOS.sys.status(); }\n".to_string();
        vfs_files.insert("/scripts/heartbeat.js".to_string(), heartbeat_src.clone());

        let mut scheduler = ScriptScheduler::new();
        scheduler.register_script_source("/scripts/heartbeat.js", &heartbeat_src);

        let mut mcp_apps = BTreeMap::new();
        mcp_apps.insert(
            "telemetry.mcp".to_string(),
            "{\"title\":\"Cluster Telemetry\",\"summary\":\"All 8 M4 cores nominal\",\"cpu_usage\":\"14\",\"memory_mb\":\"512\",\"rows\":[[\"core0\",\"active\"],[\"core1\",\"active\"]]}".to_string(),
        );
        mcp_apps.insert(
            "notes.mcp".to_string(),
            "{\"title\":\"Cobalt Notes\",\"summary\":\"2 indexed workspace notes\",\"value\":\"2\",\"label\":\"Notes\",\"unit\":\"docs\"}".to_string(),
        );

        let tools = Self::default_tool_specs();
        Self {
            tools,
            vfs_files,
            vfs_backend: None,
            js_schedules: Vec::new(),
            scheduler,
            default_capabilities: ScriptCapabilities::default_automation(),
            sys_status: SystemStatus {
                uptime_ms: 1_000,
                mem_used_bytes: 12 * 1024 * 1024,
                mem_free_bytes: 116 * 1024 * 1024,
                active_tasks: 6,
            },
            http_routes: BTreeMap::new(),
            notifications: Vec::new(),
            logs: Vec::new(),
            execution_reports: Vec::new(),
            forge: None,
            mcp_apps,
        }
    }

    /// Creates a `NativeToolRegistry` connected to a persistent `ToolVfsBackend`, restoring any
    /// existing `/scripts/*.js` and `/etc/schedules.conf` automations.
    pub fn with_vfs_backend(backend: Rc<RefCell<dyn ToolVfsBackend>>) -> Self {
        let mut reg = Self::new();
        reg.set_vfs_backend(backend);
        reg
    }

    /// Connects a persistent `ToolVfsBackend` (such as CobaltFS / kernel VFS) and restores
    /// persisted configuration, scripts, and schedules.
    pub fn set_vfs_backend(&mut self, backend: Rc<RefCell<dyn ToolVfsBackend>>) {
        // Seed defaults on backend only if they don't already exist on persistent storage,
        // and sync all existing files from the backend into the in-memory cache.
        {
            let mut b = backend.borrow_mut();
            for existing_path in b.list_dir("/") {
                let norm = ScriptCapabilities::normalize_path(&existing_path);
                if let Some(content) = b.read_file(&norm) {
                    self.vfs_files.insert(norm, content);
                }
            }
            for (default_path, default_content) in [
                ("/etc/ai.conf", "provider=forge\nmodel=gemma-2-2b-it\n"),
                (SCHEDULES_CONF_PATH, "# metalOS /etc/schedules.conf\n"),
                (
                    "/scripts/heartbeat.js",
                    "export function main() { return MetalOS.sys.status(); }\n",
                ),
            ] {
                if let Some(existing) = b.read_file(default_path) {
                    self.vfs_files.insert(default_path.to_string(), existing);
                } else {
                    let _ = b.write_file(default_path, default_content);
                    self.vfs_files
                        .insert(default_path.to_string(), default_content.to_string());
                }
            }
        }
        self.vfs_backend = Some(backend);
        let _ = self.restore_automations_from_vfs();
    }

    /// Connects the shared `Forge` service for opt-in `MetalOS.forge(...)` script invocations.
    pub fn set_forge(&mut self, forge: Rc<RefCell<Forge>>) {
        self.forge = Some(forge);
    }

    /// Configures the default `ScriptCapabilities` enforced for JavaScript isolates.
    pub fn set_default_script_capabilities(&mut self, capabilities: ScriptCapabilities) {
        self.default_capabilities = capabilities;
    }

    /// Returns the default `ScriptCapabilities` enforced for JavaScript isolates.
    pub fn default_script_capabilities(&self) -> &ScriptCapabilities {
        &self.default_capabilities
    }

    /// Assigns capability constraints for a specific script path (e.g. `/scripts/untrusted.js`).
    pub fn set_script_capabilities(&mut self, script_path: &str, capabilities: ScriptCapabilities) {
        let norm = ScriptCapabilities::normalize_path(script_path);
        self.scheduler.set_script_capabilities(&norm, capabilities);
    }

    /// Updates the live `SystemStatus` snapshot returned by `sys_status` and `MetalOS.sys.status()`.
    pub fn set_system_status(&mut self, status: SystemStatus) {
        self.sys_status = status;
    }

    /// Returns the current `SystemStatus` snapshot.
    pub fn system_status(&self) -> SystemStatus {
        self.sys_status
    }

    /// Registers an HTTP response body for `MetalOS.net.fetch(url)`.
    pub fn register_http_response(&mut self, url: &str, body: &str) {
        self.http_routes.insert(url.to_string(), body.to_string());
    }

    /// Returns a shared reference to the embedded `ScriptScheduler`.
    pub fn scheduler(&self) -> &ScriptScheduler {
        &self.scheduler
    }

    /// Returns a mutable reference to the embedded `ScriptScheduler`.
    pub fn scheduler_mut(&mut self) -> &mut ScriptScheduler {
        &mut self.scheduler
    }

    /// Returns all UI notifications emitted by scripts via `MetalOS.ui.notify(...)`.
    pub fn notifications(&self) -> &[String] {
        &self.notifications
    }

    /// Drains and returns all queued script UI notifications.
    pub fn drain_notifications(&mut self) -> Vec<String> {
        core::mem::take(&mut self.notifications)
    }

    /// Returns all log lines emitted by scripts via `MetalOS.log(...)` / `console.log(...)`.
    pub fn logs(&self) -> &[String] {
        &self.logs
    }

    /// Drains and returns all script log lines.
    pub fn drain_logs(&mut self) -> Vec<String> {
        core::mem::take(&mut self.logs)
    }

    /// Returns the history of scheduled script execution reports.
    pub fn execution_reports(&self) -> &[ScriptExecutionReport] {
        &self.execution_reports
    }

    /// Restores `/scripts/*.js` and `/etc/schedules.conf` from the VFS backing store into `ScriptScheduler`.
    pub fn restore_automations_from_vfs(&mut self) -> Result<usize, String> {
        let script_entries = self.list_vfs_entries("/scripts");
        for entry in script_entries {
            let norm = if entry.starts_with("/scripts/") {
                ScriptCapabilities::normalize_path(&entry)
            } else {
                ScriptCapabilities::normalize_path(&format!(
                    "/scripts/{}",
                    entry.trim_start_matches('/')
                ))
            };
            if norm.ends_with(".js") {
                if let Some(src) = self.read_vfs_string(&norm) {
                    self.scheduler.register_script_source(&norm, &src);
                }
            }
        }

        // Clear existing schedules before reloading from /etc/schedules.conf
        let existing_ids: Vec<String> = self
            .scheduler
            .schedules()
            .iter()
            .map(|s| s.id.clone())
            .collect();
        for id in existing_ids {
            self.scheduler.unregister_script(&id);
        }

        let Some(conf) = self.read_vfs_string(SCHEDULES_CONF_PATH) else {
            self.sync_schedule_list();
            return Ok(0);
        };
        let count = self.scheduler.load_schedules_conf_str(&conf)?;
        let scheduled_paths: Vec<String> = self
            .scheduler
            .schedules()
            .iter()
            .map(|s| s.script_path.clone())
            .collect();
        for path in scheduled_paths {
            if self.scheduler.store().get_script(&path).is_none() {
                if let Some(src) = self.read_vfs_string(&path) {
                    self.scheduler.register_script_source(&path, &src);
                }
            }
        }
        self.sync_schedule_list();
        Ok(count)
    }

    fn sync_schedule_list(&mut self) {
        self.js_schedules = self
            .scheduler
            .schedules()
            .iter()
            .map(|s| (s.script_path.clone(), s.trigger.serialize()))
            .collect();
    }

    fn default_tool_specs() -> Vec<ToolSpec> {
        vec![
            ToolSpec::new(
                "fs_read",
                "Read UTF-8 file contents from CobaltFS or VFS path.",
                vec![ToolParameter::new(
                    "path",
                    "string",
                    "Absolute file path on CobaltFS (e.g. /etc/ai.conf)",
                    true,
                )],
            ),
            ToolSpec::new(
                "fs_write",
                "Write UTF-8 file contents to CobaltFS or VFS path.",
                vec![
                    ToolParameter::new("path", "string", "Absolute file path on CobaltFS", true),
                    ToolParameter::new("content", "string", "File contents to persist", true),
                ],
            ),
            ToolSpec::new(
                "fs_list",
                "List entries under a directory prefix on CobaltFS or VFS.",
                vec![ToolParameter::new(
                    "path",
                    "string",
                    "Directory path prefix (e.g. / or /scripts)",
                    true,
                )],
            ),
            ToolSpec::new(
                "sys_status",
                "Inspect metalOS kernel memory allocation, active tasks, uptime, and VirtIO-Net status.",
                Vec::new(),
            ),
            ToolSpec::new(
                "script_create",
                "Author and persist a local JavaScript program on CobaltFS (/scripts/*.js).",
                vec![
                    ToolParameter::new(
                        "path",
                        "string",
                        "Target script path under /scripts/ (e.g. /scripts/cleanup.js)",
                        true,
                    ),
                    ToolParameter::new(
                        "source",
                        "string",
                        "JavaScript ES module source code using MetalOS.* host bindings",
                        true,
                    ),
                ],
            ),
            ToolSpec::new(
                "script_run",
                "Execute a local JavaScript script inside the embedded metalOS JS runtime.",
                vec![ToolParameter::new(
                    "path",
                    "string",
                    "Path to the JavaScript file on CobaltFS",
                    true,
                )],
            ),
            ToolSpec::new(
                "script_schedule",
                "Schedule a local JavaScript script to run on a time trigger (cron/interval) or OS event.",
                vec![
                    ToolParameter::new("path", "string", "Path to the JavaScript file", true),
                    ToolParameter::new(
                        "trigger",
                        "string",
                        "Time or event trigger (e.g. 'every:60s', 'cron:*/5 * * * *', 'fs:modify:/etc')",
                        true,
                    ),
                ],
            ),
            ToolSpec::new(
                "mcp_list_tools",
                "List available tools and resources exposed by headless MCP developer applications.",
                Vec::new(),
            ),
            ToolSpec::new(
                "mcp_call_tool",
                "Invoke a structured tool on a headless MCP developer app and synthesize Ferrofluid UI if needed.",
                vec![
                    ToolParameter::new("app", "string", "Headless MCP server identifier", true),
                    ToolParameter::new("tool", "string", "MCP tool name to invoke", true),
                    ToolParameter::new(
                        "args",
                        "string",
                        "JSON-serialized arguments for the MCP tool",
                        false,
                    ),
                ],
            ),
            ToolSpec::new(
                "mcp_read_resource",
                "Read a structured resource URI from a headless MCP developer application.",
                vec![
                    ToolParameter::new("app", "string", "Headless MCP server identifier", true),
                    ToolParameter::new("uri", "string", "Resource URI to read", true),
                ],
            ),
            ToolSpec::new(
                "ui_render_view",
                "Agent-exclusive tool to synthesize or update a generative UI view in Ferrofluid (ferrofluidd).",
                vec![
                    ToolParameter::new(
                        "kind",
                        "string",
                        "View archetype: console, metric_dashboard, table, script_schedule_list, mcp_app_view",
                        true,
                    ),
                    ToolParameter::new("title", "string", "Window or panel title", true),
                    ToolParameter::new("summary", "string", "Concise summary for the view", true),
                ],
            ),
        ]
    }

    /// Returns the slice of registered safe OS tools.
    pub fn specs(&self) -> &[ToolSpec] {
        &self.tools
    }

    /// Looks up a registered tool specification by name.
    pub fn get_spec(&self, name: &str) -> Option<&ToolSpec> {
        self.tools.iter().find(|t| t.name == name)
    }

    /// Registers an additional structured tool (e.g., discovered from an MCP server) after
    /// verifying that it does not expose shell execution.
    pub fn register_tool(&mut self, spec: ToolSpec) -> Result<(), AiError> {
        if is_forbidden_shell_tool(&spec.name) {
            return Err(AiError::ShellAccessForbidden(format!(
                "Cannot register forbidden shell tool '{}'. {}",
                spec.name, SHELL_FREE_POLICY_MESSAGE
            )));
        }
        if let Some(existing) = self.tools.iter_mut().find(|t| t.name == spec.name) {
            *existing = spec;
        } else {
            self.tools.push(spec);
        }
        Ok(())
    }

    fn read_vfs_string(&mut self, path: &str) -> Option<String> {
        let norm = ScriptCapabilities::normalize_path(path);
        if let Some(backend) = &self.vfs_backend {
            if let Some(content) = backend.borrow_mut().read_file(&norm) {
                self.vfs_files.insert(norm, content.clone());
                return Some(content);
            }
        }
        self.vfs_files.get(&norm).cloned()
    }

    fn write_vfs_raw(&mut self, path: &str, content: &str) -> Result<bool, String> {
        let norm = ScriptCapabilities::normalize_path(path);
        let existed = if let Some(backend) = &self.vfs_backend {
            let had = backend.borrow_mut().read_file(&norm).is_some();
            backend.borrow_mut().write_file(&norm, content)?;
            had
        } else {
            self.vfs_files.contains_key(&norm)
        };
        self.vfs_files.insert(norm.clone(), content.to_string());
        if norm.ends_with(".js") {
            self.scheduler.register_script_source(&norm, content);
        }
        Ok(existed)
    }

    fn delete_vfs_raw(&mut self, path: &str) -> Result<bool, String> {
        let norm = ScriptCapabilities::normalize_path(path);
        let removed_backend = if let Some(backend) = &self.vfs_backend {
            backend.borrow_mut().delete_file(&norm)?
        } else {
            false
        };
        let removed_mem = self.vfs_files.remove(&norm).is_some();
        if norm.ends_with(".js") {
            self.scheduler.store_mut().remove_script(&norm);
        }
        Ok(removed_backend || removed_mem)
    }

    fn list_vfs_entries(&mut self, prefix: &str) -> Vec<String> {
        let norm = ScriptCapabilities::normalize_path(prefix);
        let mut set = BTreeSet::new();
        if let Some(backend) = &self.vfs_backend {
            for entry in backend.borrow_mut().list_dir(&norm) {
                if entry.starts_with('/') {
                    set.insert(entry);
                } else if norm == "/" {
                    set.insert(format!("/{}", entry.trim_start_matches('/')));
                } else {
                    set.insert(format!("{}/{}", norm, entry.trim_start_matches('/')));
                }
            }
        }
        let dir_prefix = if norm == "/" {
            "/".to_string()
        } else {
            format!("{}/", norm)
        };
        for key in self.vfs_files.keys() {
            if key == &norm || key.starts_with(&dir_prefix) || key.starts_with(prefix) {
                set.insert(key.clone());
            }
        }
        set.into_iter().collect()
    }

    fn list_vfs_dir_child_names(&mut self, dir_path: &str) -> Vec<String> {
        let norm = ScriptCapabilities::normalize_path(dir_path);
        let dir_prefix = if norm == "/" {
            "/".to_string()
        } else {
            format!("{}/", norm)
        };
        let full_entries = self.list_vfs_entries(&norm);
        let mut names = BTreeSet::new();
        for key in full_entries {
            if norm == "/" {
                let rel = key.trim_start_matches('/');
                if !rel.is_empty() {
                    names.insert(rel.to_string());
                }
            } else if let Some(stripped) = key.strip_prefix(&dir_prefix) {
                let name = stripped.trim_end_matches('/');
                if !name.is_empty() && !name.contains('/') {
                    names.insert(name.to_string());
                }
            }
        }
        names.into_iter().collect()
    }

    /// Reads a file directly from the CobaltFS / VFS state.
    pub fn vfs_read(&self, path: &str) -> Option<&str> {
        let norm = ScriptCapabilities::normalize_path(path);
        self.vfs_files.get(&norm).map(|s| s.as_str())
    }

    /// Writes a file directly into the CobaltFS / VFS state and dispatches any matching
    /// `FsCreated` / `FsModified` automation triggers.
    pub fn vfs_write(&mut self, path: impl Into<String>, content: impl Into<String>) {
        let path_str = ScriptCapabilities::normalize_path(&path.into());
        let content_str = content.into();
        let existed = self.write_vfs_raw(&path_str, &content_str).unwrap_or(false);
        if path_str != "/etc/ai.conf" && path_str != SCHEDULES_CONF_PATH {
            let ev = if existed {
                OsEvent::FsModified { path: path_str }
            } else {
                OsEvent::FsCreated { path: path_str }
            };
            let _ = self.dispatch_os_event(&ev);
        }
    }

    /// Dispatches an `OsEvent` to all matching scheduled scripts and processes any cascading
    /// filesystem events emitted during script execution.
    pub fn dispatch_os_event(&mut self, event: &OsEvent) -> Vec<ScriptExecutionReport> {
        let mut all_reports = Vec::new();
        let mut queue = vec![event.clone()];
        let mut hops = 0usize;

        while !queue.is_empty() && hops < 8 {
            hops += 1;
            let batch = core::mem::take(&mut queue);
            for ev in batch {
                let (reports, emitted) = self.run_scheduler_step(|sched, host| {
                    sched.dispatch_event(&ev, host)
                });
                self.execution_reports.extend(reports.clone());
                all_reports.extend(reports);
                queue.extend(emitted);
            }
        }
        all_reports
    }

    /// Advances the scheduler clock to `now_ms` (and optional `calendar`), executing any due
    /// `OnceAtMs`, `IntervalMs`, or `Cron` scripts and cascading any emitted OS events.
    pub fn tick_scheduler(
        &mut self,
        now_ms: u64,
        calendar: Option<CalendarTime>,
    ) -> Vec<ScriptExecutionReport> {
        self.sys_status.uptime_ms = now_ms;
        let (mut all_reports, emitted) =
            self.run_scheduler_step(|sched, host| sched.tick_time(now_ms, calendar, host));
        self.execution_reports.extend(all_reports.clone());
        for ev in emitted {
            all_reports.extend(self.dispatch_os_event(&ev));
        }
        all_reports
    }

    pub fn notify_system_boot(&mut self) -> Vec<ScriptExecutionReport> {
        self.dispatch_os_event(&OsEvent::SystemBoot)
    }

    pub fn notify_file_created(&mut self, path: &str) -> Vec<ScriptExecutionReport> {
        self.dispatch_os_event(&OsEvent::FsCreated {
            path: ScriptCapabilities::normalize_path(path),
        })
    }

    pub fn notify_file_modified(&mut self, path: &str) -> Vec<ScriptExecutionReport> {
        self.dispatch_os_event(&OsEvent::FsModified {
            path: ScriptCapabilities::normalize_path(path),
        })
    }

    pub fn notify_file_deleted(&mut self, path: &str) -> Vec<ScriptExecutionReport> {
        self.dispatch_os_event(&OsEvent::FsDeleted {
            path: ScriptCapabilities::normalize_path(path),
        })
    }

    pub fn notify_network_online(&mut self) -> Vec<ScriptExecutionReport> {
        self.dispatch_os_event(&OsEvent::NetInterfaceUp)
    }

    pub fn notify_network_request(&mut self, port: u16, payload: &str) -> Vec<ScriptExecutionReport> {
        self.dispatch_os_event(&OsEvent::NetPacketReceived {
            port,
            payload: payload.to_string(),
        })
    }

    fn run_scheduler_step<R, F>(&mut self, f: F) -> (R, Vec<OsEvent>)
    where
        F: FnOnce(&mut ScriptScheduler, &mut dyn MetalOsHost) -> R,
    {
        let mut scheduler = core::mem::take(&mut self.scheduler);
        let mut emitted = Vec::new();
        let out = {
            let mut host = RegistryHostAdapter {
                registry: self,
                emitted_events: &mut emitted,
            };
            f(&mut scheduler, &mut host)
        };
        self.scheduler = scheduler;
        (out, emitted)
    }

    fn apply_script_capabilities_from_args(&mut self, norm_path: &str, args_json: &str) {
        let mut has_custom = false;
        let mut caps = ScriptCapabilities::none()
            .with_sys_status(true)
            .with_ui_notify(true);

        if let Some(read_prefix) = extract_json_string_field(args_json, "fs_read_prefix") {
            has_custom = true;
            for p in read_prefix.split(',') {
                if !p.trim().is_empty() {
                    caps = caps.with_fs_read(p.trim());
                }
            }
        } else {
            caps = caps.with_fs_read("/");
        }

        if let Some(write_prefix) = extract_json_string_field(args_json, "fs_write_prefix") {
            has_custom = true;
            for p in write_prefix.split(',') {
                if !p.trim().is_empty() {
                    caps = caps.with_fs_write(p.trim());
                }
            }
        } else {
            caps = caps.with_fs_write("/");
        }

        if let Some(domain) = extract_json_string_field(args_json, "net_domain") {
            has_custom = true;
            for d in domain.split(',') {
                if !d.trim().is_empty() {
                    caps = caps.with_net_domain(d.trim());
                }
            }
        } else {
            caps = caps.with_net_domain("*");
        }

        if let Some(allow_forge) = extract_json_string_field(args_json, "allow_forge") {
            has_custom = true;
            caps = caps.with_forge(allow_forge == "true" || allow_forge == "1");
        }

        if has_custom {
            self.scheduler.set_script_capabilities(norm_path, caps);
        }
    }

    /// Executes a `ToolCall` against the native OS tool registry and updates `ui_synth` when applicable.
    pub fn execute_tool(
        &mut self,
        call: &ToolCall,
        ui_synth: &mut AgentUiSynthesizer,
    ) -> Result<ToolExecutionOutput, AiError> {
        if is_forbidden_shell_tool(&call.name) {
            return Err(AiError::ShellAccessForbidden(format!(
                "Attempted to invoke forbidden shell tool '{}'. {}",
                call.name, SHELL_FREE_POLICY_MESSAGE
            )));
        }

        match call.name.as_str() {
            "fs_read" => {
                let raw_path = extract_json_string_field(&call.arguments_json, "path")
                    .ok_or_else(|| AiError::InvalidToolArguments("missing 'path'".to_string()))?;
                let path = ScriptCapabilities::normalize_path(&raw_path);
                match self.read_vfs_string(&path) {
                    Some(content) => Ok(ToolExecutionOutput {
                        tool_name: call.name.clone(),
                        output_json: format!(
                            "{{\"ok\":true,\"path\":\"{}\",\"content\":\"{}\"}}",
                            json_escape(&path),
                            json_escape(&content)
                        ),
                        synthesized_view: None,
                    }),
                    None => Ok(ToolExecutionOutput {
                        tool_name: call.name.clone(),
                        output_json: format!(
                            "{{\"ok\":false,\"error\":\"file not found\",\"path\":\"{}\"}}",
                            json_escape(&path)
                        ),
                        synthesized_view: None,
                    }),
                }
            }
            "fs_write" => {
                let raw_path = extract_json_string_field(&call.arguments_json, "path")
                    .ok_or_else(|| AiError::InvalidToolArguments("missing 'path'".to_string()))?;
                let path = ScriptCapabilities::normalize_path(&raw_path);
                let content = extract_json_string_field(&call.arguments_json, "content")
                    .ok_or_else(|| {
                        AiError::InvalidToolArguments("missing 'content'".to_string())
                    })?;
                let bytes = content.len();
                let existed = self
                    .write_vfs_raw(&path, &content)
                    .map_err(AiError::InvalidToolArguments)?;
                let event = if existed {
                    OsEvent::FsModified { path: path.clone() }
                } else {
                    OsEvent::FsCreated { path: path.clone() }
                };
                let triggered = self.dispatch_os_event(&event);
                Ok(ToolExecutionOutput {
                    tool_name: call.name.clone(),
                    output_json: format!(
                        "{{\"ok\":true,\"path\":\"{}\",\"bytes_written\":{},\"triggered_scripts\":{}}}",
                        json_escape(&path),
                        bytes,
                        triggered.len()
                    ),
                    synthesized_view: None,
                })
            }
            "fs_list" => {
                let prefix = extract_json_string_field(&call.arguments_json, "path")
                    .unwrap_or_else(|| "/".to_string());
                let entries = self.list_vfs_entries(&prefix);
                let joined = entries
                    .iter()
                    .map(|e| format!("\"{}\"", json_escape(e)))
                    .collect::<Vec<_>>()
                    .join(",");
                Ok(ToolExecutionOutput {
                    tool_name: call.name.clone(),
                    output_json: format!(
                        "{{\"ok\":true,\"prefix\":\"{}\",\"entries\":[{}]}}",
                        json_escape(&prefix),
                        joined
                    ),
                    synthesized_view: None,
                })
            }
            "sys_status" => {
                let heap_mb = self.sys_status.mem_used_bytes / (1024 * 1024);
                let active_tasks = self.sys_status.active_tasks;
                let view = ui_synth.synthesize_metric_dashboard(
                    "metalOS System Telemetry",
                    "ARM64 Apple M4 HVF kernel & Ferrofluid runtime metrics",
                    vec![
                        UiMetric::new("Arch", "aarch64", "EL1"),
                        UiMetric::new("Heap Used", format!("{}", heap_mb), "MB"),
                        UiMetric::new("Active Tasks", format!("{}", active_tasks), "tasks"),
                        UiMetric::new("Net Stack", "smoltcp+tls13", "up"),
                    ],
                );
                Ok(ToolExecutionOutput {
                    tool_name: call.name.clone(),
                    output_json: format!(
                        "{{\"ok\":true,\"arch\":\"aarch64\",\"uptime_ms\":{},\"heap_used_mb\":{},\"active_tasks\":{},\"net\":\"up\",\"shell\":\"disabled\"}}",
                        self.sys_status.uptime_ms, heap_mb, active_tasks
                    ),
                    synthesized_view: Some(view),
                })
            }
            "script_create" => {
                let raw_path = extract_json_string_field(&call.arguments_json, "path")
                    .ok_or_else(|| AiError::InvalidToolArguments("missing 'path'".to_string()))?;
                let path = ScriptCapabilities::normalize_path(&raw_path);
                let source = extract_json_string_field(&call.arguments_json, "source")
                    .or_else(|| extract_json_string_field(&call.arguments_json, "code"))
                    .ok_or_else(|| AiError::InvalidToolArguments("missing 'source'".to_string()))?;

                parse_program(&source).map_err(|e| {
                    AiError::InvalidToolArguments(format!(
                        "JavaScript syntax error in '{}': {}",
                        path, e
                    ))
                })?;

                self.apply_script_capabilities_from_args(&path, &call.arguments_json);
                let existed = self
                    .write_vfs_raw(&path, &source)
                    .map_err(AiError::InvalidToolArguments)?;
                self.scheduler.register_script_source(&path, &source);

                let ev = if existed {
                    OsEvent::FsModified { path: path.clone() }
                } else {
                    OsEvent::FsCreated { path: path.clone() }
                };
                let _ = self.dispatch_os_event(&ev);

                Ok(ToolExecutionOutput {
                    tool_name: call.name.clone(),
                    output_json: format!(
                        "{{\"ok\":true,\"script_path\":\"{}\",\"status\":\"created\",\"bytes\":{}}}",
                        json_escape(&path),
                        source.len()
                    ),
                    synthesized_view: None,
                })
            }
            "script_run" => {
                let raw_path = extract_json_string_field(&call.arguments_json, "path")
                    .ok_or_else(|| AiError::InvalidToolArguments("missing 'path'".to_string()))?;
                let path = ScriptCapabilities::normalize_path(&raw_path);

                // Ensure script is synced from VFS into ScriptScheduler if present on disk
                if self.scheduler.store().get_script(&path).is_none() {
                    if let Some(src) = self.read_vfs_string(&path) {
                        self.scheduler.register_script_source(&path, &src);
                    }
                }

                let (eval_res, emitted) = self
                    .run_scheduler_step(|sched, host| sched.eval_script_path(&path, host));

                for ev in emitted {
                    let _ = self.dispatch_os_event(&ev);
                }

                match eval_res {
                    Ok((val, steps)) => {
                        let val_json = json_stringify(&val);
                        let val_str = val.to_js_string();
                        Ok(ToolExecutionOutput {
                            tool_name: call.name.clone(),
                            output_json: format!(
                                "{{\"ok\":true,\"script_path\":\"{}\",\"engine\":\"metalos-js\",\"result\":\"executed_ok\",\"value\":{},\"output\":\"{}\",\"steps\":{}}}",
                                json_escape(&path),
                                val_json,
                                json_escape(&val_str),
                                steps
                            ),
                            synthesized_view: None,
                        })
                    }
                    Err(JsError::ForbiddenCapability(msg)) => {
                        if msg.contains("Shell") {
                            Err(AiError::ShellAccessForbidden(msg))
                        } else {
                            Err(AiError::PolicyViolation(msg))
                        }
                    }
                    Err(err) => Ok(ToolExecutionOutput {
                        tool_name: call.name.clone(),
                        output_json: format!(
                            "{{\"ok\":false,\"script_path\":\"{}\",\"engine\":\"metalos-js\",\"result\":\"error\",\"error\":\"{}\"}}",
                            json_escape(&path),
                            json_escape(&err.to_string())
                        ),
                        synthesized_view: None,
                    }),
                }
            }
            "script_schedule" => {
                let raw_path = extract_json_string_field(&call.arguments_json, "path")
                    .ok_or_else(|| AiError::InvalidToolArguments("missing 'path'".to_string()))?;
                let path = ScriptCapabilities::normalize_path(&raw_path);
                let trigger_str = extract_json_string_field(&call.arguments_json, "trigger")
                    .ok_or_else(|| {
                        AiError::InvalidToolArguments("missing 'trigger'".to_string())
                    })?;
                let parsed_trigger = ScheduleTrigger::deserialize(&trigger_str)
                    .map_err(AiError::InvalidToolArguments)?;

                if self.scheduler.store().get_script(&path).is_none() {
                    if let Some(src) = self.read_vfs_string(&path) {
                        self.scheduler.register_script_source(&path, &src);
                    }
                }

                let schedule_id = extract_json_string_field(&call.arguments_json, "id")
                    .unwrap_or_else(|| {
                        let idx = self.scheduler.schedules().len() + 1;
                        let stem = path
                            .rsplit('/')
                            .next()
                            .unwrap_or("job")
                            .trim_end_matches(".js");
                        format!("sched-{}-{}", idx, stem)
                    });

                self.scheduler
                    .schedule_script(&schedule_id, &path, parsed_trigger);
                let conf = self.scheduler.serialize_schedules_conf();
                self.write_vfs_raw(SCHEDULES_CONF_PATH, &conf)
                    .map_err(AiError::InvalidToolArguments)?;
                self.sync_schedule_list();

                let entries = self
                    .scheduler
                    .schedules()
                    .iter()
                    .map(|s| {
                        UiScheduleEntry::new(
                            s.script_path.clone(),
                            s.trigger.serialize(),
                            "scheduled",
                        )
                    })
                    .collect();
                let view = ui_synth.synthesize_schedule_list("Scheduled JS Automations", entries);
                Ok(ToolExecutionOutput {
                    tool_name: call.name.clone(),
                    output_json: format!(
                        "{{\"ok\":true,\"id\":\"{}\",\"script_path\":\"{}\",\"trigger\":\"{}\"}}",
                        json_escape(&schedule_id),
                        json_escape(&path),
                        json_escape(&trigger_str)
                    ),
                    synthesized_view: Some(view),
                })
            }
            "mcp_list_tools" => {
                let apps: Vec<String> = self
                    .mcp_apps
                    .keys()
                    .map(|k| format!("\"{}\"", json_escape(k)))
                    .collect();
                Ok(ToolExecutionOutput {
                    tool_name: call.name.clone(),
                    output_json: format!(
                        "{{\"ok\":true,\"headless_mcp_apps\":[{}],\"ui_policy\":\"agent_exclusive\"}}",
                        apps.join(",")
                    ),
                    synthesized_view: None,
                })
            }
            "mcp_call_tool" => {
                let app = extract_json_string_field(&call.arguments_json, "app")
                    .unwrap_or_else(|| "telemetry.mcp".to_string());
                let tool = extract_json_string_field(&call.arguments_json, "tool")
                    .unwrap_or_else(|| "inspect".to_string());
                let payload = self
                    .mcp_apps
                    .get(&app)
                    .cloned()
                    .unwrap_or_else(|| {
                        format!(
                            "{{\"title\":\"{}\",\"summary\":\"Executed MCP tool {}\",\"value\":\"ok\"}}",
                            json_escape(&app),
                            json_escape(&tool)
                        )
                    });
                let view = ui_synth.synthesize_from_mcp_output(
                    &UiCallerOrigin::AiAgent,
                    &app,
                    &tool,
                    &payload,
                )?;
                Ok(ToolExecutionOutput {
                    tool_name: call.name.clone(),
                    output_json: payload,
                    synthesized_view: Some(view),
                })
            }
            "mcp_read_resource" => {
                let app = extract_json_string_field(&call.arguments_json, "app")
                    .unwrap_or_else(|| "notes.mcp".to_string());
                let uri = extract_json_string_field(&call.arguments_json, "uri")
                    .unwrap_or_else(|| "mcp://notes/index".to_string());
                let payload = self.mcp_apps.get(&app).cloned().unwrap_or_else(|| {
                    format!(
                        "{{\"title\":\"{}\",\"summary\":\"Resource {}\"}}",
                        json_escape(&app),
                        json_escape(&uri)
                    )
                });
                let view = ui_synth.synthesize_from_mcp_output(
                    &UiCallerOrigin::AiAgent,
                    &app,
                    &uri,
                    &payload,
                )?;
                Ok(ToolExecutionOutput {
                    tool_name: call.name.clone(),
                    output_json: payload,
                    synthesized_view: Some(view),
                })
            }
            "ui_render_view" => {
                let kind_str = extract_json_string_field(&call.arguments_json, "kind")
                    .unwrap_or_else(|| "mcp_app_view".to_string());
                let kind = ViewKind::from_str(&kind_str).unwrap_or(ViewKind::McpAppView);
                let title = extract_json_string_field(&call.arguments_json, "title")
                    .unwrap_or_else(|| "Synthesized View".to_string());
                let summary = extract_json_string_field(&call.arguments_json, "summary")
                    .unwrap_or_else(|| "Agent-rendered Ferrofluid surface".to_string());
                let source_app = extract_json_string_field(&call.arguments_json, "source_app");

                let view = ui_synth.synthesize_custom_view(
                    kind,
                    title,
                    summary,
                    source_app,
                    call.arguments_json.clone(),
                );
                let descriptor = view.to_ferrofluid_descriptor();
                Ok(ToolExecutionOutput {
                    tool_name: call.name.clone(),
                    output_json: descriptor,
                    synthesized_view: Some(view),
                })
            }
            other => Err(AiError::UnknownTool(other.to_string())),
        }
    }
}

impl ToolExecutor for NativeToolRegistry {
    fn execute(&mut self, call: &WelderToolCall) -> Result<String, WelderError> {
        let args_json =
            serde_json::to_string(&call.arguments).map_err(|_| WelderError::ProtocolError)?;
        let provider_call = ToolCall::new("welder_tool", call.name.clone(), args_json);
        let mut synth = AgentUiSynthesizer::new();
        match self.execute_tool(&provider_call, &mut synth) {
            Ok(out) => Ok(out.output_json),
            Err(AiError::ShellAccessForbidden(_)) | Err(AiError::PolicyViolation(_)) => {
                Err(WelderError::ToolDenied)
            }
            Err(_) => Err(WelderError::ToolFailed),
        }
    }
}

struct RegistryHostAdapter<'a> {
    registry: &'a mut NativeToolRegistry,
    emitted_events: &'a mut Vec<OsEvent>,
}

impl<'a> MetalOsHost for RegistryHostAdapter<'a> {
    fn check_capability(&self, req: &CapabilityRequest<'_>) -> Result<(), String> {
        self.registry.default_capabilities.check(req)
    }

    fn forge(
        &mut self,
        prompt: &str,
        model: Option<&str>,
        max_tokens: u32,
    ) -> Result<String, String> {
        let Some(forge) = &self.registry.forge else {
            return Err("Forge capability unavailable".to_string());
        };
        let req = GenerationRequest {
            model: model.map(String::from),
            prompt: prompt.to_string(),
            max_tokens,
        };
        let res = forge
            .try_borrow_mut()
            .map_err(|_| "Forge service busy".to_string())?
            .generate(&req)
            .map(|out| out.text)
            .map_err(|e| format!("{}", e));
        res
    }

    fn fs_read(&mut self, path: &str) -> Option<String> {
        self.registry.read_vfs_string(path)
    }

    fn fs_write(&mut self, path: &str, content: &str) -> Result<(), String> {
        let norm = ScriptCapabilities::normalize_path(path);
        let existed = self.registry.write_vfs_raw(&norm, content)?;
        if norm != SCHEDULES_CONF_PATH && norm != "/etc/ai.conf" {
            if existed {
                self.emitted_events.push(OsEvent::FsModified { path: norm });
            } else {
                self.emitted_events.push(OsEvent::FsCreated { path: norm });
            }
        }
        Ok(())
    }

    fn fs_delete(&mut self, path: &str) -> Result<bool, String> {
        let norm = ScriptCapabilities::normalize_path(path);
        let removed = self.registry.delete_vfs_raw(&norm)?;
        if removed {
            self.emitted_events.push(OsEvent::FsDeleted { path: norm });
        }
        Ok(removed)
    }

    fn fs_list(&mut self, dir_path: &str) -> Vec<String> {
        self.registry.list_vfs_dir_child_names(dir_path)
    }

    fn net_fetch(&mut self, url: &str) -> Result<String, String> {
        if let Some(resp) = self.registry.http_routes.get(url) {
            Ok(resp.clone())
        } else {
            Err(format!("HTTP 404 Not Found: {}", url))
        }
    }

    fn sys_status(&self) -> SystemStatus {
        self.registry.sys_status
    }

    fn ui_notify(&mut self, message: &str) {
        self.registry.notifications.push(message.to_string());
    }

    fn log(&mut self, message: &str) {
        self.registry.logs.push(message.to_string());
    }

    fn drain_emitted_events(&mut self) -> Vec<OsEvent> {
        core::mem::take(self.emitted_events)
    }
}
