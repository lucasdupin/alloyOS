//! Welder-owned public connection lifecycle. Credential capabilities stay private
//! in memory; the OS broker owns tokens and delegates only browser I/O.
use alloc::{boxed::Box, collections::BTreeMap, string::String, vec::Vec};
use alloyos_mcp::account::{
    valid_scopes, AccountError, AccountOperation, AccountOutcome, AccountRequest, AccountResponse,
    CredentialReference, SecretStorage, MAX_ACCOUNTS,
};
use core::task::Poll;
use serde::{Deserialize, Serialize};

pub const CONNECTION_STATE_PATH: &str = "/etc/mcp-connections.json";
const MAX_STATE_BYTES: usize = 262144;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConnectionPhase {
    Installed,
    NeedsAuth,
    Authorizing,
    Connecting,
    Ready,
    Error,
    Disconnected,
}

/// Safe for model/UI output and ordinary VFS persistence. No secret references,
/// OAuth authorization URLs, callback codes or bearer/refresh tokens.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConnectionRecord {
    pub app_id: String,
    pub endpoint: String,
    pub phase: ConnectionPhase,
    pub scopes: Vec<String>,
    pub credential_storage: Option<SecretStorage>,
    pub expires_at: Option<u64>,
    pub last_error: Option<AccountError>,
    #[serde(default)]
    pub remote_revocation_confirmed: Option<bool>,
}
struct Entry {
    public: ConnectionRecord,
    credential: Option<CredentialReference>,
    attempt: Option<String>,
}

/// Trusted transport adapters implement this; never expose raw broker requests
/// or outcomes as tools. Implementations must cancel in-flight host auth on cancel.
pub trait AccountBroker {
    fn start(&mut self, request: &AccountRequest) -> Poll<Result<AccountResponse, AccountError>>;
    fn poll(&mut self) -> Poll<Result<AccountResponse, AccountError>>;
    fn cancel(&mut self);
    fn note_challenge(&mut self, _: &str, _: &str) -> Result<(), AccountError> { Ok(()) }
    /// Trusted in-process resolution. Never serialize this result in tool output.
    fn bearer(&mut self, _: &str, _: &str, _: &CredentialReference) -> Result<Option<String>, AccountError> {
        Ok(None)
    }
}
struct UnavailableBroker;
impl AccountBroker for UnavailableBroker {
    fn start(&mut self, _: &AccountRequest) -> Poll<Result<AccountResponse, AccountError>> {
        Poll::Ready(Err(AccountError::HostUnavailable))
    }
    fn poll(&mut self) -> Poll<Result<AccountResponse, AccountError>> {
        Poll::Ready(Err(AccountError::HostUnavailable))
    }
    fn cancel(&mut self) {}
}

pub struct AccountService {
    entries: BTreeMap<String, Entry>,
    broker: Box<dyn AccountBroker>,
    pending: Option<AccountRequest>,
    next_request: u64,
}
impl core::fmt::Debug for AccountService {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("AccountService")
            .field("accounts", &self.entries.len())
            .field("pending", &self.pending.is_some())
            .finish()
    }
}
impl Default for AccountService {
    fn default() -> Self {
        Self::new()
    }
}
impl AccountService {
    pub fn note_challenge(&mut self, endpoint: &str, challenge: &str) -> Result<(), AccountError> {self.broker.note_challenge(endpoint,challenge)}
    pub fn bearer_for_transport(&mut self, app: &str, endpoint: &str, reference: &CredentialReference) -> Result<Option<String>, AccountError> {
        self.broker.bearer(app, endpoint, reference)
    }
    pub fn new() -> Self {
        Self {
            entries: BTreeMap::new(),
            broker: Box::new(UnavailableBroker),
            pending: None,
            next_request: 1,
        }
    }
    pub fn set_broker(&mut self, broker: Box<dyn AccountBroker>) -> Result<(), AccountError> {
        if self.pending.is_some() || self.entries.values().any(|entry| entry.attempt.is_some()) {
            return Err(AccountError::Busy);
        }
        self.broker = broker;
        Ok(())
    }
    pub fn inventory(&self) -> Vec<ConnectionRecord> {
        self.entries
            .values()
            .map(|entry| entry.public.clone())
            .collect()
    }
    pub fn status(&self, app_id: &str) -> Option<&ConnectionRecord> {
        self.entries.get(app_id).map(|entry| &entry.public)
    }

