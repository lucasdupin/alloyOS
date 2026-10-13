use crate::provider::{AiError, Message, Response, TokenUsage, ToolSpec, ToolCall};
use crate::providers::{ChatGptProvider, ClaudeProvider, GeminiProvider};
use crate::{Forge, GenerationRequest, DEFAULT_MODEL};
use alloc::{
    format,
    rc::Rc,
    string::{String, ToString},
    vec::Vec,
};
use core::{cell::RefCell, task::Poll};

/// Active inference provider selection inside Forge.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActiveProvider {
    /// Resident inference service; its initial default model is Gemma 2 2B IT.
    Forge,
    Gemini,
    Claude,
    ChatGpt,
}

impl ActiveProvider {
    pub const fn as_str(self) -> &'static str {
        match self {
            ActiveProvider::Forge => "forge",
            ActiveProvider::Gemini => "gemini",
            ActiveProvider::Claude => "claude",
            ActiveProvider::ChatGpt => "chatgpt",
        }
    }

    pub fn from_name(name: &str) -> Result<Self, AiError> {
        match name.trim().to_ascii_lowercase().as_str() {
            "forge" | "local" => Ok(ActiveProvider::Forge),
            "gemini" | "google" => Ok(ActiveProvider::Gemini),
            "claude" | "anthropic" => Ok(ActiveProvider::Claude),
            "chatgpt" | "openai" | "gpt" => Ok(ActiveProvider::ChatGpt),
            other => Err(AiError::UnknownModel(other.to_string())),
        }
    }
}

