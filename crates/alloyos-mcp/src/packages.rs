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
    let entry = format!("{}main.js", root);
    let mut entry_parsed = false;
    // Remote packages may omit main.js, but every supplied module still has the
    // same parse and size limits as a local package.
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
    if manifest.is_remote() {
        if let Err(error) = crate::RemoteMcpApp::from_package_files(id, files) {
            let message = error.to_mcp_error().message;
            let file = if message.contains("resources.json") { "resources.json" } else if message.contains("prompts.json") { "prompts.json" } else { "tools.json" };
            status.diagnostics.push(PackageDiagnostic { path: format!("{}{}", root, file), message });
        }
    } else {
        if !files.contains_key(&entry) {
            status.diagnostics.push(PackageDiagnostic { path: entry.clone(), message: "missing required main.js".into() });
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
    use crate::{JsApp, McpRegistry};

    fn sample() -> PackageSnapshot {
        let mut files = BTreeMap::new();
        files.insert("/apps/sample_timer/sample_timer.mcp".into(), "name=Sample Timer\nversion=1.0.0\ncapabilities=ExposeMcpTools\n".into());
        files.insert("/apps/sample_timer/main.js".into(), "function callTool(name,args) { return '{}'; }".into());
        PackageSnapshot { files, ..PackageSnapshot::default() }
    }

    #[test]
    fn extreme_js_packages_are_rejected_and_valid_packages_recover() {
        let mut service=PackageService::new();
        let expressions=[format!("{}true","!".repeat(10000)),format!("{}1{}","(".repeat(1000),")".repeat(1000)),format!("{}1","1+".repeat(4000)),format!("a{}","()".repeat(1000)),format!("{}1","function() { return 1; } + ".repeat(200))];
        for expression in expressions {
            let mut snapshot=sample();
            snapshot.files.insert("/apps/sample_timer/main.js".into(),format!("function callTool(name,args) {{ return {}; }}",expression));
            service.apply_snapshot(snapshot);
            assert!(!service.status("sample_timer").unwrap().ready);
            service.apply_snapshot(sample());
            assert!(service.status("sample_timer").unwrap().ready);
        }
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
        let first = service.validated_revision("sample_timer").unwrap();
        snapshot.directories.push("/apps/sample_timer/nested/empty".into());
        assert_eq!(service.apply_snapshot(snapshot.clone()).len(), 1);
        assert!(!service.is_current(&first));
        let second = service.validated_revision("sample_timer").unwrap();
        snapshot.directories.clear();
        assert_eq!(service.apply_snapshot(snapshot).len(), 1);
        assert!(!service.is_current(&second));
    }

    #[test]
    fn legacy_entrypoint_cannot_replace_main_js() {
        let mut service = PackageService::new();
        let mut snapshot = sample();
        snapshot.files.get_mut("/apps/sample_timer/sample_timer.mcp").unwrap()
            .push_str("entrypoint = /apps/sample_timer/other.js\n");
        snapshot.files.insert("/apps/sample_timer/other.js".into(), "function other() {}".into());
        service.apply_snapshot(snapshot.clone());
        assert!(service.status("sample_timer").unwrap().ready);

        let main = snapshot.files.remove("/apps/sample_timer/main.js").unwrap();
        snapshot.files.insert("/apps/sample_timer/other.js".into(), main);
        service.apply_snapshot(snapshot);
        let status = service.status("sample_timer").unwrap();
        assert!(!status.ready);
        assert!(status.diagnostics.iter().any(|error|
            error.path == "/apps/sample_timer/main.js" && error.message == "missing required main.js"));
    }

    #[test]
    fn filesystem_fixture_package_validates() {
        let files = BTreeMap::from([
            ("/apps/sample_scheduler/sample_scheduler.mcp".to_string(), include_str!("../../../tests/fixtures/apps/sample_scheduler/sample_scheduler.mcp").to_string()),
            ("/apps/sample_scheduler/main.js".to_string(), include_str!("../../../tests/fixtures/apps/sample_scheduler/main.js").to_string()),
            ("/apps/sample_scheduler/SKILL.md".to_string(), include_str!("../../../tests/fixtures/apps/sample_scheduler/SKILL.md").to_string()),
        ]);
        let mut service = PackageService::new();
        service.scan_startup(PackageSnapshot { files, ..PackageSnapshot::default() });
        let status = service.status("sample_scheduler").unwrap();
        assert!(status.ready, "{:?}", status.diagnostics);
    }

    #[test]
    fn consistent_snapshots_detect_nested_create_edit_delete_and_package_removal() {
        let mut service = PackageService::new();
        let mut snapshot = sample();
        assert_eq!(service.scan_startup(snapshot.clone()).len(), 1);
        let first = service.validated_revision("sample_timer").unwrap();
        assert!(service.apply_snapshot(snapshot.clone()).is_empty());
        snapshot.files.insert("/apps/sample_timer/assets/icons/a.txt".into(), "first".into());
        assert_eq!(service.apply_snapshot(snapshot.clone()).len(), 1);
        assert!(!service.is_current(&first));
        let added = service.validated_revision("sample_timer").unwrap();
        snapshot.files.insert("/apps/sample_timer/assets/icons/a.txt".into(), "second".into());
        service.apply_snapshot(snapshot.clone());
        assert!(!service.is_current(&added));
        snapshot.files.remove("/apps/sample_timer/assets/icons/a.txt");
        assert_eq!(service.apply_snapshot(snapshot).len(), 1);
        assert!(matches!(&service.apply_snapshot(PackageSnapshot::default())[..], [PackageChange::Removed { app_id, .. }] if app_id == "sample_timer"));
        assert!(service.inventory().is_empty());
    }

    #[test]
    fn partial_write_blocks_revision_and_fix_recovers_with_diagnostic_file() {
        let mut service = PackageService::new();
        let mut snapshot = sample();
        service.apply_snapshot(snapshot.clone());
        let previous = service.validated_revision("sample_timer").unwrap();
        snapshot.files.insert("/apps/sample_timer/main.js".into(), "function callTool(".into());
        service.apply_snapshot(snapshot);
        let status = service.status("sample_timer").unwrap();
        assert!(!status.ready);
        assert!(status.diagnostics.iter().any(|error| error.path == "/apps/sample_timer/main.js"));
        assert!(service.validated_revision("sample_timer").is_none());
        assert!(!service.is_current(&previous));
        service.apply_snapshot(sample());
        assert!(service.status("sample_timer").unwrap().ready);
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
        snapshot.files.insert("/apps/sample_timer/main.js".into(), "// function callTool(name,args) {}\nfunction other() {}".into());
        service.apply_snapshot(snapshot.clone());
        assert!(!service.status("sample_timer").unwrap().ready);
        snapshot.files.insert("/apps/sample_timer/sample_timer.mcp".into(), "name=Sample Timer\nversion=1\ncapabilities=ExposeMcpTools,CreateWindow\n".into());
        service.apply_snapshot(snapshot.clone());
        assert!(service.status("sample_timer").unwrap().diagnostics.iter().any(|error| error.path.ends_with("sample_timer.mcp")));
        snapshot.files.remove("/apps/sample_timer/sample_timer.mcp");
        service.apply_snapshot(snapshot);
        assert!(service.status("sample_timer").unwrap().diagnostics[0].message.contains("missing"));
    }

    #[test]
    fn registry_blocks_unbound_changed_invalid_and_deleted_revisions() {
        let mut registry = McpRegistry::new();
        let mut snapshot = sample();
        registry.register_app(Box::new(JsApp::from_package_files("sample_timer", &snapshot.files, None).unwrap())).unwrap();
        // Match the registered executor's installed version.
        let version = registry.manifests()[0].version.clone();
        snapshot.files.insert("/apps/sample_timer/sample_timer.mcp".into(), format!("name=Sample Timer\nversion={}\ncapabilities=ExposeMcpTools\n", version));
        let mut service = PackageService::new();
        service.apply_snapshot(snapshot.clone());
        registry.sync_package_inventory(&service);
        let args = BTreeMap::new();
        assert!(registry.call_tool("sample_timer.now", &args).is_err());
        registry.bind_package_revision(service.validated_revision("sample_timer").unwrap()).unwrap();
        assert!(registry.call_tool("sample_timer.now", &args).is_ok());
        snapshot.files.insert("/apps/sample_timer/notes.txt".into(), "changed".into());
        service.apply_snapshot(snapshot.clone());
        registry.sync_package_inventory(&service);
        assert!(registry.call_tool("sample_timer.now", &args).is_err());
        registry.bind_package_revision(service.validated_revision("sample_timer").unwrap()).unwrap();
        snapshot.files.insert("/apps/sample_timer/main.js".into(), "function broken(".into());
        service.apply_snapshot(snapshot);
        registry.sync_package_inventory(&service);
        assert!(registry.call_tool("sample_timer.now", &args).is_err());
        service.apply_snapshot(PackageSnapshot::default());
        registry.sync_package_inventory(&service);
        assert!(registry.call_tool("sample_timer.now", &args).is_err());
    }
}
