use alloc::boxed::Box;
use alloc::collections::BTreeMap;
use alloc::format;
use alloc::rc::Rc;
use alloc::string::{String, ToString};
use alloc::vec;
use alloc::vec::Vec;
use core::cell::RefCell;

use crate::packages::{PackageService, ValidatedPackageRevision};

use crate::protocol::{
    mcp_map, methods, AppSkill, AppTile, McpError, McpPrompt, McpRequest, McpResource,
    McpResourceContent, McpResponse, McpTool, McpValue,
};
use crate::sandbox::{
    validate_app_manifest, AppCapability, AppManifest, McpSandboxError, RuntimeCapabilityGuard,
};

/// Caller identity for MCP Registry operations.
///
/// In alloyOS, **no app can access another app**; only `Welder` centralizes app access
/// and cross-app orchestration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum McpCaller {
    /// The central Welder AI agent orchestrator (authorized to access all registered apps).
    Welder,
    /// An individual MCP application (strictly forbidden from accessing any other app).
    App(String),
}

/// Developer application trait for alloyOS.
///
/// Each app in alloyOS can expose:
/// - **MCP entry points**: Tools, resources, and prompts
/// - **SKILLs**: Text files (`/apps/<app_id>/SKILL.md`) teaching Welder how to orchestrate the app
/// - **Tiles**: Declarative, renderer-agnostic `AppTile` descriptors that Ferrofluid embeds in the
///   chat stream or pins to the Dashboard.
pub trait McpApp {
    /// Host-provided monotonic time; apps never fabricate a wall clock.
    fn update_uptime(&mut self, _uptime_ms: u64) {}
    fn update_wall_clock(&mut self, _unix_ms: Option<u64>) {}
    fn manifest(&self) -> &AppManifest;
    fn tools(&self) -> Vec<McpTool>;
    fn resources(&self) -> Vec<McpResource>;
    fn prompts(&self) -> Vec<McpPrompt>;
    fn skills(&self) -> Vec<AppSkill> {
        Vec::new()
    }
    fn tiles(&self) -> Vec<AppTile> {
        Vec::new()
    }
    fn call_tool(
        &mut self,
        tool_name: &str,
        args: &BTreeMap<String, McpValue>,
    ) -> Result<McpValue, McpError>;
    fn read_resource(&self, uri: &str) -> Result<McpResourceContent, McpError>;
    fn get_prompt(&self, name: &str, args: &BTreeMap<String, String>) -> Result<String, McpError>;
    fn render_tile(&self, tile_id: &str) -> Result<AppTile, McpError> {
        self.tiles()
            .into_iter()
            .find(|t| t.tile_id == tile_id)
            .ok_or_else(|| {
                McpError::resource_not_found(format!(
                    "tile '{}' not found in app '{}'",
                    tile_id,
                    self.manifest().app_id
                ))
            })
    }
}

struct RegisteredApp {
    guard: RuntimeCapabilityGuard,
    app: Box<dyn McpApp>,
}

/// Central MCP Registry that validates app manifests, enforces the Zero App-Owned UI
/// and No Cross-App Access sandbox rules, and routes namespaced MCP tools, resources,
/// prompts, skills, and declarative tiles over JSON-RPC 2.0.
pub struct McpRegistry {
    apps: BTreeMap<String, RegisteredApp>,
    package_inventory: PackageService,
    package_revisions: BTreeMap<String, Option<ValidatedPackageRevision>>,
    runtime_files: Rc<RefCell<BTreeMap<String, String>>>,
    http_routes: Rc<RefCell<BTreeMap<String, String>>>,
    http_client: Rc<RefCell<crate::transport::HttpClient>>,
}

impl Default for McpRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl McpRegistry {
    pub fn update_wall_clock(&mut self, unix_ms: Option<u64>) {
        for entry in self.apps.values_mut() { entry.app.update_wall_clock(unix_ms); }
    }

    pub fn update_uptime(&mut self, uptime_ms: u64) {
        for entry in self.apps.values_mut() {
            entry.app.update_uptime(uptime_ms);
        }
    }

    pub fn new() -> Self {
        let http_routes = Rc::new(RefCell::new(BTreeMap::new()));
        Self {
            apps: BTreeMap::new(),
            package_inventory: PackageService::new(),
            package_revisions: BTreeMap::new(),
            runtime_files: Rc::new(RefCell::new(BTreeMap::new())),
            http_client: Rc::new(RefCell::new(crate::transport::HttpClient::new(http_routes.clone()))),
            http_routes,
        }
    }

