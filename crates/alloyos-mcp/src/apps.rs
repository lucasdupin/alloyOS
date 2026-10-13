use alloc::collections::BTreeMap;
use alloc::format;
use alloc::rc::Rc;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use core::cell::RefCell;

use alloyos_js::{ScriptCapabilities, ScriptScheduler};

use crate::protocol::{
    AppSkill, AppTile, McpError, McpParamSpec, McpPrompt, McpResource, McpResourceContent, McpTool,
    McpValue, TileAction, TileItem,
};
use crate::registry::McpApp;
use crate::sandbox::{AppCapability, AppManifest, McpSandboxError, RuntimeCapabilityGuard};

pub fn mcp_to_js_value(val: &McpValue) -> alloyos_js::JsValue {
    match val {
        McpValue::Null => alloyos_js::JsValue::Null,
        McpValue::Bool(b) => alloyos_js::JsValue::Bool(*b),
        McpValue::Int(n) => alloyos_js::JsValue::Number(*n),
        McpValue::Text(s) => alloyos_js::JsValue::String(s.clone()),
        McpValue::Array(items) => {
            alloyos_js::JsValue::Array(items.iter().map(mcp_to_js_value).collect())
        }
        McpValue::Map(map) => {
            let mut obj = BTreeMap::new();
            for (k, v) in map {
                obj.insert(k.clone(), mcp_to_js_value(v));
            }
            alloyos_js::JsValue::Object(obj)
        }
    }
}

pub fn parse_tools_from_json(json: &str) -> Vec<McpTool> {
    let mut tools = Vec::new();
    if let Ok(McpValue::Array(items)) = McpValue::from_json(json) {
        for item in items {
            if let McpValue::Map(map) = item {
                let name = map
                    .get("name")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let desc = map
                    .get("description")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let mut params = Vec::new();
                if let Some(McpValue::Array(param_items)) = map.get("params") {
                    for p in param_items {
                        if let McpValue::Map(pmap) = p {
                            let pname = pmap
                                .get("name")
                                .and_then(|v| v.as_str())
                                .unwrap_or("")
                                .to_string();
                            let ptype = pmap
                                .get("type")
                                .and_then(|v| v.as_str())
                                .unwrap_or("string")
                                .to_string();
                            let preq = pmap
                                .get("required")
                                .and_then(|v| v.as_bool())
                                .unwrap_or(false);
                            let pdesc = pmap
                                .get("description")
                                .and_then(|v| v.as_str())
                                .unwrap_or("")
                                .to_string();
                            params.push(McpParamSpec::new(pname, ptype, preq, pdesc));
                        }
                    }
                }
                if !name.is_empty() {
                    tools.push(McpTool::new(name, desc, params));
                }
            }
        }
    }
    tools
}

pub fn parse_resources_from_json(json: &str) -> Vec<McpResource> {
    let mut resources = Vec::new();
    if let Ok(McpValue::Array(items)) = McpValue::from_json(json) {
        for item in items {
            if let McpValue::Map(map) = item {
                let uri = map
                    .get("uri")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let name = map
                    .get("name")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let mime = map
                    .get("mime_type")
                    .and_then(|v| v.as_str())
                    .unwrap_or("application/json")
                    .to_string();
                let desc = map
                    .get("description")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                if !uri.is_empty() {
                    resources.push(McpResource::new(uri, name, mime, desc));
                }
            }
        }
    }
    resources
}

pub fn parse_prompts_from_json(json: &str) -> Vec<McpPrompt> {
    let mut prompts = Vec::new();
    if let Ok(McpValue::Array(items)) = McpValue::from_json(json) {
        for item in items {
            if let McpValue::Map(map) = item {
                let name = map
                    .get("name")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let desc = map
                    .get("description")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let mut args = Vec::new();
                if let Some(McpValue::Array(arg_items)) = map.get("params") {
                    for a in arg_items {
                        if let McpValue::Map(amap) = a {
                            let aname = amap
                                .get("name")
                                .and_then(|v| v.as_str())
                                .unwrap_or("")
                                .to_string();
                            args.push(aname);
                        }
                    }
                }
                if !name.is_empty() {
                    prompts.push(McpPrompt::new(name, desc, args));
                }
            }
        }
    }
    prompts
}

