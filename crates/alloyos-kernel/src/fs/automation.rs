//! Kernel VFS / CobaltFS bridge for Phase 6 JavaScript Automation & Scheduling (`alloyos-js` + `alloyos-welder`).
//!
//! Connects `NativeToolRegistry`, `AgentRuntime`, and `ScriptScheduler` to the mounted kernel VFS
//! (`CobaltFS` on VirtIO/ATA block storage or `RamFS`), enabling durable `/scripts/*.js` and
//! `/etc/schedules.conf` persistence across reboots and live capability-scoped host bindings.

use alloc::format;
use alloc::rc::Rc;
use alloc::string::{String, ToString};
use alloc::sync::Arc;
use alloc::vec;
use alloc::vec::Vec;
use core::cell::RefCell;
use spin::Mutex;

use crate::fs::vfs::{FsError, InodeKind, Vfs, VFS};
use alloyos_forge::{Forge, GenerationRequest};
use alloyos_js::{
    CapabilityRequest, AlloyOsHost, OsEvent, ScriptCapabilities, ScriptExecutionReport,
    SystemStatus, SCHEDULES_CONF_PATH,
};
use alloyos_welder::{AgentRuntime, ToolVfsBackend};

#[derive(Clone)]
enum VfsTarget {
    Global,
    Instance(Arc<Mutex<Vfs>>),
}

impl core::fmt::Debug for VfsTarget {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            VfsTarget::Global => write!(f, "VfsTarget::Global"),
            VfsTarget::Instance(_) => write!(f, "VfsTarget::Instance(..)"),
        }
    }
}

/// Kernel VFS / CobaltFS adapter implementing both `alloyos_welder::ToolVfsBackend` and
/// `alloyos_js::AlloyOsHost` with capability enforcement.
pub struct KernelVfsBackend {
    target: VfsTarget,
    capabilities: ScriptCapabilities,
    uptime_ms: u64,
    active_tasks: u32,
    emitted_events: Vec<OsEvent>,
    notifications: Vec<String>,
    logs: Vec<String>,
    forge: Option<Rc<RefCell<Forge>>>,
}

impl core::fmt::Debug for KernelVfsBackend {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("KernelVfsBackend")
            .field("target", &self.target)
            .field("capabilities", &self.capabilities)
            .field("uptime_ms", &self.uptime_ms)
            .field("active_tasks", &self.active_tasks)
            .finish()
    }
}

impl KernelVfsBackend {
    /// Creates a `KernelVfsBackend` backed by the global kernel `VFS` static.
    pub fn global() -> Self {
        Self {
            target: VfsTarget::Global,
            capabilities: ScriptCapabilities::default_automation(),
            uptime_ms: 0,
            active_tasks: 4,
            emitted_events: Vec::new(),
            notifications: Vec::new(),
            logs: Vec::new(),
            forge: None,
        }
    }

    /// Creates a shared `Rc<RefCell<KernelVfsBackend>>` backed by the global kernel `VFS` static.
    pub fn global_rc() -> Rc<RefCell<Self>> {
        Rc::new(RefCell::new(Self::global()))
    }

    /// Creates a `KernelVfsBackend` backed by a specific `Arc<Mutex<Vfs>>` instance
    /// (useful for deterministic CobaltFS mount/remount persistence tests).
    pub fn from_vfs(vfs: Arc<Mutex<Vfs>>) -> Self {
        Self {
            target: VfsTarget::Instance(vfs),
            capabilities: ScriptCapabilities::default_automation(),
            uptime_ms: 0,
            active_tasks: 4,
            emitted_events: Vec::new(),
            notifications: Vec::new(),
            logs: Vec::new(),
            forge: None,
        }
    }

    /// Creates a shared `Rc<RefCell<KernelVfsBackend>>` backed by a specific `Arc<Mutex<Vfs>>`.
    pub fn from_vfs_rc(vfs: Arc<Mutex<Vfs>>) -> Rc<RefCell<Self>> {
        Rc::new(RefCell::new(Self::from_vfs(vfs)))
    }

    pub fn with_capabilities(mut self, capabilities: ScriptCapabilities) -> Self {
        self.capabilities = capabilities;
        self
    }

    pub fn set_capabilities(&mut self, capabilities: ScriptCapabilities) {
        self.capabilities = capabilities;
    }

    pub fn set_forge(&mut self, forge: Rc<RefCell<Forge>>) {
        self.forge = Some(forge);
    }

    pub fn set_uptime_ms(&mut self, uptime_ms: u64) {
        self.uptime_ms = uptime_ms;
    }

    pub fn notifications(&self) -> &[String] {
        &self.notifications
    }

    pub fn logs(&self) -> &[String] {
        &self.logs
    }

