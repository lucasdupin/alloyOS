use crate::transport::{HttpClient, HttpRequest};
use crate::{
    AppCapability, AppManifest, AppSkill, McpApp, McpError, McpParamSpec, McpPrompt, McpResource,
    McpResourceContent, McpSandboxError, McpTool, McpValue, RuntimeCapabilityGuard,
};
use alloc::{collections::BTreeMap, format, rc::Rc, string::String, vec, vec::Vec};
use core::{cell::{Cell, RefCell}, task::Poll};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RemoteConnectionStatus {
    Disconnected,
    Connecting,
    Ready,
    AuthorizationRequired { challenge: String },
    Error,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ConnectPhase { Probe, Initialize, Initialized, Tools, Ready }
struct TrustedBearer(String);
impl Drop for TrustedBearer {fn drop(&mut self) {unsafe {self.0.as_bytes_mut().fill(0);}}}
struct PendingWire { id: i64, method: String }

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
    phase: ConnectPhase,
    modern: bool,
    status: RemoteConnectionStatus,
    pending_wire: Option<PendingWire>,
    pending_call: Option<(String, BTreeMap<String, McpValue>)>,
    catalog: Vec<McpTool>,
    catalog_cursor: Option<String>,
    catalog_pages: usize,
    catalog_bytes: usize,
    credential_reference: Option<String>,
    trusted_bearer: Option<TrustedBearer>,
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
            phase: ConnectPhase::Probe,
            modern: true,
            status: RemoteConnectionStatus::Disconnected,
            pending_wire: None,
            pending_call: None,
            catalog: Vec::new(),
            catalog_cursor: None,
            catalog_pages: 0,
            catalog_bytes: 0,
            credential_reference: None,
            trusted_bearer: None,
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
                McpValue::Text("2025-11-25".into()),
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
            .filter(|version| matches!(*version, "2025-11-25" | "2025-06-18" | "2025-03-26"))
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

    pub fn connection_status(&self) -> RemoteConnectionStatus { self.status.clone() }
    pub fn negotiated_protocol_version(&self)->Option<String> {
        if self.phase!=ConnectPhase::Ready {None}
        else if self.modern {Some("2026-07-28".into())}
        else {self.session.borrow().as_ref().map(|(version,_)|version.clone())}
    }

    /// Trusted OS configuration only. Public opaque references identify private
    /// OS records; they are never resolved by the host HTTP relay.
    pub fn set_credential_reference(&mut self, reference: Option<String>) -> Result<(), McpError> {
        if self.credential_reference==reference {return Ok(());}
        if reference.as_ref().is_some_and(|value| value.is_empty() || value.len() > 256
            || !value.bytes().all(|b| b.is_ascii_alphanumeric() || b"-_".contains(&b))) {
            return Err(McpError::invalid_params("invalid credential reference"));
        }
        self.cancel_pending_tool();
        self.credential_reference = reference;
        Ok(())
    }

    /// OS account service only; never expose this setter through model/JS tools.
    pub fn set_trusted_bearer(&mut self, bearer: Option<String>) -> Result<(),McpError> {
        if bearer.as_ref().is_some_and(|v|v.is_empty()||v.len()>crate::transport::MAX_BEARER_BYTES||!v.bytes().all(|b|b.is_ascii_graphic())) {
            return Err(McpError::invalid_params("invalid account credential"));
        }
        if self.trusted_bearer.as_ref().map(|v|&v.0)==bearer.as_ref() {return Ok(());}
        self.cancel_pending_tool();
        self.trusted_bearer=bearer.map(TrustedBearer);
        Ok(())
    }

    fn poll_send(&mut self, method: &str, params: BTreeMap<String,McpValue>, notification: bool)
        -> Poll<Result<(McpValue,Vec<(String,String)>),McpError>> {
        if let Err(error) = self.guard.enforce_capability(&AppCapability::ScopedNetwork) {
            return Poll::Ready(Err(error.to_mcp_error()));
        }
        let Some(client) = self.client.clone() else {
            return Poll::Ready(Err(McpError::internal_error("HTTP transport unavailable")));
        };
        let response;
        let id;
        let mut params=params;
        if let Some(pending) = &self.pending_wire {
            if pending.method != method {
                return Poll::Ready(Err(McpError::invalid_params("different remote operation pending")));
            }
            id = pending.id;
            response = client.borrow_mut().poll_request();
        } else {
            id = self.next_id.get();
            let Some(next) = id.checked_add(1) else {
                return Poll::Ready(Err(McpError::internal_error("RPC request ID exhausted")));
            };
            self.next_id.set(next);
            if self.modern {
                params.insert("_meta".into(),McpValue::Map(BTreeMap::from([
                    ("io.modelcontextprotocol/protocolVersion".into(),McpValue::Text("2026-07-28".into())),
                    ("io.modelcontextprotocol/clientCapabilities".into(),McpValue::Map(BTreeMap::new())),
                    ("io.modelcontextprotocol/clientInfo".into(),McpValue::Map(BTreeMap::from([
                        ("name".into(),McpValue::Text("alloyOS".into())),("version".into(),McpValue::Text("0.1.0".into()))]))),
                ])));
            }
            let mut rpc = BTreeMap::from([
                ("jsonrpc".into(), McpValue::Text("2.0".into())),
                ("method".into(), McpValue::Text(method.into())),
                ("params".into(),McpValue::Map(params)),
            ]);
            if !notification { rpc.insert("id".into(),McpValue::Int(id)); }
            let mut headers = vec![("Content-Type".into(),"application/json".into()),
                ("Accept".into(),"application/json, text/event-stream".into())];
            if self.modern {
                headers.push(("MCP-Protocol-Version".into(),"2026-07-28".into()));
                headers.push(("Mcp-Method".into(),method.into()));
                if let Some(name)=rpc.get("params").and_then(McpValue::as_map).and_then(|p|p.get("name").or_else(||p.get("uri"))).and_then(McpValue::as_str) {
                    headers.push(("Mcp-Name".into(),encode_header_value(name)));
                }
                if method=="tools/call" {
                    if let Err(error)=append_tool_headers(&self.tools,&rpc,&mut headers) {return Poll::Ready(Err(error));}
                }
            } else if let Some((version,session)) = &*self.session.borrow() {
                headers.push(("MCP-Protocol-Version".into(),version.clone()));
                if let Some(session) = session { headers.push(("Mcp-Session-Id".into(),session.clone())); }
            }
            if let Some(bearer)=&self.trusted_bearer {headers.push(("X-AlloyOS-Trusted-Bearer".into(),bearer.0.clone()));}
            if let Some(reference) = &self.credential_reference {
                headers.push(("X-AlloyOS-App-Id".into(),self.manifest.app_id.clone()));
                headers.push(("X-AlloyOS-Credential-Ref".into(),reference.clone()));
            }
            let request = HttpRequest { method:"POST", url:self.manifest.endpoint.clone().unwrap_or_default(),
                headers, body:McpValue::Map(rpc).to_json() };
            self.pending_wire = Some(PendingWire {id,method:method.into()});
            response = client.borrow_mut().start_request(&request);
        }
        let response = match response { Poll::Pending => return Poll::Pending, Poll::Ready(response) => response };
        self.pending_wire = None;
        let response = match response { Ok(response) => response, Err(error) => return Poll::Ready(Err(error)) };
        if response.status==400 && self.phase==ConnectPhase::Probe {
            let error=McpValue::from_json(&response.body).ok().and_then(|v|v.as_map().and_then(|m|m.get("error")).cloned());
            let code=error.as_ref().and_then(McpValue::as_map).and_then(|m|m.get("code")).and_then(McpValue::as_i64);
            let recognized=matches!(code,Some(-32020|-32021|-32022));
            let legacy_supported=error.as_ref().and_then(McpValue::as_map).and_then(|m|m.get("data")).and_then(McpValue::as_map)
                .and_then(|m|m.get("supported")).and_then(McpValue::as_array).is_some_and(|versions|
                    versions.iter().filter_map(McpValue::as_str).any(|v|matches!(v,"2025-11-25"|"2025-06-18"|"2025-03-26")));
            if !recognized || (code==Some(-32022) && legacy_supported) {
                self.modern=false;self.phase=ConnectPhase::Initialize;return Poll::Pending;
            }
        }
        if response.status == 401 {
            self.status = RemoteConnectionStatus::AuthorizationRequired { challenge: response.headers.iter()
                .find(|(name,_)|name.eq_ignore_ascii_case("www-authenticate"))
                .map(|(_,value)|value.clone()).unwrap_or_default() };
            return Poll::Ready(Err(McpError::internal_error("remote account authorization required")));
        }
        if response.status == 404 && !self.modern {
            *self.session.borrow_mut() = None;
            self.phase = ConnectPhase::Initialize;
            self.status = RemoteConnectionStatus::Disconnected;
            return Poll::Ready(Err(McpError::internal_error("remote session expired; reconnect before retrying")));
        }
        if !(200..300).contains(&response.status) {
            return Poll::Ready(Err(McpError::internal_error(format!("remote HTTP status {}",response.status))));
        }
        if notification {
            if response.status != 202 || !response.body.is_empty() {
                return Poll::Ready(Err(McpError::internal_error("invalid notification acknowledgement")));
            }
            Poll::Ready(Ok((McpValue::Null,response.headers)))
        } else {
            Poll::Ready(rpc_response(&response.body,id).map(|result|(result,response.headers)))
        }
    }

    /// Each poll performs at most one bounded transport step. No synchronous
    /// transport fallback or retries of potentially completed tool calls.
    pub fn poll_connect(&mut self) -> Poll<Result<Vec<McpTool>,McpError>> {
        if self.phase == ConnectPhase::Ready { return Poll::Ready(Ok(self.tools.clone())); }
        self.status = RemoteConnectionStatus::Connecting;
        let method;
        let params;
        match self.phase {
            ConnectPhase::Probe => {method="tools/list";params=BTreeMap::new();}
            ConnectPhase::Initialize => {
                method = "initialize";
                params = BTreeMap::from([
                    ("protocolVersion".into(),McpValue::Text("2025-11-25".into())),
                    ("capabilities".into(),McpValue::Map(BTreeMap::new())),
                    ("clientInfo".into(),McpValue::Map(BTreeMap::from([
                        ("name".into(),McpValue::Text("alloyOS".into())),
                        ("version".into(),McpValue::Text("0.1.0".into()))]))),
                ]);
            }
            ConnectPhase::Initialized => { method="notifications/initialized"; params=BTreeMap::new(); }
            ConnectPhase::Tools => { method="tools/list"; params = self.catalog_cursor.as_ref()
                .map(|cursor|BTreeMap::from([("cursor".into(),McpValue::Text(cursor.clone()))])).unwrap_or_default(); }
            ConnectPhase::Ready => unreachable!(),
        }
        let (result,headers) = match self.poll_send(method,params,self.phase == ConnectPhase::Initialized) {
            Poll::Pending => return Poll::Pending,
            Poll::Ready(Err(error)) => {
                if !matches!(self.status, RemoteConnectionStatus::AuthorizationRequired {..}) { self.status=RemoteConnectionStatus::Error; }
                self.phase=if self.modern {ConnectPhase::Probe}else{ConnectPhase::Initialize};
                *self.session.borrow_mut()=None;
                self.catalog.clear();self.catalog_cursor=None;self.catalog_pages=0;self.catalog_bytes=0;
                return Poll::Ready(Err(error));
            }
            Poll::Ready(Ok(response)) => response,
        };
        if self.phase==ConnectPhase::Probe {self.phase=ConnectPhase::Tools;}
        let processed = (|| -> Result<(),McpError> {
            match self.phase {
                ConnectPhase::Initialize => {
                    let map=result.as_map().ok_or_else(||McpError::internal_error("invalid MCP initialize result"))?;
                    let version=map.get("protocolVersion").and_then(McpValue::as_str)
                        .filter(|v|matches!(*v,"2025-11-25"|"2025-06-18"|"2025-03-26"))
                        .ok_or_else(||McpError::internal_error("unsupported MCP protocol version"))?;
                    if map.get("capabilities").and_then(McpValue::as_map).and_then(|c|c.get("tools")).and_then(McpValue::as_map).is_none()
                        || !map.get("serverInfo").and_then(McpValue::as_map).is_some_and(|info|
                            info.get("name").and_then(McpValue::as_str).is_some() && info.get("version").and_then(McpValue::as_str).is_some()) {
                        return Err(McpError::internal_error("remote server does not advertise tools"));
                    }
                    let session=headers.iter().find(|(name,_)|name.eq_ignore_ascii_case("mcp-session-id")).map(|(_,value)|value.clone());
                    if session.as_ref().is_some_and(|v| v.is_empty() || v.len()>256 || !v.bytes().all(|b|(0x21..=0x7e).contains(&b))) {
                        return Err(McpError::internal_error("invalid MCP session ID"));
                    }
                    *self.session.borrow_mut()=Some((version.into(),session));
                    self.phase=ConnectPhase::Initialized;
                    self.catalog.clear();self.catalog_cursor=None;self.catalog_pages=0;self.catalog_bytes=0;
                }
                ConnectPhase::Initialized => self.phase=ConnectPhase::Tools,
                ConnectPhase::Tools => {
                    let map=result.as_map().ok_or_else(||McpError::internal_error("invalid tools/list result"))?;
                    let tools=map.get("tools").ok_or_else(||McpError::internal_error("missing tools/list tools"))?;
                    let json=tools.to_json();
                    self.catalog_bytes=self.catalog_bytes.saturating_add(json.len());
                    if self.catalog_bytes>crate::transport::MAX_HTTP_BODY_BYTES {return Err(McpError::internal_error("remote tool catalog byte budget exceeded"));}
                    let mut page=parse_remote_tools(&json).map_err(McpError::internal_error)?;
                    if self.modern {page.retain(|tool|tool_header_bindings(tool).is_ok());}
                    if self.catalog.len()+page.len()>1000 || page.iter().any(|tool|self.catalog.iter().any(|old|old.name==tool.name)) {
                        return Err(McpError::internal_error("remote tool catalog exceeds bounds or duplicates names"));
                    }
                    self.catalog.extend(page);self.catalog_pages+=1;
                    self.catalog_cursor=match map.get("nextCursor") {
                        None => None,
                        Some(McpValue::Text(cursor)) if !cursor.is_empty() && cursor.len()<=1024 && self.catalog_pages<16 => Some(cursor.clone()),
                        _ => return Err(McpError::internal_error("invalid or excessive tools/list cursor")),
                    };
                    if self.catalog_cursor.is_none() {
                        self.tools=core::mem::take(&mut self.catalog);
                        self.phase=ConnectPhase::Ready;self.status=RemoteConnectionStatus::Ready;
                    }
                }
                ConnectPhase::Probe=>unreachable!(),
                ConnectPhase::Ready => {},
            }
            Ok(())
        })();
        if let Err(error)=processed { self.cancel_pending_tool();self.status=RemoteConnectionStatus::Error;return Poll::Ready(Err(error)); }
        if self.phase==ConnectPhase::Ready {Poll::Ready(Ok(self.tools.clone()))} else {Poll::Pending}
    }
}