/// Forge-owned conversation gateway. Welder supplies history and tool schemas;
/// model adapters, credentials and execution remain inside Forge.
#[derive(Clone)]
pub struct ModelGateway {
    active: ActiveProvider,
    pending: Option<u64>,
    pending_tools: Vec<ToolSpec>,
    forge: Rc<RefCell<Forge>>,
    forge_model: String,
    gemini: GeminiProvider,
    claude: ClaudeProvider,
    chatgpt: ChatGptProvider,
}
impl core::fmt::Debug for ModelGateway {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("ModelGateway")
            .field("active", &self.active)
            .field("model", &self.active_model_id())
            .finish_non_exhaustive()
    }
}
impl Default for ModelGateway {
    fn default() -> Self {
        Self::new()
    }
}
impl ModelGateway {
    pub fn new() -> Self {
        Self {
            active: ActiveProvider::Gemini,
            pending: None,
            pending_tools: Vec::new(),
            forge: Rc::new(RefCell::new(Forge::new(0))),
            forge_model: DEFAULT_MODEL.to_string(),
            gemini: GeminiProvider::new(),
            claude: ClaudeProvider::new(),
            chatgpt: ChatGptProvider::new(),
        }
    }
    pub fn set_forge(&mut self, forge: Rc<RefCell<Forge>>) {
        self.forge_model = forge.borrow().default_model().to_string();
        self.forge = forge;
    }
    /// Returns the short identifier of the currently active model provider (`"forge"`, `"gemini"`, `"claude"`, `"chatgpt"`).
    pub fn active_provider_name(&self) -> &'static str {
        self.active.as_str()
    }

    /// Returns the model identifier string of the currently active provider.
    pub fn active_model_id(&self) -> &str {
        match self.active {
            ActiveProvider::Forge => &self.forge_model,
            ActiveProvider::Gemini => &self.gemini.model,
            ActiveProvider::Claude => &self.claude.model,
            ActiveProvider::ChatGpt => &self.chatgpt.model,
        }
    }

    /// Selects the active model provider. Welder persists the returned configuration.
    pub fn switch_provider(&mut self, provider_name: &str) -> Result<ActiveProvider, AiError> {
        let next = ActiveProvider::from_name(provider_name)?;
        self.active = next;
        Ok(next)
    }

    /// Serializes the current runtime configuration in `/etc/ai.conf` key-value format.
    pub fn serialize_config(&self) -> String {
        format!(
            "# alloyOS Pluggable AI Runtime Configuration (/etc/ai.conf)\n\
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
                ActiveProvider::Forge => {
                    self.forge
                        .borrow_mut()
                        .set_default(&model_override)
                        .map_err(|e| AiError::LocalInferenceError(e.to_string()))?;
                    self.forge_model = model_override;
                }
                ActiveProvider::Gemini => self.gemini.model = model_override,
                ActiveProvider::Claude => self.claude.model = model_override,
                ActiveProvider::ChatGpt => self.chatgpt.model = model_override,
            }
        }

        Ok(())
    }

    pub fn set_model(&mut self, model: &str) -> Result<(), AiError> {
        match self.active {
            ActiveProvider::Forge => {
                self.forge
                    .borrow_mut()
                    .set_default(model)
                    .map_err(|e| AiError::LocalInferenceError(e.to_string()))?;
                self.forge_model = model.to_string();
            }
            ActiveProvider::Gemini => self.gemini.model = model.to_string(),
            ActiveProvider::Claude => self.claude.model = model.to_string(),
            ActiveProvider::ChatGpt => self.chatgpt.model = model.to_string(),
        }
        Ok(())
    }
    /// Cycle through ready models in registry order, wrapping after the last.
    pub fn cycle_model(&mut self) -> Result<(), AiError> {
        if self.is_pending() {
            return Err(AiError::PolicyViolation("A reply is already pending".into()));
        }
        let models: Vec<_> = self.forge.borrow().models().into_iter()
            .filter(|model| model.state == "ready")
            .collect();
        if models.is_empty() {
            return Err(AiError::PolicyViolation("No models are ready. Load a local model or connect a cloud provider.".into()));
        }
        if models.len() == 1 && models[0].id == self.active_model_id() {
            return Err(AiError::PolicyViolation("No other model is ready. Load another model or connect a cloud provider.".into()));
        }
        let next = models.iter().position(|model| model.id == self.active_model_id())
            .map_or(0, |index| (index + 1) % models.len());
        let model = &models[next];
        let provider = match &model.source {
            crate::ModelSource::Local => ActiveProvider::Forge,
            crate::ModelSource::Cloud { provider } => ActiveProvider::from_name(provider)?,
        };
        self.active = provider;
        self.set_model(&model.id)
    }

    pub fn is_pending(&self) -> bool {
        self.pending.is_some()
    }
    pub fn start_chat(&mut self, messages: &[Message]) -> Result<(), AiError> {
        self.start_chat_with_tools(messages, &[])
    }
    pub fn start_chat_with_tools(&mut self, messages: &[Message], tools: &[ToolSpec]) -> Result<(), AiError> {
        if self.pending.is_some() {
            return Err(AiError::PolicyViolation(
                "A reply is already pending".into(),
            ));
        }
        let mut prompt = String::new();
        if !tools.is_empty() {
            prompt.push_str("system: Use OS tools to obtain live facts. Discover available apps with mcp_list_tools before invoking their tools or resources. Never invent app identifiers or print code instead of a tool call. Reply with exactly one JSON object: {\"kind\":\"tools\",\"calls\":[{\"name\":\"mcp_list_tools\",\"arguments\":{}}]} to use tools, or {\"kind\":\"final\",\"text\":\"your answer\"} after reading results. Available tools:\n");
            for tool in tools {
                prompt.push_str(&format!("{}:", tool.name));
                for parameter in &tool.parameters {
                    prompt.push_str(&format!(" {}:{}{}", parameter.name, parameter.param_type, if parameter.required { " (required)" } else { "" }));
                }
                prompt.push('\n');
            }
        }
        for message in messages {
            prompt.push_str(message.role.as_str());
            prompt.push_str(": ");
            prompt.push_str(&message.content);
            for call in &message.tool_calls {
                prompt.push_str(&format!("\nrequested tool {} {}", call.name, call.arguments_json));
            }
            prompt.push('\n');
        }
        let job = self
            .forge
            .borrow_mut()
            .start_generation(&GenerationRequest {
                model: Some(self.active_model_id().to_string()),
                prompt,
                max_tokens: if tools.is_empty() { 128 } else { 256 },
            })
            .map_err(|e| self.inference_error(e))?;
        self.pending_tools = tools.to_vec();
        self.pending = Some(job);
        Ok(())
    }
    pub fn partial_reply(&self) -> Option<String> {
        let text = self.forge
            .borrow()
            .partial_text(self.pending?)
            .map(ToString::to_string)?;
        if self.pending_tools.is_empty() {
            return Some(text);
        }
        extract_streamable_text(&text)
    }
    pub fn poll_chat(&mut self) -> Poll<Result<Response, AiError>> {
        let Some(id) = self.pending else {
            return Poll::Pending;
        };
        let result = self.forge.borrow_mut().poll_generation(id);
        match result {
            Poll::Pending => Poll::Pending,
            Poll::Ready(result) => {
                self.pending = None;
                Poll::Ready(
                    result.map_err(|e| self.inference_error(e)).and_then(|g| {
                        let response = decode_tool_response(g.text, &self.pending_tools, g.model,
                            TokenUsage::new(0, g.generated_tokens));
                        self.pending_tools.clear();
                        response
                    })
                )
            }
        }
    }
    fn inference_error(&self, error: crate::ForgeError) -> AiError {
        if self.active == ActiveProvider::Forge {
            AiError::LocalInferenceError(error.to_string())
        } else {
            AiError::NetworkError(error.to_string())
        }
    }

    /// Execute through the shared Forge registry, including explicitly attached
    /// cloud transports. Missing backends always return errors.
    pub fn chat(
        &mut self,
        messages: &[Message],
        tools: &[ToolSpec],
        on_token: &mut dyn FnMut(&str),
    ) -> Result<Response, AiError> {
        if self.pending.is_none() {
            self.start_chat_with_tools(messages, tools)?;
            let mut emitted_len = 0usize;
            loop {
                match self.poll_chat() {
                    Poll::Pending => {
                        if let Some(partial) = self.partial_reply() {
                            if partial.len() > emitted_len {
                                on_token(&partial[emitted_len..]);
                                emitted_len = partial.len();
                            }
                        }
                    }
                    Poll::Ready(Ok(response)) => {
                        if response.text.len() > emitted_len {
                            on_token(&response.text[emitted_len..]);
                        }
                        return Ok(response);
                    }
                    Poll::Ready(Err(err)) => return Err(err),
                }
            }
        }
        let mut prompt = String::new();
        for message in messages {
            prompt.push_str(message.role.as_str());
            prompt.push_str(": ");
            prompt.push_str(&message.content);
            prompt.push('\n');
        }
        let generated = self
            .forge
            .borrow_mut()
            .generate(&GenerationRequest {
                model: Some(self.active_model_id().to_string()),
                prompt,
                max_tokens: 128,
            })
            .map_err(|e| match self.active {
                ActiveProvider::Forge => AiError::LocalInferenceError(e.to_string()),
                _ => AiError::NetworkError(e.to_string()),
            })?;
        let response = decode_tool_response(generated.text, tools, generated.model,
            TokenUsage::new(0, generated.generated_tokens))?;
        on_token(&response.text);
        Ok(response)
    }
}