pub fn parse_app_tile_from_json(json: &str) -> Result<AppTile, String> {
    let val = McpValue::from_json(json).map_err(|e| format!("json parse error: {:?}", e))?;
    let map = match val {
        McpValue::Map(m) => m,
        _ => return Err("tile JSON must be an object".to_string()),
    };
    let tile_id = map
        .get("tile_id")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let title = map
        .get("title")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let subtitle = map
        .get("subtitle")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let refresh_resource_uri = map
        .get("refresh_resource_uri")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());
    let mut items = Vec::new();
    if let Some(McpValue::Array(raw_items)) = map.get("items") {
        for item in raw_items {
            if let McpValue::Map(imap) = item {
                let id = imap
                    .get("id")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let ititle = imap
                    .get("title")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let isub = imap
                    .get("subtitle")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let ibadge = imap
                    .get("badge")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let istatus = imap
                    .get("status")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                items.push(TileItem::new(id, ititle, isub, ibadge, istatus));
            }
        }
    }
    let mut actions = Vec::new();
    if let Some(McpValue::Array(raw_actions)) = map.get("actions") {
        for act in raw_actions {
            if let McpValue::Map(amap) = act {
                let id = amap
                    .get("id")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let label = amap
                    .get("label")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let tool_name = amap
                    .get("tool_name")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let mut default_args = BTreeMap::new();
                if let Some(McpValue::Map(def_args)) = amap.get("default_args") {
                    for (k, v) in def_args {
                        default_args.insert(k.clone(), v.clone());
                    }
                }
                actions.push(TileAction::new(id, label, tool_name, default_args));
            }
        }
    }
    Ok(AppTile {
        tile_id,
        title,
        subtitle,
        refresh_resource_uri,
        items,
        actions,
    })
}

/// Generic, filesystem-discovered MCP application executed in `alloyos-js`.
pub struct JsApp {
    manifest: AppManifest,
    source: String,
    guard: RuntimeCapabilityGuard,
    tools: Vec<McpTool>,
    resources: Vec<McpResource>,
    prompts: Vec<McpPrompt>,
    skills: Vec<AppSkill>,
    scheduler: Option<Rc<RefCell<ScriptScheduler>>>,
    vfs_files: Option<Rc<RefCell<BTreeMap<String, String>>>>,
    uptime_ms: u64,
    tile_ids: Vec<String>,
}

impl JsApp {
    pub fn new(
        manifest: AppManifest,
        source: String,
        tools: Vec<McpTool>,
        resources: Vec<McpResource>,
        prompts: Vec<McpPrompt>,
        skills: Vec<AppSkill>,
        scheduler: Option<Rc<RefCell<ScriptScheduler>>>,
    ) -> Result<Self, McpSandboxError> {
        let guard = RuntimeCapabilityGuard::new(manifest.clone())?;
        Ok(Self {
            manifest,
            source,
            guard,
            tools,
            resources,
            prompts,
            skills,
            scheduler,
            vfs_files: Some(Rc::new(RefCell::new(BTreeMap::new()))),
            uptime_ms: 0,
            tile_ids: Vec::new(),
        })
    }

    pub fn with_vfs_files(mut self, vfs: Rc<RefCell<BTreeMap<String, String>>>) -> Self {
        self.vfs_files = Some(vfs);
        self
    }

