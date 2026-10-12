use alloc::collections::VecDeque;
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec;
use alloc::vec::Vec;

use crate::provider::{
    extract_json_object_field, extract_json_string_field, json_escape, AiError, AiModelProvider,
    Message, Response, Role, TokenUsage, ToolCall, ToolSpec,
};
use crate::tools::{
    is_forbidden_shell_tool, validate_console_input_no_shell, NativeToolRegistry,
    ToolExecutionOutput, SHELL_FREE_POLICY_MESSAGE,
};
use crate::ui_synth::{AgentUiSynthesizer, SynthesizedUiView};

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

/// Intelligent bare-metal local execution responder used when running offline without an external
/// HTTPS SSE stream queued. Inspects the user's intent, streams tokens via `on_token`, and emits
/// structured native OS `ToolCall`s while strictly refusing shell commands.
fn offline_bare_metal_responder(
    provider_label: &str,
    model_name: &str,
    messages: &[Message],
    tools: &[ToolSpec],
    on_token: &mut dyn FnMut(&str),
) -> Result<Response, AiError> {
    let last_user_msg = messages
        .iter()
        .rev()
        .find(|m| m.role == Role::User)
        .map(|m| m.content.as_str())
        .unwrap_or("");

    validate_console_input_no_shell(last_user_msg)?;

    let lower = last_user_msg.to_ascii_lowercase();
    let mut tool_calls = Vec::new();
    let reply_text: String;

    let has_tool = |name: &str| tools.iter().any(|t| t.name == name);

    if (lower.contains("status") || lower.contains("memory") || lower.contains("telemetry"))
        && has_tool("sys_status")
    {
        tool_calls.push(ToolCall::new(
            format!("{}-call-1", provider_label),
            "sys_status",
            "{}",
        ));
        reply_text = format!(
            "[{}] Inspecting metalOS ARM64 system telemetry and synthesizing Mercury dashboard.",
            provider_label
        );
    } else if (lower.contains("read ") || lower.contains("/etc/ai.conf")) && has_tool("fs_read") {
        let path = if lower.contains("/etc/schedules.conf") {
            "/etc/schedules.conf"
        } else {
            "/etc/ai.conf"
        };
        tool_calls.push(ToolCall::new(
            format!("{}-call-1", provider_label),
            "fs_read",
            format!("{{\"path\":\"{}\"}}", path),
        ));
        reply_text = format!(
            "[{}] Reading '{}' from CobaltFS via structured fs_read tool.",
            provider_label, path
        );
    } else if (lower.contains("schedule") || lower.contains("cron") || lower.contains("every"))
        && has_tool("script_schedule")
    {
        tool_calls.push(ToolCall::new(
            format!("{}-call-1", provider_label),
            "script_schedule",
            "{\"path\":\"/scripts/heartbeat.js\",\"trigger\":\"every:60s\"}",
        ));
        reply_text = format!(
            "[{}] Scheduling local JavaScript automation '/scripts/heartbeat.js' on ARM Generic Timer.",
            provider_label
        );
    } else if (lower.contains("script") || lower.contains("javascript") || lower.contains(".js"))
        && has_tool("script_run")
    {
        tool_calls.push(ToolCall::new(
            format!("{}-call-1", provider_label),
            "script_run",
            "{\"path\":\"/scripts/heartbeat.js\"}",
        ));
        reply_text = format!(
            "[{}] Executing local JavaScript '/scripts/heartbeat.js' in metalos-js runtime.",
            provider_label
        );
    } else if lower.contains("mcp") && has_tool("mcp_call_tool") {
        tool_calls.push(ToolCall::new(
            format!("{}-call-1", provider_label),
            "mcp_call_tool",
            "{\"app\":\"telemetry.mcp\",\"tool\":\"cluster_health\",\"args\":\"{}\"}",
        ));
        reply_text = format!(
            "[{}] Querying headless MCP server 'telemetry.mcp' and synthesizing Mercury UI view.",
            provider_label
        );
    } else if (lower.contains("list") || lower.contains("files")) && has_tool("fs_list") {
        tool_calls.push(ToolCall::new(
            format!("{}-call-1", provider_label),
            "fs_list",
            "{\"path\":\"/\"}",
        ));
        reply_text = format!(
            "[{}] Listing CobaltFS root directory entries via fs_list.",
            provider_label
        );
    } else {
        reply_text = format!(
            "[{} ({})] Ready on metalOS Mercury Console. Active tools: {}.",
            provider_label,
            model_name,
            tools.len()
        );
    }

    // Stream token-by-token (word chunks preserving spaces) through `on_token`.
    let mut start = 0;
    for (idx, ch) in reply_text.char_indices() {
        if ch == ' ' {
            let token = &reply_text[start..=idx];
            on_token(token);
            start = idx + 1;
        }
    }
    if start < reply_text.len() {
        on_token(&reply_text[start..]);
    }

    let prompt_tokens = (last_user_msg.len() as u32 / 4).max(1);
    let completion_tokens = (reply_text.len() as u32 / 4).max(1);
    Ok(Response::new(
        reply_text,
        tool_calls,
        model_name.to_string(),
        TokenUsage::new(prompt_tokens, completion_tokens),
    ))
}

