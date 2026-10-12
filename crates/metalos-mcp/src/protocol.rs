use alloc::collections::BTreeMap;
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec;
use alloc::vec::Vec;

/// Parameter specification for an MCP tool.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpParamSpec {
    pub name: String,
    pub param_type: String,
    pub required: bool,
    pub description: String,
}

impl McpParamSpec {
    pub fn new(
        name: impl Into<String>,
        param_type: impl Into<String>,
        required: bool,
        description: impl Into<String>,
    ) -> Self {
        Self {
            name: name.into(),
            param_type: param_type.into(),
            required,
            description: description.into(),
        }
    }

    pub fn to_mcp_value(&self) -> McpValue {
        let mut map = BTreeMap::new();
        map.insert("name".to_string(), McpValue::Text(self.name.clone()));
        map.insert("type".to_string(), McpValue::Text(self.param_type.clone()));
        map.insert("required".to_string(), McpValue::Bool(self.required));
        map.insert(
            "description".to_string(),
            McpValue::Text(self.description.clone()),
        );
        McpValue::Map(map)
    }
}

/// Model Context Protocol tool definition.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpTool {
    pub name: String,
    pub description: String,
    pub parameters: Vec<McpParamSpec>,
}

impl McpTool {
    pub fn new(
        name: impl Into<String>,
        description: impl Into<String>,
        parameters: Vec<McpParamSpec>,
    ) -> Self {
        Self {
            name: name.into(),
            description: description.into(),
            parameters,
        }
    }

    pub fn to_mcp_value(&self) -> McpValue {
        let mut map = BTreeMap::new();
        map.insert("name".to_string(), McpValue::Text(self.name.clone()));
        map.insert(
            "description".to_string(),
            McpValue::Text(self.description.clone()),
        );
        map.insert(
            "parameters".to_string(),
            McpValue::Array(self.parameters.iter().map(|p| p.to_mcp_value()).collect()),
        );
        McpValue::Map(map)
    }
}

/// Model Context Protocol resource descriptor.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpResource {
    pub uri: String,
    pub name: String,
    pub mime_type: String,
    pub description: String,
}

impl McpResource {
    pub fn new(
        uri: impl Into<String>,
        name: impl Into<String>,
        mime_type: impl Into<String>,
        description: impl Into<String>,
    ) -> Self {
        Self {
            uri: uri.into(),
            name: name.into(),
            mime_type: mime_type.into(),
            description: description.into(),
        }
    }

    pub fn to_mcp_value(&self) -> McpValue {
        let mut map = BTreeMap::new();
        map.insert("uri".to_string(), McpValue::Text(self.uri.clone()));
        map.insert("name".to_string(), McpValue::Text(self.name.clone()));
        map.insert(
            "mimeType".to_string(),
            McpValue::Text(self.mime_type.clone()),
        );
        map.insert(
            "description".to_string(),
            McpValue::Text(self.description.clone()),
        );
        McpValue::Map(map)
    }
}

/// Returned payload when reading an MCP resource.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpResourceContent {
    pub uri: String,
    pub mime_type: String,
    pub text: String,
}

impl McpResourceContent {
    pub fn new(
        uri: impl Into<String>,
        mime_type: impl Into<String>,
        text: impl Into<String>,
    ) -> Self {
        Self {
            uri: uri.into(),
            mime_type: mime_type.into(),
            text: text.into(),
        }
    }

    pub fn to_mcp_value(&self) -> McpValue {
        let mut map = BTreeMap::new();
        map.insert("uri".to_string(), McpValue::Text(self.uri.clone()));
        map.insert(
            "mimeType".to_string(),
            McpValue::Text(self.mime_type.clone()),
        );
        map.insert("text".to_string(), McpValue::Text(self.text.clone()));
        McpValue::Map(map)
    }
}

/// Model Context Protocol prompt template descriptor.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpPrompt {
    pub name: String,
    pub description: String,
    pub arguments: Vec<String>,
}

impl McpPrompt {
    pub fn new(
        name: impl Into<String>,
        description: impl Into<String>,
        arguments: Vec<String>,
    ) -> Self {
        Self {
            name: name.into(),
            description: description.into(),
            arguments,
        }
    }