    /// Registers a mock or cached HTTP response for `AlloyOS.net.fetch(url)`.
    pub fn register_http_response(&mut self, url: &str, body: &str) {
        self.http_routes.borrow_mut().insert(url.to_string(), body.to_string());
    }

    /// Returns a shared reference to the HTTP routes map.
    pub fn http_routes(&self) -> Rc<RefCell<BTreeMap<String, String>>> {
        self.http_routes.clone()
    }

    pub fn http_client(&self) -> Rc<RefCell<crate::transport::HttpClient>> {
        self.http_client.clone()
    }

    pub fn set_http_transport(&mut self, transport: Box<dyn crate::transport::HttpTransport>) {
        self.http_client.borrow_mut().set_transport(transport);
    }

    /// Install OS validation results. Newly observed, invalid, deleted, or changed
    /// packages remain blocked until the execution owner binds a current revision.
    pub fn sync_package_inventory(&mut self, service: &PackageService) {
        for status in service.inventory() {
            self.package_revisions
                .entry(status.app_id.clone())
                .or_insert(None);
        }
        self.package_inventory = service.clone();
    }

    /// The owner must call this only after loading this exact immutable revision.
    /// This binds validation to a registered executor; it does not load JS itself.
    pub fn bind_package_revision(
        &mut self,
        revision: ValidatedPackageRevision,
    ) -> Result<(), McpError> {
        if !self.package_inventory.is_current(&revision) {
            return Err(McpError::invalid_params(
                "package revision is not currently validated",
            ));
        }
        let app = self
            .apps
            .get(revision.app_id())
            .ok_or_else(|| McpError::invalid_params("package executor is not registered"))?;
        let status = self.package_inventory.status(revision.app_id()).unwrap();
        if status.installed_version.as_deref() != Some(app.app.manifest().version.as_str()) {
            return Err(McpError::invalid_params(
                "registered executor version differs from package",
            ));
        }
        self.package_revisions
            .insert(revision.app_id().into(), Some(revision));
        Ok(())
    }

    fn verify_package_revision(&self, app_id: &str) -> Result<(), McpError> {
        if let Some(revision) = self.package_revisions.get(app_id) {
            if !revision
                .as_ref()
                .map(|revision| self.package_inventory.is_current(revision))
                .unwrap_or(false)
            {
                return Err(McpError::invalid_params(format!(
                    "package '{}' has no loaded current validated revision",
                    app_id
                )));
            }
        }
        Ok(())
    }

    /// Register a developer MCP app after validating its manifest against the
    /// Zero App-Owned UI, manifest-declared scheduler access, and sandbox rules.
    pub(crate) fn register_app(&mut self, app: Box<dyn McpApp>) -> Result<(), McpSandboxError> {
        let manifest = app.manifest().clone();
        validate_app_manifest(&manifest)?;

        if self.apps.contains_key(&manifest.app_id) {
            return Err(McpSandboxError::InvalidManifest {
                app_id: manifest.app_id.clone(),
                reason: "duplicate app_id already registered".to_string(),
            });
        }

        let guard = RuntimeCapabilityGuard::new(manifest.clone())?;
        self.apps
            .insert(manifest.app_id.clone(), RegisteredApp { guard, app });
        Ok(())
    }

    /// Unregister an MCP application by app_id.
    pub fn unregister_app(&mut self, app_id: &str) -> bool {
        crate::process::stop(app_id);
        self.package_revisions.remove(app_id);
        self.apps.remove(app_id).is_some()
    }

