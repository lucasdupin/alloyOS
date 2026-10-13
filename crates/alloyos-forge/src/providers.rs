use alloc::collections::VecDeque;
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec;
use alloc::vec::Vec;

use crate::policy::{is_forbidden_shell_tool, SHELL_FREE_POLICY_MESSAGE};
use crate::provider::{
    extract_json_object_field, extract_json_string_field, json_escape, AiError, AiModelProvider,
    Message, Response, Role, TokenUsage, ToolCall, ToolSpec,
};

/// Builds a JSON Schema `"parameters"` object from a slice of `ToolParameter`s.
fn build_json_schema_parameters(
    params: &[crate::provider::ToolParameter],
    uppercase_types: bool,
) -> String {
    let obj_type = if uppercase_types { "OBJECT" } else { "object" };
    let mut props = Vec::with_capacity(params.len());
    let mut required = Vec::new();

    for p in params {
        let p_type = if uppercase_types {
            p.param_type.to_ascii_uppercase()
        } else {
            p.param_type.to_ascii_lowercase()
        };
        props.push(format!(
            "\"{}\":{{\"type\":\"{}\",\"description\":\"{}\"}}",
            json_escape(&p.name),
            json_escape(&p_type),
            json_escape(&p.description)
        ));
        if p.required {
            required.push(format!("\"{}\"", json_escape(&p.name)));
        }
    }

    format!(
        "{{\"type\":\"{}\",\"properties\":{{{}}},\"required\":[{}]}}",
        obj_type,
        props.join(","),
        required.join(",")
    )
}

// ============================================================================
// 1. Google Gemini Provider ("gemini", default model "gemini-3.8-flash")
// ============================================================================

/// First-class Google Gemini adapter supporting `streamGenerateContent?alt=sse`.
#[derive(Debug, Clone)]
pub struct GeminiProvider {
    pub model: String,
    pub api_key: Option<String>,
    pub last_request_json: Option<String>,
    queued_sse_streams: VecDeque<String>,
}

impl Default for GeminiProvider {
    fn default() -> Self {
        Self::new()
    }
}

impl GeminiProvider {
    pub const DEFAULT_MODEL: &'static str = crate::cloud::GEMINI_MODEL;

    pub fn new() -> Self {
        Self {
            model: Self::DEFAULT_MODEL.to_string(),
            api_key: None,
            last_request_json: None,
            queued_sse_streams: VecDeque::new(),
        }
    }

    pub fn with_model(model: impl Into<String>) -> Self {
        Self {
            model: model.into(),
            api_key: None,
            last_request_json: None,
            queued_sse_streams: VecDeque::new(),
        }
    }

    /// Returns the Google Gemini streaming SSE endpoint URL.
    pub fn endpoint_url(&self) -> String {
        format!(
            "https://generativelanguage.googleapis.com/v1beta/models/{}:streamGenerateContent?alt=sse",
            self.model
        )
    }

    /// Enqueues a raw SSE response stream for deterministic testing or bridged HTTPS transport.
    pub fn queue_sse_stream(&mut self, sse_payload: impl Into<String>) {
        self.queued_sse_streams.push_back(sse_payload.into());
    }

