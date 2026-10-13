use alloc::string::{String, ToString};
use alloc::vec::Vec;
use core::fmt;

/// Conversation participant role across all frontier model providers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    System,
    User,
    Assistant,
    Tool,
}

impl Role {
    pub const fn as_str(self) -> &'static str {
        match self {
            Role::System => "system",
            Role::User => "user",
            Role::Assistant => "assistant",
            Role::Tool => "tool",
        }
    }
}

/// A structured tool invocation requested by an AI model.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    pub arguments_json: String,
}

impl ToolCall {
    pub fn new(
        id: impl Into<String>,
        name: impl Into<String>,
        arguments_json: impl Into<String>,
    ) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
            arguments_json: arguments_json.into(),
        }
    }
}

/// A single message in the conversation transcript.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Message {
    pub role: Role,
    pub content: String,
    pub tool_call_id: Option<String>,
    pub tool_calls: Vec<ToolCall>,
}

impl Message {
    pub fn system(content: impl Into<String>) -> Self {
        Self {
            role: Role::System,
            content: content.into(),
            tool_call_id: None,
            tool_calls: Vec::new(),
        }
    }

    pub fn user(content: impl Into<String>) -> Self {
        Self {
            role: Role::User,
            content: content.into(),
            tool_call_id: None,
            tool_calls: Vec::new(),
        }
    }

    pub fn assistant(content: impl Into<String>) -> Self {
        Self {
            role: Role::Assistant,
            content: content.into(),
            tool_call_id: None,
            tool_calls: Vec::new(),
        }
    }

    pub fn assistant_with_tools(content: impl Into<String>, tool_calls: Vec<ToolCall>) -> Self {
        Self {
            role: Role::Assistant,
            content: content.into(),
            tool_call_id: None,
            tool_calls,
        }
    }

    pub fn tool_result(tool_call_id: impl Into<String>, content: impl Into<String>) -> Self {
        Self {
            role: Role::Tool,
            content: content.into(),
            tool_call_id: Some(tool_call_id.into()),
            tool_calls: Vec::new(),
        }
    }
}

/// Parameter descriptor for a native OS or MCP tool.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolParameter {
    pub name: String,
    pub param_type: String,
    pub description: String,
    pub required: bool,
}

impl ToolParameter {
    pub fn new(
        name: impl Into<String>,
        param_type: impl Into<String>,
        description: impl Into<String>,
        required: bool,
    ) -> Self {
        Self {
            name: name.into(),
            param_type: param_type.into(),
            description: description.into(),
            required,
        }
    }
}

/// Schema specification for a tool exposed to the AI model runtime.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolSpec {
    pub name: String,
    pub description: String,
    pub parameters: Vec<ToolParameter>,
}

impl ToolSpec {
    pub fn new(
        name: impl Into<String>,
        description: impl Into<String>,
        parameters: Vec<ToolParameter>,
    ) -> Self {
        Self {
            name: name.into(),
            description: description.into(),
            parameters,
        }
    }
}

/// Token accounting telemetry returned by model completions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct TokenUsage {
    pub prompt_tokens: u32,
    pub completion_tokens: u32,
    pub total_tokens: u32,
}

impl TokenUsage {
    pub const fn new(prompt_tokens: u32, completion_tokens: u32) -> Self {
        Self {
            prompt_tokens,
            completion_tokens,
            total_tokens: prompt_tokens.saturating_add(completion_tokens),
        }
    }
}

/// Unified model response containing streamed text, tool calls, model metadata, and token usage.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Response {
    pub text: String,
    pub tool_calls: Vec<ToolCall>,
    pub model_name: String,
    pub usage: TokenUsage,
}

impl Response {
    pub fn new(
        text: impl Into<String>,
        tool_calls: Vec<ToolCall>,
        model_name: impl Into<String>,
        usage: TokenUsage,
    ) -> Self {
        Self {
            text: text.into(),
            tool_calls,
            model_name: model_name.into(),
            usage,
        }
    }
}

/// Errors produced by the AI runtime, provider adapters, or OS policy guards.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AiError {
    /// Resident local model load or execution error; never falls back to cloud silently.
    LocalInferenceError(String),
    /// Attempted to register or execute a POSIX shell command/tool or `!` escape in a shell-free OS.
    ShellAccessForbidden(String),
    /// Requested model provider or identifier is not recognized.
    UnknownModel(String),
    /// Requested tool does not exist in the native or MCP tool registry.
    UnknownTool(String),
    /// Tool arguments failed validation or JSON parsing.
    InvalidToolArguments(String),
    /// Transport or HTTP/SSE framing error.
    NetworkError(String),
    /// Provider payload could not be parsed.
    ParseError(String),
    /// `/etc/ai.conf` configuration error.
    ConfigError(String),
    /// Architectural policy violation (e.g., third-party app attempting direct UI creation).
    PolicyViolation(String),
}