    fn with_vfs<R, F: FnOnce(&Vfs) -> R>(&self, f: F) -> Option<R> {
        match &self.target {
            VfsTarget::Global => {
                let guard = VFS.lock();
                guard.as_ref().map(f)
            }
            VfsTarget::Instance(vfs_arc) => {
                let guard = vfs_arc.lock();
                Some(f(&guard))
            }
        }
    }

    fn ensure_parent_dirs(vfs: &Vfs, norm_path: &str) {
        let mut current = String::new();
        let parts: Vec<&str> = norm_path
            .trim_start_matches('/')
            .split('/')
            .filter(|s| !s.is_empty())
            .collect();
        if parts.len() <= 1 {
            return;
        }
        for dir_part in &parts[..parts.len() - 1] {
            current.push('/');
            current.push_str(dir_part);
            if vfs.lookup(&current).is_err() {
                let _ = vfs.create(&current, InodeKind::Directory);
            }
        }
    }

    fn collect_files_recursive(vfs: &Vfs, dir_path: &str, out: &mut Vec<String>, depth: usize) {
        if depth > 8 {
            return;
        }
        let Ok(entries) = vfs.readdir(dir_path) else {
            return;
        };
        let prefix = if dir_path == "/" {
            String::new()
        } else {
            dir_path.trim_end_matches('/').to_string()
        };

        for entry in entries {
            if entry == "." || entry == ".." || entry == "./" || entry == "../" {
                continue;
            }
            let is_dir = entry.ends_with('/');
            let clean_name = entry.trim_end_matches('/');
            if clean_name.is_empty() {
                continue;
            }
            let full_path = format!("{}/{}", prefix, clean_name);
            if full_path.starts_with("/dev") || full_path.starts_with("/proc") {
                continue;
            }
            if is_dir {
                Self::collect_files_recursive(vfs, &full_path, out, depth + 1);
            } else {
                out.push(full_path);
            }
        }
    }

    pub fn current_system_status(&self) -> SystemStatus {
        let (total_f, used_f, free_f) = crate::mm::frame_allocator::memory_stats();
        let (mem_used_bytes, mem_free_bytes) = if total_f > 0 {
            ((used_f as u64) * 4096, (free_f as u64) * 4096)
        } else {
            (12 * 1024 * 1024, 116 * 1024 * 1024)
        };
        SystemStatus {
            uptime_ms: self.uptime_ms,
            mem_used_bytes,
            mem_free_bytes,
            active_tasks: self.active_tasks,
        }
    }
}

impl ToolVfsBackend for KernelVfsBackend {
    fn package_snapshot(&mut self) -> Result<alloyos_mcp::PackageSnapshot, String> {
        self.with_vfs(super::packages::snapshot)
            .unwrap_or_else(|| Err("VFS unavailable".into()))
    }

    fn read_file(&mut self, path: &str) -> Option<String> {
        let norm = ScriptCapabilities::normalize_path(path);
        self.with_vfs(|vfs| {
            let inode = vfs.lookup(&norm).ok()?;
            if inode.kind != InodeKind::File {
                return None;
            }
            if inode.size == 0 {
                return Some(String::new());
            }
            let mut buf = vec![0u8; inode.size];
            let n = vfs.root_fs.read(inode.ino, 0, &mut buf).ok()?;
            buf.truncate(n);
            String::from_utf8(buf).ok()
        })
        .flatten()
    }

    fn write_file(&mut self, path: &str, content: &str) -> Result<(), String> {
        let norm = ScriptCapabilities::normalize_path(path);
        self.with_vfs(|vfs| {
            Self::ensure_parent_dirs(vfs, &norm);
            // If file already exists, unlink first so CobaltFS truncates to exact new length
            if let Ok(existing) = vfs.lookup(&norm) {
                if existing.kind == InodeKind::Directory {
                    return Err(format!("Path is a directory: {}", norm));
                }
                let _ = vfs.unlink(&norm);
            }
            let inode = vfs
                .create(&norm, InodeKind::File)
                .map_err(|e| format!("VFS create '{}' failed: {:?}", norm, e))?;
            if !content.is_empty() {
                vfs.root_fs
                    .write(inode.ino, 0, content.as_bytes())
                    .map_err(|e| format!("VFS write '{}' failed: {:?}", norm, e))?;
            }
            Ok(())
        })
        .unwrap_or_else(|| Err("Kernel VFS not initialized".to_string()))
    }

    fn delete_file(&mut self, path: &str) -> Result<bool, String> {
        let norm = ScriptCapabilities::normalize_path(path);
        self.with_vfs(|vfs| match vfs.unlink(&norm) {
            Ok(()) => Ok(true),
            Err(FsError::NotFound) => Ok(false),
            Err(e) => Err(format!("VFS unlink '{}' failed: {:?}", norm, e)),
        })
        .unwrap_or_else(|| Err("Kernel VFS not initialized".to_string()))
    }