    /// Dynamically discover and register all valid applications from a package snapshot.
    /// Loads only packages present under `/apps/<app_id>/` in the snapshot.
    pub fn discover_and_register_packages(
        &mut self,
        snapshot: &crate::packages::PackageSnapshot,
        scheduler: Option<Rc<RefCell<alloyos_js::ScriptScheduler>>>,
    ) -> Result<usize, McpSandboxError> {
        let _ = self.package_inventory.scan_startup(snapshot.clone());
        let app_ids: Vec<_> = self
            .package_inventory
            .inventory()
            .iter()
            .map(|status| status.app_id.clone())
            .collect();
        let removed: Vec<_> = self
            .package_revisions
            .keys()
            .filter(|id| !app_ids.contains(id))
            .cloned()
            .collect();
        for id in removed {
            self.unregister_app(&id);
        }
        {
            let mut files = self.runtime_files.borrow_mut();
            files.retain(|path, _| !path.starts_with("/apps/"));
            files.extend(snapshot.files.clone());
            let packages = self
                .package_inventory
                .inventory()
                .iter()
                .map(|status| {
                    let diagnostics = status
                        .diagnostics
                        .iter()
                        .map(|diagnostic| {
                            McpValue::Map(mcp_map(&[
                                ("path", McpValue::Text(diagnostic.path.clone())),
                                ("message", McpValue::Text(diagnostic.message.clone())),
                            ]))
                        })
                        .collect();
                    McpValue::Map(mcp_map(&[
                        ("app_id", McpValue::Text(status.app_id.clone())),
                        (
                            "version",
                            status
                                .installed_version
                                .clone()
                                .map(McpValue::Text)
                                .unwrap_or(McpValue::Null),
                        ),
                        ("revision", McpValue::Int(status.revision as i64)),
                        (
                            "validated_revision",
                            status
                                .validated_revision
                                .map(|revision| McpValue::Int(revision as i64))
                                .unwrap_or(McpValue::Null),
                        ),
                        ("ready", McpValue::Bool(status.ready)),
                        ("diagnostics", McpValue::Array(diagnostics)),
                    ]))
                })
                .collect();
            files.insert(
                "/proc/app-packages.json".into(),
                McpValue::Map(mcp_map(&[("packages", McpValue::Array(packages))])).to_json(),
            );
        }
        let vfs = self.runtime_files.clone();
        let mut count = 0;
        for app_id in app_ids {
            // Preserve remote sessions across scans of unchanged packages.
            if let Some(revision) = self.package_inventory.validated_revision(&app_id) {
                if self.package_revisions.get(&app_id) == Some(&Some(revision))
                    && self.apps.get(&app_id).is_some_and(|entry| entry.app.manifest().is_remote()) {
                    count += 1;
                    continue;
                }
            }
            // Invalid packages remain in inventory, but must never execute.
            self.unregister_app(&app_id);
            let Some(revision) = self.package_inventory.validated_revision(&app_id) else {
                continue;
            };
            let manifest_path = format!("/apps/{}/{}.mcp", app_id, app_id);
            let is_remote = snapshot
                .files
                .get(&manifest_path)
                .and_then(|text| AppManifest::parse_in_folder(&app_id, text).ok())
                .map(|m| m.is_remote())
                .unwrap_or(false);

            let app: Box<dyn McpApp> = if is_remote {
                Box::new(
                    crate::apps::RemoteMcpApp::from_package_files(
                        &app_id,
                        &snapshot.files,
                    )?
                    .with_http_client(self.http_client.clone()),
                )
            } else {
                Box::new(
                    crate::apps::JsApp::from_package_files(
                        &app_id,
                        &snapshot.files,
                        scheduler.clone(),
                    )?
                    .with_vfs_files(vfs.clone())
                    .with_http_routes(self.http_routes.clone()),
                )
            };
            self.register_app(app)?;
            self.bind_package_revision(revision).map_err(|error| {
                McpSandboxError::InvalidManifest {
                    app_id: app_id.clone(),
                    reason: error.message,
                }
            })?;
            count += 1;
        }

        Ok(count)
    }

    /// App writes captured by the host; the OS owner persists these after calls.
    pub fn runtime_files(&self) -> BTreeMap<String, String> {
        self.runtime_files.borrow().clone()
    }

    /// Number of registered MCP apps.
    pub fn app_count(&self) -> usize {
        self.apps.len()
    }

    /// Returns the manifests of all registered MCP apps.
    pub fn manifests(&self) -> Vec<&AppManifest> {
        self.apps
            .values()
            .map(|entry| entry.guard.manifest())
            .collect()
    }

    /// Retrieve the runtime capability guard for a registered app.
    pub fn guard_for(&self, app_id: &str) -> Option<&RuntimeCapabilityGuard> {
        self.apps.get(app_id).map(|entry| &entry.guard)
    }

    fn verify_caller_access(
        &self,
        caller: &McpCaller,
        target_app_id: &str,
    ) -> Result<(), McpError> {
        match caller {
            McpCaller::Welder => self.verify_package_revision(target_app_id),
            McpCaller::App(caller_app_id) => Err(McpSandboxError::CrossAppAccessForbidden {
                caller_app_id: caller_app_id.clone(),
                target_app_id: target_app_id.to_string(),
            }
            .to_mcp_error()),
        }
    }