pub fn extract_streamable_text(raw: &str) -> Option<String> {
    let trimmed = raw.trim_start();
    if trimmed.is_empty() {
        return None;
    }
    let (is_code_block, inner) = if let Some(rest) = trimmed.strip_prefix("```json") {
        (true, rest.trim_start())
    } else if let Some(rest) = trimmed.strip_prefix("```") {
        (true, rest.trim_start())
    } else {
        (false, trimmed)
    };

    if !inner.starts_with('{') {
        if is_code_block {
            return None;
        }
        return Some(raw.to_string());
    }

    if has_tool_call_indicator(inner) {
        return None;
    }

    extract_response_field(inner)
}

fn has_tool_call_indicator(json_str: &str) -> bool {
    json_str.contains("\"calls\"")
        || json_str.contains("\"tool_calls\"")
        || json_str.contains("\"kind\":\"tools\"")
        || json_str.contains("\"kind\": \"tools\"")
        || json_str.contains("\"kind\":\"tool\"")
        || json_str.contains("\"kind\": \"tool\"")
}

fn find_response_string_start(json_str: &str) -> Option<usize> {
    const CANDIDATE_KEYS: &[&str] = &[
        "\"text\"",
        "\"response\"",
        "\"answer\"",
        "\"message\"",
        "\"content\"",
    ];
    for key in CANDIDATE_KEYS {
        let mut search_from = 0;
        while let Some(rel_pos) = json_str[search_from..].find(key) {
            let pos = search_from + rel_pos;
            let after_key = &json_str[pos + key.len()..];
            let mut char_indices = after_key.char_indices();
            let mut colon_found = false;
            for (_idx, ch) in char_indices.by_ref() {
                if ch == ':' {
                    colon_found = true;
                    break;
                } else if !ch.is_whitespace() {
                    break;
                }
            }
            if colon_found {
                for (idx, ch) in char_indices {
                    if ch == '"' {
                        return Some(pos + key.len() + idx + 1);
                    } else if !ch.is_whitespace() {
                        break;
                    }
                }
            }
            search_from = pos + key.len();
        }
    }
    None
}