fn encode_header_value(value:&str)->String {
    if value.trim()==value && value.bytes().all(|b|(0x20..=0x7e).contains(&b))
        && !(value.starts_with("=?base64?") && value.ends_with("?=")) {return value.into();}
    const ABC:&[u8]=b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut result=String::from("=?base64?");
    for chunk in value.as_bytes().chunks(3) {
        let n=((chunk[0]as u32)<<16)|((chunk.get(1).copied().unwrap_or(0)as u32)<<8)|chunk.get(2).copied().unwrap_or(0)as u32;
        result.push(ABC[(n>>18)as usize]as char);result.push(ABC[((n>>12)&63)as usize]as char);
        result.push(if chunk.len()>1 {ABC[((n>>6)&63)as usize]as char}else{'='});
        result.push(if chunk.len()>2 {ABC[(n&63)as usize]as char}else{'='});
    }
    result.push_str("?=");result
}

type HeaderBinding=(String,Vec<String>,String);
fn tool_header_bindings(tool:&McpTool)->Result<Vec<HeaderBinding>,McpError> {
    fn visit(value:&McpValue,path:&mut Vec<String>,reachable:bool,depth:usize,out:&mut Vec<HeaderBinding>)->Result<(),McpError> {
        if depth>32 {return Err(McpError::invalid_params("tool schema nesting exceeds limit"));}
        if let Some(map)=value.as_map() {
            if let Some(header)=map.get("x-mcp-header") {
                let header=header.as_str().ok_or_else(||McpError::invalid_params("invalid tool header annotation"))?;
                let kind=map.get("type").and_then(McpValue::as_str).unwrap_or("");
                if !reachable || path.is_empty() || header.len()>64 || header.is_empty()
                    || !header.bytes().all(|b|b.is_ascii_alphanumeric()||b"!#$%&'*+-.^_`|~".contains(&b))
                    || !matches!(kind,"string"|"integer"|"boolean") || out.iter().any(|(name,_,_)|name.eq_ignore_ascii_case(header)) {
                    return Err(McpError::invalid_params("invalid tool header annotation"));
                }
                out.push((header.into(),path.clone(),kind.into()));
            }
            for (key,child) in map {
                if key=="properties" {
                    if let Some(properties)=child.as_map() {for (key,child) in properties {
                        path.push(key.clone());visit(child,path,reachable,depth+1,out)?;path.pop();
                    }}
                } else if key!="x-mcp-header" {visit(child,path,false,depth+1,out)?;}
            }
        } else if let Some(array)=value.as_array() {for child in array {visit(child,path,false,depth+1,out)?;}}
        Ok(())
    }
    let mut bindings=Vec::new();
    if let Some(schema)=&tool.input_schema {visit(schema,&mut Vec::new(),true,0,&mut bindings)?;}
    Ok(bindings)
}