    /// Builds a Google Gemini `streamGenerateContent?alt=sse` JSON request body, including
    /// `systemInstruction`, `contents`, and `functionDeclarations` for tools.
    pub fn build_request_json(
        &self,
        messages: &[Message],
        tools: &[ToolSpec],
    ) -> Result<String, AiError> {
        for tool in tools {
            if is_forbidden_shell_tool(&tool.name) {
                return Err(AiError::ShellAccessForbidden(format!(
                    "GeminiProvider refused forbidden shell tool '{}'. {}",
                    tool.name, SHELL_FREE_POLICY_MESSAGE
                )));
            }
        }

        let mut system_parts = Vec::new();
        let mut content_items = Vec::new();

        for msg in messages {
            match msg.role {
                Role::System => {
                    system_parts.push(format!("{{\"text\":\"{}\"}}", json_escape(&msg.content)));
                }
                Role::User => {
                    content_items.push(format!(
                        "{{\"role\":\"user\",\"parts\":[{{\"text\":\"{}\"}}]}}",
                        json_escape(&msg.content)
                    ));
                }
                Role::Assistant => {
                    content_items.push(format!(
                        "{{\"role\":\"model\",\"parts\":[{{\"text\":\"{}\"}}]}}",
                        json_escape(&msg.content)
                    ));
                }
                Role::Tool => {
                    let call_id = msg.tool_call_id.as_deref().unwrap_or("tool");
                    content_items.push(format!(
                        "{{\"role\":\"user\",\"parts\":[{{\"functionResponse\":{{\"name\":\"{}\",\"response\":{{\"result\":\"{}\"}}}}}}]}}",
                        json_escape(call_id),
                        json_escape(&msg.content)
                    ));
                }
            }
        }

        let mut root_fields = Vec::new();
        if !system_parts.is_empty() {
            root_fields.push(format!(
                "\"systemInstruction\":{{\"parts\":[{}]}}",
                system_parts.join(",")
            ));
        }

        root_fields.push(format!("\"contents\":[{}]", content_items.join(",")));

        if !tools.is_empty() {
            let mut decls = Vec::with_capacity(tools.len());
            for t in tools {
                let params_schema = build_json_schema_parameters(&t.parameters, true);
                decls.push(format!(
                    "{{\"name\":\"{}\",\"description\":\"{}\",\"parameters\":{}}}",
                    json_escape(&t.name),
                    json_escape(&t.description),
                    params_schema
                ));
            }
            root_fields.push(format!(
                "\"tools\":[{{\"functionDeclarations\":[{}]}}]",
                decls.join(",")
            ));
        }

        Ok(format!("{{{}}}", root_fields.join(",")))
    }

    /// Parses a Google Gemini `streamGenerateContent?alt=sse` stream, invoking `on_token` for
    /// each `candidates[0].content.parts[].text` chunk and extracting `functionCall` payloads.
    pub fn parse_sse_stream(
        &self,
        sse_stream: &str,
        on_token: &mut dyn FnMut(&str),
    ) -> Result<Response, AiError> {
        let mut full_text = String::new();
        let mut tool_calls = Vec::new();
        let mut chunks_parsed = 0u32;

        for raw_line in sse_stream.lines() {
            let line = raw_line.trim();
            if line.is_empty() || line.starts_with(':') {
                continue;
            }
            let Some(data) = line.strip_prefix("data:") else {
                continue;
            };
            let payload = data.trim();
            if payload.is_empty() || payload == "[DONE]" {
                continue;
            }
            chunks_parsed += 1;

            if let Some(fc_obj) = extract_json_object_field(payload, "functionCall") {
                let name = extract_json_string_field(&fc_obj, "name").ok_or_else(|| {
                    AiError::ParseError("Gemini functionCall missing 'name'".to_string())
                })?;
                if is_forbidden_shell_tool(&name) {
                    return Err(AiError::ShellAccessForbidden(format!(
                        "Gemini stream attempted forbidden shell tool '{}'",
                        name
                    )));
                }
                let args_json =
                    extract_json_object_field(&fc_obj, "args").unwrap_or_else(|| "{}".to_string());
                let call_id = format!("gemini-call-{}", tool_calls.len() + 1);
                tool_calls.push(ToolCall::new(call_id, name, args_json));
            }

            if let Some(text_chunk) = extract_json_string_field(payload, "text") {
                on_token(&text_chunk);
                full_text.push_str(&text_chunk);
            }
        }

        Ok(Response::new(
            full_text,
            tool_calls,
            self.model.clone(),
            TokenUsage::new(chunks_parsed * 8, chunks_parsed * 12),
        ))
    }
}