// ============================================================================
// 1. Google Gemini Provider ("gemini", default model "gemini-2.5-pro")
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
    pub const DEFAULT_MODEL: &'static str = "gemini-2.5-pro";

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

        offline_bare_metal_responder("gemini", &self.model, messages, tools, on_token)
    }
}

// ============================================================================
// 2. Anthropic Claude Provider ("claude", default model "claude-sonnet-4-20250514")
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
    pub const DEFAULT_MODEL: &'static str = "claude-sonnet-4-20250514";

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
                            tool_calls.push(ToolCall::new(
                                "toolu_1",
                                fallback_name,
                                String::new(),
                            ));
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

        offline_bare_metal_responder("claude", &self.model, messages, tools, on_token)
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
    pub const DEFAULT_MODEL: &'static str = "gpt-4.1";

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

        offline_bare_metal_responder("chatgpt", &self.model, messages, tools, on_token)
    }
}

// ============================================================================
// 4. Runtime Model Router (`AiRuntime`) & `/etc/ai.conf` Persistence
// ============================================================================

/// Active frontier provider selection inside `AiRuntime`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActiveProvider {
    Gemini,
    Claude,
    ChatGpt,
}

impl ActiveProvider {
    pub const fn as_str(self) -> &'static str {
        match self {
            ActiveProvider::Gemini => "gemini",
            ActiveProvider::Claude => "claude",
            ActiveProvider::ChatGpt => "chatgpt",
        }
    }

    pub fn from_name(name: &str) -> Result<Self, AiError> {
        match name.trim().to_ascii_lowercase().as_str() {
            "gemini" | "google" => Ok(ActiveProvider::Gemini),
            "claude" | "anthropic" => Ok(ActiveProvider::Claude),
            "chatgpt" | "openai" | "gpt" => Ok(ActiveProvider::ChatGpt),
            other => Err(AiError::UnknownModel(other.to_string())),
        }
    }
}

/// Result of handling a single conversational or slash-command turn in the Mercury AI Console.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConsoleTurnOutput {
    pub active_provider: &'static str,
    pub active_model: String,
    pub response_text: String,
    pub tool_outputs: Vec<ToolExecutionOutput>,
    pub config_persisted: bool,
}

/// Pluggable AI Runtime (`libmetal-ai`) managing frontier model providers (`GeminiProvider`,
/// `ClaudeProvider`, `ChatGptProvider`), `/etc/ai.conf` persistence, conversation history,
/// shell-free native OS tools, and agent-exclusive Mercury UI synthesis.
#[derive(Debug, Clone)]
pub struct AiRuntime {
    active: ActiveProvider,
    pub gemini: GeminiProvider,
    pub claude: ClaudeProvider,
    pub chatgpt: ChatGptProvider,
    pub history: Vec<Message>,
    pub tool_registry: NativeToolRegistry,
    pub ui_synthesizer: AgentUiSynthesizer,
}

