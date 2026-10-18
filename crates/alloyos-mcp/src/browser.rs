//! Optional development-host browser/callback assistance. OAuth state, PKCE,
//! client configuration and credentials belong to the OS, not this helper.
use alloc::string::String;
use serde::{Deserialize, Serialize};
pub const MAX_BROWSER_FRAME_BYTES: usize = 32768;
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BrowserRequest {
    pub request_id: u64,
    pub app_id: String,
    pub endpoint: String,
    pub attempt: String,
    pub operation: BrowserOperation,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case", deny_unknown_fields)]
pub enum BrowserOperation {
    Prepare { port: u16 },
    Open { authorization_url: String },
    Poll,
    Cancel,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BrowserResponse {
    pub request_id: u64,
    pub app_id: String,
    pub endpoint: String,
    pub attempt: String,
    pub outcome: BrowserOutcome,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case", deny_unknown_fields)]
pub enum BrowserOutcome {
    Prepared {
        redirect_uri: String,
    },
    Opened,
    Pending,
    /// Sensitive transient callback data for OS validation, never model output.
    Callback {
        query: String,
    },
    Cancelled,
    Error {
        error: BrowserError,
    },
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BrowserError {
    HostUnavailable,
    InvalidRequest,
    Busy,
    BrowserUnavailable,
    CallbackRejected,
    Expired,
}
impl core::fmt::Debug for BrowserRequest {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str("BrowserRequest([private flow])")
    }
}
impl core::fmt::Debug for BrowserResponse {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str("BrowserResponse([private callback])")
    }
}
impl BrowserRequest {
    pub fn validate(&self) -> Result<(), BrowserError> {
        if self.request_id == 0
            || self.app_id.is_empty()
            || self.app_id.len() > 128
            || !self
                .app_id
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_'))
            || self.endpoint.len() > 8192
            || !self.endpoint.starts_with("https://")
            || self
                .endpoint
                .bytes()
                .any(|b| b.is_ascii_control() || b.is_ascii_whitespace())
            || !(16..=128).contains(&self.attempt.len())
            || !self
                .attempt
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_'))
            || matches!(&self.operation,BrowserOperation::Open{authorization_url} if authorization_url.len()>16384)
        {
            return Err(BrowserError::InvalidRequest);
        }
        Ok(())
    }
}
impl BrowserResponse {
    pub fn validate_for(&self, request: &BrowserRequest) -> Result<(), BrowserError> {
        if self.request_id != request.request_id
            || self.app_id != request.app_id
            || self.endpoint != request.endpoint
            || self.attempt != request.attempt
            || matches!(&self.outcome,BrowserOutcome::Callback{query} if query.len()>8192 || query.bytes().any(|b|b.is_ascii_control()))
        {
            return Err(BrowserError::InvalidRequest);
        }
        Ok(())
    }
}