    /// Aggregate all MCP tools across registered apps that declare `ExposeMcpTools`.
    /// Ensures every tool name is namespaced as `<app_id>.<tool_name>`.
    pub fn list_tools(&self) -> Vec<McpTool> {
        let mut all_tools = Vec::new();
        for (app_id, entry) in &self.apps {
            if entry
                .guard
                .enforce_capability(&AppCapability::ExposeMcpTools)
                .is_ok()
            {
                let prefix = format!("{}.", app_id);
                for mut tool in entry.app.tools() {
                    if !tool.name.starts_with(&prefix) {
                        tool.name = format!("{}.{}", app_id, tool.name);
                    }
                    all_tools.push(tool);
                }
            }
        }
        all_tools
    }

    /// Aggregate all MCP resources across registered apps that declare `ExposeMcpResources`.
    pub fn list_resources(&self) -> Vec<McpResource> {
        let mut all_resources = Vec::new();
        for entry in self.apps.values() {
            if entry
                .guard
                .enforce_capability(&AppCapability::ExposeMcpResources)
                .is_ok()
            {
                all_resources.extend(entry.app.resources());
            }
        }
        all_resources
    }

    /// Aggregate all MCP prompts across registered apps that declare `ExposeMcpPrompts`.
    /// Ensures every prompt name is namespaced as `<app_id>.<prompt_name>`.
    pub fn list_prompts(&self) -> Vec<McpPrompt> {
        let mut all_prompts = Vec::new();
        for (app_id, entry) in &self.apps {
            if entry
                .guard
                .enforce_capability(&AppCapability::ExposeMcpPrompts)
                .is_ok()
            {
                let prefix = format!("{}.", app_id);
                for mut prompt in entry.app.prompts() {
                    if !prompt.name.starts_with(&prefix) {
                        prompt.name = format!("{}.{}", app_id, prompt.name);
                    }
                    all_prompts.push(prompt);
                }
            }
        }
        all_prompts
    }

    /// Aggregate all SKILL text files across registered apps that declare `ExposeSkills`.
    pub fn list_skills(&self) -> Vec<AppSkill> {
        let mut all_skills = Vec::new();
        for entry in self.apps.values() {
            if entry
                .guard
                .enforce_capability(&AppCapability::ExposeSkills)
                .is_ok()
            {
                all_skills.extend(entry.app.skills());
            }
        }
        all_skills
    }

    /// Read an `AppSkill` by `app_id`, skill `name`, or VFS `path` (`/apps/<app_id>/SKILL.md`).
    pub fn read_skill(&self, query: &str) -> Result<AppSkill, McpError> {
        let trimmed = query.trim();
        for (app_id, entry) in &self.apps {
            if entry
                .guard
                .enforce_capability(&AppCapability::ExposeSkills)
                .is_ok()
            {
                for skill in entry.app.skills() {
                    if app_id == trimmed || skill.name == trimmed || skill.path == trimmed {
                        return Ok(skill);
                    }
                }
            }
        }
        Err(McpError::resource_not_found(format!(
            "skill '{}' not found in any registered MCP app",
            trimmed
        )))
    }

    /// Aggregate all declarative `AppTile`s across registered apps that declare `ExposeTiles`.
    pub fn list_tiles(&self) -> Vec<AppTile> {
        let mut all_tiles = Vec::new();
        for entry in self.apps.values() {
            if entry
                .guard
                .enforce_capability(&AppCapability::ExposeTiles)
                .is_ok()
            {
                all_tiles.extend(entry.app.tiles());
            }
        }
        all_tiles
    }

    /// Render a live declarative `AppTile` by `tile_id` (`<app_id>.<tile_name>`).
    pub fn render_tile(&self, tile_id: &str) -> Result<AppTile, McpError> {
        let (app_id, _) = tile_id.split_once('.').ok_or_else(|| {
            McpError::invalid_params(format!(
                "tile_id '{}' must be namespaced as '<app_id>.<tile_name>'",
                tile_id
            ))
        })?;
        self.verify_package_revision(app_id)?;
        let entry = self.apps.get(app_id).ok_or_else(|| {
            McpError::resource_not_found(format!(
                "no registered MCP app '{}' for tile '{}'",
                app_id, tile_id
            ))
        })?;
        entry
            .guard
            .enforce_capability(&AppCapability::ExposeTiles)
            .map_err(|e| e.to_mcp_error())?;
        entry.app.render_tile(tile_id)
    }