impl Default for AiRuntime {
    fn default() -> Self {
        Self::new()
    }
}

impl AiRuntime {
    pub const CONFIG_PATH: &'static str = "/etc/ai.conf";

    pub fn new() -> Self {
        let mut runtime = Self {
            active: ActiveProvider::Gemini,
            gemini: GeminiProvider::new(),
            claude: ClaudeProvider::new(),
            chatgpt: ChatGptProvider::new(),
            history: vec![Message::system(
                "You are the metalOS Mercury AI Agent running on bare-metal ARM64. \
                 POSIX shells (sh, bash) do not exist for the agent; use structured OS tools, \
                 local JavaScript scripts, and headless MCP apps. Only you can synthesize UI.",
            )],
            tool_registry: NativeToolRegistry::new(),
            ui_synthesizer: AgentUiSynthesizer::new(),
        };
        runtime.persist_config_to_vfs();
        runtime
    }

    /// Returns the short identifier of the currently active model provider (`"gemini"`, `"claude"`, `"chatgpt"`).
    pub fn active_provider_name(&self) -> &'static str {
        self.active.as_str()
    }

    /// Returns the model identifier string of the currently active provider.
    pub fn active_model_id(&self) -> &str {
        match self.active {
            ActiveProvider::Gemini => &self.gemini.model,
            ActiveProvider::Claude => &self.claude.model,
            ActiveProvider::ChatGpt => &self.chatgpt.model,
        }
    }

    /// Hot-swaps the active model provider (`"gemini"`, `"claude"`, or `"chatgpt"`) and persists
    /// the updated configuration to `/etc/ai.conf` on CobaltFS.
    pub fn switch_provider(&mut self, provider_name: &str) -> Result<ActiveProvider, AiError> {
        let next = ActiveProvider::from_name(provider_name)?;
        self.active = next;
        self.persist_config_to_vfs();
        Ok(next)
    }

    /// Serializes the current runtime configuration in `/etc/ai.conf` key-value format.
    pub fn serialize_config(&self) -> String {
        format!(
            "# metalOS Pluggable AI Runtime Configuration (/etc/ai.conf)\n\
             provider={}\n\
             model={}\n\
             gemini_model={}\n\
             claude_model={}\n\
             chatgpt_model={}\n\
             ui_mode=agent_exclusive\n",
            self.active.as_str(),
            self.active_model_id(),
            self.gemini.model,
            self.claude.model,
            self.chatgpt.model,
        )
    }

    /// Loads runtime provider and model settings from `/etc/ai.conf` text content.
    pub fn load_config_str(&mut self, config_text: &str) -> Result<(), AiError> {
        let mut parsed_provider: Option<ActiveProvider> = None;
        let mut active_model_override: Option<String> = None;

        for raw_line in config_text.lines() {
            let line = raw_line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let Some((key, val)) = line.split_once('=') else {
                return Err(AiError::ConfigError(format!(
                    "Malformed line in /etc/ai.conf: '{}'",
                    line
                )));
            };
            let k = key.trim();
            let v = val.trim();
            match k {
                "provider" => {
                    parsed_provider = Some(ActiveProvider::from_name(v)?);
                }
                "model" => {
                    active_model_override = Some(v.to_string());
                }
                "gemini_model" => {
                    self.gemini.model = v.to_string();
                }
                "claude_model" => {
                    self.claude.model = v.to_string();
                }
                "chatgpt_model" => {
                    self.chatgpt.model = v.to_string();
                }
                _ => {}
            }
        }

        if let Some(provider) = parsed_provider {
            self.active = provider;
        }
        if let Some(model_override) = active_model_override {
            match self.active {
                ActiveProvider::Gemini => self.gemini.model = model_override,
                ActiveProvider::Claude => self.claude.model = model_override,
                ActiveProvider::ChatGpt => self.chatgpt.model = model_override,
            }
        }

        self.persist_config_to_vfs();
        Ok(())
    }

    /// Reloads `/etc/ai.conf` directly from the virtualized CobaltFS store.
    pub fn reload_config_from_vfs(&mut self) -> Result<(), AiError> {
        let conf = self
            .tool_registry
            .vfs_read(Self::CONFIG_PATH)
            .ok_or_else(|| AiError::ConfigError("/etc/ai.conf not found in VFS".to_string()))?
            .to_string();
        self.load_config_str(&conf)
    }

    fn persist_config_to_vfs(&mut self) {
        let serialized = self.serialize_config();
        self.tool_registry.vfs_write(Self::CONFIG_PATH, serialized);
    }

    /// Processes a user input line from the Mercury AI Console:
    /// - Rejects `!` shell escapes and `sh`/`bash` commands with `AiError::ShellAccessForbidden`
    /// - Handles `/model gemini`, `/model claude`, `/model chatgpt` hot-swapping and persists `/etc/ai.conf`
    /// - Streams tokens from the active `AiModelProvider` and executes any emitted native OS `ToolCall`s.
    pub fn handle_console_input(
        &mut self,
        input: &str,
        on_token: &mut dyn FnMut(&str),
    ) -> Result<ConsoleTurnOutput, AiError> {
        let trimmed = input.trim();
        validate_console_input_no_shell(trimmed)?;

        if let Some(rest) = trimmed.strip_prefix("/model") {
            let target = rest.trim();
            if target.is_empty() {
                let status_msg = format!(
                    "Active provider: {} (model: {})",
                    self.active_provider_name(),
                    self.active_model_id()
                );
                on_token(&status_msg);
                return Ok(ConsoleTurnOutput {
                    active_provider: self.active_provider_name(),
                    active_model: self.active_model_id().to_string(),
                    response_text: status_msg,
                    tool_outputs: Vec::new(),
                    config_persisted: false,
                });
            }

            let mut parts = target.split_whitespace();
            let provider_arg = parts.next().unwrap_or("");
            let custom_model_arg = parts.next();

            let switched = self.switch_provider(provider_arg)?;
            if let Some(custom_model) = custom_model_arg {
                match switched {
                    ActiveProvider::Gemini => self.gemini.model = custom_model.to_string(),
                    ActiveProvider::Claude => self.claude.model = custom_model.to_string(),
                    ActiveProvider::ChatGpt => self.chatgpt.model = custom_model.to_string(),
                }
                self.persist_config_to_vfs();
            }

            let msg = format!(
                "Switched active AI model provider to '{}' ({}) and persisted to /etc/ai.conf.",
                self.active_provider_name(),
                self.active_model_id()
            );
            on_token(&msg);
            return Ok(ConsoleTurnOutput {
                active_provider: self.active_provider_name(),
                active_model: self.active_model_id().to_string(),
                response_text: msg,
                tool_outputs: Vec::new(),
                config_persisted: true,
            });
        }

        self.history.push(Message::user(trimmed));
        let tools_snapshot = self.tool_registry.specs().to_vec();

        let response = match self.active {
            ActiveProvider::Gemini => {
                self.gemini
                    .stream_chat(&self.history, &tools_snapshot, on_token)?
            }
            ActiveProvider::Claude => {
                self.claude
                    .stream_chat(&self.history, &tools_snapshot, on_token)?
            }
            ActiveProvider::ChatGpt => {
                self.chatgpt
                    .stream_chat(&self.history, &tools_snapshot, on_token)?
            }
        };

        let mut tool_outputs = Vec::new();
        for call in &response.tool_calls {
            let output = self
                .tool_registry
                .execute_tool(call, &mut self.ui_synthesizer)?;
            self.history
                .push(Message::tool_result(&call.id, &output.output_json));
            tool_outputs.push(output);
        }

        self.history.push(Message::assistant_with_tools(
            &response.text,
            response.tool_calls.clone(),
        ));

        Ok(ConsoleTurnOutput {
            active_provider: self.active_provider_name(),
            active_model: response.model_name,
            response_text: response.text,
            tool_outputs,
            config_persisted: false,
        })
    }

    /// Returns all UI views synthesized by the AI agent for Mercury (`mercuryd`).
    pub fn synthesized_views(&self) -> &[SynthesizedUiView] {
        self.ui_synthesizer.views()
    }
}