    pub fn to_mcp_value(&self) -> McpValue {
        let mut map = BTreeMap::new();
        map.insert("name".to_string(), McpValue::Text(self.name.clone()));
        map.insert(
            "description".to_string(),
            McpValue::Text(self.description.clone()),
        );
        map.insert(
            "arguments".to_string(),
            McpValue::Array(
                self.arguments
                    .iter()
                    .map(|a| McpValue::Text(a.clone()))
                    .collect(),
            ),
        );
        McpValue::Map(map)
    }
}

/// Structured JSON-compatible value used across MCP messages.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum McpValue {
    Null,
    Bool(bool),
    Int(i64),
    Text(String),
    Array(Vec<McpValue>),
    Map(BTreeMap<String, McpValue>),
}

impl McpValue {
    pub fn as_str(&self) -> Option<&str> {
        match self {
            McpValue::Text(s) => Some(s.as_str()),
            _ => None,
        }
    }

    pub fn as_i64(&self) -> Option<i64> {
        match self {
            McpValue::Int(v) => Some(*v),
            _ => None,
        }
    }

    pub fn as_bool(&self) -> Option<bool> {
        match self {
            McpValue::Bool(b) => Some(*b),
            _ => None,
        }
    }

    pub fn as_array(&self) -> Option<&[McpValue]> {
        match self {
            McpValue::Array(arr) => Some(arr.as_slice()),
            _ => None,
        }
    }

    pub fn as_map(&self) -> Option<&BTreeMap<String, McpValue>> {
        match self {
            McpValue::Map(m) => Some(m),
            _ => None,
        }
    }

    /// Serialize this `McpValue` into compact JSON text.
    pub fn to_json(&self) -> String {
        let mut out = String::new();
        self.write_json(&mut out);
        out
    }

    fn write_json(&self, out: &mut String) {
        match self {
            McpValue::Null => out.push_str("null"),
            McpValue::Bool(true) => out.push_str("true"),
            McpValue::Bool(false) => out.push_str("false"),
            McpValue::Int(n) => out.push_str(&n.to_string()),
            McpValue::Text(s) => write_escaped_json_string(s, out),
            McpValue::Array(items) => {
                out.push('[');
                for (i, item) in items.iter().enumerate() {
                    if i > 0 {
                        out.push(',');
                    }
                    item.write_json(out);
                }
                out.push(']');
            }
            McpValue::Map(entries) => {
                out.push('{');
                for (i, (k, v)) in entries.iter().enumerate() {
                    if i > 0 {
                        out.push(',');
                    }
                    write_escaped_json_string(k, out);
                    out.push(':');
                    v.write_json(out);
                }
                out.push('}');
            }
        }
    }

    /// Parse a JSON string into an `McpValue`.
    pub fn from_json(input: &str) -> Result<Self, McpError> {
        let mut parser = JsonParser::new(input.as_bytes());
        let val = parser.parse_value()?;
        parser.skip_whitespace();
        if parser.pos != parser.bytes.len() {
            return Err(McpError::parse_error("trailing characters after JSON value"));
        }
        Ok(val)
    }
}

fn write_escaped_json_string(s: &str, out: &mut String) {
    out.push('"');
    for ch in s.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => {
                out.push_str(&format!("\\u{:04x}", c as u32));
            }
            c => out.push(c),
        }
    }
    out.push('"');
}

struct JsonParser<'a> {
    bytes: &'a [u8],
    pos: usize,
}

