//! Forge-owned cloud API adapters. HTTPS is an injected transport capability.
use crate::{CloudModel, CloudTransport, Forge, ForgeError, Generation, GenerationRequest};
use alloc::{
    boxed::Box,
    string::{String, ToString},
    vec,
    vec::Vec,
};
use core::task::Poll;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

// Responses remain bounded independently of larger conversation requests.
pub const HTTP_BODY_LIMIT: usize = 65536;
pub const HTTP_REQUEST_BODY_LIMIT: usize = 1024 * 1024;
pub const HTTP_FRAME_LIMIT: usize = 4 * HTTP_REQUEST_BODY_LIMIT + 16384;
pub const CLAUDE_MODEL: &str = "claude-sonnet-4-6";
pub const GEMINI_MODEL: &str = "gemini-3.8-flash";
pub const OPENAI_MODEL: &str = "gpt-4.1";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ApiProvider {
    Claude,
    Gemini,
    OpenAi,
}
impl ApiProvider {
    pub fn name(self) -> &'static str {
        match self {
            Self::Claude => "claude",
            Self::Gemini => "gemini",
            Self::OpenAi => "openai",
        }
    }
    pub fn key_name(self) -> &'static str {
        match self {
            Self::Claude => "ANTHROPIC_API_KEY",
            Self::Gemini => "GEMINI_API_KEY",
            Self::OpenAi => "OPENAI_API_KEY",
        }
    }
    pub fn default_model(self) -> &'static str {
        match self {
            Self::Claude => CLAUDE_MODEL,
            Self::Gemini => GEMINI_MODEL,
            Self::OpenAi => OPENAI_MODEL,
        }
    }
}

