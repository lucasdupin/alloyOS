use crate::provider::{AiError, Message, Response, TokenUsage, ToolSpec};
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
        if self.pending.is_some() {
            return Err(AiError::PolicyViolation(
                "A reply is already pending".into(),
            ));
        }
        let mut prompt = String::new();
        for message in messages {
            prompt.push_str(message.role.as_str());
            prompt.push_str(": ");
            prompt.push_str(&message.content);
            prompt.push('\n');
        }
        let job = self
            .forge
            .borrow_mut()
            .start_generation(&GenerationRequest {
                model: Some(self.active_model_id().to_string()),
                prompt,
                max_tokens: 128,
            })
            .map_err(|e| self.inference_error(e))?;
        self.pending = Some(job);
        Ok(())
    }
    pub fn partial_reply(&self) -> Option<String> {
        self.forge
            .borrow()
            .partial_text(self.pending?)
            .map(ToString::to_string)
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
                    result
                        .map(|g| {
                            Response::new(
                                g.text,
                                Vec::new(),
                                g.model,
                                TokenUsage::new(0, g.generated_tokens),
                            )
                        })
                        .map_err(|e| self.inference_error(e)),
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
        _tools: &[ToolSpec],
        on_token: &mut dyn FnMut(&str),
    ) -> Result<Response, AiError> {
        if self.pending.is_none() {
            self.start_chat(messages)?;
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
        on_token(&generated.text);
        Ok(Response::new(
            generated.text,
            Vec::new(),
            generated.model,
            TokenUsage::new(0, generated.generated_tokens),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{CloudModel, CloudTransport, ForgeError, Generation};
    use alloc::boxed::Box;

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
