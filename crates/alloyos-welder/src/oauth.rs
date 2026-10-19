//! Portable OS-owned OAuth. Private values must never enter the public VFS cache,
//! model tool arguments/results or app manifests. The optional browser port owns
//! only browser launch and callback delivery, never credentials.
use alloc::{
    boxed::Box,
    string::{String, ToString},
    vec::Vec,
};
use alloyos_mcp::{
    account::{AccountError, SecretStorage},
    browser::{BrowserRequest, BrowserResponse},
    transport::{HttpRequest, HttpResponse},
};
use core::task::Poll;
use serde::{Deserialize, Serialize};

/// A trusted OS service port, not an MCP tool or an app capability.
pub trait BrowserAssistant {
    fn start(&mut self, request: &BrowserRequest) -> Poll<Result<BrowserResponse, AccountError>>;
    fn poll(&mut self) -> Poll<Result<BrowserResponse, AccountError>>;
    fn cancel(&mut self);
}
pub trait OAuthNetwork {
    fn start(&mut self, request: &HttpRequest) -> Poll<Result<HttpResponse, AccountError>>;
    fn poll(&mut self) -> Poll<Result<HttpResponse, AccountError>>;
    fn cancel(&mut self);
}
/// Implementations must fail closed without a cryptographic entropy source.
pub trait OAuthCrypto {
    fn random(&mut self, output: &mut [u8]) -> Result<(), AccountError>;
    fn sha256(&self, input: &[u8]) -> [u8; 32];
    fn unix_seconds(&self) -> Option<u64>;
}
/// Trusted private storage. Implementations must exclude these records from all
/// model/app VFS reads, ancestor listings, caches, snapshots and export paths.
/// Durability does not imply encryption at rest.
pub trait CredentialStore {
    fn load(&mut self, app: &str, endpoint: &str) -> Result<Option<PrivateAccount>, AccountError>;
    fn save(&mut self, account: &PrivateAccount) -> Result<SecretStorage, AccountError>;
    fn forget(&mut self, app: &str, endpoint: &str) -> Result<(), AccountError>;
    fn forget_profile(&mut self, _: &str, _: &str) -> Result<(), AccountError> {Err(AccountError::StorageUnavailable)}
    fn profile(&mut self, app: &str, endpoint: &str) -> Result<Option<ClientProfile>, AccountError>;
    fn save_profile(&mut self, _: &str, _: &ClientProfile) -> Result<SecretStorage, AccountError> {Err(AccountError::StorageUnavailable)}
}
pub use alloyos_mcp::account::ClientProfile;
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PrivateAccount {
    pub app_id: String,
    pub endpoint: String,
    pub reference: String,
    pub issuer: String,
    pub client_id: String,
    pub issued_at: u64,
    pub access_token: String,
    pub refresh_token: Option<String>,
    pub token_endpoint: String,
    pub revocation_endpoint: Option<String>,
    pub scopes: Vec<String>,
    pub expires_at: Option<u64>,
}
impl core::fmt::Debug for PrivateAccount {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str("PrivateAccount([redacted])")
    }
}
pub fn base64url(input: &[u8]) -> String {
    const ALPHABET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let mut out = String::new();
    for chunk in input.chunks(3) {
        let n = ((chunk[0] as u32) << 16)
            | ((chunk.get(1).copied().unwrap_or(0) as u32) << 8)
            | chunk.get(2).copied().unwrap_or(0) as u32;
        out.push(ALPHABET[(n >> 18) as usize] as char);
        out.push(ALPHABET[((n >> 12) & 63) as usize] as char);
        if chunk.len() > 1 {
            out.push(ALPHABET[((n >> 6) & 63) as usize] as char);
        }
        if chunk.len() > 2 {
            out.push(ALPHABET[(n & 63) as usize] as char);
        }
    }
    out
}
pub fn form_encode(value: &str) -> String {
    let mut out = String::new();
    const HEX: &[u8] = b"0123456789ABCDEF";
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~') {
            out.push(byte as char)
        } else {
            out.push('%');
            out.push(HEX[(byte >> 4) as usize] as char);
            out.push(HEX[(byte & 15) as usize] as char)
        }
    }
    out
}
fn decode(value: &str) -> Result<String, AccountError> {
    let mut out = Vec::new();
    let bytes = value.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'%' => {
                if i + 2 >= bytes.len() {
                    return Err(AccountError::CallbackMismatch);
                }
                let n = |b: u8| {
                    (b as char)
                        .to_digit(16)
                        .map(|x| x as u8)
                        .ok_or(AccountError::CallbackMismatch)
                };
                out.push((n(bytes[i + 1])? << 4) | n(bytes[i + 2])?);
                i += 3
            }
            b'+' => {
                out.push(b' ');
                i += 1
            }
            b => {
                out.push(b);
                i += 1
            }
        }
    }
    String::from_utf8(out).map_err(|_| AccountError::CallbackMismatch)
}
/// Exact duplicate-sensitive callback validation, owned by the OS. Issuer
/// response parameter is required; the browser helper is not an OAuth authority.
pub fn callback_code(query: &str, state: &str, issuer: &str) -> Result<String, AccountError> {
    callback_code_for_metadata(query, state, issuer, true)
}
/// MCP 2026-07-28 / RFC9207: advertised support requires `iss`; otherwise
/// omission is allowed, but any supplied issuer must still match exactly.
pub fn callback_code_for_metadata(
    query: &str,
    state: &str,
    issuer: &str,
    require_issuer: bool,
) -> Result<String, AccountError> {
    if query.len() > 8192 {
        return Err(AccountError::CallbackMismatch);
    }
    let mut values = alloc::collections::BTreeMap::new();
    for pair in query.split('&') {
        let (k, v) = pair.split_once('=').ok_or(AccountError::CallbackMismatch)?;
        let key = decode(k)?;
        let value = decode(v)?;
        if values.insert(key, value).is_some() {
            return Err(AccountError::CallbackMismatch);
        }
    }
    if values.get("state").map(String::as_str) != Some(state)
        || values.get("iss").is_some_and(|value| value != issuer)
        || (require_issuer && !values.contains_key("iss"))
    {
        return Err(AccountError::CallbackMismatch);
    }
    if values.contains_key("error") {
        return Err(AccountError::AuthorizationDenied);
    }
    values
        .remove("code")
        .filter(|v| !v.is_empty() && v.len() <= 4096 && !v.bytes().any(|b| b.is_ascii_control()))
        .ok_or(AccountError::CallbackMismatch)
}

