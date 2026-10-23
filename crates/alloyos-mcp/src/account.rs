//! OS account service contract. Tokens and PKCE verifiers never enter public
//! connection state or model-visible tools and storage.
use alloc::{string::String, vec::Vec};
use serde::{Deserialize, Serialize};

pub const MAX_ACCOUNT_FRAME_BYTES: usize = 32768;
pub const MAX_ACCOUNTS: usize = 32;
pub const MAX_SCOPES: usize = 32;

/// A single bounded Bearer challenge. Multiple schemes, duplicate parameters,
/// controls and escape sequences fail closed instead of differing across layers.
pub struct BearerChallenge {
    pub resource_metadata: Option<String>,
    pub scopes: Vec<String>,
    pub error: Option<String>,
}
pub fn parse_bearer_challenge(value: &str) -> Result<BearerChallenge, AccountError> {
    if value.len()>8192 || value.bytes().any(|b|b.is_ascii_control() || !b.is_ascii()) {
        return Err(AccountError::UnsupportedAuthorization);
    }
    let mut rest=value.trim();
    if !rest.get(..6).is_some_and(|s|s.eq_ignore_ascii_case("bearer"))
        || !rest.get(6..).is_some_and(|s|s.is_empty() || s.starts_with(' ')) {
        return Err(AccountError::UnsupportedAuthorization);
    }
    rest=rest[6..].trim();
    let mut result=BearerChallenge{resource_metadata:None,scopes:Vec::new(),error:None};
    let mut seen=alloc::collections::BTreeSet::new();
    while !rest.is_empty() {
        let (name,tail)=rest.split_once('=').ok_or(AccountError::UnsupportedAuthorization)?;
        let name=name.trim().to_ascii_lowercase();
        if name.is_empty() || !name.bytes().all(|b|b.is_ascii_alphanumeric() || b"!#$%&'*+-.^_`|~".contains(&b)) || !seen.insert(name.clone()) {
            return Err(AccountError::UnsupportedAuthorization);
        }
        let tail=tail.trim_start();
        let (value,tail)=if let Some(tail)=tail.strip_prefix('"') {
            tail.split_once('"').ok_or(AccountError::UnsupportedAuthorization)?
        } else {
            let (value,tail)=tail.find(',').map_or((tail,""),|index|tail.split_at(index));
            let value=value.trim_end();
            if value.is_empty() || !value.bytes().all(|b|b.is_ascii_alphanumeric() || b"!#$%&'*+-.^_`|~".contains(&b)) {return Err(AccountError::UnsupportedAuthorization);}
            // Keep separator handling identical for quoted and token values.
            (value, tail)
        };
        if value.contains('\\') {return Err(AccountError::UnsupportedAuthorization);}
        match name.as_str() {
            "resource_metadata"=>result.resource_metadata=Some(value.into()),
            "error"=>result.error=Some(value.into()),
            "scope"=>{
                result.scopes=value.split_ascii_whitespace().map(String::from).collect();
                if !valid_scopes(&result.scopes) || result.scopes.iter().collect::<alloc::collections::BTreeSet<_>>().len()!=result.scopes.len() {return Err(AccountError::UnsupportedAuthorization);}
            },
            _=>{},
        }
        rest=tail.trim();
        if !rest.is_empty() {
            rest=rest.strip_prefix(',').ok_or(AccountError::UnsupportedAuthorization)?.trim();
            if rest.is_empty(){return Err(AccountError::UnsupportedAuthorization);}
        }
    }
    Ok(result)
}

/// Resource metadata may validate the exact endpoint or its exact origin via
/// root well-known fallback. This bound alone does not prove discovery succeeded.
pub fn valid_oauth_resource_binding(endpoint: &str, resource: &str) -> bool {
    if crate::transport::remote_mcp_authority(endpoint).is_none()
        || crate::transport::remote_mcp_authority(resource).is_none() {return false;}
    if endpoint == resource {return true;}
    let authority = endpoint[8..].split(['/', '?']).next().unwrap_or("");
    resource == &endpoint[..8 + authority.len()]
}

/// Explicitly reviewed provider authorization behavior; never an arbitrary query map.
#[derive(Clone, Default, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag="provider",rename_all="snake_case",deny_unknown_fields)]
pub enum AuthorizationOptions {
    #[default]
    Standard,
    GoogleOffline { reprompt_consent: bool },
}
impl AuthorizationOptions {
    pub fn query_suffix(&self)->&'static str {
        match self {
            Self::Standard=>"",
            Self::GoogleOffline{reprompt_consent:false}=>"&access_type=offline",
            Self::GoogleOffline{reprompt_consent:true}=>"&access_type=offline&prompt=consent",
        }
    }
}

