use alloc::string::String;

/// Errors returned by the Ferrofluid System UI authorization boundary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FerrofluidError {
    /// Strict security gate violation: third-party developer MCP apps and
    /// unauthenticated callers are strictly forbidden from creating windows,
    /// surfaces, or mutating/rendering UI. Only the AI agent may create or
    /// update UI in alloyOS.
    UiCreationForbiddenForApp,
    /// The caller claimed to be the AI agent, but provided a missing or
    /// mismatched `AgentUiToken`.
    InvalidAgentToken,
}

/// Identifies the security principal attempting to interact with `ferrofluidd`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CallerPrincipal {
    /// The privileged alloyOS AI Agent runtime, authorized to synthesize and
    /// update generative UI surfaces and widgets.
    AiAgent { token_id: u64 },
    /// A third-party developer application. In alloyOS, developer apps are
    /// strictly headless MCP servers and cannot create windows, surfaces, or UI.
    DeveloperMcpApp { app_id: String },
    /// An unauthenticated process or unknown caller.
    Unauthenticated,
}

/// Cryptographic/capability token held exclusively by the alloyOS AI Agent
/// runtime to authenticate UI surface creation and generative widget updates.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AgentUiToken {
    pub token_id: u64,
    pub capability_secret: u64,
}

impl AgentUiToken {
    pub const DEFAULT_TOKEN_ID: u64 = 0x4645_5252_4F46_4C01; // "FERROFL\x01"
    pub const DEFAULT_SECRET: u64 = 0xA6E4_7001_9988_7766;

    pub const fn new(token_id: u64, capability_secret: u64) -> Self {
        Self {
            token_id,
            capability_secret,
        }
    }

    pub const fn system_default() -> Self {
        Self::new(Self::DEFAULT_TOKEN_ID, Self::DEFAULT_SECRET)
    }

    pub const fn principal(&self) -> CallerPrincipal {
        CallerPrincipal::AiAgent {
            token_id: self.token_id,
        }
    }
}

/// Enforces the alloyOS Agent-Exclusive UI Security Gate.
pub struct SecurityGate {
    expected_token: AgentUiToken,
}

impl SecurityGate {
    pub const fn new(expected_token: AgentUiToken) -> Self {
        Self { expected_token }
    }

    pub const fn expected_token(&self) -> AgentUiToken {
        self.expected_token
    }

    /// Validates that the caller is the authenticated AI agent and holds the
    /// valid `AgentUiToken`. Rejects `DeveloperMcpApp` and `Unauthenticated`
    /// with `FerrofluidError::UiCreationForbiddenForApp`.
    pub fn authorize(
        &self,
        principal: &CallerPrincipal,
        token: Option<&AgentUiToken>,
    ) -> Result<(), FerrofluidError> {
        match principal {
            CallerPrincipal::DeveloperMcpApp { .. } | CallerPrincipal::Unauthenticated => {
                Err(FerrofluidError::UiCreationForbiddenForApp)
            }
            CallerPrincipal::AiAgent { token_id } => {
                let provided = token.ok_or(FerrofluidError::InvalidAgentToken)?;
                if *token_id == self.expected_token.token_id
                    && provided.token_id == self.expected_token.token_id
                    && provided.capability_secret == self.expected_token.capability_secret
                {
                    Ok(())
                } else {
                    Err(FerrofluidError::InvalidAgentToken)
                }
            }
        }
    }
}