use crate::accounts::AccountBroker;
use alloyos_mcp::account::{
    AccountOperation, AccountOutcome, AccountRequest, AccountResponse, CredentialReference,
};
use alloyos_mcp::browser::{BrowserOperation, BrowserOutcome};
use serde_json::Value;

pub struct OsAccountBroker {
    store: Box<dyn CredentialStore>,
    browser: Box<dyn BrowserAssistant>,
    network: Box<dyn OAuthNetwork>,
    crypto: Box<dyn OAuthCrypto>,
    job: Option<Job>,
    flow: Option<Flow>,
    next_browser: u64,
    challenges: alloc::collections::BTreeMap<String, (Option<String>, Vec<String>)>,
}
struct Flow {
    app: String,
    endpoint: String,
    profile: ClientProfile,
    scopes: Vec<String>,
    attempt: String,
    state: String,
    verifier: String,
    redirect: String,
    authorization_endpoint: String,
    require_issuer: bool,
    token_endpoint: String,
    revocation_endpoint: Option<String>,
    started: Option<u64>,
}
struct Job {
    request: AccountRequest,
    stage: Stage,
}
enum Stage {
    Resource,
    ResourceRoot,
    Issuer,
    IssuerOidc,
    Prepare,
    Open,
    Callback,
    Token,
    Refresh(PrivateAccount),
    Revoke,
}
impl OsAccountBroker {
    pub fn new(
        store: Box<dyn CredentialStore>,
        browser: Box<dyn BrowserAssistant>,
        network: Box<dyn OAuthNetwork>,
        crypto: Box<dyn OAuthCrypto>,
    ) -> Self {
        Self {
            store,
            browser,
            network,
            crypto,
            job: None,
            flow: None,
            next_browser: 1,
            challenges: alloc::collections::BTreeMap::new(),
        }
    }
    pub fn bearer(
        &mut self,
        app: &str,
        endpoint: &str,
        reference: &CredentialReference,
    ) -> Result<String, AccountError> {
        let record = self
            .store
            .load(app, endpoint)?
            .ok_or(AccountError::CredentialUnavailable)?;
        let profile = self
            .store
            .profile(app,endpoint)?
            .ok_or(AccountError::ClientRegistrationRequired)?;
        if profile.endpoint != endpoint
            || profile.issuer != record.issuer
            || profile.client_id != record.client_id
            || record.scopes.iter().any(|scope|!profile.allowed_scopes.contains(scope))
        {
            return Err(AccountError::CredentialUnavailable);
        }
        if record.app_id != app
            || record.endpoint != endpoint
            || record.reference != reference.for_transport()
            || !valid_token(&record.access_token)
            || self
                .crypto
                .unix_seconds()
                .is_none_or(|now| now < record.issued_at)
        {
            return Err(AccountError::CredentialUnavailable);
        }
        if record.expires_at.is_some_and(|expiration| {
            self.crypto
                .unix_seconds()
                .is_none_or(|now| now >= expiration)
        }) {
            return Err(AccountError::CredentialUnavailable);
        }
        Ok(record.access_token)
    }
    fn random_id(&mut self) -> Result<String, AccountError> {
        let mut bytes = [0u8; 32];
        self.crypto.random(&mut bytes)?;
        Ok(base64url(&bytes))
    }
    fn response(request: &AccountRequest, outcome: AccountOutcome) -> AccountResponse {
        AccountResponse {
            request_id: request.request_id,
            app_id: request.app_id.clone(),
            endpoint: request.endpoint.clone(),
            outcome,
        }
    }
    fn authorized(&mut self, record: PrivateAccount) -> Result<AccountOutcome, AccountError> {
        let credential = CredentialReference::new(record.reference.clone())?;
        let storage = self.store.save(&record)?;
        Ok(AccountOutcome::Authorized {
            credential,
            expires_at: record.expires_at,
            scopes: record.scopes.clone(),
            storage,
        })
    }
    fn http(
        &mut self,
        request: AccountRequest,
        stage: Stage,
        http: HttpRequest,
    ) -> Poll<Result<AccountResponse, AccountError>> {
        self.job = Some(Job { request, stage });
        match self.network.start(&http) {
            Poll::Pending => Poll::Pending,
            Poll::Ready(result) => self.http_result(result),
        }
    }
    fn browser_request(
        &mut self,
        operation: BrowserOperation,
    ) -> Result<BrowserRequest, AccountError> {
        let flow = self.flow.as_ref().ok_or(AccountError::InvalidRequest)?;
        self.next_browser = self
            .next_browser
            .checked_add(1)
            .ok_or(AccountError::InvalidRequest)?;
        Ok(BrowserRequest {
            request_id: self.next_browser,
            app_id: flow.app.clone(),
            endpoint: flow.endpoint.clone(),
            attempt: flow.attempt.clone(),
            operation,
        })
    }
    fn browser_start(
        &mut self,
        request: AccountRequest,
        stage: Stage,
        operation: BrowserOperation,
    ) -> Poll<Result<AccountResponse, AccountError>> {
        let browser = match self.browser_request(operation) {
            Ok(request) => request,
            Err(error) => return Poll::Ready(Err(error)),
        };
        self.job = Some(Job { request, stage });
        match self.browser.start(&browser) {
            Poll::Pending => Poll::Pending,
            Poll::Ready(response) => self.browser_result(response),
        }
    }
    fn http_result(
        &mut self,
        result: Result<HttpResponse, AccountError>,
    ) -> Poll<Result<AccountResponse, AccountError>> {
        let Some(job) = self.job.take() else {
            return Poll::Ready(Err(AccountError::InvalidRequest));
        };
        if matches!(job.stage, Stage::Revoke) {
            let remote_revoked = result
                .as_ref()
                .is_ok_and(|response| (200..300).contains(&response.status));
            return Poll::Ready(Ok(Self::response(
                &job.request,
                AccountOutcome::Disconnected { remote_revoked },
            )));
        }
        let response = match result {
            Ok(response) if response.body.len() <= 131072 => response,
            Ok(_) => return self.fail(AccountError::InvalidBrokerResponse),
            Err(error) => return self.fail(error),
        };
        if response.status == 404 {
            if matches!(job.stage, Stage::Resource) {
                let endpoint = &job.request.endpoint[8..];
                let authority = endpoint
                    .split(['/', '?'])
                    .next()
                    .unwrap_or(endpoint)
                    .to_string();
                return self.http(
                    job.request,
                    Stage::ResourceRoot,
                    get(alloc::format!(
                        "https://{authority}/.well-known/oauth-protected-resource"
                    )),
                );
            }
            if matches!(job.stage, Stage::Issuer) {
                let issuer = self
                    .flow
                    .as_ref()
                    .unwrap()
                    .profile
                    .issuer
                    .trim_end_matches('/');
                let url = alloc::format!("{issuer}/.well-known/openid-configuration");
                return self.http(job.request, Stage::IssuerOidc, get(url));
            }
        }
        if !(200..300).contains(&response.status) {
            return self.fail(
                if matches!(
                    job.stage,
                    Stage::Resource | Stage::ResourceRoot | Stage::Issuer | Stage::IssuerOidc
                ) {
                    AccountError::DiscoveryFailed
                } else {
                    AccountError::TokenExchangeFailed
                },
            );
        }
        let value: Value =
            match serde_json::from_str::<UniqueValue>(&response.body).map(|value| value.0) {
                Ok(value) => value,
                Err(_) => return self.fail(AccountError::InvalidBrokerResponse),
            };
        match job.stage {
            Stage::Resource | Stage::ResourceRoot => {
                let flow = self.flow.as_mut().unwrap();
                if value.get("resource").and_then(Value::as_str) != Some(flow.endpoint.as_str())
                    || !value
                        .get("authorization_servers")
                        .and_then(Value::as_array)
                        .is_some_and(|issuers| {
                            issuers
                                .iter()
                                .any(|issuer| issuer.as_str() == Some(flow.profile.issuer.as_str()))
                        })
                {
                    return self.fail(AccountError::UnsupportedAuthorization);
                }
                if let Some(advertised) = value.get("scopes_supported") {
                    let Some(advertised) = advertised.as_array() else {
                        return self.fail(AccountError::UnsupportedAuthorization);
                    };
                    if advertised.len() > alloyos_mcp::account::MAX_SCOPES {
                        return self.fail(AccountError::UnsupportedAuthorization);
                    }
                    let Some(scopes) = advertised.iter().map(|scope| scope.as_str().map(ToString::to_string)).collect::<Option<Vec<_>>>() else {
                        return self.fail(AccountError::UnsupportedAuthorization);
                    };
                    if !alloyos_mcp::account::valid_scopes(&scopes)
                        || scopes.iter().collect::<alloc::collections::BTreeSet<_>>().len() != scopes.len() {
                        return self.fail(AccountError::UnsupportedAuthorization);
                    }
                    // Explicitly selected or challenged scopes take precedence.
                    // Resource metadata never expands the native profile grant.
                    if flow.scopes.is_empty() {
                        if scopes.iter().any(|scope| !flow.profile.allowed_scopes.contains(scope)) {
                            return self.fail(AccountError::ClientRegistrationRequired);
                        }
                        flow.scopes = scopes;
                    }
                }
                let issuer = flow.profile.issuer.trim_end_matches('/');
                // RFC8414 inserts the well-known component before issuer path.
                let suffix = &issuer[8..];
                let (authority, path) =
                    suffix.split_once('/').map_or((suffix, ""), |(a, p)| (a, p));
                let url = alloc::format!(
                    "https://{authority}/.well-known/oauth-authorization-server{}{}",
                    if path.is_empty() { "" } else { "/" },
                    path
                );
                self.http(job.request, Stage::Issuer, get(url))
            }
            Stage::Issuer | Stage::IssuerOidc => {
                let flow = self.flow.as_mut().unwrap();
                if value.get("issuer").and_then(Value::as_str) != Some(flow.profile.issuer.as_str())
                    || !value
                        .get("code_challenge_methods_supported")
                        .and_then(Value::as_array)
                        .is_some_and(|methods| {
                            methods.iter().any(|method| method.as_str() == Some("S256"))
                        })
                {
                    return self.fail(AccountError::UnsupportedAuthorization);
                }
                flow.require_issuer =
                    match value.get("authorization_response_iss_parameter_supported") {
                        None => false,
                        Some(Value::Bool(value)) => *value,
                        _ => return self.fail(AccountError::UnsupportedAuthorization),
                    };
                flow.authorization_endpoint = match endpoint(&value, "authorization_endpoint") {
                    Ok(v) => v,
                    Err(e) => return self.fail(e),
                };
                flow.token_endpoint = match endpoint(&value, "token_endpoint") {
                    Ok(v) => v,
                    Err(e) => return self.fail(e),
                };
                flow.revocation_endpoint = value
                    .get("revocation_endpoint")
                    .and_then(Value::as_str)
                    .filter(|url| public_url(url))
                    .map(ToString::to_string);
                let port = flow.profile.loopback_port;
                self.browser_start(
                    job.request,
                    Stage::Prepare,
                    BrowserOperation::Prepare { port },
                )
            }
            Stage::Token | Stage::Refresh(_) => {
                let old = if let Stage::Refresh(record) = job.stage {
                    Some(record)
                } else {
                    None
                };
                let flow = self.flow.as_ref();
                let requested = old
                    .as_ref()
                    .map(|r| r.scopes.clone())
                    .unwrap_or_else(|| flow.unwrap().scopes.clone());
                let scopes = match value.get("scope") {
                    Some(Value::String(scopes)) => scopes
                        .split_ascii_whitespace()
                        .map(ToString::to_string)
                        .collect::<Vec<_>>(),
                    None => requested.clone(),
                    _ => return self.fail(AccountError::TokenExchangeFailed),
                };
                if !alloyos_mcp::account::valid_scopes(&scopes)
                    || scopes.iter().any(|scope| !requested.contains(scope))
                {
                    return self.fail(AccountError::TokenExchangeFailed);
                }
                if value
                    .get("token_type")
                    .and_then(Value::as_str)
                    .is_none_or(|s| !s.eq_ignore_ascii_case("bearer"))
                {
                    return self.fail(AccountError::TokenExchangeFailed);
                }
                let token = match value
                    .get("access_token")
                    .and_then(Value::as_str)
                    .filter(|s| valid_token(s))
                {
                    Some(s) => s.to_string(),
                    None => return self.fail(AccountError::TokenExchangeFailed),
                };
                let refresh = match value.get("refresh_token") {
                    None => old.as_ref().and_then(|r| r.refresh_token.clone()),
                    Some(Value::String(s)) if valid_token(s) => Some(s.clone()),
                    _ => return self.fail(AccountError::TokenExchangeFailed),
                };
                let expiration = match value.get("expires_in") {
                    None => None,
                    Some(v) => match v
                        .as_u64()
                        .filter(|n| *n > 0 && *n <= 31536000)
                        .and_then(|n| self.crypto.unix_seconds()?.checked_add(n))
                    {
                        Some(v) => Some(v),
                        None => return self.fail(AccountError::TokenExchangeFailed),
                    },
                };
                let issued_at = match self.crypto.unix_seconds() {
                    Some(now) => now,
                    None => return self.fail(AccountError::TokenExchangeFailed),
                };
                let record = if let Some(old) = old {
                    PrivateAccount {
                        access_token: token,
                        refresh_token: refresh,
                        scopes,
                        expires_at: expiration,
                        issued_at,
                        ..old
                    }
                } else {
                    let reference = match self.random_id() {
                        Ok(reference) => reference,
                        Err(error) => return self.fail(error),
                    };
                    let flow = self.flow.as_ref().unwrap();
                    PrivateAccount {
                        app_id: flow.app.clone(),
                        endpoint: flow.endpoint.clone(),
                        reference,
                        issuer: flow.profile.issuer.clone(),
                        client_id: flow.profile.client_id.clone(),
                        issued_at,
                        access_token: token,
                        refresh_token: refresh,
                        token_endpoint: flow.token_endpoint.clone(),
                        revocation_endpoint: flow.revocation_endpoint.clone(),
                        scopes,
                        expires_at: expiration,
                    }
                };
                let outcome = self.authorized(record);
                self.flow = None;
                Poll::Ready(outcome.map(|outcome| Self::response(&job.request, outcome)))
            }
            _ => self.fail(AccountError::InvalidRequest),
        }
    }
    fn fail(&mut self, error: AccountError) -> Poll<Result<AccountResponse, AccountError>> {
        self.flow = None;
        self.job = None;
        self.browser.cancel();
        self.network.cancel();
        Poll::Ready(Err(error))
    }
    fn browser_result(
        &mut self,
        result: Result<BrowserResponse, AccountError>,
    ) -> Poll<Result<AccountResponse, AccountError>> {
        let Some(job) = self.job.take() else {
            return Poll::Ready(Err(AccountError::InvalidRequest));
        };
        let response = match result {
            Ok(response) => response,
            Err(error) => return self.fail(error),
        };
        let flow = self.flow.as_ref().unwrap();
        if response.request_id != self.next_browser
            || response.app_id != flow.app
            || response.endpoint != flow.endpoint
            || response.attempt != flow.attempt
        {
            return self.fail(AccountError::CallbackMismatch);
        }
        if flow
            .started
            .zip(self.crypto.unix_seconds())
            .is_some_and(|(start, now)| now.saturating_sub(start) > 600)
        {
            return self.fail(AccountError::AuthorizationExpired);
        }
        match (job.stage, response.outcome) {
            (Stage::Prepare, BrowserOutcome::Prepared { redirect_uri }) => {
                // The assistant may only supply a numeric loopback callback.
                if !redirect_uri.starts_with("http://127.0.0.1:")
                    || redirect_uri.len() > 256
                    || redirect_uri
                        .bytes()
                        .any(|b| b.is_ascii_whitespace() || b.is_ascii_control())
                    || redirect_uri.contains(['?', '#', '@'])
                    || !redirect_uri[17..]
                        .split_once('/')
                        .is_some_and(|(port, path)| {
                            !path.is_empty()
                                && port.parse::<u16>().is_ok_and(|port| {
                                    port != 0
                                        && (flow.profile.loopback_port == 0
                                            || port == flow.profile.loopback_port)
                                })
                        })
                {
                    return self.fail(AccountError::CallbackMismatch);
                }
                let challenge = base64url(&self.crypto.sha256(flow.verifier.as_bytes()));
                let scope = if flow.scopes.is_empty() { String::new() } else { alloc::format!("&scope={}",form_encode(&flow.scopes.join(" "))) };
                let url=alloc::format!("{}{}response_type=code&client_id={}&redirect_uri={}{}&state={}&code_challenge={}&code_challenge_method=S256&resource={}",flow.authorization_endpoint,if flow.authorization_endpoint.contains('?'){"&"}else{"?"},form_encode(&flow.profile.client_id),form_encode(&redirect_uri),scope,form_encode(&flow.state),challenge,form_encode(&flow.endpoint));
                self.flow.as_mut().unwrap().redirect = redirect_uri;
                self.browser_start(
                    job.request,
                    Stage::Open,
                    BrowserOperation::Open {
                        authorization_url: url,
                    },
                )
            }
            (Stage::Open, BrowserOutcome::Opened) => Poll::Ready(Ok(Self::response(
                &job.request,
                AccountOutcome::Authorizing {
                    attempt: flow.attempt.clone(),
                },
            ))),
            (Stage::Callback, BrowserOutcome::Pending) => Poll::Ready(Ok(Self::response(
                &job.request,
                AccountOutcome::Pending {
                    attempt: flow.attempt.clone(),
                },
            ))),
            (Stage::Callback, BrowserOutcome::Callback { query }) => {
                let code = match callback_code_for_metadata(
                    &query,
                    &flow.state,
                    &flow.profile.issuer,
                    flow.require_issuer,
                ) {
                    Ok(code) => code,
                    Err(error) => return self.fail(error),
                };
                let mut body=alloc::format!("grant_type=authorization_code&code={}&redirect_uri={}&client_id={}&code_verifier={}&resource={}",form_encode(&code),form_encode(&flow.redirect),form_encode(&flow.profile.client_id),form_encode(&flow.verifier),form_encode(&flow.endpoint));
                if let Some(secret) = &flow.profile.client_secret {
                    body.push_str("&client_secret=");
                    body.push_str(&form_encode(secret));
                }
                let http = post(flow.token_endpoint.clone(), body);
                self.http(job.request, Stage::Token, http)
            }
            _ => self.fail(AccountError::BrowserUnavailable),
        }
    }
}
fn public_url(url: &str) -> bool {
    alloyos_mcp::transport::remote_mcp_authority(url).is_some() && !url.contains('#')
}
fn endpoint(value: &Value, key: &str) -> Result<String, AccountError> {
    value
        .get(key)
        .and_then(Value::as_str)
        .filter(|url| public_url(url))
        .map(ToString::to_string)
        .ok_or(AccountError::UnsupportedAuthorization)
}
fn valid_token(token: &str) -> bool {
    !token.is_empty()
        && token.len() <= alloyos_mcp::transport::MAX_BEARER_BYTES
        && token.bytes().all(|b| b.is_ascii_graphic())
}
fn get(url: String) -> HttpRequest {
    HttpRequest {
        method: "GET",
        url,
        headers: Vec::new(),
        body: String::new(),
    }
}
fn post(url: String, body: String) -> HttpRequest {
    HttpRequest {
        method: "POST",
        url,
        headers: alloc::vec![(
            "Content-Type".into(),
            "application/x-www-form-urlencoded".into()
        )],
        body,
    }
}
impl AccountBroker for OsAccountBroker {
    fn start(&mut self, request: &AccountRequest) -> Poll<Result<AccountResponse, AccountError>> {
        if let Err(error) = request.validate() {
            return Poll::Ready(Err(error));
        }
        if self.job.is_some() {
            return Poll::Ready(Err(AccountError::Busy));
        }
        match &request.operation {
            AccountOperation::Begin { scopes } => {
                let advertised = self.challenges.get(&request.endpoint).cloned();
                let scopes = if scopes.is_empty() {
                    advertised
                        .as_ref()
                        .map(|(_, scopes)| scopes.clone())
                        .unwrap_or_default()
                } else {
                    scopes.clone()
                };
                if self.flow.is_some() {
                    return Poll::Ready(Err(AccountError::Busy));
                }
                if self.crypto.unix_seconds().is_none() {
                    return Poll::Ready(Err(AccountError::HostUnavailable));
                }
                let profile = match self.store.profile(&request.app_id,&request.endpoint) {
                    Ok(Some(profile)) => profile,
                    Ok(None) => return Poll::Ready(Err(AccountError::ClientRegistrationRequired)),
                    Err(e) => return Poll::Ready(Err(e)),
                };
                if profile.endpoint != request.endpoint
                    || !public_url(&profile.issuer)
                    || profile.issuer.contains('?')
                    || profile.client_id.is_empty()
                    || profile.client_id.len() > 512
                    || !alloyos_mcp::account::valid_scopes(&profile.allowed_scopes)
                    || scopes.iter().any(|s| !profile.allowed_scopes.contains(s))
                    || profile
                        .client_secret
                        .as_ref()
                        .is_some_and(|s| !valid_token(s))
                {
                    return Poll::Ready(Err(AccountError::InvalidRequest));
                }
                let attempt = match self.random_id() {
                    Ok(id) => id,
                    Err(e) => return Poll::Ready(Err(e)),
                };
                let state = match self.random_id() {
                    Ok(id) => id,
                    Err(e) => return Poll::Ready(Err(e)),
                };
                let verifier = match self.random_id() {
                    Ok(id) => id,
                    Err(e) => return Poll::Ready(Err(e)),
                };
                self.flow = Some(Flow {
                    app: request.app_id.clone(),
                    endpoint: request.endpoint.clone(),
                    profile,
                    scopes: scopes.clone(),
                    attempt,
                    state,
                    verifier,
                    redirect: String::new(),
                    authorization_endpoint: String::new(),
                    require_issuer: false,
                    token_endpoint: String::new(),
                    revocation_endpoint: None,
                    started: self.crypto.unix_seconds(),
                });
                let suffix = request.endpoint[8..].split('?').next().unwrap_or("");
                let (authority, path) =
                    suffix.split_once('/').map_or((suffix, ""), |(a, p)| (a, p));
                let metadata = advertised
                    .and_then(|(metadata, _)| metadata)
                    .unwrap_or_else(|| {
                        alloc::format!(
                            "https://{authority}/.well-known/oauth-protected-resource{}{}",
                            if path.is_empty() { "" } else { "/" },
                            path
                        )
                    });
                self.http(request.clone(), Stage::Resource, get(metadata))
            }
            AccountOperation::Poll { attempt } => {
                if !self.flow.as_ref().is_some_and(|f| {
                    f.app == request.app_id
                        && f.endpoint == request.endpoint
                        && f.attempt == *attempt
                }) {
                    return Poll::Ready(Err(AccountError::CallbackMismatch));
                }
                self.browser_start(request.clone(), Stage::Callback, BrowserOperation::Poll)
            }
            AccountOperation::Status => match self.store.load(&request.app_id, &request.endpoint) {
                Ok(Some(record))
                    if record.app_id == request.app_id
                        && record.endpoint == request.endpoint
                        && self
                            .crypto
                            .unix_seconds()
                            .is_some_and(|now| now >= record.issued_at) =>
                {
                    let result = self.authorized(record);
                    Poll::Ready(result.map(|outcome| Self::response(request, outcome)))
                }
                Ok(_) => Poll::Ready(Err(AccountError::CredentialUnavailable)),
                Err(e) => Poll::Ready(Err(e)),
            },
            AccountOperation::Refresh { credential } => {
                let record = match self.store.load(&request.app_id, &request.endpoint) {
                    Ok(Some(record))
                        if record.app_id == request.app_id
                            && record.endpoint == request.endpoint
                            && record.reference == credential.for_transport() =>
                    {
                        record
                    }
                    Ok(_) => return Poll::Ready(Err(AccountError::CredentialUnavailable)),
                    Err(e) => return Poll::Ready(Err(e)),
                };
                if self
                    .crypto
                    .unix_seconds()
                    .is_none_or(|now| now < record.issued_at)
                {
                    return Poll::Ready(Err(AccountError::CredentialUnavailable));
                }
                let refresh = match &record.refresh_token {
                    Some(token) => token,
                    None => return Poll::Ready(Err(AccountError::RefreshFailed)),
                };
                let profile = match self.store.profile(&request.app_id,&request.endpoint) {
                    Ok(Some(profile))
                        if profile.endpoint == request.endpoint
                            && profile.issuer == record.issuer
                            && profile.client_id == record.client_id
                            && record.scopes.iter().all(|scope|profile.allowed_scopes.contains(scope)) =>
                    {
                        profile
                    }
                    _ => return Poll::Ready(Err(AccountError::ClientRegistrationRequired)),
                };
                let mut body = alloc::format!(
                    "grant_type=refresh_token&refresh_token={}&client_id={}&resource={}",
                    form_encode(refresh),
                    form_encode(&profile.client_id),
                    form_encode(&record.endpoint)
                );
                if let Some(secret) = profile.client_secret {
                    body.push_str("&client_secret=");
                    body.push_str(&form_encode(&secret));
                }
                let url = record.token_endpoint.clone();
                if !public_url(&url) {
                    return Poll::Ready(Err(AccountError::RefreshFailed));
                }
                self.http(request.clone(), Stage::Refresh(record), post(url, body))
            }
            AccountOperation::Disconnect { credential } => {
                let record = match self.store.load(&request.app_id, &request.endpoint) {
                    Ok(record) => record,
                    Err(_) => None,
                };
                if let (Some(record), Some(reference)) = (&record, credential) {
                    if record.reference != reference.for_transport() {
                        return Poll::Ready(Err(AccountError::CredentialUnavailable));
                    }
                }
                // Durable local revocation is authoritative before any provider I/O.
                if let Err(e) = self.store.forget(&request.app_id, &request.endpoint) {
                    return Poll::Ready(Err(e));
                }
                self.browser.cancel();
                self.flow = None;
                if let Some(record) = record {
                    if let Some(url) = record.revocation_endpoint.filter(|url| public_url(url)) {
                        let mut body = alloc::format!(
                            "client_id={}&token={}",
                            form_encode(&record.client_id),
                            form_encode(
                                record
                                    .refresh_token
                                    .as_deref()
                                    .unwrap_or(&record.access_token)
                            )
                        );
                        if let Ok(Some(profile)) = self.store.profile(&record.app_id,&record.endpoint) {
                            if profile.issuer == record.issuer
                                && profile.client_id == record.client_id
                            {
                                if let Some(secret) = profile.client_secret {
                                    body.push_str("&client_secret=");
                                    body.push_str(&form_encode(&secret));
                                }
                            }
                        }
                        return self.http(request.clone(), Stage::Revoke, post(url, body));
                    }
                }
                Poll::Ready(Ok(Self::response(
                    request,
                    AccountOutcome::Disconnected {
                        remote_revoked: false,
                    },
                )))
            }
        }
    }
    fn poll(&mut self) -> Poll<Result<AccountResponse, AccountError>> {
        let browser = matches!(
            self.job.as_ref().map(|j| &j.stage),
            Some(Stage::Prepare | Stage::Open | Stage::Callback)
        );
        if browser {
            match self.browser.poll() {
                Poll::Pending => Poll::Pending,
                Poll::Ready(response) => self.browser_result(response),
            }
        } else {
            match self.network.poll() {
                Poll::Pending => Poll::Pending,
                Poll::Ready(response) => self.http_result(response),
            }
        }
    }
    fn cancel(&mut self) {
        self.job = None;
        self.flow = None;
        self.browser.cancel();
        self.network.cancel();
    }
    fn provision_profile(&mut self, app:&str, profile:&ClientProfile)->Result<SecretStorage,AccountError>{
        profile.validate()?;
        if self.flow.as_ref().is_some_and(|flow|flow.app!=app) {return Err(AccountError::Busy)}
        self.cancel();
        self.store.forget(app,&profile.endpoint)?;
        self.store.save_profile(app,profile)
    }
    fn forget_profile(&mut self,app:&str,endpoint:&str)->Result<(),AccountError>{
        if self.flow.as_ref().is_some_and(|flow|flow.app!=app){return Err(AccountError::Busy)}
        self.cancel();
        self.store.forget(app,endpoint)?;
        self.store.forget_profile(app,endpoint)
    }
    fn note_challenge(&mut self, endpoint: &str, challenge: &str) -> Result<(), AccountError> {
        if !public_url(endpoint)
            || challenge.len() > 8192
            || challenge.bytes().any(|b| b.is_ascii_control())
        {
            return Err(AccountError::UnsupportedAuthorization);
        }
        let mut rest = challenge.trim();
        if !rest
            .get(..6)
            .is_some_and(|scheme| scheme.eq_ignore_ascii_case("bearer"))
        {
            return Err(AccountError::UnsupportedAuthorization);
        }
        rest = rest[6..].trim();
        let mut metadata = None;
        let mut scopes = Vec::new();
        let mut seen = alloc::collections::BTreeSet::new();
        while !rest.is_empty() {
            let (name, value) = rest
                .split_once('=')
                .ok_or(AccountError::UnsupportedAuthorization)?;
            let name = name.trim().to_ascii_lowercase();
            if !seen.insert(name.clone()) {
                return Err(AccountError::UnsupportedAuthorization);
            }
            let value = value.trim_start();
            let (value, tail) = if let Some(value) = value.strip_prefix('"') {
                let (end, tail) = value
                    .split_once('"')
                    .ok_or(AccountError::UnsupportedAuthorization)?;
                (end, tail)
            } else {
                value
                    .split_once(',')
                    .map_or((value, ""), |(value, tail)| (value, tail))
            };
            if value.contains('\\') {
                return Err(AccountError::UnsupportedAuthorization);
            }
            if name == "resource_metadata" {
                if !public_url(value) {
                    return Err(AccountError::UnsupportedAuthorization);
                }
                metadata = Some(value.to_string());
            }
            if name == "scope" {
                scopes = value
                    .split_ascii_whitespace()
                    .map(ToString::to_string)
                    .collect();
                if !alloyos_mcp::account::valid_scopes(&scopes) {
                    return Err(AccountError::UnsupportedAuthorization);
                }
            }
            rest = tail.trim();
            if let Some(tail) = rest.strip_prefix(',') {
                rest = tail.trim()
            } else if !rest.is_empty() {
                return Err(AccountError::UnsupportedAuthorization);
            }
        }
        if self.challenges.len() >= 32 && !self.challenges.contains_key(endpoint) {
            return Err(AccountError::Busy);
        }
        self.challenges
            .insert(endpoint.to_string(), (metadata, scopes));
        Ok(())
    }
    fn bearer(
        &mut self,
        app: &str,
        endpoint: &str,
        reference: &CredentialReference,
    ) -> Result<Option<String>, AccountError> {
        OsAccountBroker::bearer(self, app, endpoint, reference).map(Some)
    }
}

