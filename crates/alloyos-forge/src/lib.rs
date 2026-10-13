#![no_std]
#![feature(coroutines, coroutine_trait, stmt_expr_attributes)]
extern crate alloc;
use alloc::{
    boxed::Box,
    collections::BTreeMap,
    string::{String, ToString},
    vec::Vec,
};
use core::{fmt, task::Poll};

pub const DEFAULT_MODEL: &str = "gemma-2-2b-it";
pub const MAX_PROMPT_BYTES: usize = 8192;
pub const MAX_OUTPUT_TOKENS: u32 = 256;
pub const MAX_FRAME_BYTES: usize = 32768;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ForgeError {
    InvalidRequest,
    UnknownModel,
    WeightsMissing,
    BackendUnavailable,
    MemoryBudgetExceeded,
    GenerationFailed,
    AuthenticationFailed,
    RateLimited,
    HttpStatus(u16),
    TransportUnavailable,
    InvalidResponse,
    Busy,
}
impl ForgeError {
    pub const fn code(&self) -> &'static str {
        match self {
            Self::InvalidRequest => "invalid_request",
            Self::UnknownModel => "unknown_model",
            Self::WeightsMissing => "weights_missing",
            Self::BackendUnavailable => "backend_unavailable",
            Self::MemoryBudgetExceeded => "memory_budget_exceeded",
            Self::GenerationFailed => "generation_failed",
            Self::AuthenticationFailed => "authentication_failed",
            Self::RateLimited => "rate_limited",
            Self::HttpStatus(_) => "http_error",
            Self::TransportUnavailable => "https_transport_unavailable",
            Self::InvalidResponse => "invalid_api_response",
            Self::Busy => "inference_busy",
        }
    }
}
impl fmt::Display for ForgeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if let Self::HttpStatus(status) = self {
            write!(f, "http_error_{status}")
        } else {
            f.write_str(self.code())
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GenerationRequest {
    pub model: Option<String>,
    pub prompt: String,
    pub max_tokens: u32,
}
impl GenerationRequest {
    pub fn validate(&self) -> Result<(), ForgeError> {
        if self.prompt.is_empty()
            || self.prompt.len() > MAX_PROMPT_BYTES
            || self.max_tokens == 0
            || self.max_tokens > MAX_OUTPUT_TOKENS
        {
            Err(ForgeError::InvalidRequest)
        } else {
            Ok(())
        }
    }
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Generation {
    pub model: String,
    pub text: String,
    pub generated_tokens: u32,
}

/// Forge owns every backend: resident local weights or an explicit cloud transport.
pub trait InferenceBackend: Send {
    fn partial_text(&self) -> Option<&str> {
        None
    }
    fn resident_bytes(&self) -> u64;
    fn generate(&mut self, request: &GenerationRequest) -> Result<Generation, ForgeError>;
    /// Cooperative backends override these methods; legacy CPU backends complete inline.
    fn start_generation(
        &mut self,
        request: &GenerationRequest,
    ) -> Result<Option<Generation>, ForgeError> {
        self.generate(request).map(Some)
    }
    fn poll_generation(&mut self) -> Poll<Result<Generation, ForgeError>> {
        Poll::Ready(Err(ForgeError::InvalidRequest))
    }
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelStatus {
    pub source: ModelSource,
    pub id: String,
    pub state: &'static str,
    pub weight_bytes: u64,
    pub error: Option<&'static str>,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ModelSource {
    Local,
    Cloud { provider: String },
}

/// Only Forge adapters resolve this reference to volatile boot credentials.
#[derive(Clone)]
pub struct CloudModel {
    pub id: String,
    pub provider: String,
    pub remote_model: String,
    pub credential_reference: String,
}
impl CloudModel {
    pub fn new(
        id: &str,
        provider: &str,
        remote_model: &str,
        reference: &str,
    ) -> Result<Self, ForgeError> {
        if ![id, provider, remote_model, reference].iter().all(|s| {
            !s.is_empty()
                && s.len() <= 128
                && s.bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"-_.".contains(&b))
        }) {
            return Err(ForgeError::InvalidRequest);
        }
        Ok(Self {
            id: id.into(),
            provider: provider.into(),
            remote_model: remote_model.into(),
            credential_reference: reference.into(),
        })
    }
}
pub trait CloudTransport: Send {
    fn generate(
        &mut self,
        model: &CloudModel,
        request: &GenerationRequest,
    ) -> Result<Generation, ForgeError>;
    fn start_generation(
        &mut self,
        model: &CloudModel,
        request: &GenerationRequest,
    ) -> Result<Option<Generation>, ForgeError> {
        self.generate(model, request).map(Some)
    }
    fn poll_generation(&mut self) -> Poll<Result<Generation, ForgeError>> {
        Poll::Ready(Err(ForgeError::InvalidRequest))
    }
    fn partial_text(&self) -> Option<&str> {
        None
    }
}
struct CloudBackend {
    model: CloudModel,
    transport: Box<dyn CloudTransport>,
}
impl InferenceBackend for CloudBackend {
    fn resident_bytes(&self) -> u64 {
        0
    }
    fn generate(&mut self, request: &GenerationRequest) -> Result<Generation, ForgeError> {
        self.transport.generate(&self.model, request)
    }
    fn start_generation(
        &mut self,
        request: &GenerationRequest,
    ) -> Result<Option<Generation>, ForgeError> {
        self.transport.start_generation(&self.model, request)
    }
    fn poll_generation(&mut self) -> Poll<Result<Generation, ForgeError>> {
        self.transport.poll_generation()
    }
    fn partial_text(&self) -> Option<&str> {
        self.transport.partial_text()
    }
}
struct Model {
    source: ModelSource,
    state: &'static str,
    error: Option<&'static str>,
    backend: Option<Box<dyn InferenceBackend>>,
}
struct PendingGeneration {
    id: u64,
    model: String,
    max_tokens: u32,
    immediate: Option<Generation>,
}

/// Long-lived service; registrations and loaded weights survive individual requests.
pub struct Forge {
    models: BTreeMap<String, Model>,
    default_model: String,
    budget: u64,
    resident: u64,
    next_job: u64,
    pending: Option<PendingGeneration>,
}
impl Forge {
    pub fn new(memory_budget: u64) -> Self {
        let mut service = Self {
            models: BTreeMap::new(),
            default_model: DEFAULT_MODEL.to_string(),
            budget: memory_budget,
            resident: 0,
            next_job: 0,
            pending: None,
        };
        service
            .register(DEFAULT_MODEL)
            .expect("valid built-in model ID");
        service
    }
    pub fn register(&mut self, id: &str) -> Result<(), ForgeError> {
        if id.is_empty()
            || id.len() > 128
            || !id
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"-_.".contains(&b))
            || self.models.contains_key(id)
        {
            return Err(ForgeError::InvalidRequest);
        }
        self.models.insert(
            id.to_string(),
            Model {
                source: ModelSource::Local,
                state: "weights_missing",
                error: None,
                backend: None,
            },
        );
        Ok(())
    }
    /// Register a cloud model without pretending that a real transport is ready.
    pub fn register_cloud(&mut self, model: &CloudModel) -> Result<(), ForgeError> {
        CloudModel::new(
            &model.id,
            &model.provider,
            &model.remote_model,
            &model.credential_reference,
        )?;
        self.register(&model.id)?;
        let entry = self
            .models
            .get_mut(&model.id)
            .ok_or(ForgeError::UnknownModel)?;
        entry.source = ModelSource::Cloud {
            provider: model.provider.clone(),
        };
        entry.state = "backend_unavailable";
        Ok(())
    }
    pub fn attach_cloud(
        &mut self,
        model: CloudModel,
        transport: Box<dyn CloudTransport>,
    ) -> Result<(), ForgeError> {
        let entry = self.models.get(&model.id).ok_or(ForgeError::UnknownModel)?;
        if entry.source
            != (ModelSource::Cloud {
                provider: model.provider.clone(),
            })
        {
            return Err(ForgeError::InvalidRequest);
        }
        let id = model.id.clone();
        self.attach(&id, Box::new(CloudBackend { model, transport }))
    }
    pub fn set_default(&mut self, id: &str) -> Result<(), ForgeError> {
        if !self.models.contains_key(id) {
            return Err(ForgeError::UnknownModel);
        }
        self.default_model = id.to_string();
        Ok(())
    }
    pub fn default_model(&self) -> &str {
        &self.default_model
    }
    pub fn models(&self) -> Vec<ModelStatus> {
        self.models
            .iter()
            .map(|(id, m)| ModelStatus {
                id: id.clone(),
                source: m.source.clone(),
                state: m.state,
                error: m.error,
                weight_bytes: m.backend.as_ref().map_or(0, |b| b.resident_bytes()),
            })
            .collect()
    }
    pub fn attach(
        &mut self,
        id: &str,
        backend: Box<dyn InferenceBackend>,
    ) -> Result<(), ForgeError> {
        let m = self.models.get_mut(id).ok_or(ForgeError::UnknownModel)?;
        let old = m.backend.as_ref().map_or(0, |b| b.resident_bytes());
        let new = backend.resident_bytes();
        let total = self
            .resident
            .checked_sub(old)
            .and_then(|v| v.checked_add(new))
            .ok_or(ForgeError::MemoryBudgetExceeded)?;
        if total > self.budget {
            return Err(ForgeError::MemoryBudgetExceeded);
        }
        m.backend = Some(backend);
        m.state = "ready";
        m.error = None;
        self.resident = total;
        Ok(())
    }
    /// The image contains weights, but no executable Gemma backend is linked yet.
    pub fn mark_backend_unavailable(&mut self, id: &str) -> Result<(), ForgeError> {
        let m = self.models.get_mut(id).ok_or(ForgeError::UnknownModel)?;
        if m.backend.is_some() {
            return Err(ForgeError::InvalidRequest);
        }
        m.state = "backend_unavailable";
        Ok(())
    }
    /// Preserve a safe diagnostic code without logging weights or request contents.
    pub fn mark_load_error(&mut self, id: &str, error: &gguf::LoadError) -> Result<(), ForgeError> {
        self.mark_backend_unavailable(id)?;
        self.models
            .get_mut(id)
            .ok_or(ForgeError::UnknownModel)?
            .error = Some(error.code());
        Ok(())
    }
    pub fn transform_backends(&mut self, mut wrap: impl FnMut(&str, Box<dyn InferenceBackend>) -> Box<dyn InferenceBackend>) {
        for (id, model) in &mut self.models {
            if let Some(backend) = model.backend.take() { model.backend = Some(wrap(id, backend)); }
        }
    }
    pub fn partial_text(&self, id: u64) -> Option<&str> {
        let job = self.pending.as_ref().filter(|job| job.id == id)?;
        self.models
            .get(&job.model)?
            .backend
            .as_ref()?
            .partial_text()
    }

    pub fn pending_uses_transport(&self) -> bool {
        self.pending
            .as_ref()
            .and_then(|job| self.models.get(&job.model))
            .is_some_and(|model| matches!(model.source, ModelSource::Cloud { .. }))
    }

    pub fn has_pending_generation(&self) -> bool {
        self.pending.is_some()
    }

    /// Start once, then release the service borrow between bounded polls.
    pub fn start_generation(&mut self, request: &GenerationRequest) -> Result<u64, ForgeError> {
        if self.pending.is_some() {
            return Err(ForgeError::Busy);
        }
        request.validate()?;
        let id = self
            .next_job
            .checked_add(1)
            .ok_or(ForgeError::InvalidRequest)?;
        let model_id = request
            .model
            .as_deref()
            .unwrap_or(&self.default_model)
            .to_string();
        let model = self
            .models
            .get_mut(&model_id)
            .ok_or(ForgeError::UnknownModel)?;
        let backend = model
            .backend
            .as_mut()
            .ok_or(if model.state == "backend_unavailable" {
                ForgeError::BackendUnavailable
            } else {
                ForgeError::WeightsMissing
            })?;
        let immediate = backend.start_generation(request)?;
        self.next_job = id;
        self.pending = Some(PendingGeneration {
            id,
            model: model_id,
            max_tokens: request.max_tokens,
            immediate,
        });
        Ok(id)
    }
    pub fn poll_generation(&mut self, id: u64) -> Poll<Result<Generation, ForgeError>> {
        let Some(job) = self.pending.as_mut() else {
            return Poll::Ready(Err(ForgeError::InvalidRequest));
        };
        if job.id != id {
            return Poll::Ready(Err(ForgeError::InvalidRequest));
        }
        let result = if let Some(output) = job.immediate.take() {
            Poll::Ready(Ok(output))
        } else {
            match self
                .models
                .get_mut(&job.model)
                .and_then(|m| m.backend.as_mut())
            {
                Some(backend) => backend.poll_generation(),
                None => Poll::Ready(Err(ForgeError::BackendUnavailable)),
            }
        };
        match result {
            Poll::Pending => Poll::Pending,
            Poll::Ready(result) => {
                let job = self.pending.take().expect("pending job");
                Poll::Ready(result.and_then(|output| {
                    if output.model != job.model
                        || output.generated_tokens > job.max_tokens
                        || output.text.len() > MAX_FRAME_BYTES / 2
                    {
                        Err(ForgeError::GenerationFailed)
                    } else {
                        Ok(output)
                    }
                }))
            }
        }
    }

    pub fn generate(&mut self, request: &GenerationRequest) -> Result<Generation, ForgeError> {
        if self.pending.is_some() {
            return Err(ForgeError::Busy);
        }
        request.validate()?;
        let id = request.model.as_deref().unwrap_or(&self.default_model);
        let model = self.models.get_mut(id).ok_or(ForgeError::UnknownModel)?;
        let backend = model
            .backend
            .as_mut()
            .ok_or(if model.state == "backend_unavailable" {
                ForgeError::BackendUnavailable
            } else {
                ForgeError::WeightsMissing
            })?;
        let output = backend.generate(request)?;
        if output.model != id
            || output.generated_tokens > request.max_tokens
            || output.text.len() > MAX_FRAME_BYTES / 2
        {
            return Err(ForgeError::GenerationFailed);
        }
        Ok(output)
    }
}

pub mod gemma2;
pub mod gguf;
pub mod protocol;
pub mod tokenizer;
pub use gemma2::Gemma2;

pub mod gateway;
pub mod policy;
pub mod provider;
pub mod providers;
pub use gateway::{ActiveProvider, ModelGateway};

#[cfg(test)]
mod provider_tests;

pub mod cloud;
