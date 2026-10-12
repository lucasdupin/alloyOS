//! Forge-owned cloud API adapters. HTTPS is an injected transport capability.
use crate::{CloudModel, CloudTransport, Forge, ForgeError, Generation, GenerationRequest};
use alloc::{
    boxed::Box,
    string::{String, ToString},
    vec,
    vec::Vec,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

pub const HTTP_BODY_LIMIT: usize = 65536;
pub const HTTP_FRAME_LIMIT: usize = 262144;
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
}

/// Owns an ephemeral API credential, never exposed to Welder or persisted to configuration.
pub struct ApiTransport {
    provider: ApiProvider,
    key: String,
    client: Box<dyn HttpsClient>,
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
        if body.len() > HTTP_BODY_LIMIT {
            return Err(ForgeError::InvalidRequest);
        }
        Ok(HttpRequest { url, headers, body })
    }
    fn parse(&self, model: &CloudModel, response: HttpResponse) -> Result<Generation, ForgeError> {
        match response.status {
            200..=299 => {}
            401 | 403 => return Err(ForgeError::AuthenticationFailed),
            429 => return Err(ForgeError::RateLimited),
            code => return Err(ForgeError::HttpStatus(code)),
        }
        if response.body.len() > HTTP_BODY_LIMIT {
            return Err(ForgeError::InvalidResponse);
        }
        let value: Value =
            serde_json::from_str(&response.body).map_err(|_| ForgeError::InvalidResponse)?;
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
                let candidate = value["candidates"]
                    .as_array()
                    .and_then(|v| v.first())
                    .ok_or(ForgeError::InvalidResponse)?;
                for part in candidate["content"]["parts"]
                    .as_array()
                    .ok_or(ForgeError::InvalidResponse)?
                {
                    if part["thought"] != true {
                        if let Some(chunk) = part["text"].as_str() {
                            text.push_str(chunk);
                        }
                    }
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
            return Err(ForgeError::InvalidResponse);
        }
        let generated_tokens = u32::try_from(tokens.ok_or(ForgeError::InvalidResponse)?)
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
