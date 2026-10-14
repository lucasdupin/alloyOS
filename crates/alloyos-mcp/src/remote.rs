use crate::transport::{HttpClient, HttpRequest};
use crate::{
    AppCapability, AppManifest, AppSkill, McpApp, McpError, McpParamSpec, McpPrompt, McpResource,
    McpResourceContent, McpSandboxError, McpTool, McpValue, RuntimeCapabilityGuard,
};
use alloc::{collections::BTreeMap, format, rc::Rc, string::String, vec, vec::Vec};
use core::cell::{Cell, RefCell};

pub struct RemoteMcpApp {
    manifest: AppManifest,
    guard: RuntimeCapabilityGuard,
    tools: Vec<McpTool>,
    resources: Vec<McpResource>,
    prompts: Vec<McpPrompt>,
    skills: Vec<AppSkill>,
    client: Option<Rc<RefCell<HttpClient>>>,
    next_id: Cell<i64>,
    session: RefCell<Option<(String, Option<String>)>>,
}

pub fn parse_remote_tools(json: &str) -> Result<Vec<McpTool>, String> {
    if json.len() > crate::transport::MAX_HTTP_BODY_BYTES {
        return Err("tools.json exceeds size limit".into());
    }
    let items = match McpValue::from_json(json).map_err(|_| "invalid tools.json")? {
        McpValue::Array(items) => items,
        _ => return Err("tools.json must be an array".into()),
    };
    let mut tools = Vec::new();
    if items.len() > 1000 {
        return Err("remote tool count exceeds limit".into());
    }
    for item in items {
        let map = item.as_map().ok_or("tool must be an object")?;
        let name = map
            .get("name")
            .and_then(McpValue::as_str)
            .filter(|name| !name.is_empty() && !name.chars().any(char::is_control))
            .ok_or("tool must have a nonempty name")?;
        if tools.iter().any(|tool: &McpTool| tool.name == name) {
            return Err("duplicate tool name".into());
        }
        let mut parameters = Vec::new();
        let schema = map.get("inputSchema");
        if let Some(schema) = schema {
            let schema_map = schema.as_map().ok_or("inputSchema must be an object")?;
            if schema_map.get("type").and_then(McpValue::as_str) != Some("object") {
                return Err("inputSchema type must be object".into());
            }
            let required = schema_map.get("required");
            if let Some(required) = required {
                match required {
                    McpValue::Array(items) if items.iter().all(|item| item.as_str().is_some()) => {}
                    _ => return Err("inputSchema required must be an array of strings".into()),
                }
            }
            if let Some(properties) = schema_map.get("properties") {
                for (key, value) in properties
                    .as_map()
                    .ok_or("inputSchema properties must be an object")?
                {
                    // JSON Schema also permits boolean subschemas.
                    let property = value.as_map();
                    if property.is_none() && value.as_bool().is_none() {
                        return Err("schema property must be an object or boolean".into());
                    }
                    let required = matches!(required, Some(McpValue::Array(items)) if items.iter().any(|item| item.as_str() == Some(key)));
                    parameters.push(McpParamSpec::new(
                        key,
                        property
                            .and_then(|property| property.get("type"))
                            .and_then(McpValue::as_str)
                            .unwrap_or("any"),
                        required,
                        property
                            .and_then(|property| property.get("description"))
                            .and_then(McpValue::as_str)
                            .unwrap_or(""),
                    ));
                }
            }
        } else if let Some(params) = map.get("params") {
            let items = match params {
                McpValue::Array(items) => items,
                _ => return Err("params must be an array".into()),
            };
            for param in items {
                let param = param.as_map().ok_or("parameter must be an object")?;
                let name = param
                    .get("name")
                    .and_then(McpValue::as_str)
                    .filter(|name| !name.is_empty())
                    .ok_or("parameter name missing")?;
                parameters.push(McpParamSpec::new(
                    name,
                    param
                        .get("type")
                        .and_then(McpValue::as_str)
                        .unwrap_or("string"),
                    param
                        .get("required")
                        .and_then(McpValue::as_bool)
                        .unwrap_or(false),
                    param
                        .get("description")
                        .and_then(McpValue::as_str)
                        .unwrap_or(""),
                ));
            }
        } else {
            return Err("tool requires inputSchema or params".into());
        }
        let mut tool = McpTool::new(
            name,
            map.get("description")
                .and_then(McpValue::as_str)
                .unwrap_or(""),
            parameters,
        );
        tool.input_schema = schema.cloned();
        tools.push(tool);
    }
    Ok(tools)
}

