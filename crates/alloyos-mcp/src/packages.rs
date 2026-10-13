//! OS-owned package inventory. Snapshots must be captured consistently by the VFS
//! owner; this service never reads mutable files piecemeal or executes app code.
use alloc::{collections::BTreeMap, format, string::{String, ToString}, vec::Vec};
use alloyos_js::{parse_program, validate_app_handlers, AppHandler};
use crate::sandbox::{validate_app_manifest, AppCapability, AppManifest};

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PackageSnapshot {
    /// Absolute paths for every file below /apps/, including nested assets.
    pub files: BTreeMap<String, String>,
    /// Absolute directory paths, including empty package roots.
    pub directories: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PackageDiagnostic {
    pub path: String,
    pub message: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PackageStatus {
    pub app_id: String,
    pub installed_version: Option<String>,
    pub revision: u64,
    pub validated_revision: Option<u64>,
    pub ready: bool,
    pub diagnostics: Vec<PackageDiagnostic>,
}

/// Issued only by validation. Execution owners must bind the loaded app to this
/// exact revision; a previously valid token does not authorize a newer package.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ValidatedPackageRevision {
    app_id: String,
    revision: u64,
}
impl ValidatedPackageRevision {
    pub fn app_id(&self) -> &str { &self.app_id }
    pub fn revision(&self) -> u64 { self.revision }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PackageChange {
    Updated(PackageStatus),
    Removed { app_id: String, revision: u64 },
}

#[derive(Clone, Debug, Default)]
pub struct PackageService {
    files: BTreeMap<String, BTreeMap<String, String>>,
    directories: BTreeMap<String, Vec<String>>,
    statuses: BTreeMap<String, PackageStatus>,
    revision: u64,
    dirty_at_ms: Option<u64>,
}
impl PackageService {
    pub const COALESCE_MS: u64 = 100;
    pub fn new() -> Self { Self::default() }
    pub fn inventory(&self) -> Vec<&PackageStatus> { self.statuses.values().collect() }
    pub fn status(&self, app_id: &str) -> Option<&PackageStatus> { self.statuses.get(app_id) }
    pub fn validated_revision(&self, app_id: &str) -> Option<ValidatedPackageRevision> {
        let status = self.status(app_id)?;
        status.validated_revision.map(|revision| ValidatedPackageRevision { app_id: app_id.into(), revision })
    }
    pub fn is_current(&self, revision: &ValidatedPackageRevision) -> bool {
        self.validated_revision(revision.app_id()).as_ref() == Some(revision)
    }
    pub fn scan_startup(&mut self, snapshot: PackageSnapshot) -> Vec<PackageChange> {
        self.apply_snapshot(snapshot)
    }
    /// Reset the quiet period for any recursive package write/create/delete.
    pub fn mark_dirty(&mut self, now_ms: u64) { self.dirty_at_ms = Some(now_ms); }
    pub fn ready_to_scan(&self, now_ms: u64) -> bool {
        self.dirty_at_ms.map(|at| now_ms.saturating_sub(at) >= Self::COALESCE_MS).unwrap_or(false)
    }
    /// None means the write burst is still settling; capture snapshot only once ready.
    pub fn poll_snapshot(&mut self, now_ms: u64, snapshot: PackageSnapshot) -> Option<Vec<PackageChange>> {
        if !self.ready_to_scan(now_ms) { return None; }
        self.dirty_at_ms = None;
        Some(self.apply_snapshot(snapshot))
    }
    pub fn apply_snapshot(&mut self, snapshot: PackageSnapshot) -> Vec<PackageChange> {
        let mut packages: BTreeMap<String, BTreeMap<String, String>> = BTreeMap::new();
        let mut directories: BTreeMap<String, Vec<String>> = BTreeMap::new();
        for path in snapshot.directories {
            if let Some(rest) = path.strip_prefix("/apps/") {
                let id = rest.trim_end_matches('/').split('/').next().unwrap_or("");
                if !id.is_empty() {
                    packages.entry(id.into()).or_default();
                    directories.entry(id.into()).or_default().push(path);
                }
            }
        }
        for (path, contents) in snapshot.files {
            if let Some(rest) = path.strip_prefix("/apps/") {
                if let Some((id, suffix)) = rest.split_once('/') {
                    if !id.is_empty() && !suffix.is_empty() {
                        packages.entry(id.into()).or_default().insert(path, contents);
                    }
                }
            }
        }
        for paths in directories.values_mut() { paths.sort(); paths.dedup(); }
        if packages == self.files && directories == self.directories { return Vec::new(); }
        self.revision += 1;
        let mut changes = Vec::new();
        for id in self.files.keys().filter(|id| !packages.contains_key(*id)) {
            self.statuses.remove(id);
            changes.push(PackageChange::Removed { app_id: id.clone(), revision: self.revision });
        }
        for (id, files) in &packages {
            if self.files.get(id) == Some(files) && self.directories.get(id) == directories.get(id) { continue; }
            let status = validate_package(id, files, self.revision);
            self.statuses.insert(id.clone(), status.clone());
            changes.push(PackageChange::Updated(status));
        }
        self.files = packages;
        self.directories = directories;
        changes
    }
}

fn validate_package(id: &str, files: &BTreeMap<String, String>, revision: u64) -> PackageStatus {
    let mut status = PackageStatus { app_id: id.into(), installed_version: None, revision,
        validated_revision: None, ready: false, diagnostics: Vec::new() };
    let root = format!("/apps/{}/", id);
    for path in files.keys() {
        if path.split('/').any(|part| part == "." || part == ".." || part.contains('\\')) {
            status.diagnostics.push(PackageDiagnostic { path: path.clone(), message: "noncanonical package path".into() });
        }
    }
    let manifest_path = format!("{}{}.mcp", root, id);
    let manifest = match files.get(&manifest_path) {
        None => { status.diagnostics.push(PackageDiagnostic { path: manifest_path, message: "missing package manifest".into() }); return status; }
        Some(text) => match AppManifest::parse_in_folder(id, text) {
            Ok(manifest) => manifest,
            Err(error) => { status.diagnostics.push(PackageDiagnostic { path: manifest_path, message: error.to_mcp_error().message }); return status; }
        }
    };
    status.installed_version = Some(manifest.version.clone());
    if manifest.app_id != id {
        status.diagnostics.push(PackageDiagnostic { path: manifest_path.clone(), message: "manifest app_id must match package folder".into() });
    }
    if let Err(error) = validate_app_manifest(&manifest) {
        status.diagnostics.push(PackageDiagnostic { path: manifest_path.clone(), message: error.to_mcp_error().message });
    }
    let entry = manifest.entrypoint.clone().unwrap_or_else(|| format!("{}main.js", root));
    if !entry.starts_with(&root) {
        status.diagnostics.push(PackageDiagnostic { path: manifest_path, message: "entrypoint must remain inside the package".into() });
    }
    let mut entry_parsed = false;
    // Parse every JS file, not just main.js: nested modules are part of this revision.
    for (path, source) in files.iter().filter(|(path, _)| path.ends_with(".js")) {
        if source.len() > alloyos_js::app::MAX_APP_SOURCE_BYTES {
            status.diagnostics.push(PackageDiagnostic { path: path.clone(), message: "JavaScript source exceeds 256 KiB".into() });
            continue;
        }
        match parse_program(source) {
            Ok(_) => { if path == &entry { entry_parsed = true; } }
            Err(error) => status.diagnostics.push(PackageDiagnostic { path: path.clone(), message: error.to_string() }),
        }
    }
    if !files.contains_key(&entry) {
        status.diagnostics.push(PackageDiagnostic { path: entry.clone(), message: "missing JavaScript entrypoint".into() });
    }
    if entry_parsed {
        let required: Vec<_> = [
            (AppCapability::ExposeMcpTools, AppHandler::CallTool),
            (AppCapability::ExposeMcpResources, AppHandler::ReadResource),
            (AppCapability::ExposeMcpPrompts, AppHandler::GetPrompt),
            (AppCapability::ExposeTiles, AppHandler::RenderTile),
        ].into_iter().filter_map(|(capability, handler)| manifest.has_capability(&capability).then_some(handler)).collect();
        if let Err(error) = validate_app_handlers(&files[&entry], &required) {
            status.diagnostics.push(PackageDiagnostic { path: entry.clone(), message: error.to_string() });
        }
    }
    if manifest.has_capability(&AppCapability::ExposeSkills) && !files.contains_key(&format!("{}SKILL.md", root)) {
        status.diagnostics.push(PackageDiagnostic { path: format!("{}SKILL.md", root), message: "missing declared skill file".into() });
    }
    status.ready = status.diagnostics.is_empty();
    if status.ready { status.validated_revision = Some(revision); }
    status
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::boxed::Box;
    use crate::{ClockApp, McpRegistry};

    fn sample() -> PackageSnapshot {
        let mut files = BTreeMap::new();
        files.insert("/apps/clock/clock.mcp".into(), "name=Clock\nversion=1.0.0\ncapabilities=ExposeMcpTools\n".into());
        files.insert("/apps/clock/main.js".into(), "function callTool(name,args) { return '{}'; }".into());
        PackageSnapshot { files, ..PackageSnapshot::default() }
    }

    #[test]
    fn empty_package_directory_is_visible_with_missing_manifest() {
        let mut service = PackageService::new();
        service.apply_snapshot(PackageSnapshot { directories: alloc::vec!["/apps/empty".into()], ..PackageSnapshot::default() });
        let status = service.status("empty").unwrap();
        assert!(!status.ready);
        assert_eq!(status.diagnostics[0].path, "/apps/empty/empty.mcp");
    }

    #[test]
    fn nested_empty_directory_edits_advance_revision() {
        let mut service = PackageService::new();
        let mut snapshot = sample();
        service.apply_snapshot(snapshot.clone());
        let first = service.validated_revision("clock").unwrap();
        snapshot.directories.push("/apps/clock/nested/empty".into());
        assert_eq!(service.apply_snapshot(snapshot.clone()).len(), 1);
        assert!(!service.is_current(&first));
        let second = service.validated_revision("clock").unwrap();
        snapshot.directories.clear();
        assert_eq!(service.apply_snapshot(snapshot).len(), 1);
        assert!(!service.is_current(&second));
    }

    #[test]
    fn canonical_tasks_package_validates() {
        let files = BTreeMap::from([
            (crate::TASKS_MANIFEST_PATH.to_string(), crate::TASKS_MANIFEST_MCP.to_string()),
            (crate::TASKS_MAIN_PATH.to_string(), crate::TASKS_MAIN_JS.to_string()),
            (crate::TASKS_SKILL_PATH.to_string(), crate::TASKS_SKILL_MD.to_string()),
        ]);
        let mut service = PackageService::new();
        service.scan_startup(PackageSnapshot { files, ..PackageSnapshot::default() });
        let status = service.status("tasks").unwrap();
        assert!(status.ready, "{:?}", status.diagnostics);
    }

    #[test]
    fn consistent_snapshots_detect_nested_create_edit_delete_and_package_removal() {
        let mut service = PackageService::new();
        let mut snapshot = sample();
        assert_eq!(service.scan_startup(snapshot.clone()).len(), 1);
        let first = service.validated_revision("clock").unwrap();
        assert!(service.apply_snapshot(snapshot.clone()).is_empty());
        snapshot.files.insert("/apps/clock/assets/icons/a.txt".into(), "first".into());
        assert_eq!(service.apply_snapshot(snapshot.clone()).len(), 1);
        assert!(!service.is_current(&first));
        let added = service.validated_revision("clock").unwrap();
        snapshot.files.insert("/apps/clock/assets/icons/a.txt".into(), "second".into());
        service.apply_snapshot(snapshot.clone());
        assert!(!service.is_current(&added));
        snapshot.files.remove("/apps/clock/assets/icons/a.txt");
        assert_eq!(service.apply_snapshot(snapshot).len(), 1);
        assert!(matches!(&service.apply_snapshot(PackageSnapshot::default())[..], [PackageChange::Removed { app_id, .. }] if app_id == "clock"));
        assert!(service.inventory().is_empty());
    }

    #[test]
    fn partial_write_blocks_revision_and_fix_recovers_with_diagnostic_file() {
        let mut service = PackageService::new();
        let mut snapshot = sample();
        service.apply_snapshot(snapshot.clone());
        let previous = service.validated_revision("clock").unwrap();
        snapshot.files.insert("/apps/clock/main.js".into(), "function callTool(".into());
        service.apply_snapshot(snapshot);
        let status = service.status("clock").unwrap();
        assert!(!status.ready);
        assert!(status.diagnostics.iter().any(|error| error.path == "/apps/clock/main.js"));
        assert!(service.validated_revision("clock").is_none());
        assert!(!service.is_current(&previous));
        service.apply_snapshot(sample());
        assert!(service.status("clock").unwrap().ready);
        assert!(!service.is_current(&previous));
    }

    #[test]
    fn watcher_coalesces_write_bursts_until_quiet() {
        let mut service = PackageService::new();
        service.mark_dirty(10);
        service.mark_dirty(80);
        assert!(service.poll_snapshot(150, sample()).is_none());
        assert_eq!(service.poll_snapshot(180, sample()).unwrap().len(), 1);
        assert!(!service.ready_to_scan(999));
        service.mark_dirty(200);
        assert!(service.poll_snapshot(300, sample()).unwrap().is_empty());
    }

    #[test]
    fn missing_layout_handlers_and_permissions_remain_visible_invalid() {
        let mut service = PackageService::new();
        let mut snapshot = sample();
        snapshot.files.insert("/apps/clock/main.js".into(), "// function callTool(name,args) {}\nfunction other() {}".into());
        service.apply_snapshot(snapshot.clone());
        assert!(!service.status("clock").unwrap().ready);
        snapshot.files.insert("/apps/clock/clock.mcp".into(), "name=Clock\nversion=1\ncapabilities=ExposeMcpTools,ScriptSchedulerAccess\n".into());
        service.apply_snapshot(snapshot.clone());
        assert!(service.status("clock").unwrap().diagnostics.iter().any(|error| error.path.ends_with("clock.mcp")));
        snapshot.files.remove("/apps/clock/clock.mcp");
        service.apply_snapshot(snapshot);
        assert!(service.status("clock").unwrap().diagnostics[0].message.contains("missing"));
    }

    #[test]
    fn registry_blocks_unbound_changed_invalid_and_deleted_revisions() {
        let mut registry = McpRegistry::new();
        registry.register_app(Box::new(ClockApp::new())).unwrap();
        let mut snapshot = sample();
        // Match the registered built-in executor's installed version.
        let version = registry.manifests()[0].version.clone();
        snapshot.files.insert("/apps/clock/clock.mcp".into(), format!("name=Clock\nversion={}\ncapabilities=ExposeMcpTools\n", version));
        let mut service = PackageService::new();
        service.apply_snapshot(snapshot.clone());
        registry.sync_package_inventory(&service);
        let args = BTreeMap::new();
        assert!(registry.call_tool("clock.now", &args).is_err());
        registry.bind_package_revision(service.validated_revision("clock").unwrap()).unwrap();
        assert!(registry.call_tool("clock.now", &args).is_ok());
        snapshot.files.insert("/apps/clock/notes.txt".into(), "changed".into());
        service.apply_snapshot(snapshot.clone());
        registry.sync_package_inventory(&service);
        assert!(registry.call_tool("clock.now", &args).is_err());
        registry.bind_package_revision(service.validated_revision("clock").unwrap()).unwrap();
        snapshot.files.insert("/apps/clock/main.js".into(), "function broken(".into());
        service.apply_snapshot(snapshot);
        registry.sync_package_inventory(&service);
        assert!(registry.call_tool("clock.now", &args).is_err());
        service.apply_snapshot(PackageSnapshot::default());
        registry.sync_package_inventory(&service);
        assert!(registry.call_tool("clock.now", &args).is_err());
    }
}