    /// Call only from the OS package service, after validated manifest discovery.
    pub fn register_installed(&mut self, app_id: &str, endpoint: &str) -> Result<(), AccountError> {
        if app_id.is_empty()
            || app_id.len() > 128
            || !app_id
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
        {
            return Err(AccountError::InvalidRequest);
        }
        if !crate::remote_discovery::hosted_endpoint(endpoint) {
            return Err(AccountError::InvalidRequest);
        }
        if self
            .entries
            .get(app_id)
            .is_some_and(|entry| entry.public.endpoint == endpoint)
        {
            return Ok(());
        }
        if !self.entries.contains_key(app_id) && self.entries.len() >= MAX_ACCOUNTS {
            return Err(AccountError::InvalidRequest);
        }
        if self
            .pending
            .as_ref()
            .is_some_and(|pending| pending.app_id == app_id)
        {
            self.broker.cancel();
            self.pending = None;
        }
        self.entries.insert(
            app_id.into(),
            Entry {
                public: ConnectionRecord {
                    app_id: app_id.into(),
                    endpoint: endpoint.into(),
                    phase: ConnectionPhase::Installed,
                    scopes: Vec::new(),
                    credential_storage: None,
                    expires_at: None,
                    last_error: None,
                    remote_revocation_confirmed: None,
                },
                credential: None,
                attempt: None,
            },
        );
        Ok(())
    }