impl RemoteMcpApp {
    pub fn new(
        manifest: AppManifest,
        tools: Vec<McpTool>,
        resources: Vec<McpResource>,
        prompts: Vec<McpPrompt>,
        skills: Vec<AppSkill>,
    ) -> Result<Self, McpSandboxError> {
        if !manifest.is_remote() {
            return Err(McpSandboxError::InvalidManifest {
                app_id: manifest.app_id.clone(),
                reason: "remote app requires endpoint".into(),
            });
        }
        let guard = RuntimeCapabilityGuard::new(manifest.clone())?;
        Ok(Self {
            manifest,
            guard,
            tools,
            resources,
            prompts,
            skills,
            client: None,
            next_id: Cell::new(1),
            session: RefCell::new(None),
        })
    }
    pub fn with_http_client(mut self, client: Rc<RefCell<HttpClient>>) -> Self {
        self.client = Some(client);
        self
    }
    pub fn with_http_routes(self, routes: Rc<RefCell<BTreeMap<String, String>>>) -> Self {
        self.with_http_client(Rc::new(RefCell::new(HttpClient::new(routes))))
    }
    pub fn from_package_files(
        app_id: &str,
        files: &BTreeMap<String, String>,
    ) -> Result<Self, McpSandboxError> {
        let root = format!("/apps/{}/", app_id);
        let invalid = |reason: String| McpSandboxError::InvalidManifest {
            app_id: app_id.into(),
            reason,
        };
        let manifest = AppManifest::parse_in_folder(
            app_id,
            files
                .get(&format!("{}{}.mcp", root, app_id))
                .ok_or_else(|| invalid("missing manifest".into()))?,
        )?;
        let tools = if manifest.has_capability(&AppCapability::ExposeMcpTools) {
            parse_remote_tools(
                files
                    .get(&format!("{}tools.json", root))
                    .ok_or_else(|| invalid("missing tools.json".into()))?,
            )
            .map_err(invalid)?
        } else {
            Vec::new()
        };
        let mut resources = Vec::new();
        if manifest.has_capability(&AppCapability::ExposeMcpResources) {
            let text = files
                .get(&format!("{}resources.json", root))
                .ok_or_else(|| invalid("missing resources.json".into()))?;
            let value =
                McpValue::from_json(text).map_err(|_| invalid("invalid resources.json".into()))?;
            for item in value
                .as_array()
                .ok_or_else(|| invalid("resources.json must be an array".into()))?
            {
                let map = item
                    .as_map()
                    .ok_or_else(|| invalid("resources.json item must be an object".into()))?;
                let uri = map
                    .get("uri")
                    .and_then(McpValue::as_str)
                    .filter(|uri| !uri.is_empty())
                    .ok_or_else(|| invalid("resources.json requires uri".into()))?;
                let name = map
                    .get("name")
                    .and_then(McpValue::as_str)
                    .ok_or_else(|| invalid("resources.json requires name".into()))?;
                resources.push(McpResource::new(
                    uri,
                    name,
                    map.get("mimeType")
                        .or_else(|| map.get("mime_type"))
                        .and_then(McpValue::as_str)
                        .unwrap_or("text/plain"),
                    map.get("description")
                        .and_then(McpValue::as_str)
                        .unwrap_or(""),
                ));
            }
        }
        let mut prompts = Vec::new();
        if manifest.has_capability(&AppCapability::ExposeMcpPrompts) {
            let text = files
                .get(&format!("{}prompts.json", root))
                .ok_or_else(|| invalid("missing prompts.json".into()))?;
            let value =
                McpValue::from_json(text).map_err(|_| invalid("invalid prompts.json".into()))?;
            for item in value
                .as_array()
                .ok_or_else(|| invalid("prompts.json must be an array".into()))?
            {
                let map = item
                    .as_map()
                    .ok_or_else(|| invalid("prompts.json item must be an object".into()))?;
                let name = map
                    .get("name")
                    .and_then(McpValue::as_str)
                    .filter(|name| !name.is_empty())
                    .ok_or_else(|| invalid("prompts.json requires name".into()))?;
                let mut arguments = Vec::new();
                if let Some(items) = map.get("arguments").or_else(|| map.get("params")) {
                    for item in items
                        .as_array()
                        .ok_or_else(|| invalid("prompts.json arguments must be an array".into()))?
                    {
                        arguments.push(
                            item.as_map()
                                .and_then(|map| map.get("name"))
                                .and_then(McpValue::as_str)
                                .ok_or_else(|| {
                                    invalid("prompts.json argument requires name".into())
                                })?
                                .into(),
                        );
                    }
                }
                prompts.push(McpPrompt::new(
                    name,
                    map.get("description")
                        .and_then(McpValue::as_str)
                        .unwrap_or(""),
                    arguments,
                ));
            }
        }
        let skills = files
            .get(&format!("{}SKILL.md", root))
            .map(|text| {
                vec![AppSkill::new(
                    app_id,
                    &manifest.name,
                    format!("{}SKILL.md", root),
                    "Remote MCP application skill",
                    text,
                )]
            })
            .unwrap_or_default();
        Self::new(manifest, tools, resources, prompts, skills)
    }