impl AiModelProvider for GeminiProvider {
    fn name(&self) -> &'static str {
        "gemini"
    }

    fn stream_chat(
        &mut self,
        messages: &[Message],
        tools: &[ToolSpec],
        on_token: &mut dyn FnMut(&str),
    ) -> Result<Response, AiError> {
        let req_json = self.build_request_json(messages, tools)?;
        self.last_request_json = Some(req_json);

        if let Some(sse_stream) = self.queued_sse_streams.pop_front() {
            return self.parse_sse_stream(&sse_stream, on_token);
        }

        Err(AiError::NetworkError(
            "provider transport unavailable".to_string(),
        ))
    }
}

// ============================================================================
// 2. Anthropic Claude Provider ("claude", default model "claude-sonnet-4-6")
// ============================================================================

/// First-class Anthropic Claude adapter supporting `/v1/messages` SSE streaming.
#[derive(Debug, Clone)]
pub struct ClaudeProvider {
    pub model: String,
    pub max_tokens: u32,
    pub api_key: Option<String>,
    pub last_request_json: Option<String>,
    queued_sse_streams: VecDeque<String>,
}

impl Default for ClaudeProvider {
    fn default() -> Self {
        Self::new()
    }
}

impl ClaudeProvider {
    pub const DEFAULT_MODEL: &'static str = crate::cloud::CLAUDE_MODEL;

    pub fn new() -> Self {
        Self {
            model: Self::DEFAULT_MODEL.to_string(),
            max_tokens: 4096,
            api_key: None,
            last_request_json: None,
            queued_sse_streams: VecDeque::new(),
        }
    }

    pub fn with_model(model: impl Into<String>) -> Self {
        Self {
            model: model.into(),
            max_tokens: 4096,
            api_key: None,
            last_request_json: None,
            queued_sse_streams: VecDeque::new(),
        }
    }