impl<'a> JsonParser<'a> {
    fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, pos: 0 }
    }

    fn skip_whitespace(&mut self) {
        while self.pos < self.bytes.len() {
            match self.bytes[self.pos] {
                b' ' | b'\t' | b'\n' | b'\r' => self.pos += 1,
                _ => break,
            }
        }
    }

    fn peek(&mut self) -> Option<u8> {
        self.skip_whitespace();
        self.bytes.get(self.pos).copied()
    }

    fn parse_value(&mut self) -> Result<McpValue, McpError> {
        let b = self
            .peek()
            .ok_or_else(|| McpError::parse_error("unexpected end of JSON input"))?;
        match b {
            b'n' => self.parse_null(),
            b't' | b'f' => self.parse_bool(),
            b'"' => self.parse_string().map(McpValue::Text),
            b'[' => self.parse_array(),
            b'{' => self.parse_object(),
            b'-' | b'0'..=b'9' => self.parse_int(),
            _ => Err(McpError::parse_error("unexpected token in JSON")),
        }
    }

    fn parse_null(&mut self) -> Result<McpValue, McpError> {
        if self.bytes[self.pos..].starts_with(b"null") {
            self.pos += 4;
            Ok(McpValue::Null)
        } else {
            Err(McpError::parse_error("invalid literal, expected null"))
        }
    }

    fn parse_bool(&mut self) -> Result<McpValue, McpError> {
        if self.bytes[self.pos..].starts_with(b"true") {
            self.pos += 4;
            Ok(McpValue::Bool(true))
        } else if self.bytes[self.pos..].starts_with(b"false") {
            self.pos += 5;
            Ok(McpValue::Bool(false))
        } else {
            Err(McpError::parse_error("invalid boolean literal"))
        }
    }

    fn parse_int(&mut self) -> Result<McpValue, McpError> {
        let start = self.pos;
        if self.bytes.get(self.pos) == Some(&b'-') {
            self.pos += 1;
        }
        let digits_start = self.pos;
        while self.pos < self.bytes.len() && self.bytes[self.pos].is_ascii_digit() {
            self.pos += 1;
        }
        if self.pos == digits_start {
            return Err(McpError::parse_error("expected digits in integer"));
        }
        let s = core::str::from_utf8(&self.bytes[start..self.pos])
            .map_err(|_| McpError::parse_error("invalid utf-8 in integer"))?;
        let val = s
            .parse::<i64>()
            .map_err(|_| McpError::parse_error("integer overflow or invalid number"))?;
        Ok(McpValue::Int(val))
    }

    fn parse_string(&mut self) -> Result<String, McpError> {
        self.skip_whitespace();
        if self.bytes.get(self.pos) != Some(&b'"') {
            return Err(McpError::parse_error("expected '\"' to start string"));
        }
        self.pos += 1;
        let mut out = Vec::new();
        while self.pos < self.bytes.len() {
            let b = self.bytes[self.pos];
            self.pos += 1;
            match b {
                b'"' => {
                    return String::from_utf8(out)
                        .map_err(|_| McpError::parse_error("invalid UTF-8 in JSON string"));
                }
                b'\\' => {
                    if self.pos >= self.bytes.len() {
                        return Err(McpError::parse_error("unterminated escape sequence"));
                    }
                    let esc = self.bytes[self.pos];
                    self.pos += 1;
                    match esc {
                        b'"' => out.push(b'"'),
                        b'\\' => out.push(b'\\'),
                        b'/' => out.push(b'/'),
                        b'n' => out.push(b'\n'),
                        b'r' => out.push(b'\r'),
                        b't' => out.push(b'\t'),
                        b'u' => {
                            if self.pos + 4 > self.bytes.len() {
                                return Err(McpError::parse_error("incomplete unicode escape"));
                            }
                            let hex = core::str::from_utf8(&self.bytes[self.pos..self.pos + 4])
                                .map_err(|_| McpError::parse_error("invalid unicode hex"))?;
                            let code = u32::from_str_radix(hex, 16)
                                .map_err(|_| McpError::parse_error("invalid unicode hex digits"))?;
                            let ch = char::from_u32(code)
                                .ok_or_else(|| McpError::parse_error("invalid unicode scalar"))?;
                            let mut buf = [0u8; 4];
                            out.extend_from_slice(ch.encode_utf8(&mut buf).as_bytes());
                            self.pos += 4;
                        }
                        _ => return Err(McpError::parse_error("unsupported escape character")),
                    }
                }
                other => out.push(other),
            }
        }
        Err(McpError::parse_error("unterminated JSON string"))
    }

    fn parse_array(&mut self) -> Result<McpValue, McpError> {
        self.pos += 1; // consume '['
        let mut items = Vec::new();
        if self.peek() == Some(b']') {
            self.pos += 1;
            return Ok(McpValue::Array(items));
        }
        loop {
            let val = self.parse_value()?;
            items.push(val);
            match self.peek() {
                Some(b',') => {
                    self.pos += 1;
                }
                Some(b']') => {
                    self.pos += 1;
                    return Ok(McpValue::Array(items));
                }
                _ => return Err(McpError::parse_error("expected ',' or ']' in array")),
            }
        }
    }

    fn parse_object(&mut self) -> Result<McpValue, McpError> {
        self.pos += 1; // consume '{'
        let mut map = BTreeMap::new();
        if self.peek() == Some(b'}') {
            self.pos += 1;
            return Ok(McpValue::Map(map));
        }
        loop {
            let key = self.parse_string()?;
            if self.peek() != Some(b':') {
                return Err(McpError::parse_error("expected ':' after object key"));
            }
            self.pos += 1;
            let val = self.parse_value()?;
            map.insert(key, val);
            match self.peek() {
                Some(b',') => {
                    self.pos += 1;
                }
                Some(b'}') => {
                    self.pos += 1;
                    return Ok(McpValue::Map(map));
                }
                _ => return Err(McpError::parse_error("expected ',' or '}' in object")),
            }
        }
    }
}