    pub fn from_package_files(
        app_id: &str,
        files: &BTreeMap<String, String>,
        scheduler: Option<Rc<RefCell<ScriptScheduler>>>,
    ) -> Result<Self, McpSandboxError> {
        let manifest_path = format!("/apps/{}/{}.mcp", app_id, app_id);
        let manifest_content =
            files
                .get(&manifest_path)
                .ok_or_else(|| McpSandboxError::InvalidManifest {
                    app_id: app_id.to_string(),
                    reason: format!("missing manifest at {}", manifest_path),
                })?;

        let manifest = AppManifest::parse_in_folder(app_id, manifest_content)?;
        crate::sandbox::validate_app_manifest(&manifest)?;

        let entrypoint = format!("/apps/{}/main.js", app_id);
        let source = files.get(&entrypoint).cloned().unwrap_or_default();

        let tools_path = format!("/apps/{}/tools.json", app_id);
        let tools = if let Some(content) = files.get(&tools_path) {
            parse_tools_from_json(content)
        } else {
            Vec::new()
        };

        let resources_path = format!("/apps/{}/resources.json", app_id);
        let resources = if let Some(content) = files.get(&resources_path) {
            parse_resources_from_json(content)
        } else {
            Vec::new()
        };

        let prompts_path = format!("/apps/{}/prompts.json", app_id);
        let prompts = if let Some(content) = files.get(&prompts_path) {
            parse_prompts_from_json(content)
        } else {
            Vec::new()
        };

        let skill_path = format!("/apps/{}/SKILL.md", app_id);
        let mut skills = Vec::new();
        if let Some(content) = files.get(&skill_path) {
            skills.push(AppSkill::new(
                app_id,
                &manifest.name,
                &skill_path,
                &format!("{} application skill", manifest.name),
                content,
            ));
        }

        let mut app = Self::new(
            manifest, source, tools, resources, prompts, skills, scheduler,
        )?;
        if let Some(content) = files.get(&format!("/apps/{}/tiles.json", app_id)) {
            if let Ok(McpValue::Array(ids)) = McpValue::from_json(content) {
                app.tile_ids = ids
                    .iter()
                    .filter_map(|id| id.as_str().map(ToString::to_string))
                    .collect();
            }
        }
        app.vfs_files = Some(Rc::new(RefCell::new(files.clone())));
        Ok(app)
    }

    fn make_host(&self) -> alloyos_js::VirtualHostEnv {
        let mut capabilities = ScriptCapabilities::for_mcp_app(&self.manifest.app_id);
        if self
            .manifest
            .has_capability(&AppCapability::ScriptSchedulerAccess)
        {
            capabilities.allow_scheduler = true;
            capabilities
                .fs_read_prefixes
                .extend(["/scripts".into(), "/etc/schedules.conf".into()]);
            capabilities
                .fs_write_prefixes
                .extend(["/scripts".into(), "/etc/schedules.conf".into()]);
        }
        let mut host = alloyos_js::VirtualHostEnv::with_capabilities(capabilities);
        host.status.uptime_ms = self.uptime_ms;
        if let Some(ref sched) = self.scheduler {
            host.scheduler = Some(sched.clone());
        }
        if let Some(ref vfs) = self.vfs_files {
            host.files = vfs.borrow().clone();
        }
        host
    }

    fn sync_host(&mut self, host: alloyos_js::VirtualHostEnv) {
        if let Some(ref vfs) = self.vfs_files {
            *vfs.borrow_mut() = host.files;
        }
    }
}

impl McpApp for JsApp {
    fn update_uptime(&mut self, uptime_ms: u64) {
        self.uptime_ms = uptime_ms;
    }

    fn manifest(&self) -> &AppManifest {
        &self.manifest
    }

    fn tools(&self) -> Vec<McpTool> {
        self.tools.clone()
    }

    fn resources(&self) -> Vec<McpResource> {
        self.resources.clone()
    }

    fn prompts(&self) -> Vec<McpPrompt> {
        self.prompts.clone()
    }

    fn skills(&self) -> Vec<AppSkill> {
        self.skills.clone()
    }

    fn tiles(&self) -> Vec<AppTile> {
        self.tile_ids
            .iter()
            .filter_map(|id| self.render_tile(id).ok())
            .collect()
    }