    /// Returns the Anthropic Messages API endpoint URL.
    pub fn endpoint_url(&self) -> &'static str {
        "https://api.anthropic.com/v1/messages"
    }

    /// Enqueues a raw SSE response stream for deterministic testing or bridged HTTPS transport.
    pub fn queue_sse_stream(&mut self, sse_payload: impl Into<String>) {
        self.queued_sse_streams.push_back(sse_payload.into());
    }

    /// Builds an Anthropic `/v1/messages` streaming JSON request body.
    pub fn build_request_json(
        &self,
        messages: &[Message],
        tools: &[ToolSpec],
    ) -> Result<String, AiError> {
        for tool in tools {
            if is_forbidden_shell_tool(&tool.name) {
                return Err(AiError::ShellAccessForbidden(format!(
                    "ClaudeProvider refused forbidden shell tool '{}'. {}",
                    tool.name, SHELL_FREE_POLICY_MESSAGE
                )));
            }
        }

        let mut system_prompt = String::new();
        let mut msg_items = Vec::new();

        for msg in messages {
            match msg.role {
                Role::System => {
                    if !system_prompt.is_empty() {
                        system_prompt.push('\n');
                    }
                    system_prompt.push_str(&msg.content);
                }
                Role::User => {
                    msg_items.push(format!(
                        "{{\"role\":\"user\",\"content\":\"{}\"}}",
                        json_escape(&msg.content)
                    ));
                }
                Role::Assistant => {
                    msg_items.push(format!(
                        "{{\"role\":\"assistant\",\"content\":\"{}\"}}",
                        json_escape(&msg.content)
                    ));
                }
                Role::Tool => {
                    let tool_id = msg.tool_call_id.as_deref().unwrap_or("toolu_01");
                    msg_items.push(format!(
                        "{{\"role\":\"user\",\"content\":[{{\"type\":\"tool_result\",\"tool_use_id\":\"{}\",\"content\":\"{}\"}}]}}",
                        json_escape(tool_id),
                        json_escape(&msg.content)
                    ));
                }
            }
        }

        let mut fields = vec![
            format!("\"model\":\"{}\"", json_escape(&self.model)),
            format!("\"max_tokens\":{}", self.max_tokens),
            "\"stream\":true".to_string(),
        ];

        if !system_prompt.is_empty() {
            fields.push(format!("\"system\":\"{}\"", json_escape(&system_prompt)));
        }

        fields.push(format!("\"messages\":[{}]", msg_items.join(",")));

        if !tools.is_empty() {
            let mut tool_defs = Vec::with_capacity(tools.len());
            for t in tools {
                let input_schema = build_json_schema_parameters(&t.parameters, false);
                tool_defs.push(format!(
                    "{{\"name\":\"{}\",\"description\":\"{}\",\"input_schema\":{}}}",
                    json_escape(&t.name),
                    json_escape(&t.description),
                    input_schema
                ));
            }
            fields.push(format!("\"tools\":[{}]", tool_defs.join(",")));
        }

        Ok(format!("{{{}}}", fields.join(",")))
    }

    /// Parses an Anthropic `/v1/messages` SSE stream, extracting `content_block_delta`
    /// (`text_delta` and `input_json_delta`) frames as well as `tool_use` blocks.
    pub fn parse_sse_stream(
        &self,
        sse_stream: &str,
        on_token: &mut dyn FnMut(&str),
    ) -> Result<Response, AiError> {
        let mut full_text = String::new();
        let mut tool_calls: Vec<ToolCall> = Vec::new();
        let mut events_count = 0u32;

        for raw_line in sse_stream.lines() {
            let line = raw_line.trim();
            if line.is_empty() || line.starts_with("event:") || line.starts_with(':') {
                continue;
            }
            let Some(data) = line.strip_prefix("data:") else {
                continue;
            };
            let payload = data.trim();
            if payload.is_empty() || payload == "[DONE]" {
                continue;
            }
            events_count += 1;

            let frame_type = extract_json_string_field(payload, "type").unwrap_or_default();

            if frame_type == "content_block_start" {
                if let Some(block_obj) = extract_json_object_field(payload, "content_block") {
                    let block_type =
                        extract_json_string_field(&block_obj, "type").unwrap_or_default();
                    if block_type == "tool_use" {
                        let id = extract_json_string_field(&block_obj, "id")
                            .unwrap_or_else(|| format!("toolu_{}", tool_calls.len() + 1));
                        let name = extract_json_string_field(&block_obj, "name")
                            .unwrap_or_else(|| "unknown".to_string());
                        if is_forbidden_shell_tool(&name) {
                            return Err(AiError::ShellAccessForbidden(format!(
                                "Claude stream attempted forbidden shell tool '{}'",
                                name
                            )));
                        }
                        tool_calls.push(ToolCall::new(id, name, String::new()));
                    }
                }
            } else if frame_type == "content_block_delta"
                || payload.contains("\"text_delta\"")
                || payload.contains("\"input_json_delta\"")
            {
                let delta_obj = extract_json_object_field(payload, "delta")
                    .unwrap_or_else(|| payload.to_string());
                let delta_type = extract_json_string_field(&delta_obj, "type").unwrap_or_default();

                if delta_type == "text_delta" {
                    if let Some(text_chunk) = extract_json_string_field(&delta_obj, "text") {
                        on_token(&text_chunk);
                        full_text.push_str(&text_chunk);
                    }
                } else if delta_type == "input_json_delta" {
                    if let Some(partial_json) =
                        extract_json_string_field(&delta_obj, "partial_json")
                    {
                        if tool_calls.is_empty() {
                            let fallback_name = extract_json_string_field(payload, "name")
                                .unwrap_or_else(|| "tool".to_string());
                            if is_forbidden_shell_tool(&fallback_name) {
                                return Err(AiError::ShellAccessForbidden(format!(
                                    "Claude stream attempted forbidden shell tool '{}'",
                                    fallback_name
                                )));
                            }
                            tool_calls.push(ToolCall::new("toolu_1", fallback_name, String::new()));
                        }
                        if let Some(last_call) = tool_calls.last_mut() {
                            last_call.arguments_json.push_str(&partial_json);
                        }
                    }
                }
            }
        }

        for tc in &mut tool_calls {
            if tc.arguments_json.is_empty() {
                tc.arguments_json = "{}".to_string();
            }
        }

        Ok(Response::new(
            full_text,
            tool_calls,
            self.model.clone(),
            TokenUsage::new(events_count * 6, events_count * 10),
        ))
    }
}