    fn send(
        &self,
        method: &str,
        params: BTreeMap<String, McpValue>,
        notification: bool,
    ) -> Result<(McpValue, Vec<(String, String)>), McpError> {
        self.guard
            .enforce_capability(&AppCapability::ScopedNetwork)
            .map_err(|error| error.to_mcp_error())?;
        let id = self.next_id.get();
        if !notification {
            self.next_id.set(
                id.checked_add(1)
                    .ok_or_else(|| McpError::internal_error("RPC request ID exhausted"))?,
            );
        }
        let mut rpc = BTreeMap::from([
            ("jsonrpc".into(), McpValue::Text("2.0".into())),
            ("method".into(), McpValue::Text(method.into())),
            ("params".into(), McpValue::Map(params)),
        ]);
        if !notification {
            rpc.insert("id".into(), McpValue::Int(id));
        }
        let mut headers = vec![
            ("Content-Type".into(), "application/json".into()),
            (
                "Accept".into(),
                "application/json, text/event-stream".into(),
            ),
        ];
        if let Some((version, session_id)) = &*self.session.borrow() {
            headers.push(("MCP-Protocol-Version".into(), version.clone()));
            if let Some(session_id) = session_id {
                headers.push(("Mcp-Session-Id".into(), session_id.clone()));
            }
        }
        let request = HttpRequest {
            method: "POST",
            url: self
                .manifest
                .endpoint
                .clone()
                .ok_or_else(|| McpError::internal_error("missing endpoint"))?,
            headers,
            body: McpValue::Map(rpc).to_json(),
        };
        let response = self
            .client
            .as_ref()
            .ok_or_else(|| McpError::internal_error("HTTP transport unavailable"))?
            .borrow_mut()
            .request(&request)?;
        if notification {
            if response.status != 202 || !response.body.is_empty() {
                return Err(McpError::internal_error(
                    "invalid notification acknowledgement",
                ));
            }
            return Ok((McpValue::Null, response.headers));
        }
        let value = rpc_response(&response.body, id)?;
        Ok((value, response.headers))
    }