    /// Invoke a namespaced MCP tool (`<app_id>.<tool_name>`) with explicit caller verification.
    ///
    /// Enforces that **only `McpCaller::Welder`** may invoke app tools; any `McpCaller::App`
    /// attempting to invoke another app (or orchestrate across apps) is rejected with
    /// `CrossAppAccessForbidden`.
    pub fn call_tool_as(
        &mut self,
        caller: &McpCaller,
        namespaced_tool: &str,
        args: &BTreeMap<String, McpValue>,
    ) -> Result<McpValue, McpError> {
        let (app_id, _short_name) = namespaced_tool.split_once('.').ok_or_else(|| {
            McpError::invalid_params(format!(
                "tool name '{}' must be namespaced as '<app_id>.<tool_name>'",
                namespaced_tool
            ))
        })?;

        self.verify_caller_access(caller, app_id)?;

        let entry = self
            .apps
            .get_mut(app_id)
            .ok_or_else(|| McpError::method_not_found(format!("unknown MCP app '{}'", app_id)))?;

        entry
            .guard
            .enforce_capability(&AppCapability::ExposeMcpTools)
            .map_err(|e| e.to_mcp_error())?;

        entry.app.call_tool(namespaced_tool, args)
    }

    /// Invoke a namespaced MCP tool (`<app_id>.<tool_name>`) as the central `Welder` orchestrator.
    pub fn call_tool(
        &mut self,
        namespaced_tool: &str,
        args: &BTreeMap<String, McpValue>,
    ) -> Result<McpValue, McpError> {
        self.call_tool_as(&McpCaller::Welder, namespaced_tool, args)
    }

    /// Read an MCP resource by URI (`mcp://<app_id>/<path>`) with explicit caller verification.
    pub fn read_resource_as(
        &self,
        caller: &McpCaller,
        uri: &str,
    ) -> Result<McpResourceContent, McpError> {
        let without_scheme = uri.strip_prefix("mcp://").ok_or_else(|| {
            McpError::invalid_params(format!("resource URI '{}' must start with 'mcp://'", uri))
        })?;

        let (app_id, _rest) = without_scheme
            .split_once('/')
            .unwrap_or((without_scheme, ""));

        if app_id.is_empty() {
            return Err(McpError::invalid_params("resource URI missing app_id host"));
        }

        self.verify_caller_access(caller, app_id)?;

        self.verify_package_revision(app_id)?;
        let entry = self.apps.get(app_id).ok_or_else(|| {
            McpError::resource_not_found(format!(
                "no registered MCP app '{}' for URI '{}'",
                app_id, uri
            ))
        })?;

        entry
            .guard
            .enforce_capability(&AppCapability::ExposeMcpResources)
            .map_err(|e| e.to_mcp_error())?;

        entry.app.read_resource(uri)
    }

    /// Read an MCP resource by URI (`mcp://<app_id>/<path>`) as the central `Welder` orchestrator.
    pub fn read_resource(&self, uri: &str) -> Result<McpResourceContent, McpError> {
        self.read_resource_as(&McpCaller::Welder, uri)
    }

    /// Render an MCP prompt by namespaced name (`<app_id>.<prompt_name>`) or unambiguous short name.
    pub fn get_prompt(
        &self,
        name: &str,
        args: &BTreeMap<String, String>,
    ) -> Result<String, McpError> {
        if let Some((app_id, _short_name)) = name.split_once('.') {
            self.verify_package_revision(app_id)?;
            let entry = self.apps.get(app_id).ok_or_else(|| {
                McpError::method_not_found(format!("unknown MCP app '{}'", app_id))
            })?;
            entry
                .guard
                .enforce_capability(&AppCapability::ExposeMcpPrompts)
                .map_err(|e| e.to_mcp_error())?;
            return entry.app.get_prompt(name, args);
        }

        for (app_id, entry) in &self.apps {
            if self.verify_package_revision(app_id).is_err() {
                continue;
            }
            if entry
                .guard
                .enforce_capability(&AppCapability::ExposeMcpPrompts)
                .is_ok()
            {
                if let Ok(rendered) = entry.app.get_prompt(name, args) {
                    return Ok(rendered);
                }
            }
        }

        Err(McpError::method_not_found(format!(
            "prompt '{}' not found in any registered MCP app",
            name
        )))
    }