/// Standard JSON-RPC 2.0 / MCP error codes.
pub const ERR_PARSE_ERROR: i32 = -32700;
pub const ERR_INVALID_REQUEST: i32 = -32600;
pub const ERR_METHOD_NOT_FOUND: i32 = -32601;
pub const ERR_INVALID_PARAMS: i32 = -32602;
pub const ERR_INTERNAL_ERROR: i32 = -32603;
pub const ERR_SANDBOX_VIOLATION: i32 = -32001;
pub const ERR_RESOURCE_NOT_FOUND: i32 = -32002;

/// JSON-RPC 2.0 error payload for MCP responses.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpError {
    pub code: i32,
    pub message: String,
}

impl McpError {
    pub fn new(code: i32, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }

    pub fn parse_error(msg: impl Into<String>) -> Self {
        Self::new(ERR_PARSE_ERROR, msg)
    }

    pub fn invalid_request(msg: impl Into<String>) -> Self {
        Self::new(ERR_INVALID_REQUEST, msg)
    }

    pub fn method_not_found(msg: impl Into<String>) -> Self {
        Self::new(ERR_METHOD_NOT_FOUND, msg)
    }

    pub fn invalid_params(msg: impl Into<String>) -> Self {
        Self::new(ERR_INVALID_PARAMS, msg)
    }

    pub fn internal_error(msg: impl Into<String>) -> Self {
        Self::new(ERR_INTERNAL_ERROR, msg)
    }

    pub fn sandbox_violation(msg: impl Into<String>) -> Self {
        Self::new(ERR_SANDBOX_VIOLATION, msg)
    }

    pub fn resource_not_found(msg: impl Into<String>) -> Self {
        Self::new(ERR_RESOURCE_NOT_FOUND, msg)
    }

    pub fn to_mcp_value(&self) -> McpValue {
        let mut map = BTreeMap::new();
        map.insert("code".to_string(), McpValue::Int(self.code as i64));
        map.insert("message".to_string(), McpValue::Text(self.message.clone()));
        McpValue::Map(map)
    }
}

/// Supported Model Context Protocol methods.
pub mod methods {
    pub const INITIALIZE: &str = "initialize";
    pub const TOOLS_LIST: &str = "tools/list";
    pub const TOOLS_CALL: &str = "tools/call";
    pub const RESOURCES_LIST: &str = "resources/list";
    pub const RESOURCES_READ: &str = "resources/read";
    pub const PROMPTS_LIST: &str = "prompts/list";
    pub const PROMPTS_GET: &str = "prompts/get";
}

/// JSON-RPC 2.0 MCP Request envelope.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpRequest {
    pub jsonrpc: String,
    pub id: u64,
    pub method: String,
    pub params: BTreeMap<String, McpValue>,
}

impl McpRequest {
    pub fn new(id: u64, method: impl Into<String>, params: BTreeMap<String, McpValue>) -> Self {
        Self {
            jsonrpc: "2.0".to_string(),
            id,
            method: method.into(),
            params,
        }
    }

    pub fn to_mcp_value(&self) -> McpValue {
        let mut map = BTreeMap::new();
        map.insert("jsonrpc".to_string(), McpValue::Text(self.jsonrpc.clone()));
        map.insert("id".to_string(), McpValue::Int(self.id as i64));
        map.insert("method".to_string(), McpValue::Text(self.method.clone()));
        map.insert("params".to_string(), McpValue::Map(self.params.clone()));
        McpValue::Map(map)
    }

    pub fn to_json(&self) -> String {
        self.to_mcp_value().to_json()
    }