    fn initialize(&self) -> Result<(), McpError> {
        if self.session.borrow().is_some() {
            return Ok(());
        }
        let params = BTreeMap::from([
            (
                "protocolVersion".into(),
                McpValue::Text("2025-06-18".into()),
            ),
            ("capabilities".into(), McpValue::Map(BTreeMap::new())),
            (
                "clientInfo".into(),
                McpValue::Map(BTreeMap::from([
                    ("name".into(), McpValue::Text("alloyOS".into())),
                    ("version".into(), McpValue::Text("0.1.0".into())),
                ])),
            ),
        ]);
        let (result, headers) = self.send("initialize", params, false)?;
        let map = result
            .as_map()
            .ok_or_else(|| McpError::internal_error("invalid MCP initialize result"))?;
        if map.get("capabilities").and_then(McpValue::as_map).is_none()
            || !map
                .get("serverInfo")
                .and_then(McpValue::as_map)
                .is_some_and(|info| {
                    info.get("name").and_then(McpValue::as_str).is_some()
                        && info.get("version").and_then(McpValue::as_str).is_some()
                })
        {
            return Err(McpError::internal_error(
                "initialize result requires capabilities and serverInfo",
            ));
        }
        let version = map
            .get("protocolVersion")
            .and_then(McpValue::as_str)
            .filter(|version| matches!(*version, "2025-06-18" | "2025-03-26"))
            .ok_or_else(|| {
                McpError::internal_error("unsupported or missing MCP protocol version")
            })?;
        let session_id = headers
            .iter()
            .find(|(name, _)| name.eq_ignore_ascii_case("mcp-session-id"))
            .map(|(_, value)| value.clone());
        if session_id.as_ref().is_some_and(|value| {
            value.is_empty() || !value.bytes().all(|byte| (0x21..=0x7e).contains(&byte))
        }) {
            return Err(McpError::internal_error("invalid MCP session ID"));
        }
        *self.session.borrow_mut() = Some((version.into(), session_id));
        if let Err(error) = self.send("notifications/initialized", BTreeMap::new(), true) {
            *self.session.borrow_mut() = None;
            return Err(error);
        }
        Ok(())
    }
    fn rpc(&self, method: &str, params: BTreeMap<String, McpValue>) -> Result<McpValue, McpError> {
        self.initialize()?;
        match self.send(method, params, false) {
            Ok((result, _)) => Ok(result),
            Err(error) => {
                *self.session.borrow_mut() = None;
                Err(error)
            }
        }
    }
}

fn rpc_response(body: &str, id: i64) -> Result<McpValue, McpError> {
    let response = if body.trim_start().starts_with('{') {
        McpValue::from_json(body)
            .map_err(|_| McpError::internal_error("invalid JSON-RPC response"))?
    } else {
        let normalized = body.replace("\r\n", "\n");
        let mut found = None;
        for event in normalized.split("\n\n") {
            let data = event
                .lines()
                .filter_map(|line| {
                    line.strip_prefix("data:")
                        .map(|value| value.strip_prefix(' ').unwrap_or(value))
                })
                .collect::<Vec<_>>()
                .join("\n");
            if data.is_empty() {
                continue;
            }
            let value = McpValue::from_json(&data)
                .map_err(|_| McpError::internal_error("invalid SSE data"))?;
            if value.as_map().and_then(|map| map.get("id")) == Some(&McpValue::Int(id)) {
                found = Some(value);
                break;
            }
        }
        found.ok_or_else(|| {
            McpError::internal_error("SSE stream did not contain matching RPC response")
        })?
    };
    let map = response
        .as_map()
        .ok_or_else(|| McpError::internal_error("RPC response must be an object"))?;
    if map.get("jsonrpc").and_then(McpValue::as_str) != Some("2.0")
        || map.get("id") != Some(&McpValue::Int(id))
    {
        return Err(McpError::internal_error(
            "RPC response version or ID mismatch",
        ));
    }
    match (map.get("result"), map.get("error")) {
        (Some(result), None) => Ok(result.clone()),
        (None, Some(error)) => Err(McpError::internal_error(format!(
            "remote MCP error: {}",
            error.to_json()
        ))),
        _ => Err(McpError::internal_error(
            "RPC response requires exactly one result or error",
        )),
    }
}