    /// Handle a parsed `McpRequest` and return a structured `McpResponse`.
    pub fn handle_request(&mut self, request: &McpRequest) -> McpResponse {
        if request.jsonrpc != "2.0" {
            return McpResponse::error(
                request.id,
                McpError::invalid_request("jsonrpc version must be '2.0'"),
            );
        }

        let outcome = match request.method.as_str() {
            methods::INITIALIZE => self.handle_initialize(),
            methods::TOOLS_LIST => self.handle_tools_list(),
            methods::TOOLS_CALL => self.handle_tools_call(&request.params),
            methods::RESOURCES_LIST => self.handle_resources_list(),
            methods::RESOURCES_READ => self.handle_resources_read(&request.params),
            methods::PROMPTS_LIST => self.handle_prompts_list(),
            methods::PROMPTS_GET => self.handle_prompts_get(&request.params),
            methods::SKILLS_LIST => self.handle_skills_list(),
            methods::SKILLS_READ => self.handle_skills_read(&request.params),
            methods::TILES_LIST => self.handle_tiles_list(),
            methods::TILES_RENDER => self.handle_tiles_render(&request.params),
            other => Err(McpError::method_not_found(format!(
                "unsupported MCP method '{}'",
                other
            ))),
        };

        match outcome {
            Ok(result) => McpResponse::success(request.id, result),
            Err(err) => McpResponse::error(request.id, err),
        }
    }

    /// Handle a raw JSON-RPC 2.0 string over IPC and return a serialized JSON-RPC 2.0 response.
    pub fn handle_json_rpc(&mut self, raw_json: &str) -> String {
        match McpRequest::from_json(raw_json) {
            Ok(req) => self.handle_request(&req).to_json(),
            Err(err) => McpResponse::error(0, err).to_json(),
        }
    }

    fn extract_caller(params: &BTreeMap<String, McpValue>) -> McpCaller {
        if let Some(caller_app) = params.get("caller_app").and_then(|v| v.as_str()) {
            McpCaller::App(caller_app.to_string())
        } else {
            McpCaller::Welder
        }
    }

    fn handle_initialize(&self) -> Result<McpValue, McpError> {
        let app_ids: Vec<McpValue> = self
            .apps
            .keys()
            .map(|id| McpValue::Text(id.clone()))
            .collect();

        let capabilities = McpValue::Map(mcp_map(&[
            ("tools", McpValue::Map(BTreeMap::new())),
            ("resources", McpValue::Map(BTreeMap::new())),
            ("prompts", McpValue::Map(BTreeMap::new())),
            ("skills", McpValue::Map(BTreeMap::new())),
            ("tiles", McpValue::Map(BTreeMap::new())),
            ("zeroAppOwnedUi", McpValue::Bool(true)),
            ("welderCentralizedOrchestration", McpValue::Bool(true)),
        ]));

        let server_info = McpValue::Map(mcp_map(&[
            ("name", McpValue::Text("alloyos-mcp-host".to_string())),
            ("version", McpValue::Text("0.1.0".to_string())),
        ]));

        Ok(McpValue::Map(mcp_map(&[
            ("protocolVersion", McpValue::Text("2024-11-05".to_string())),
            ("capabilities", capabilities),
            ("serverInfo", server_info),
            ("installedApps", McpValue::Array(app_ids)),
        ])))
    }

    fn handle_tools_list(&self) -> Result<McpValue, McpError> {
        let tools: Vec<McpValue> = self
            .list_tools()
            .into_iter()
            .map(|t| t.to_mcp_value())
            .collect();
        Ok(McpValue::Map(mcp_map(&[("tools", McpValue::Array(tools))])))
    }

    fn handle_tools_call(
        &mut self,
        params: &BTreeMap<String, McpValue>,
    ) -> Result<McpValue, McpError> {
        let caller = Self::extract_caller(params);
        let name = params
            .get("name")
            .and_then(|v| v.as_str())
            .ok_or_else(|| McpError::invalid_params("tools/call requires string 'name' param"))?;

        let empty_args = BTreeMap::new();
        let args = match params.get("arguments") {
            Some(McpValue::Map(m)) => m,
            Some(McpValue::Null) | None => &empty_args,
            _ => {
                return Err(McpError::invalid_params(
                    "tools/call 'arguments' must be an object",
                ))
            }
        };

        self.call_tool_as(&caller, name, args)
    }