/// OAuth JSON is security metadata. Reject duplicate members at every depth
/// instead of allowing a last-member-wins interpretation to disagree with peers.
struct UniqueValue(Value);
impl<'de> Deserialize<'de> for UniqueValue {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct Visitor;
        impl<'de> serde::de::Visitor<'de> for Visitor {
            type Value = UniqueValue;
            fn expecting(&self, f: &mut core::fmt::Formatter) -> core::fmt::Result {
                f.write_str("JSON without duplicate members")
            }
            fn visit_unit<E: serde::de::Error>(self) -> Result<Self::Value, E> {
                Ok(UniqueValue(Value::Null))
            }
            fn visit_bool<E: serde::de::Error>(self, v: bool) -> Result<Self::Value, E> {
                Ok(UniqueValue(Value::Bool(v)))
            }
            fn visit_i64<E: serde::de::Error>(self, v: i64) -> Result<Self::Value, E> {
                Ok(UniqueValue(Value::Number(v.into())))
            }
            fn visit_u64<E: serde::de::Error>(self, v: u64) -> Result<Self::Value, E> {
                Ok(UniqueValue(Value::Number(v.into())))
            }
            fn visit_f64<E: serde::de::Error>(self, v: f64) -> Result<Self::Value, E> {
                serde_json::Number::from_f64(v)
                    .map(|n| UniqueValue(Value::Number(n)))
                    .ok_or_else(|| E::custom("invalid JSON number"))
            }
            fn visit_str<E: serde::de::Error>(self, v: &str) -> Result<Self::Value, E> {
                Ok(UniqueValue(Value::String(v.into())))
            }
            fn visit_seq<A: serde::de::SeqAccess<'de>>(
                self,
                mut seq: A,
            ) -> Result<Self::Value, A::Error> {
                let mut values = Vec::new();
                while let Some(value) = seq.next_element::<UniqueValue>()? {
                    values.push(value.0)
                }
                Ok(UniqueValue(Value::Array(values)))
            }
            fn visit_map<A: serde::de::MapAccess<'de>>(
                self,
                mut map: A,
            ) -> Result<Self::Value, A::Error> {
                let mut values = serde_json::Map::new();
                while let Some(key) = map.next_key::<String>()? {
                    if values.contains_key(&key) {
                        return Err(serde::de::Error::custom("duplicate JSON member"));
                    }
                    values.insert(key, map.next_value::<UniqueValue>()?.0);
                }
                Ok(UniqueValue(Value::Object(values)))
            }
        }
        deserializer.deserialize_any(Visitor)
    }
}
#[cfg(test)]
mod json_tests {
    use super::*;
    #[test]
    fn rejects_duplicate_oauth_json_recursively() {
        for json in [
            r#"{"issuer":"a","issuer":"b"}"#,
            r#"{"nested":{"access_token":"a","access_token":"b"}}"#,
            r#"[{"scope":"a","scope":"b"}]"#,
        ] {
            assert!(serde_json::from_str::<UniqueValue>(json).is_err())
        }
    }
}