fn extract_response_field(json_str: &str) -> Option<String> {
    let start_idx = find_response_string_start(json_str)?;
    let content = extract_json_string_content(&json_str[start_idx..]);
    if content.is_empty() {
        None
    } else {
        Some(content)
    }
}

fn extract_json_string_content(from_quote: &str) -> String {
    let mut out = String::new();
    let mut chars = from_quote.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch == '"' {
            break;
        }
        if ch == '\\' {
            if let Some(&next_ch) = chars.peek() {
                chars.next();
                match next_ch {
                    'n' => out.push('\n'),
                    'r' => out.push('\r'),
                    't' => out.push('\t'),
                    '"' => out.push('"'),
                    '\\' => out.push('\\'),
                    '/' => out.push('/'),
                    'u' => {
                        let mut hex = String::new();
                        for _ in 0..4 {
                            if let Some(&h) = chars.peek() {
                                if h.is_ascii_hexdigit() {
                                    hex.push(h);
                                    chars.next();
                                } else {
                                    break;
                                }
                            }
                        }
                        if hex.len() == 4 {
                            if let Ok(code) = u32::from_str_radix(&hex, 16) {
                                if let Some(c) = char::from_u32(code) {
                                    out.push(c);
                                    continue;
                                }
                            }
                        }
                        out.push_str("\\u");
                        out.push_str(&hex);
                    }
                    other => out.push(other),
                }
            } else {
                break;
            }
        } else {
            out.push(ch);
        }
    }
    out
}

#[derive(serde::Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum ToolReply {
    Final { text: String },
    Tools { calls: Vec<RequestedTool> },
}

#[derive(serde::Deserialize)]
struct RequestedTool {
    name: String,
    #[serde(default = "default_arguments")]
    arguments: serde_json::Value,
}

fn default_arguments() -> serde_json::Value {
    serde_json::Value::Object(serde_json::Map::new())
}