// Deliberately no Debug: this contains authorization headers.
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HttpRequest {
    pub url: String,
    pub headers: Vec<(String, String)>,
    pub body: String,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HttpResponse {
    pub status: u16,
    pub body: String,
}
pub trait HttpsClient: Send {
    fn post(&mut self, request: &HttpRequest) -> Result<HttpResponse, ForgeError>;
    fn start_post(&mut self, request: &HttpRequest) -> Poll<Result<HttpResponse, ForgeError>> {
        Poll::Ready(self.post(request))
    }
    fn poll_post(&mut self) -> Poll<Result<HttpResponse, ForgeError>> {
        Poll::Ready(Err(ForgeError::InvalidRequest))
    }
    fn partial_response_body(&self) -> Option<&str> {
        None
    }
}

/// Owns an ephemeral API credential, never exposed to Welder or persisted to configuration.
pub struct ApiTransport {
    provider: ApiProvider,
    key: String,
    client: Box<dyn HttpsClient>,
    pending: Option<CloudModel>,
    partial: String,
}
impl Drop for ApiTransport {
    fn drop(&mut self) {
        // Best effort erase of this owned credential (transport buffers have independent lifetimes).
        unsafe {
            for byte in self.key.as_bytes_mut() {
                core::ptr::write_volatile(byte, 0);
            }
        }
    }
}
impl ApiTransport {
    pub fn new(
        provider: ApiProvider,
        key: &str,
        client: Box<dyn HttpsClient>,
    ) -> Result<Self, ForgeError> {
        if key.is_empty() || key.len() > 4096 || key.bytes().any(|b| b.is_ascii_control()) {
            return Err(ForgeError::InvalidRequest);
        }
        Ok(Self {
            provider,
            key: key.to_string(),
            client,
            pending: None,
            partial: String::new(),
        })
    }
    pub fn request(
        &self,
        model: &CloudModel,
        input: &GenerationRequest,
    ) -> Result<HttpRequest, ForgeError> {
        input.validate()?;
        CloudModel::new(
            &model.id,
            &model.provider,
            &model.remote_model,
            &model.credential_reference,
        )?;
        if model.provider != self.provider.name()
            || model.credential_reference != self.provider.key_name()
        {
            return Err(ForgeError::InvalidRequest);
        }
        let mut headers = vec![("Content-Type".into(), "application/json".into())];
        let (url, body) = match self.provider {
            ApiProvider::Claude => {
                headers.push(("x-api-key".into(), self.key.clone()));
                headers.push(("anthropic-version".into(), "2023-06-01".into()));
                (
                    "https://api.anthropic.com/v1/messages".into(),
                    json!({"model": model.remote_model, "max_tokens": input.max_tokens, "messages": [{"role":"user", "content": input.prompt}]}),
                )
            }
            ApiProvider::Gemini => {
                headers.push(("x-goog-api-key".into(), self.key.clone()));
                (alloc::format!("https://generativelanguage.googleapis.com/v1beta/models/{}:generateContent", model.remote_model), json!({"contents":[{"role":"user","parts":[{"text":input.prompt}]}], "generationConfig":{"maxOutputTokens": input.max_tokens, "thinkingConfig":{"thinkingLevel":"low"}}}))
            }
            ApiProvider::OpenAi => {
                headers.push((
                    "Authorization".into(),
                    alloc::format!("Bearer {}", self.key),
                ));
                (
                    "https://api.openai.com/v1/responses".into(),
                    json!({"model":model.remote_model, "input":input.prompt, "max_output_tokens":input.max_tokens, "store":false}),
                )
            }
        };
        let body = body.to_string();
        if body.len() > HTTP_REQUEST_BODY_LIMIT {
            return Err(ForgeError::InvalidRequest);
        }
        Ok(HttpRequest { url, headers, body })
    }

    fn is_sse_payload(body: &str) -> bool {
        let trimmed = body.trim_start();
        trimmed.starts_with("event:")
            || trimmed.starts_with("data:")
            || trimmed.contains("\ndata:")
            || trimmed.contains("\r\ndata:")
    }

    fn append_event_delta(
        provider: ApiProvider,
        value: &Value,
        text: &mut String,
        tokens: &mut Option<u64>,
    ) -> Result<(), ForgeError> {
        if value["type"] == "error" || value["status"] == "failed" || !value["error"].is_null() {
            return Err(ForgeError::GenerationFailed);
        }
        match provider {
            ApiProvider::Claude => {
                if let Some(chunk) = value["delta"]["text"].as_str() {
                    text.push_str(chunk);
                } else if value["type"] == "content_block_start" {
                    if let Some(chunk) = value["content_block"]["text"].as_str() {
                        text.push_str(chunk);
                    }
                } else if value["type"] == "message" {
                    if let Some(blocks) = value["content"].as_array() {
                        for block in blocks {
                            if block["type"] == "text" {
                                if let Some(chunk) = block["text"].as_str() {
                                    text.push_str(chunk);
                                }
                            }
                        }
                    }
                }
                if let Some(u) = value["usage"]["output_tokens"]
                    .as_u64()
                    .or_else(|| value["message"]["usage"]["output_tokens"].as_u64())
                {
                    *tokens = Some(u);
                }
            }
            ApiProvider::Gemini => {
                if let Some(candidates) = value["candidates"].as_array() {
                    if let Some(candidate) = candidates.first() {
                        if let Some(parts) = candidate["content"]["parts"].as_array() {
                            for part in parts {
                                if part["thought"] != true {
                                    if let Some(chunk) = part["text"].as_str() {
                                        text.push_str(chunk);
                                    }
                                }
                            }
                        }
                    }
                }
                if let Some(u) = value["usageMetadata"]["candidatesTokenCount"].as_u64() {
                    *tokens = Some(u);
                }
            }
            ApiProvider::OpenAi => {
                if value["type"] == "response.output_text.delta" {
                    if let Some(chunk) = value["delta"].as_str() {
                        text.push_str(chunk);
                    }
                } else if let Some(chunk) = value["delta"]["text"]
                    .as_str()
                    .or_else(|| value["delta"]["content"].as_str())
                {
                    text.push_str(chunk);
                } else if let Some(choices) = value["choices"].as_array() {
                    if let Some(first) = choices.first() {
                        if let Some(chunk) = first["delta"]["content"].as_str() {
                            text.push_str(chunk);
                        }
                    }
                } else if let Some(outputs) = value["output"].as_array() {
                    for output in outputs {
                        if output["type"] == "message" && output["role"] == "assistant" {
                            if let Some(blocks) = output["content"].as_array() {
                                for block in blocks {
                                    if block["type"] == "output_text" {
                                        if let Some(chunk) = block["text"].as_str() {
                                            text.push_str(chunk);
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
                if let Some(u) = value["usage"]["output_tokens"]
                    .as_u64()
                    .or_else(|| value["response"]["usage"]["output_tokens"].as_u64())
                {
                    *tokens = Some(u);
                }
            }
        }
        Ok(())
    }

    fn parse_sse(&self, model: &CloudModel, body: &str) -> Result<Generation, ForgeError> {
        let mut text = String::new();
        let mut tokens: Option<u64> = None;
        let mut saw_data_event = false;

        for raw_line in body.lines() {
            let line = raw_line.trim();
            if line.is_empty() || line.starts_with(':') || line.starts_with("event:") {
                continue;
            }
            if let Some(rest) = line.strip_prefix("data:") {
                let data = rest.trim();
                if data.is_empty() || data == "[DONE]" {
                    saw_data_event = true;
                    continue;
                }
                saw_data_event = true;
                let value: Value =
                    serde_json::from_str(data).map_err(|_| ForgeError::InvalidResponse)?;
                Self::append_event_delta(self.provider, &value, &mut text, &mut tokens)?;
            } else {
                return Err(ForgeError::InvalidResponse);
            }
        }

        if !saw_data_event || text.is_empty() {
            return Err(ForgeError::InvalidResponse);
        }
        let count = tokens.unwrap_or_else(|| (text.split_whitespace().count().max(1)) as u64);
        let generated_tokens = u32::try_from(count).map_err(|_| ForgeError::InvalidResponse)?;
        Ok(Generation {
            model: model.id.clone(),
            text,
            generated_tokens,
        })
    }

    fn refresh_partial(&mut self) {
        let Some(raw) = self.client.partial_response_body() else {
            return;
        };
        if raw.is_empty() {
            return;
        }
        if Self::is_sse_payload(raw) {
            let mut acc = String::new();
            let mut tokens = None;
            for raw_line in raw.lines() {
                let line = raw_line.trim();
                if let Some(rest) = line.strip_prefix("data:") {
                    let data = rest.trim();
                    if data.is_empty() || data == "[DONE]" {
                        continue;
                    }
                    if let Ok(value) = serde_json::from_str::<Value>(data) {
                        let _ =
                            Self::append_event_delta(self.provider, &value, &mut acc, &mut tokens);
                    }
                }
            }
            if !acc.is_empty() {
                self.partial = acc;
            }
        } else if let Ok(value) = serde_json::from_str::<Value>(raw) {
            let mut acc = String::new();
            let mut tokens = None;
            let _ = Self::append_event_delta(self.provider, &value, &mut acc, &mut tokens);
            if !acc.is_empty() {
                self.partial = acc;
            }
        }
    }

    fn parse(&self, model: &CloudModel, response: HttpResponse) -> Result<Generation, ForgeError> {
        if !(200..=299).contains(&response.status) && response.body.len() <= HTTP_BODY_LIMIT {
            // Expose only the API's structured message, never arbitrary response bodies.
            if let Ok(value) = serde_json::from_str::<Value>(&response.body) {
                if let Some(message) = value["error"]["message"]
                    .as_str()
                    .filter(|s| !s.trim().is_empty())
                {
                    let message: String = message
                        .replace(&self.key, "[redacted]")
                        .chars()
                        .map(|c| if c.is_control() { ' ' } else { c })
                        .take(2048)
                        .collect();
                    return Err(ForgeError::ApiError {
                        status: response.status,
                        provider: self.provider.name().into(),
                        model: model.id.clone(),
                        message,
                    });
                }
            }
        }
        match response.status {
            200..=299 => {}
            401 | 403 => return Err(ForgeError::AuthenticationFailed),
            429 => return Err(ForgeError::RateLimited),
            code => return Err(ForgeError::HttpStatus(code)),
        }
        if response.body.len() > HTTP_BODY_LIMIT {
            return Err(ForgeError::InvalidResponse);
        }
        if Self::is_sse_payload(&response.body) {
            return self.parse_sse(model, &response.body);
        }
        let value: Value =
            serde_json::from_str(&response.body).map_err(|_| ForgeError::ApiResponseError { provider: self.provider.name().into(), model: model.id.clone(), reason: "response is not valid JSON" })?;
        let response_error = |reason| ForgeError::ApiResponseError { provider: self.provider.name().into(), model: model.id.clone(), reason };
        let mut text = String::new();
        let tokens = match self.provider {
            ApiProvider::Claude => {
                if value["type"] != "message" {
                    return Err(ForgeError::InvalidResponse);
                }
                for block in value["content"]
                    .as_array()
                    .ok_or(ForgeError::InvalidResponse)?
                {
                    if block["type"] == "text" {
                        text.push_str(block["text"].as_str().ok_or(ForgeError::InvalidResponse)?);
                    }
                }
                value["usage"]["output_tokens"].as_u64()
            }
            ApiProvider::Gemini => {
                if value["promptFeedback"]["blockReason"].as_str().is_some() {
                    return Err(response_error("Gemini blocked the prompt; no answer was generated"));
                }
                let candidate = value["candidates"]
                    .as_array()
                    .and_then(|v| v.first())
                    .ok_or_else(|| response_error("response has no candidate (blocked or malformed response)"))?;
                if candidate["finishReason"] == "MAX_TOKENS" {
                    return Err(response_error("generation exhausted its output/thinking token budget; increase max_tokens"));
                }
                match candidate["finishReason"].as_str() {
                    Some("SAFETY" | "RECITATION" | "BLOCKLIST" | "PROHIBITED_CONTENT" | "SPII" | "IMAGE_SAFETY") => {
                        return Err(response_error("Gemini blocked the generated answer"));
                    }
                    _ => {}
                }
                let mut has_thoughts = false;
                for part in candidate["content"]["parts"]
                    .as_array()
                    .ok_or_else(|| response_error("candidate has no content parts"))?
                {
                    has_thoughts |= part["thought"] == true;
                    if part["thought"] != true {
                        if let Some(chunk) = part["text"].as_str() {
                            text.push_str(chunk);
                        }
                    }
                }
                if text.trim().is_empty() {
                    return Err(response_error(if has_thoughts || value["usageMetadata"]["thoughtsTokenCount"].as_u64().unwrap_or(0) > 0 {
                        "Gemini returned thinking without a visible answer"
                    } else {
                        "Gemini returned no usable text; response parts were empty or non-text"
                    }));
                }
                value["usageMetadata"]["candidatesTokenCount"].as_u64()
            }
            ApiProvider::OpenAi => {
                if value["status"] == "failed" || !value["error"].is_null() {
                    return Err(ForgeError::GenerationFailed);
                }
                for output in value["output"]
                    .as_array()
                    .ok_or(ForgeError::InvalidResponse)?
                {
                    if output["type"] == "message" && output["role"] == "assistant" {
                        for block in output["content"]
                            .as_array()
                            .ok_or(ForgeError::InvalidResponse)?
                        {
                            if block["type"] == "output_text" {
                                text.push_str(
                                    block["text"].as_str().ok_or(ForgeError::InvalidResponse)?,
                                );
                            }
                        }
                    }
                }
                value["usage"]["output_tokens"].as_u64()
            }
        };
        if text.is_empty() {
            return Err(response_error("response contains no visible text"));
        }
        let generated_tokens = u32::try_from(tokens.ok_or_else(|| response_error("response is missing output token usage"))?)
            .map_err(|_| ForgeError::InvalidResponse)?;
        Ok(Generation {
            model: model.id.clone(),
            text,
            generated_tokens,
        })
    }
}
impl CloudTransport for ApiTransport {
    fn generate(
        &mut self,
        model: &CloudModel,
        request: &GenerationRequest,
    ) -> Result<Generation, ForgeError> {
        let outbound = self.request(model, request)?;
        let result = self.client.post(&outbound)?;
        self.parse(model, result)
    }
    fn start_generation(
        &mut self,
        model: &CloudModel,
        request: &GenerationRequest,
    ) -> Result<Option<Generation>, ForgeError> {
        if self.pending.is_some() {
            return Err(ForgeError::Busy);
        }
        self.partial.clear();
        let outbound = self.request(model, request)?;
        match self.client.start_post(&outbound) {
            Poll::Ready(response) => self.parse(model, response?).map(Some),
            Poll::Pending => {
                self.pending = Some(model.clone());
                self.refresh_partial();
                Ok(None)
            }
        }
    }
    fn poll_generation(&mut self) -> Poll<Result<Generation, ForgeError>> {
        if self.pending.is_none() {
            return Poll::Ready(Err(ForgeError::InvalidRequest));
        }
        match self.client.poll_post() {
            Poll::Pending => {
                self.refresh_partial();
                Poll::Pending
            }
            Poll::Ready(response) => {
                let model = self.pending.take().expect("pending cloud model");
                self.partial.clear();
                Poll::Ready(response.and_then(|response| self.parse(&model, response)))
            }
        }
    }
    fn partial_text(&self) -> Option<&str> {
        if self.pending.is_some() && !self.partial.is_empty() {
            Some(&self.partial)
        } else {
            None
        }
    }
}
impl Forge {
    /// Register an API backend without changing the local default or enabling fallback.
    pub fn connect_api(
        &mut self,
        provider: ApiProvider,
        model_id: &str,
        key: &str,
        client: Box<dyn HttpsClient>,
    ) -> Result<(), ForgeError> {
        let model = CloudModel::new(model_id, provider.name(), model_id, provider.key_name())?;
        let transport = ApiTransport::new(provider, key, client)?;
        self.register_cloud(&model)?;
        self.attach_cloud(model, Box::new(transport))
    }
}