fn append_tool_headers(tools:&[McpTool],rpc:&BTreeMap<String,McpValue>,headers:&mut Vec<(String,String)>)->Result<(),McpError> {
    let params=rpc.get("params").and_then(McpValue::as_map).ok_or_else(||McpError::invalid_params("missing tool parameters"))?;
    let name=params.get("name").and_then(McpValue::as_str).unwrap_or("");
    let tool=tools.iter().find(|tool|tool.name==name).ok_or_else(||McpError::method_not_found("unknown remote tool"))?;
    for (header,path,kind) in tool_header_bindings(tool)? {
        let mut value=params.get("arguments");
        for key in path {value=value.and_then(McpValue::as_map).and_then(|map|map.get(&key));}
        let value=match value {None|Some(McpValue::Null)=>continue,Some(value)=>value};
        let text=match kind.as_str() {
            "string"=>value.as_str().map(String::from),
            "boolean"=>value.as_bool().map(|value|if value {"true".into()}else{"false".into()}),
            "integer"=>value.as_i64().filter(|value|(-9_007_199_254_740_991..=9_007_199_254_740_991).contains(value)).map(|value|format!("{value}")),
            _=>None,
        }.ok_or_else(||McpError::invalid_params("invalid mirrored tool argument"))?;
        headers.push((format!("Mcp-Param-{header}"),encode_header_value(&text)));
    }
    Ok(())
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
        (Some(result), None) => {
            if result.as_map().and_then(|map|map.get("resultType")).and_then(McpValue::as_str)
                .is_some_and(|kind|kind!="complete") {
                return Err(McpError::internal_error("remote result requires unsupported client input or extension"));
            }
            Ok(result.clone())
        },
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
    fn poll_connect(&mut self) -> Poll<Result<Vec<McpTool>,McpError>> { RemoteMcpApp::poll_connect(self) }
    fn connection_status(&self) -> RemoteConnectionStatus { RemoteMcpApp::connection_status(self) }
    fn set_trusted_bearer(&mut self, bearer:Option<String>)->Result<(),McpError> {RemoteMcpApp::set_trusted_bearer(self,bearer)}
    fn set_credential_reference(&mut self, reference:Option<String>) -> Result<(),McpError> {
        RemoteMcpApp::set_credential_reference(self,reference)
    }
    fn poll_call_tool(&mut self, tool_name:&str,args:&BTreeMap<String,McpValue>) -> Poll<Result<McpValue,McpError>> {
        if let Err(error)=self.guard.enforce_capability(&AppCapability::ExposeMcpTools) {return Poll::Ready(Err(error.to_mcp_error()));}
        if let Some((name,pending_args))=&self.pending_call {
            if name!=tool_name || pending_args!=args {return Poll::Ready(Err(McpError::invalid_params("different remote call pending")));}
        } else {self.pending_call=Some((tool_name.into(),args.clone()));}
        match self.poll_connect() {
            Poll::Pending=>return Poll::Pending,
            Poll::Ready(Err(error))=>{self.pending_call=None;return Poll::Ready(Err(error));},
            Poll::Ready(Ok(_))=>{},
        }
        let prefix=format!("{}.",self.manifest.app_id);
        let name=tool_name.strip_prefix(&prefix).unwrap_or(tool_name);
        if !self.tools.iter().any(|tool|tool.name==name) {
            self.pending_call=None;return Poll::Ready(Err(McpError::method_not_found("unknown remote tool")));
        }
        let response=self.poll_send("tools/call",BTreeMap::from([
            ("name".into(),McpValue::Text(name.into())),("arguments".into(),McpValue::Map(args.clone()))]),false);
        if response.is_ready() {self.pending_call=None;}
        match response {
            Poll::Pending=>Poll::Pending,
            Poll::Ready(Ok((value,_)))=>Poll::Ready(Ok(value)),
            Poll::Ready(Err(error))=>{
                if !matches!(self.status,RemoteConnectionStatus::AuthorizationRequired{..}) {self.status=RemoteConnectionStatus::Error;}
                self.phase=if self.modern {ConnectPhase::Probe}else{ConnectPhase::Initialize};*self.session.borrow_mut()=None;
                Poll::Ready(Err(error))
            }
        }
    }
    fn cancel_pending_tool(&mut self) {
        if self.pending_wire.take().is_some() {if let Some(client)=&self.client {client.borrow_mut().cancel_request();}}
        self.pending_call=None;self.phase=ConnectPhase::Probe;self.modern=true;
        self.status=RemoteConnectionStatus::Disconnected;*self.session.borrow_mut()=None;
        self.catalog.clear();self.catalog_cursor=None;self.catalog_pages=0;self.catalog_bytes=0;
    }
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