    fn call_tool(
        &mut self,
        tool_name: &str,
        args: &BTreeMap<String, McpValue>,
    ) -> Result<McpValue, McpError> {
        self.guard
            .enforce_capability(&AppCapability::ExposeMcpTools)
            .map_err(|e| e.to_mcp_error())?;

        let mut host = self.make_host();
        let mut js_args_map = BTreeMap::new();
        for (k, v) in args {
            js_args_map.insert(k.clone(), mcp_to_js_value(v));
        }

        let js_args = [
            alloyos_js::JsValue::String(tool_name.to_string()),
            alloyos_js::JsValue::Object(js_args_map),
        ];

        let result = alloyos_js::invoke_app_handler(
            &self.source,
            alloyos_js::AppHandler::CallTool,
            &js_args,
            50_000,
            &mut host,
        )
        .map_err(|e| McpError::internal_error(format!("JS callTool error: {}", e)))?;

        self.sync_host(host);

        let parsed = McpValue::from_json(&result.text)
            .unwrap_or_else(|_| McpValue::Text(result.text.clone()));

        if let McpValue::Map(ref m) = parsed {
            if let Some(McpValue::Text(ref err)) = m.get("error") {
                if err.starts_with("Unknown tool") || err.starts_with("unknown") {
                    return Err(McpError::method_not_found(err.clone()));
                }
            }
        }

        Ok(parsed)
    }

    fn read_resource(&self, uri: &str) -> Result<McpResourceContent, McpError> {
        self.guard
            .enforce_capability(&AppCapability::ExposeMcpResources)
            .map_err(|e| e.to_mcp_error())?;

        let mut host = self.make_host();
        let js_args = [alloyos_js::JsValue::String(uri.to_string())];

        let result = alloyos_js::invoke_app_handler(
            &self.source,
            alloyos_js::AppHandler::ReadResource,
            &js_args,
            50_000,
            &mut host,
        )
        .map_err(|e| McpError::internal_error(format!("JS readResource error: {}", e)))?;

        let text = result.text;
        if let Ok(McpValue::Map(ref m)) = McpValue::from_json(&text) {
            if let Some(McpValue::Text(ref err)) = m.get("error") {
                if err.starts_with("Unknown resource") || err.starts_with("unknown") {
                    return Err(McpError::resource_not_found(err.clone()));
                }
            }
        }

        let mime = if uri.ends_with(".md") {
            "text/markdown"
        } else {
            "application/json"
        };
        Ok(McpResourceContent::new(uri, mime, text))
    }

    fn get_prompt(&self, name: &str, args: &BTreeMap<String, String>) -> Result<String, McpError> {
        self.guard
            .enforce_capability(&AppCapability::ExposeMcpPrompts)
            .map_err(|e| e.to_mcp_error())?;

        let mut host = self.make_host();
        let mut js_args_map = BTreeMap::new();
        for (k, v) in args {
            js_args_map.insert(k.clone(), alloyos_js::JsValue::String(v.clone()));
        }

        let js_args = [
            alloyos_js::JsValue::String(name.to_string()),
            alloyos_js::JsValue::Object(js_args_map),
        ];

        let result = alloyos_js::invoke_app_handler(
            &self.source,
            alloyos_js::AppHandler::GetPrompt,
            &js_args,
            50_000,
            &mut host,
        )
        .map_err(|e| McpError::internal_error(format!("JS getPrompt error: {}", e)))?;

        Ok(result.text)
    }

    fn render_tile(&self, tile_id: &str) -> Result<AppTile, McpError> {
        self.guard
            .enforce_capability(&AppCapability::ExposeTiles)
            .map_err(|e| e.to_mcp_error())?;

        let mut host = self.make_host();
        let js_args = [alloyos_js::JsValue::String(tile_id.to_string())];

        let result = alloyos_js::invoke_app_handler(
            &self.source,
            alloyos_js::AppHandler::RenderTile,
            &js_args,
            50_000,
            &mut host,
        )
        .map_err(|e| McpError::internal_error(format!("JS renderTile error: {}", e)))?;

        parse_app_tile_from_json(&result.text)
            .map_err(|e| McpError::internal_error(format!("invalid tile json: {}", e)))
    }
}