    fn handle_resources_list(&self) -> Result<McpValue, McpError> {
        let resources: Vec<McpValue> = self
            .list_resources()
            .into_iter()
            .map(|r| r.to_mcp_value())
            .collect();
        Ok(McpValue::Map(mcp_map(&[(
            "resources",
            McpValue::Array(resources),
        )])))
    }

    fn handle_resources_read(
        &self,
        params: &BTreeMap<String, McpValue>,
    ) -> Result<McpValue, McpError> {
        let caller = Self::extract_caller(params);
        let uri = params.get("uri").and_then(|v| v.as_str()).ok_or_else(|| {
            McpError::invalid_params("resources/read requires string 'uri' param")
        })?;
        let content = self.read_resource_as(&caller, uri)?;
        Ok(McpValue::Map(mcp_map(&[(
            "contents",
            McpValue::Array(vec![content.to_mcp_value()]),
        )])))
    }

    fn handle_prompts_list(&self) -> Result<McpValue, McpError> {
        let prompts: Vec<McpValue> = self
            .list_prompts()
            .into_iter()
            .map(|p| p.to_mcp_value())
            .collect();
        Ok(McpValue::Map(mcp_map(&[(
            "prompts",
            McpValue::Array(prompts),
        )])))
    }

    fn handle_prompts_get(
        &self,
        params: &BTreeMap<String, McpValue>,
    ) -> Result<McpValue, McpError> {
        let name = params
            .get("name")
            .and_then(|v| v.as_str())
            .ok_or_else(|| McpError::invalid_params("prompts/get requires string 'name' param"))?;

        let mut string_args = BTreeMap::new();
        if let Some(McpValue::Map(raw_args)) = params.get("arguments") {
            for (k, v) in raw_args {
                match v {
                    McpValue::Text(s) => {
                        string_args.insert(k.clone(), s.clone());
                    }
                    other => {
                        string_args.insert(k.clone(), other.to_json());
                    }
                }
            }
        }

        let rendered = self.get_prompt(name, &string_args)?;
        let message = McpValue::Map(mcp_map(&[
            ("role", McpValue::Text("user".to_string())),
            (
                "content",
                McpValue::Map(mcp_map(&[
                    ("type", McpValue::Text("text".to_string())),
                    ("text", McpValue::Text(rendered.clone())),
                ])),
            ),
        ]));

        Ok(McpValue::Map(mcp_map(&[
            ("prompt", McpValue::Text(rendered)),
            ("messages", McpValue::Array(vec![message])),
        ])))
    }

    fn handle_skills_list(&self) -> Result<McpValue, McpError> {
        let skills: Vec<McpValue> = self
            .list_skills()
            .into_iter()
            .map(|s| s.to_mcp_value())
            .collect();
        Ok(McpValue::Map(mcp_map(&[(
            "skills",
            McpValue::Array(skills),
        )])))
    }

    fn handle_skills_read(
        &self,
        params: &BTreeMap<String, McpValue>,
    ) -> Result<McpValue, McpError> {
        let query = params
            .get("app_id")
            .or_else(|| params.get("path"))
            .or_else(|| params.get("name"))
            .and_then(|v| v.as_str())
            .ok_or_else(|| {
                McpError::invalid_params("skills/read requires string 'app_id' or 'path' param")
            })?;
        let skill = self.read_skill(query)?;
        Ok(McpValue::Map(mcp_map(&[("skill", skill.to_mcp_value())])))
    }

    fn handle_tiles_list(&self) -> Result<McpValue, McpError> {
        let tiles: Vec<McpValue> = self
            .list_tiles()
            .into_iter()
            .map(|t| t.to_mcp_value())
            .collect();
        Ok(McpValue::Map(mcp_map(&[("tiles", McpValue::Array(tiles))])))
    }

    fn handle_tiles_render(
        &self,
        params: &BTreeMap<String, McpValue>,
    ) -> Result<McpValue, McpError> {
        let tile_id = params
            .get("tile_id")
            .and_then(|v| v.as_str())
            .ok_or_else(|| McpError::invalid_params("tiles/render requires string 'tile_id'"))?;
        let tile = self.render_tile(tile_id)?;
        Ok(McpValue::Map(mcp_map(&[("tile", tile.to_mcp_value())])))
    }
}