impl AiModelProvider for ClaudeProvider {
    fn name(&self) -> &'static str {
        "claude"
    }

    fn stream_chat(
        &mut self,
        messages: &[Message],
        tools: &[ToolSpec],
        on_token: &mut dyn FnMut(&str),
    ) -> Result<Response, AiError> {
        let req_json = self.build_request_json(messages, tools)?;
        self.last_request_json = Some(req_json);

        if let Some(sse_stream) = self.queued_sse_streams.pop_front() {
            return self.parse_sse_stream(&sse_stream, on_token);
        }

        Err(AiError::NetworkError(
            "provider transport unavailable".to_string(),
        ))
    }
}

// ============================================================================
// 3. OpenAI ChatGPT Provider ("chatgpt", default model "gpt-4.1")
// ============================================================================

/// First-class OpenAI ChatGPT adapter supporting `/v1/chat/completions` streaming SSE.
#[derive(Debug, Clone)]
pub struct ChatGptProvider {
    pub model: String,
    pub api_key: Option<String>,
    pub last_request_json: Option<String>,
    queued_sse_streams: VecDeque<String>,
}

impl Default for ChatGptProvider {
    fn default() -> Self {
        Self::new()
    }
}

impl ChatGptProvider {
    pub const DEFAULT_MODEL: &'static str = crate::cloud::OPENAI_MODEL;

    pub fn new() -> Self {
        Self {
            model: Self::DEFAULT_MODEL.to_string(),
            api_key: None,
            last_request_json: None,
            queued_sse_streams: VecDeque::new(),
        }
    }

    pub fn with_model(model: impl Into<String>) -> Self {
        Self {
            model: model.into(),
            api_key: None,
            last_request_json: None,
            queued_sse_streams: VecDeque::new(),
        }
    }