/// Trusted native input. Never place this structure in public display state,
/// model context, logs, or ordinary VFS caches.
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ClientProfile {
    pub endpoint: String,
    pub issuer: String,
    pub client_id: String,
    pub allowed_scopes: Vec<String>,
    pub loopback_port: u16,
    pub client_secret: Option<String>,
    #[serde(default)]
    pub authorization_options: AuthorizationOptions,
}
impl core::fmt::Debug for ClientProfile {
    fn fmt(&self,f:&mut core::fmt::Formatter<'_>)->core::fmt::Result{f.write_str("ClientProfile([private])")}
}
impl ClientProfile {
    pub fn validate(&self)->Result<(),AccountError>{
        let endpoint=|value:&str|value.len()<=8192 && crate::transport::remote_mcp_authority(value).is_some() && !value.contains('#');
        if !matches!(self.authorization_options,AuthorizationOptions::Standard) && self.issuer!="https://accounts.google.com" {return Err(AccountError::InvalidRequest);}
        if !endpoint(&self.endpoint)||!endpoint(&self.issuer)||self.issuer.contains('?')||self.client_id.is_empty()||self.client_id.len()>512||self.client_id.bytes().any(|b|b.is_ascii_control()||b.is_ascii_whitespace())||!valid_scopes(&self.allowed_scopes)||self.client_secret.as_ref().is_some_and(|secret|secret.is_empty()||secret.len()>4000||secret.bytes().any(|b|!b.is_ascii_graphic())) {return Err(AccountError::InvalidRequest)}
        Ok(())
    }
}
#[derive(Clone,Debug,PartialEq,Eq,Serialize,Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PendingClientProfileSetup {pub id:String,pub app_id:String,pub name:String,pub endpoint:String}
#[derive(Clone,Debug,Serialize,Deserialize)]
pub struct ProfileProvisionResult {pub app_id:String,pub storage:SecretStorage}

/// An OS-minted, app/resource-bound capability. Debug deliberately hides it.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct CredentialReference(String);
impl core::fmt::Debug for CredentialReference {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter.write_str("CredentialReference([redacted])")
    }
}
impl CredentialReference {
    pub fn new(value: String) -> Result<Self, AccountError> {
        if !(32..=128).contains(&value.len())
            || !value
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
        {
            return Err(AccountError::InvalidBrokerResponse);
        }
        Ok(Self(value))
    }
    /// Trusted transport only; never include this value in model tool results.
    pub fn for_transport(&self) -> &str {
        &self.0
    }
    pub fn validate(&self) -> Result<(), AccountError> {
        Self::new(self.0.clone()).map(|_| ())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SecretStorage {
    Volatile,
    /// OS-private durable storage. This does not promise encryption at rest.
    OsPrivateStore,
}

/// Fixed errors prevent a host/issuer response from leaking credentials into UI.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AccountError {
    HostUnavailable,
    InvalidRequest,
    InvalidBrokerResponse,
    Busy,
    DiscoveryFailed,
    UnsupportedAuthorization,
    ClientRegistrationRequired,
    BrowserUnavailable,
    AuthorizationDenied,
    AuthorizationExpired,
    CallbackMismatch,
    TokenExchangeFailed,
    CredentialUnavailable,
    RefreshFailed,
    RevocationFailed,
    StorageUnavailable,
    TransportFailed,
}
impl core::fmt::Display for AccountError {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter.write_str(match self {
            Self::HostUnavailable => "Account authorization host is unavailable.",
            Self::InvalidRequest => "Invalid account request.",
            Self::InvalidBrokerResponse => "Invalid account broker response.",
            Self::Busy => "Another account operation is pending.",
            Self::DiscoveryFailed => "Authorization metadata discovery failed.",
            Self::UnsupportedAuthorization => "Server authorization is unsupported.",
            Self::ClientRegistrationRequired => {
                "This server requires an OAuth client registration."
            }
            Self::BrowserUnavailable => "The host browser could not be opened.",
            Self::AuthorizationDenied => "Account authorization was declined.",
            Self::AuthorizationExpired => "Account authorization expired. Try connecting again.",
            Self::CallbackMismatch => "Account authorization callback was rejected.",
            Self::TokenExchangeFailed => "Account authorization token exchange failed.",
            Self::CredentialUnavailable => {
                "Saved account credentials are unavailable. Connect again."
            }
            Self::RefreshFailed => "Account credentials could not be refreshed.",
            Self::RevocationFailed => "Server credential revocation failed.",
            Self::StorageUnavailable => "Account credential storage is unavailable.",
            Self::TransportFailed => "The account broker transport failed.",
        })
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AccountRequest {
    pub request_id: u64,
    pub app_id: String,
    pub endpoint: String,
    pub operation: AccountOperation,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case", deny_unknown_fields)]
pub enum AccountOperation {
    /// Explicit connection intent. The OS owns discovery, PKCE-S256, client
    /// configuration and credentials; an optional assistant opens the browser.
    Begin {
        scopes: Vec<String>,
    },
    Poll {
        attempt: String,
    },
    Refresh {
        credential: CredentialReference,
    },
    Disconnect {
        credential: Option<CredentialReference>,
    },
    /// Recover OS-held credentials by app + exact resource after reboot.
    Status,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AccountResponse {
    pub request_id: u64,
    pub app_id: String,
    pub endpoint: String,
    pub outcome: AccountOutcome,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case", deny_unknown_fields)]
pub enum AccountOutcome {
    Authorizing {
        attempt: String,
    },
    Pending {
        attempt: String,
    },
    /// Authorization granted, NOT proof the MCP service is ready. The guest
    /// must still complete initialize + capability/tool discovery successfully.
    Authorized {
        credential: CredentialReference,
        expires_at: Option<u64>,
        scopes: Vec<String>,
        storage: SecretStorage,
    },
    Disconnected {
        remote_revoked: bool,
    },
    Error {
        error: AccountError,
    },
}

pub fn valid_scopes(scopes: &[String]) -> bool {
    scopes.len() <= MAX_SCOPES
        && scopes.iter().all(|scope| {
            !scope.is_empty()
                && scope.len() <= 256
                && scope
                    .bytes()
                    .all(|byte| (0x21..=0x7e).contains(&byte) && !matches!(byte, b'"' | b'\\'))
        })
}
pub fn valid_attempt(attempt: &str) -> bool {
    (16..=128).contains(&attempt.len())
        && attempt
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
}
impl AccountRequest {
    pub fn validate(&self) -> Result<(), AccountError> {
        if self.request_id == 0
            || self.app_id.is_empty()
            || self.app_id.len() > 128
            || !self
                .app_id
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
            || self.endpoint.len() > 8192
            || crate::transport::remote_mcp_authority(&self.endpoint).is_none()
            || self
                .endpoint
                .bytes()
                .any(|byte| byte.is_ascii_control() || byte.is_ascii_whitespace())
        {
            return Err(AccountError::InvalidRequest);
        }
        match &self.operation {
            AccountOperation::Begin { scopes } if !valid_scopes(scopes) => {
                Err(AccountError::InvalidRequest)
            }
            AccountOperation::Poll { attempt } if !valid_attempt(attempt) => {
                Err(AccountError::InvalidRequest)
            }
            AccountOperation::Refresh { credential } => credential.validate(),
            AccountOperation::Disconnect {
                credential: Some(credential),
            } => credential.validate(),
            _ => Ok(()),
        }
    }
}
impl AccountResponse {
    pub fn validate_for(&self, request: &AccountRequest) -> Result<(), AccountError> {
        if self.request_id != request.request_id
            || self.app_id != request.app_id
            || self.endpoint != request.endpoint
        {
            return Err(AccountError::InvalidBrokerResponse);
        }
        match &self.outcome {
            AccountOutcome::Authorizing { attempt } | AccountOutcome::Pending { attempt }
                if !valid_attempt(attempt) =>
            {
                Err(AccountError::InvalidBrokerResponse)
            }
            AccountOutcome::Authorized {
                credential, scopes, ..
            } => {
                credential.validate()?;
                if !valid_scopes(scopes) {
                    return Err(AccountError::InvalidBrokerResponse);
                }
                Ok(())
            }
            _ => Ok(()),
        }
    }
}

#[cfg(test)]
mod challenge_tests {
    use super::*;
    #[test]
    fn bearer_challenges_reject_ambiguous_or_malformed_parameters() {
        for value in ["Bearerish scope=read", "Bearer scope=read,", "Bearer scope=read,,error=x", "Bearer scope=\"read\" scope=\"write\"", "Bearer scope=\"a a\"", "Bearer scope=\"a\", SCOPE=\"b\"", "Bearer scope=\"unterminated", "Bearer error=insufficient_scope\r\n", "Bearer =x"] {
            assert!(parse_bearer_challenge(value).is_err(),"{value}");
        }
        let parsed=parse_bearer_challenge("bEaReR error=insufficient_scope , scope=\"a b\"").unwrap();
        assert_eq!(parsed.error.as_deref(),Some("insufficient_scope"));
        assert_eq!(parsed.scopes,["a","b"]);
        assert!(parse_bearer_challenge("Bearer").is_ok());
    }
}

#[cfg(test)]
mod authorization_option_tests {
    use super::*;
    #[test]
    fn typed_options_cannot_override_authorization_protocol_parameters() {
        for json in [r#"{"provider":"google_offline","reprompt_consent":true,"state":"override"}"#,
            r#"{"provider":"google_offline","reprompt_consent":"yes"}"#,
            r#"{"provider":"other","client_id":"override"}"#] {
            assert!(serde_json::from_str::<AuthorizationOptions>(json).is_err());
        }
        assert_eq!(AuthorizationOptions::default(),AuthorizationOptions::Standard);
    }
}