    fn list_dir(&mut self, dir_path: &str) -> Vec<String> {
        let norm = ScriptCapabilities::normalize_path(dir_path);
        self.with_vfs(|vfs| {
            let mut out = Vec::new();
            Self::collect_files_recursive(vfs, &norm, &mut out, 0);
            out
        })
        .unwrap_or_default()
    }

    fn is_persistent(&self) -> bool {
        self.with_vfs(|vfs| vfs.backend_name == "CobaltFS")
            .unwrap_or(false)
    }
}

impl AlloyOsHost for KernelVfsBackend {
    fn check_capability(&self, req: &CapabilityRequest<'_>) -> Result<(), String> {
        self.capabilities.check(req)
    }

    fn forge(
        &mut self,
        prompt: &str,
        model: Option<&str>,
        max_tokens: u32,
    ) -> Result<String, String> {
        let Some(forge) = &self.forge else {
            return Err("Forge capability unavailable".to_string());
        };
        let req = GenerationRequest {
            model: model.map(String::from),
            prompt: prompt.to_string(),
            max_tokens,
        };
        forge
            .try_borrow_mut()
            .map_err(|_| "Forge service busy".to_string())?
            .generate(&req)
            .map(|out| out.text)
            .map_err(|e| format!("{}", e))
    }

    fn fs_read(&mut self, path: &str) -> Option<String> {
        self.read_file(path)
    }

    fn fs_write(&mut self, path: &str, content: &str) -> Result<(), String> {
        let norm = ScriptCapabilities::normalize_path(path);
        if norm == "/knowledge" || norm.starts_with("/knowledge/") {
            return Err(format!(
                "Security violation: scripts and apps cannot directly write to user knowledge repository '{}'; profile updates are owned by Welder",
                norm
            ));
        }
        let existed = self.read_file(&norm).is_some();
        self.write_file(&norm, content)?;
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
        if norm == "/knowledge" || norm.starts_with("/knowledge/") {
            return Err(format!(
                "Security violation: scripts and apps cannot directly delete user knowledge repository '{}'; profile updates are owned by Welder",
                norm
            ));
        }
        let removed = self.delete_file(&norm)?;
        if removed {
            self.emitted_events.push(OsEvent::FsDeleted { path: norm });
        }
        Ok(removed)
    }

    fn fs_list(&mut self, dir_path: &str) -> Vec<String> {
        let norm = ScriptCapabilities::normalize_path(dir_path);
        let prefix = if norm == "/" {
            "/".to_string()
        } else {
            format!("{}/", norm)
        };
        let mut names: Vec<String> = self
            .list_dir(&norm)
            .into_iter()
            .filter_map(|full| {
                let rel = full.strip_prefix(&prefix)?;
                if !rel.is_empty() && !rel.contains('/') {
                    Some(rel.to_string())
                } else {
                    None
                }
            })
            .collect();
        names.sort();
        names.dedup();
        names
    }

    fn net_fetch(&mut self, url: &str) -> Result<String, String> {
        Err(format!("Kernel net_fetch offline for URL: {}", url))
    }

    fn sys_status(&self) -> SystemStatus {
        self.current_system_status()
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

/// Initializes a kernel-backed `AgentRuntime` attached to the global `VFS` (CobaltFS or RamFS),
/// wires the resident `Forge` instance for both conversational turns and `AlloyOS.forge(...)`,
/// restores persisted `/etc/ai.conf`, `/scripts/*.js`, and `/etc/schedules.conf`, and dispatches
/// `OsEvent::SystemBoot` + `OsEvent::NetInterfaceUp`.
pub fn init_kernel_agent_runtime(forge: Rc<RefCell<Forge>>) -> AgentRuntime {
    let backend = KernelVfsBackend::global_rc();
    backend.borrow_mut().set_forge(forge.clone());
    let mut runtime = AgentRuntime::with_vfs_backend(backend);
    runtime.set_forge(forge);
    let _ = runtime.notify_system_boot();
    let _ = runtime.notify_network_online();
    runtime
}

/// Advances the kernel JS automation scheduler on `AgentRuntime` to `uptime_ms`, refreshing live
/// kernel frame-allocator memory telemetry first.
pub fn tick_kernel_automations(
    agent: &mut AgentRuntime,
    uptime_ms: u64,
) -> Vec<ScriptExecutionReport> {
    let (total_f, used_f, free_f) = crate::mm::frame_allocator::memory_stats();
    let (mem_used_bytes, mem_free_bytes) = if total_f > 0 {
        ((used_f as u64) * 4096, (free_f as u64) * 4096)
    } else {
        (12 * 1024 * 1024, 116 * 1024 * 1024)
    };
    agent.tool_registry.set_system_status(SystemStatus {
        uptime_ms,
        mem_used_bytes,
        mem_free_bytes,
        active_tasks: 4,
    });
    agent.tick_scheduler(uptime_ms, None)
}