fn decode_tool_response(
    text: String,
    tools: &[ToolSpec],
    model: String,
    usage: TokenUsage,
) -> Result<Response, AiError> {
    if tools.is_empty() {
        return Ok(Response::new(text, Vec::new(), model, usage));
    }
    let trimmed = text.trim();
    let payload = trimmed
        .strip_prefix("```json")
        .and_then(|s| s.trim().strip_suffix("```"))
        .or_else(|| trimmed.strip_prefix("```").and_then(|s| s.trim().strip_suffix("```")))
        .unwrap_or(trimmed)
        .trim();

    if !payload.starts_with('{') {
        if trimmed.starts_with("```") {
            return Err(AiError::ParseError(
                "Model returned code instead of an OS tool request".into(),
            ));
        }
        return Ok(Response::new(text, Vec::new(), model, usage));
    }

    if let Ok(reply) = serde_json::from_str::<ToolReply>(payload) {
        match reply {
            ToolReply::Final { text } => return Ok(Response::new(text, Vec::new(), model, usage)),
            ToolReply::Tools { calls } => {
                return build_tool_calls_response(calls, tools, model, usage);
            }
        }
    }

    if let Ok(val) = serde_json::from_str::<serde_json::Value>(payload) {
        let is_tools = val.get("kind").and_then(|k| k.as_str()) == Some("tools")
            || val.get("kind").and_then(|k| k.as_str()) == Some("tool")
            || val.get("calls").is_some()
            || val.get("tool_calls").is_some();

        if is_tools {
            if let Some(calls_val) = val.get("calls").or_else(|| val.get("tool_calls")) {
                if let Ok(calls) = serde_json::from_value::<Vec<RequestedTool>>(calls_val.clone()) {
                    return build_tool_calls_response(calls, tools, model, usage);
                } else {
                    return Err(AiError::ParseError("Invalid OS tool response".into()));
                }
            }
            return Err(AiError::PolicyViolation("Invalid tool call count".into()));
        }

        for key in &["text", "response", "answer", "message", "content"] {
            if let Some(s) = val.get(*key).and_then(|v| v.as_str()) {
                return Ok(Response::new(s.to_string(), Vec::new(), model, usage));
            }
        }
    }

    if let Some(extracted) = extract_response_field(payload) {
        return Ok(Response::new(extracted, Vec::new(), model, usage));
    }

    if has_tool_call_indicator(payload) {
        return Err(AiError::ParseError("Invalid OS tool response".into()));
    }

    Ok(Response::new(text, Vec::new(), model, usage))
}