    /// Returns the OpenAI Chat Completions API endpoint URL.
    pub fn endpoint_url(&self) -> &'static str {
        "https://api.openai.com/v1/chat/completions"
    }

    /// Enqueues a raw SSE response stream for deterministic testing or bridged HTTPS transport.
    pub fn queue_sse_stream(&mut self, sse_payload: impl Into<String>) {
        self.queued_sse_streams.push_back(sse_payload.into());
    }

    /// Builds an OpenAI `/v1/chat/completions` streaming JSON request body.
    pub fn build_request_json(
        &self,
        messages: &[Message],
        tools: &[ToolSpec],
    ) -> Result<String, AiError> {
        for tool in tools {
            if is_forbidden_shell_tool(&tool.name) {
                return Err(AiError::ShellAccessForbidden(format!(
                    "ChatGptProvider refused forbidden shell tool '{}'. {}",
                    tool.name, SHELL_FREE_POLICY_MESSAGE
                )));
            }
        }

        let mut msg_items = Vec::with_capacity(messages.len());
        for msg in messages {
            match msg.role {
                Role::System | Role::User | Role::Assistant => {
                    msg_items.push(format!(
                        "{{\"role\":\"{}\",\"content\":\"{}\"}}",
                        msg.role.as_str(),
                        json_escape(&msg.content)
                    ));
                }
                Role::Tool => {
                    let call_id = msg.tool_call_id.as_deref().unwrap_or("call_1");
                    msg_items.push(format!(
                        "{{\"role\":\"tool\",\"tool_call_id\":\"{}\",\"content\":\"{}\"}}",
                        json_escape(call_id),
                        json_escape(&msg.content)
                    ));
                }
            }
        }

        let mut fields = vec![
            format!("\"model\":\"{}\"", json_escape(&self.model)),
            "\"stream\":true".to_string(),
            format!("\"messages\":[{}]", msg_items.join(",")),
        ];

        if !tools.is_empty() {
            let mut tool_defs = Vec::with_capacity(tools.len());
            for t in tools {
                let params_schema = build_json_schema_parameters(&t.parameters, false);
                tool_defs.push(format!(
                    "{{\"type\":\"function\",\"function\":{{\"name\":\"{}\",\"description\":\"{}\",\"parameters\":{}}}}}",
                    json_escape(&t.name),
                    json_escape(&t.description),
                    params_schema
                ));
            }
            fields.push(format!("\"tools\":[{}]", tool_defs.join(",")));
        }

        Ok(format!("{{{}}}", fields.join(",")))
    }

    /// Parses an OpenAI `/v1/chat/completions` SSE stream, extracting `choices[0].delta.content`
    /// and `choices[0].delta.tool_calls` frames and terminating cleanly on `data: [DONE]`.
    pub fn parse_sse_stream(
        &self,
        sse_stream: &str,
        on_token: &mut dyn FnMut(&str),
    ) -> Result<Response, AiError> {
        let mut full_text = String::new();
        let mut tool_calls: Vec<ToolCall> = Vec::new();
        let mut frames_count = 0u32;

        for raw_line in sse_stream.lines() {
            let line = raw_line.trim();
            if line.is_empty() || line.starts_with(':') {
                continue;
            }
            let Some(data) = line.strip_prefix("data:") else {
                continue;
            };
            let payload = data.trim();
            if payload.is_empty() {
                continue;
            }
            if payload == "[DONE]" {
                break;
            }
            frames_count += 1;

            let delta_obj =
                extract_json_object_field(payload, "delta").unwrap_or_else(|| payload.to_string());

            if let Some(content_token) = extract_json_string_field(&delta_obj, "content") {
                if !content_token.is_empty() {
                    on_token(&content_token);
                    full_text.push_str(&content_token);
                }
            }

            if delta_obj.contains("\"tool_calls\"") || delta_obj.contains("\"function\"") {
                let fn_obj = extract_json_object_field(&delta_obj, "function")
                    .unwrap_or_else(|| delta_obj.clone());
                let maybe_id = extract_json_string_field(&delta_obj, "id");
                let maybe_name = extract_json_string_field(&fn_obj, "name");
                let maybe_args = extract_json_string_field(&fn_obj, "arguments");

                if let Some(name) = maybe_name {
                    if is_forbidden_shell_tool(&name) {
                        return Err(AiError::ShellAccessForbidden(format!(
                            "ChatGPT stream attempted forbidden shell tool '{}'",
                            name
                        )));
                    }
                    let id = maybe_id.unwrap_or_else(|| format!("call_{}", tool_calls.len() + 1));
                    let initial_args = maybe_args.unwrap_or_default();
                    tool_calls.push(ToolCall::new(id, name, initial_args));
                } else if let Some(args_fragment) = maybe_args {
                    if let Some(last_call) = tool_calls.last_mut() {
                        last_call.arguments_json.push_str(&args_fragment);
                    } else {
                        let id = maybe_id.unwrap_or_else(|| "call_1".to_string());
                        tool_calls.push(ToolCall::new(id, "unknown", args_fragment));
                    }
                }
            }
        }

        for tc in &mut tool_calls {
            if tc.arguments_json.is_empty() {
                tc.arguments_json = "{}".to_string();
            }
        }

        Ok(Response::new(
            full_text,
            tool_calls,
            self.model.clone(),
            TokenUsage::new(frames_count * 5, frames_count * 9),
        ))
    }
}

impl AiModelProvider for ChatGptProvider {
    fn name(&self) -> &'static str {
        "chatgpt"
    }

    fn stream_chat(
        &mut self,
        messages: &[Message],
        tools: &[ToolSpec],
        on_token: &mut dyn FnMut(&str),
    ) -> Result<Response, AiError> {
        let req_json = self.build_request_json(messages, tools)?;
        self.last_request_json = Some(req_json);

        if let Some(sse_stream) = self.queued_sse_streams.pop_front() {
            return self.parse_sse_stream(&sse_stream, on_token);
        }

        Err(AiError::NetworkError(
            "provider transport unavailable".to_string(),
        ))
    }
}

// ============================================================================