impl fmt::Display for AiError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            AiError::LocalInferenceError(msg) => write!(f, "local inference error: {}", msg),
            AiError::ShellAccessForbidden(msg) => write!(f, "shell access forbidden: {}", msg),
            AiError::UnknownModel(model) => write!(f, "unknown AI model provider: {}", model),
            AiError::UnknownTool(tool) => write!(f, "unknown tool: {}", tool),
            AiError::InvalidToolArguments(msg) => write!(f, "invalid tool arguments: {}", msg),
            AiError::NetworkError(msg) => write!(f, "network error: {}", msg),
            AiError::ParseError(msg) => write!(f, "parse error: {}", msg),
            AiError::ConfigError(msg) => write!(f, "config error: {}", msg),
            AiError::PolicyViolation(msg) => write!(f, "policy violation: {}", msg),
        }
    }
}

/// Unified pluggable frontier model provider trait (Forge).
pub trait AiModelProvider {
    fn name(&self) -> &'static str;
    fn stream_chat(
        &mut self,
        messages: &[Message],
        tools: &[ToolSpec],
        on_token: &mut dyn FnMut(&str),
    ) -> Result<Response, AiError>;
}

/// Escapes a string for embedding inside a JSON string literal.
pub fn json_escape(input: &str) -> String {
    let mut out = String::with_capacity(input.len() + 8);
    for ch in input.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => {
                let code = c as u32;
                out.push_str("\\u00");
                let hi = (code >> 4) & 0xF;
                let lo = code & 0xF;
                out.push(char::from_digit(hi, 16).unwrap_or('0'));
                out.push(char::from_digit(lo, 16).unwrap_or('0'));
            }
            c => out.push(c),
        }
    }
    out
}

/// Extracts the string value of a JSON key `"key": "value"` from a JSON object slice,
/// unescaping standard JSON escape sequences.
pub fn extract_json_string_field(json: &str, key: &str) -> Option<String> {
    let mut pattern = String::with_capacity(key.len() + 3);
    pattern.push('"');
    pattern.push_str(key);
    pattern.push('"');

    let mut search_start = 0;
    while let Some(pos) = json[search_start..].find(&pattern) {
        let abs_pos = search_start + pos;
        let after_key = &json[abs_pos + pattern.len()..];
        let trimmed = after_key.trim_start();
        if let Some(after_colon) = trimmed.strip_prefix(':') {
            let val_trimmed = after_colon.trim_start();
            if let Some(after_quote) = val_trimmed.strip_prefix('"') {
                return parse_json_quoted_string(after_quote).map(|(val, _)| val);
            }
        }
        search_start = abs_pos + pattern.len();
    }
    None
}

/// Extracts a raw JSON object `{...}` assigned to `"key": { ... }`.
pub fn extract_json_object_field(json: &str, key: &str) -> Option<String> {
    let mut pattern = String::with_capacity(key.len() + 3);
    pattern.push('"');
    pattern.push_str(key);
    pattern.push('"');

    let mut search_start = 0;
    while let Some(pos) = json[search_start..].find(&pattern) {
        let abs_pos = search_start + pos;
        let after_key = &json[abs_pos + pattern.len()..];
        let trimmed = after_key.trim_start();
        if let Some(after_colon) = trimmed.strip_prefix(':') {
            let val_trimmed = after_colon.trim_start();
            if val_trimmed.starts_with('{') {
                if let Some(obj_slice) = extract_balanced_braces(val_trimmed) {
                    return Some(obj_slice.to_string());
                }
            }
        }
        search_start = abs_pos + pattern.len();
    }
    None
}

/// Parses the contents of a JSON string starting immediately after the opening `"`.
/// Returns the unescaped `String` and the byte index immediately following the closing `"`.
pub fn parse_json_quoted_string(s: &str) -> Option<(String, usize)> {
    let mut out = String::new();
    let mut chars = s.char_indices();
    while let Some((idx, ch)) = chars.next() {
        match ch {
            '"' => return Some((out, idx + 1)),
            '\\' => {
                let (_, esc) = chars.next()?;
                match esc {
                    '"' => out.push('"'),
                    '\\' => out.push('\\'),
                    '/' => out.push('/'),
                    'n' => out.push('\n'),
                    'r' => out.push('\r'),
                    't' => out.push('\t'),
                    'b' => out.push('\u{0008}'),
                    'f' => out.push('\u{000C}'),
                    'u' => {
                        let mut hex = 0u32;
                        for _ in 0..4 {
                            let (_, h) = chars.next()?;
                            hex = (hex << 4) | h.to_digit(16)?;
                        }
                        if let Some(decoded) = char::from_u32(hex) {
                            out.push(decoded);
                        }
                    }
                    other => out.push(other),
                }
            }
            other => out.push(other),
        }
    }
    None
}

/// Extracts a balanced `{ ... }` substring starting at index 0 of `s`.
pub fn extract_balanced_braces(s: &str) -> Option<&str> {
    if !s.starts_with('{') {
        return None;
    }
    let mut depth = 0usize;
    let mut in_string = false;
    let mut escape = false;

    for (idx, ch) in s.char_indices() {
        if in_string {
            if escape {
                escape = false;
            } else if ch == '\\' {
                escape = true;
            } else if ch == '"' {
                in_string = false;
            }
            continue;
        }

        match ch {
            '"' => in_string = true,
            '{' => depth += 1,
            '}' => {
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    return Some(&s[..=idx]);
                }
            }
            _ => {}
        }
    }
    None
}