fn build_tool_calls_response(
    calls: Vec<RequestedTool>,
    tools: &[ToolSpec],
    model: String,
    usage: TokenUsage,
) -> Result<Response, AiError> {
    if calls.is_empty() || calls.len() > 8 {
        return Err(AiError::PolicyViolation("Invalid tool call count".into()));
    }
    let mut requested = Vec::new();
    for (index, call) in calls.into_iter().enumerate() {
        let spec = tools
            .iter()
            .find(|t| t.name == call.name)
            .ok_or_else(|| AiError::UnknownTool(call.name.clone()))?;
        let args = call
            .arguments
            .as_object()
            .ok_or_else(|| AiError::InvalidToolArguments(call.name.clone()))?;
        for parameter in &spec.parameters {
            if parameter.required && !args.contains_key(&parameter.name) {
                return Err(AiError::InvalidToolArguments(format!(
                    "{} requires {}",
                    call.name, parameter.name
                )));
            }
        }
        requested.push(ToolCall::new(
            format!("call-{}", index + 1),
            call.name,
            call.arguments.to_string(),
        ));
    }
    Ok(Response::new(String::new(), requested, model, usage))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{CloudModel, CloudTransport, ForgeError, Generation};
    use alloc::boxed::Box;

    #[test]
    fn tool_protocol_validates_requests_and_keeps_code_out_of_chat() {
        let tools = [ToolSpec::new("mcp_list_tools", "Discover apps", Vec::new())];
        let decode = |text: &str| decode_tool_response(text.into(), &tools, "fixture".into(), TokenUsage::default());
        let response = decode(r#"{"kind":"tools","calls":[{"name":"mcp_list_tools","arguments":{}}]}"#).unwrap();
        assert!(response.text.is_empty());
        assert_eq!(response.tool_calls[0].name, "mcp_list_tools");
        assert!(matches!(decode(r#"{"kind":"tools","calls":[{"name":"unavailable","arguments":{}}]}"#), Err(AiError::UnknownTool(_))));
        assert!(decode(r#"{"kind":"tools","calls":[]}"#).is_err());
        assert!(decode(r#"{"kind":"tools","calls":[{"name":"mcp_list_tools","arguments":"code"}]}"#).is_err());
        assert!(decode("```javascript\nrunSomething();\n```").is_err());
        assert_eq!(decode("```json\n{\"kind\":\"final\",\"text\":\"Result\"}\n```").unwrap().text, "Result");
    }

    #[test]
    fn streaming_extracts_plain_text_and_json_response_part_but_suppresses_tool_protocol() {
        // Plain text streams directly
        assert_eq!(extract_streamable_text("Hi"), Some("Hi".into()));
        assert_eq!(extract_streamable_text("Hello there!"), Some("Hello there!".into()));

        // Tool calls are suppressed from stream
        assert_eq!(extract_streamable_text(r#"{"kind":"tools","calls":[{"name":"mcp_list_tools"#), None);
        assert_eq!(extract_streamable_text(r#"{"kind":"tools","calls":[{"name":"mcp_list_tools","arguments":{}}]}"#), None);

        // Streaming inside final JSON text field
        assert_eq!(extract_streamable_text(r#"{"kind":"final","text":"It is"#), Some("It is".into()));
        assert_eq!(extract_streamable_text(r#"{"kind":"final","text":"It is 10:00 AM."}"#), Some("It is 10:00 AM.".into()));
        assert_eq!(extract_streamable_text("```json\n{\"kind\":\"final\",\"text\":\"Hello\\nworld\"}"), Some("Hello\nworld".into()));

        // Alternative response keys
        assert_eq!(extract_streamable_text(r#"{"response":"Direct answer"#), Some("Direct answer".into()));

        // Code block that is not JSON is suppressed
        assert_eq!(extract_streamable_text("```python\nprint(1)"), None);
    }

    #[test]
    fn tool_response_decoding_is_tolerant_of_thoughts_and_informal_replies() {
        let tools = [ToolSpec::new("mcp_list_tools", "Discover apps", Vec::new())];
        let decode = |text: &str| decode_tool_response(text.into(), &tools, "fixture".into(), TokenUsage::default());

        // Model included thought before tool call
        let res = decode(r#"{"thought":"check apps","kind":"tools","calls":[{"name":"mcp_list_tools","arguments":{}}]}"#).unwrap();
        assert_eq!(res.tool_calls.len(), 1);

        // Model included thought before final response
        let res = decode(r#"{"thought":"answering user","kind":"final","text":"The clock app only has uptime."}"#).unwrap();
        assert_eq!(res.text, "The clock app only has uptime.");

        // Model omitted 'kind' or used alternative response field
        let res = decode(r#"{"response":"The time is unavailable right now."}"#).unwrap();
        assert_eq!(res.text, "The time is unavailable right now.");

        // Model had trailing conversation after JSON
        let res = decode(r#"{"kind":"final","text":"Uptime is 42s."}\nLet me know if you need anything else."#).unwrap();
        assert_eq!(res.text, "Uptime is 42s.");

        // Model reply was truncated before closing brace
        let res = decode(r#"{"kind":"final","text":"Monotonic clock only"#).unwrap();
        assert_eq!(res.text, "Monotonic clock only");
    }

    #[test]
    fn model_cycle_reports_when_no_alternative_is_ready() {
        let mut gateway = ModelGateway::new();
        let initial = gateway.active_model_id().to_string();
        assert!(gateway.cycle_model().is_err());
        assert_eq!(gateway.active_model_id(), initial);
        let mut forge = Forge::new(0);
        let model = CloudModel::new("only", "claude", "only", "test-credential").unwrap();
        forge.register_cloud(&model).unwrap();
        forge.attach_cloud(model, Box::new(CheckingTransport)).unwrap();
        gateway.set_forge(Rc::new(RefCell::new(forge)));
        gateway.cycle_model().unwrap();
        assert_eq!(gateway.active_model_id(), "only");
        assert!(gateway.cycle_model().is_err());
        assert_eq!(gateway.active_model_id(), "only");
    }

    #[test]
    fn model_cycle_skips_unavailable_and_wraps() {
        let mut forge = Forge::new(0);
        for id in ["a", "b", "c"] {
            let model = CloudModel::new(id, "claude", id, "test-credential").unwrap();
            forge.register_cloud(&model).unwrap();
            if id != "b" {
                forge.attach_cloud(model, Box::new(CheckingTransport)).unwrap();
            }
        }
        let mut gateway = ModelGateway::new();
        gateway.set_forge(Rc::new(RefCell::new(forge)));
        gateway.cycle_model().unwrap();
        assert_eq!(gateway.active_model_id(), "a");
        assert_eq!(gateway.active_provider_name(), "claude");
        gateway.cycle_model().unwrap();
        assert_eq!(gateway.active_model_id(), "c");
        gateway.cycle_model().unwrap();
        assert_eq!(gateway.active_model_id(), "a");
        gateway.pending = Some(1);
        assert!(gateway.cycle_model().is_err());
        assert_eq!(gateway.active_model_id(), "a");
    }

    struct CheckingTransport;
    impl CloudTransport for CheckingTransport {
        fn generate(
            &mut self,
            model: &CloudModel,
            request: &GenerationRequest,
        ) -> Result<Generation, ForgeError> {
            assert_eq!(model.provider, "claude");
            assert_eq!(model.credential_reference, "test-credential");
            assert!(request.prompt.contains("system: Be concise"));
            assert!(request.prompt.contains("user: Earlier question"));
            assert!(request.prompt.contains("assistant: Earlier answer"));
            assert!(request.prompt.contains("user: Follow-up"));
            Ok(Generation {
                model: model.id.clone(),
                text: "Transport response".into(),
                generated_tokens: 2,
            })
        }
    }

    #[test]
    fn conversation_uses_explicit_shared_transport_and_keeps_context() {
        let forge = Rc::new(RefCell::new(Forge::new(0)));
        let cloud = CloudModel::new(
            "claude-sonnet-4-6",
            "claude",
            "claude-sonnet-4-6",
            "test-credential",
        )
        .unwrap();
        forge.borrow_mut().register_cloud(&cloud).unwrap();
        forge
            .borrow_mut()
            .attach_cloud(cloud, Box::new(CheckingTransport))
            .unwrap();
        let mut gateway = ModelGateway::new();
        gateway.set_forge(forge);
        let messages = [
            Message::system("Be concise"),
            Message::user("Earlier question"),
            Message::assistant("Earlier answer"),
            Message::user("Follow-up"),
        ];
        let mut tokens = String::new();
        gateway.switch_provider("local").unwrap();
        // An available cloud transport must not be used implicitly for a failed local model.
        assert!(matches!(
            gateway.chat(&messages, &[], &mut |t| tokens.push_str(t)),
            Err(AiError::LocalInferenceError(_))
        ));
        assert!(tokens.is_empty());
        gateway.switch_provider("claude").unwrap();
        let response = gateway
            .chat(&messages, &[], &mut |t| tokens.push_str(t))
            .unwrap();
        assert_eq!(response.text, "Transport response");
        assert_eq!(tokens, response.text);
        assert_eq!(response.model_name, "claude-sonnet-4-6");
    }

    #[test]
    fn selected_cloud_without_transport_does_not_generate_a_reply() {
        let mut gateway = ModelGateway::new();
        gateway.switch_provider("claude").unwrap();
        let mut emitted = false;
        assert!(matches!(
            gateway.chat(&[Message::user("hello")], &[], &mut |_| emitted = true),
            Err(AiError::NetworkError(_))
        ));
        assert!(!emitted);
    }
}