    /// Explicit user authorization intent; never called by discovery/installation.
    pub fn begin_authorization(
        &mut self,
        app_id: &str,
        scopes: Vec<String>,
    ) -> Poll<Result<ConnectionRecord, AccountError>> {
        if !valid_scopes(&scopes) {
            return Poll::Ready(Err(AccountError::InvalidRequest));
        }
        if self
            .entries
            .get(app_id)
            .is_some_and(|entry| entry.attempt.is_some())
        {
            return Poll::Ready(Err(AccountError::Busy));
        }
        self.start(app_id, AccountOperation::Begin { scopes })
    }
    pub fn recover_credentials(
        &mut self,
        app_id: &str,
    ) -> Poll<Result<ConnectionRecord, AccountError>> {
        self.start(app_id, AccountOperation::Status)
    }
    pub fn prepare_connection(&mut self, app_id: &str) -> Result<(), AccountError> {
        let entry = self
            .entries
            .get_mut(app_id)
            .ok_or(AccountError::InvalidRequest)?;
        if entry.public.phase == ConnectionPhase::Disconnected {
            entry.public.phase = ConnectionPhase::Installed;
        }
        entry.public.remote_revocation_confirmed = None;
        Ok(())
    }
    pub fn cancel_authorization(
        &mut self,
        app_id: &str,
    ) -> Poll<Result<ConnectionRecord, AccountError>> {
        if self
            .pending
            .as_ref()
            .is_some_and(|request| request.app_id == app_id)
        {
            self.broker.cancel();
            self.pending = None;
        }
        self.disconnect(app_id)
    }
    pub fn refresh(&mut self, app_id: &str) -> Poll<Result<ConnectionRecord, AccountError>> {
        let Some(credential) = self
            .entries
            .get(app_id)
            .and_then(|entry| entry.credential.clone())
        else {
            return Poll::Ready(Err(AccountError::CredentialUnavailable));
        };
        self.start(app_id, AccountOperation::Refresh { credential })
    }
    pub fn disconnect(&mut self, app_id: &str) -> Poll<Result<ConnectionRecord, AccountError>> {
        if self
            .pending
            .as_ref()
            .is_some_and(|pending| pending.app_id == app_id)
        {
            self.broker.cancel();
            self.pending = None;
        }
        let Some(entry) = self.entries.get_mut(app_id) else {
            return Poll::Ready(Err(AccountError::InvalidRequest));
        };
        let credential = entry.credential.take();
        // Cut off further requests immediately, even if remote revocation fails.
        entry.attempt = None;
        entry.public.phase = ConnectionPhase::Disconnected;
        entry.public.remote_revocation_confirmed = None;
        entry.public.last_error = None;
        entry.public.expires_at = None;
        entry.public.credential_storage = None;
        self.start(app_id, AccountOperation::Disconnect { credential })
    }
    fn start(
        &mut self,
        app_id: &str,
        operation: AccountOperation,
    ) -> Poll<Result<ConnectionRecord, AccountError>> {
        if self.pending.is_some() {
            return Poll::Ready(Err(AccountError::Busy));
        }
        let Some(entry) = self.entries.get(app_id) else {
            return Poll::Ready(Err(AccountError::InvalidRequest));
        };
        let request = AccountRequest {
            request_id: self.next_request,
            app_id: app_id.into(),
            endpoint: entry.public.endpoint.clone(),
            operation,
        };
        if let Err(error) = request.validate() {
            return Poll::Ready(Err(error));
        }
        let Some(next_request) = self.next_request.checked_add(1) else {
            return Poll::Ready(Err(AccountError::InvalidRequest));
        };
        self.next_request = next_request;
        self.pending = Some(request.clone());
        let result = self.broker.start(&request);
        self.complete(result)
    }
    pub fn poll(&mut self) -> Poll<Result<ConnectionRecord, AccountError>> {
        if self.pending.is_some() {
            let result = self.broker.poll();
            return self.complete(result);
        }
        let Some((app, attempt)) = self.entries.iter().find_map(|(id, entry)| {
            entry
                .attempt
                .as_ref()
                .map(|attempt| (id.clone(), attempt.clone()))
        }) else {
            return Poll::Ready(Err(AccountError::InvalidRequest));
        };
        self.start(&app, AccountOperation::Poll { attempt })
    }
    pub fn has_pending(&self) -> bool {
        self.pending.is_some() || self.entries.values().any(|entry| entry.attempt.is_some())
    }
    fn complete(
        &mut self,
        result: Poll<Result<AccountResponse, AccountError>>,
    ) -> Poll<Result<ConnectionRecord, AccountError>> {
        let Poll::Ready(result) = result else {
            return Poll::Pending;
        };
        let Some(request) = self.pending.take() else {
            return Poll::Ready(Err(AccountError::InvalidRequest));
        };
        let result = result.and_then(|response| {
            response.validate_for(&request)?;
            Ok(response.outcome)
        });
        let entry = self.entries.get_mut(&request.app_id).unwrap();
        let result = match result {
            Ok(AccountOutcome::Authorizing { attempt } | AccountOutcome::Pending { attempt }) => {
                if !matches!(
                    request.operation,
                    AccountOperation::Begin { .. } | AccountOperation::Poll { .. }
                ) {
                    Err(AccountError::InvalidBrokerResponse)
                } else {
                    entry.attempt = Some(attempt);
                    entry.public.phase = ConnectionPhase::Authorizing;
                    entry.public.last_error = None;
                    Ok(())
                }
            }
            Ok(AccountOutcome::Authorized {
                credential,
                expires_at,
                scopes,
                storage,
            }) => {
                if matches!(request.operation, AccountOperation::Disconnect { .. }) {
                    Err(AccountError::InvalidBrokerResponse)
                } else {
                    entry.credential = Some(credential);
                    entry.attempt = None;
                    entry.public.phase = ConnectionPhase::Connecting;
                    entry.public.scopes = scopes;
                    entry.public.expires_at = expires_at;
                    entry.public.credential_storage = Some(storage);
                    entry.public.last_error = None;
                    Ok(())
                }
            }
            Ok(AccountOutcome::Disconnected { remote_revoked }) => {
                entry.credential = None;
                entry.attempt = None;
                entry.public.phase = ConnectionPhase::Disconnected;
                entry.public.credential_storage = None;
                entry.public.expires_at = None;
                entry.public.remote_revocation_confirmed = Some(remote_revoked);
                entry.public.last_error = if remote_revoked
                    || !matches!(request.operation, AccountOperation::Disconnect { .. })
                {
                    None
                } else {
                    Some(AccountError::RevocationFailed)
                };
                Ok(())
            }
            Ok(AccountOutcome::Error { error }) | Err(error) => Err(error),
        };
        if let Err(error) = result {
            entry.attempt = None;
            if !matches!(request.operation, AccountOperation::Disconnect { .. }) {
                entry.public.phase = if matches!(
                    error,
                    AccountError::CredentialUnavailable
                        | AccountError::AuthorizationDenied
                        | AccountError::AuthorizationExpired
                        | AccountError::RefreshFailed
                ) {
                    ConnectionPhase::NeedsAuth
                } else {
                    ConnectionPhase::Error
                };
            }
            entry.public.last_error = Some(error);
            return Poll::Ready(Err(error));
        }
        Poll::Ready(Ok(entry.public.clone()))
    }