    pub fn from_json(input: &str) -> Result<Self, McpError> {
        let val = McpValue::from_json(input)?;
        let obj = val
            .as_map()
            .ok_or_else(|| McpError::invalid_request("MCP request must be a JSON object"))?;

        let jsonrpc = obj
            .get("jsonrpc")
            .and_then(|v| v.as_str())
            .ok_or_else(|| McpError::invalid_request("missing or invalid 'jsonrpc' field"))?;
        if jsonrpc != "2.0" {
            return Err(McpError::invalid_request("unsupported jsonrpc version"));
        }

        let id = obj
            .get("id")
            .and_then(|v| v.as_i64())
            .and_then(|v| u64::try_from(v).ok())
            .ok_or_else(|| McpError::invalid_request("missing or invalid 'id' field"))?;

        let method = obj
            .get("method")
            .and_then(|v| v.as_str())
            .ok_or_else(|| McpError::invalid_request("missing or invalid 'method' field"))?
            .to_string();

        let params = match obj.get("params") {
            Some(McpValue::Map(m)) => m.clone(),
            Some(McpValue::Null) | None => BTreeMap::new(),
            _ => {
                return Err(McpError::invalid_params(
                    "'params' field must be an object or null",
                ))
            }
        };

        Ok(Self {
            jsonrpc: jsonrpc.to_string(),
            id,
            method,
            params,
        })
    }
}

/// JSON-RPC 2.0 MCP Response envelope.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpResponse {
    pub jsonrpc: String,
    pub id: u64,
    pub result: Option<McpValue>,
    pub error: Option<McpError>,
}

impl McpResponse {
    pub fn success(id: u64, result: McpValue) -> Self {
        Self {
            jsonrpc: "2.0".to_string(),
            id,
            result: Some(result),
            error: None,
        }
    }

    pub fn error(id: u64, error: McpError) -> Self {
        Self {
            jsonrpc: "2.0".to_string(),
            id,
            result: None,
            error: Some(error),
        }
    }

    pub fn is_ok(&self) -> bool {
        self.error.is_none() && self.result.is_some()
    }

    pub fn to_mcp_value(&self) -> McpValue {
        let mut map = BTreeMap::new();
        map.insert("jsonrpc".to_string(), McpValue::Text(self.jsonrpc.clone()));
        map.insert("id".to_string(), McpValue::Int(self.id as i64));
        if let Some(ref res) = self.result {
            map.insert("result".to_string(), res.clone());
        }
        if let Some(ref err) = self.error {
            map.insert("error".to_string(), err.to_mcp_value());
        }
        McpValue::Map(map)
    }

    pub fn to_json(&self) -> String {
        self.to_mcp_value().to_json()
    }

    pub fn from_json(input: &str) -> Result<Self, McpError> {
        let val = McpValue::from_json(input)?;
        let obj = val
            .as_map()
            .ok_or_else(|| McpError::invalid_request("MCP response must be a JSON object"))?;

        let jsonrpc = obj
            .get("jsonrpc")
            .and_then(|v| v.as_str())
            .ok_or_else(|| McpError::invalid_request("missing 'jsonrpc'"))?
            .to_string();

        let id = obj
            .get("id")
            .and_then(|v| v.as_i64())
            .and_then(|v| u64::try_from(v).ok())
            .unwrap_or(0);

        let result = obj.get("result").cloned();
        let error = if let Some(err_val) = obj.get("error") {
            let err_obj = err_val
                .as_map()
                .ok_or_else(|| McpError::invalid_request("invalid error object"))?;
            let code = err_obj
                .get("code")
                .and_then(|v| v.as_i64())
                .unwrap_or(ERR_INTERNAL_ERROR as i64) as i32;
            let message = err_obj
                .get("message")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown error")
                .to_string();
            Some(McpError::new(code, message))
        } else {
            None
        };

        Ok(Self {
            jsonrpc,
            id,
            result,
            error,
        })
    }
}

/// Helper to build a single-key `BTreeMap<String, McpValue>`.
pub fn mcp_map(entries: &[(&str, McpValue)]) -> BTreeMap<String, McpValue> {
    let mut map = BTreeMap::new();
    for (k, v) in entries {
        map.insert((*k).to_string(), v.clone());
    }
    map
}

/// Helper to create a text content array response following MCP `tools/call` convention.
pub fn tool_text_result(text: impl Into<String>, structured: McpValue) -> McpValue {
    let content_item = McpValue::Map(mcp_map(&[
        ("type", McpValue::Text("text".to_string())),
        ("text", McpValue::Text(text.into())),
    ]));
    McpValue::Map(mcp_map(&[
        ("content", McpValue::Array(vec![content_item])),
        ("data", structured),
    ]))
}