impl McpApp for RemoteMcpApp {
    fn manifest(&self) -> &AppManifest {
        &self.manifest
    }
    fn tools(&self) -> Vec<McpTool> {
        self.tools.clone()
    }
    fn resources(&self) -> Vec<McpResource> {
        self.resources
            .iter()
            .enumerate()
            .map(|(index, resource)| {
                let mut resource = resource.clone();
                resource.uri = format!("mcp://{}/resources/{}", self.manifest.app_id, index);
                resource
            })
            .collect()
    }
    fn prompts(&self) -> Vec<McpPrompt> {
        self.prompts.clone()
    }
    fn skills(&self) -> Vec<AppSkill> {
        self.skills.clone()
    }
    fn call_tool(
        &mut self,
        tool_name: &str,
        args: &BTreeMap<String, McpValue>,
    ) -> Result<McpValue, McpError> {
        self.guard
            .enforce_capability(&AppCapability::ExposeMcpTools)
            .map_err(|error| error.to_mcp_error())?;
        let prefix = format!("{}.", self.manifest.app_id);
        let name = tool_name.strip_prefix(&prefix).unwrap_or(tool_name);
        if !self.tools.iter().any(|tool| tool.name == name) {
            return Err(McpError::method_not_found("unknown remote tool"));
        }
        self.rpc(
            "tools/call",
            BTreeMap::from([
                ("name".into(), McpValue::Text(name.into())),
                ("arguments".into(), McpValue::Map(args.clone())),
            ]),
        )
    }
    fn read_resource(&self, uri: &str) -> Result<McpResourceContent, McpError> {
        self.guard
            .enforce_capability(&AppCapability::ExposeMcpResources)
            .map_err(|error| error.to_mcp_error())?;
        let prefix = format!("mcp://{}/resources/", self.manifest.app_id);
        let index = uri
            .strip_prefix(&prefix)
            .and_then(|index| index.parse::<usize>().ok())
            .ok_or_else(|| McpError::resource_not_found("unknown remote resource alias"))?;
        let resource = self
            .resources
            .get(index)
            .ok_or_else(|| McpError::resource_not_found("unknown remote resource"))?;
        let result = self.rpc(
            "resources/read",
            BTreeMap::from([("uri".into(), McpValue::Text(resource.uri.clone()))]),
        )?;
        // This OS interface supports a single text resource; reject other shapes
        // explicitly rather than dropping multiple content blocks or binary data.
        let contents = result.as_map().and_then(|map| map.get("contents"));
        if let Some(McpValue::Array(contents)) = contents {
            if contents.len() == 1 {
                if let Some(content) = contents[0].as_map() {
                    if let (Some(remote_uri), Some(text)) = (
                        content.get("uri").and_then(McpValue::as_str),
                        content.get("text").and_then(McpValue::as_str),
                    ) {
                        if remote_uri != resource.uri {
                            return Err(McpError::internal_error("remote resource URI mismatch"));
                        }
                        return Ok(McpResourceContent::new(
                            uri,
                            content
                                .get("mimeType")
                                .and_then(McpValue::as_str)
                                .unwrap_or("text/plain"),
                            text,
                        ));
                    }
                }
            }
        }
        Err(McpError::internal_error(
            "remote resource must contain one text content block",
        ))
    }
    fn get_prompt(&self, name: &str, args: &BTreeMap<String, String>) -> Result<String, McpError> {
        self.guard
            .enforce_capability(&AppCapability::ExposeMcpPrompts)
            .map_err(|error| error.to_mcp_error())?;
        let prefix = format!("{}.", self.manifest.app_id);
        let name = name.strip_prefix(&prefix).unwrap_or(name);
        if !self.prompts.iter().any(|prompt| prompt.name == name) {
            return Err(McpError::method_not_found("unknown remote prompt"));
        }
        self.rpc(
            "prompts/get",
            BTreeMap::from([
                ("name".into(), McpValue::Text(name.into())),
                (
                    "arguments".into(),
                    McpValue::Map(
                        args.iter()
                            .map(|(key, value)| (key.clone(), McpValue::Text(value.clone())))
                            .collect(),
                    ),
                ),
            ]),
        )
        .map(|result| result.to_json())
    }
}