    /// Only authenticated OS transport code may observe this capability. The
    /// host checks the reference against app + exact endpoint before injection.
    pub fn credential_for_transport(
        &mut self,
        app_id: &str,
        endpoint: &str,
        now: u64,
    ) -> Option<CredentialReference> {
        let entry = self.entries.get_mut(app_id)?;
        if entry.public.endpoint != endpoint
            || !matches!(
                entry.public.phase,
                ConnectionPhase::Connecting | ConnectionPhase::Ready
            )
        {
            return None;
        }
        if entry.public.expires_at.is_some_and(|expiry| expiry <= now) {
            entry.public.phase = ConnectionPhase::NeedsAuth;
            entry.public.last_error = Some(AccountError::CredentialUnavailable);
            return None;
        }
        entry.credential.clone()
    }
    /// `ready` must come from successful MCP initialize + tools/capability
    /// discovery, never from OAuth success or installed package metadata.
    pub fn observe_transport(
        &mut self,
        app_id: &str,
        endpoint: &str,
        ready: bool,
        needs_auth: bool,
    ) -> Result<(), AccountError> {
        let entry = self
            .entries
            .get_mut(app_id)
            .ok_or(AccountError::InvalidRequest)?;
        if entry.public.endpoint != endpoint
            || matches!(
                entry.public.phase,
                ConnectionPhase::Authorizing | ConnectionPhase::Disconnected
            )
        {
            return Err(AccountError::InvalidRequest);
        }
        entry.public.phase = if ready {
            ConnectionPhase::Ready
        } else if needs_auth {
            ConnectionPhase::NeedsAuth
        } else {
            ConnectionPhase::Connecting
        };
        entry.public.last_error = None;
        Ok(())
    }
    pub fn transport_failed(&mut self, app_id: &str) {
        if let Some(entry) = self.entries.get_mut(app_id) {
            entry.public.phase = ConnectionPhase::Error;
            entry.public.last_error = Some(AccountError::TransportFailed);
        }
    }
    pub fn public_json(&self) -> Result<String, AccountError> {
        let state = serde_json::to_string(&self.inventory())
            .map_err(|_| AccountError::StorageUnavailable)?;
        if state.len() > MAX_STATE_BYTES {
            return Err(AccountError::StorageUnavailable);
        }
        Ok(state)
    }
    pub fn restore_public_json(&mut self, state: &str) -> Result<(), AccountError> {
        if self.has_pending() || state.len() > MAX_STATE_BYTES {
            return Err(AccountError::InvalidRequest);
        }
        let records: Vec<ConnectionRecord> =
            serde_json::from_str(state).map_err(|_| AccountError::InvalidRequest)?;
        if records.len() > MAX_ACCOUNTS {
            return Err(AccountError::InvalidRequest);
        }
        // Validate before replacing live state. Reboot never restores readiness
        // or an in-progress authorization without checking the host again.
        let mut restored = AccountService::new();
        for record in records {
            if !valid_scopes(&record.scopes) || restored.entries.contains_key(&record.app_id) {
                return Err(AccountError::InvalidRequest);
            }
            restored.register_installed(&record.app_id, &record.endpoint)?;
            let entry = restored.entries.get_mut(&record.app_id).unwrap();
            entry.public.scopes = record.scopes;
            entry.public.phase = if record.phase == ConnectionPhase::Disconnected {
                ConnectionPhase::Disconnected
            } else {
                ConnectionPhase::Installed
            };
        }
        self.entries = restored.entries;
        Ok(())
    }
}
