//! User-triggered account tools; public output never includes broker capabilities.
use super::*;
use crate::accounts::{AccountBroker, ConnectionPhase, CONNECTION_STATE_PATH};
use alloyos_mcp::{account::AccountError, RemoteConnectionStatus};

#[derive(Clone)]
pub(super) struct PendingAccountCall {
    call: ToolCall,
    app: String,
    endpoint: String,
    stage: Stage,
    scopes: Vec<String>,
    recovering: bool,
}
#[derive(Clone, Copy)]
enum Stage {
    Recover,
    Probe,
    StartAuthorization,
    Broker,
    Verify,
    Disconnect,
    Refresh,
}
impl NativeToolRegistry {
    pub fn set_account_broker(
        &mut self,
        broker: alloc::boxed::Box<dyn AccountBroker>,
    ) -> Result<(), AccountError> {
        self.accounts.borrow_mut().set_broker(broker)
    }
    pub(super) fn restore_account_state(&mut self) {
        if let Some(raw) = self.vfs_files.get(CONNECTION_STATE_PATH) {
            let _ = self.accounts.borrow_mut().restore_public_json(raw);
        }
    }
    fn account_binding(&mut self, app: &str) -> Result<String, AiError> {
        let endpoint = self
            .mcp_registry
            .borrow()
            .manifests()
            .into_iter()
            .find(|manifest| manifest.app_id == app && manifest.is_remote())
            .and_then(|manifest| manifest.endpoint.clone())
            .ok_or_else(|| AiError::InvalidToolArguments("remote app is not installed".into()))?;
        self.accounts
            .borrow_mut()
            .register_installed(app, &endpoint)
            .map_err(|error| AiError::InvalidToolArguments(error.to_string()))?;
        Ok(endpoint)
    }
    fn account_output(
        &mut self,
        call: &ToolCall,
        app: &str,
        error: Option<AccountError>,
    ) -> Result<ToolExecutionOutput, AiError> {
        let record = self.accounts.borrow().status(app).cloned();
        let public = self
            .accounts
            .borrow()
            .public_json()
            .map_err(|error| AiError::ConfigError(error.to_string()))?;
        let persistence_error = self.write_vfs_raw(CONNECTION_STATE_PATH, &public).err();
        Ok(ToolExecutionOutput { tool_name: call.name.clone(), output_json: serde_json::json!({
            "ok":error.is_none(), "connection":record, "error":error.map(|error|error.to_string()),
            "status_saved":persistence_error.is_none(), "storage_error":persistence_error.map(|_|"Connection status could not be saved."),
        }).to_string(), synthesized_view: None })
    }
    pub(super) fn cancel_account_tool(&mut self, id: &str) {
        if let Some(pending) = self.pending_account_call.take() {
            if pending.call.id == id {
                let _ = self
                    .accounts
                    .borrow_mut()
                    .cancel_authorization(&pending.app);
                let _ = self
                    .mcp_registry
                    .borrow_mut()
                    .set_remote_credential_reference(&pending.app, None);
                let _ = self.mcp_registry.borrow_mut().set_remote_trusted_bearer(&pending.app, None);
                self.mcp_registry.borrow_mut().cancel_pending_tools();
            } else {
                self.pending_account_call = Some(pending);
            }
        }
    }
    /// Drive pending local cancellation/revocation after a model request ends.
    /// The kernel calls this cooperatively; no browser is opened by this poll.
    pub fn poll_account_connections(&mut self) {
        if self.pending_account_call.is_some() {
            return;
        }
        if !self.accounts.borrow().has_pending() {
            self.poll_approved_package_removal();
            return;
        }
        let result = self.accounts.borrow_mut().poll();
        if result.is_ready() {
            let public = self.accounts.borrow().public_json();
            if let Ok(public) = public {
                let _ = self.write_vfs_raw(CONNECTION_STATE_PATH, &public);
            }
        }
        self.poll_approved_package_removal();
    }
    pub(super) fn poll_account_tool(
        &mut self,
        call: &ToolCall,
    ) -> Poll<Result<ToolExecutionOutput, AiError>> {
        if call.name == "mcp_connection_status" {
            let args: serde_json::Value = match serde_json::from_str(&call.arguments_json) {
                Ok(args) => args,
                Err(_) => {
                    return Poll::Ready(Err(AiError::InvalidToolArguments(
                        "invalid connection arguments".into(),
                    )))
                }
            };
            let Some(app) = args["app"].as_str() else {
                return Poll::Ready(Err(AiError::InvalidToolArguments(
                    "missing remote app ID".into(),
                )));
            };
            if let Err(error) = self.account_binding(app) {
                return Poll::Ready(Err(error));
            }
            return Poll::Ready(self.account_output(call, app, None));
        }
        if self
            .pending_account_call
            .as_ref()
            .is_some_and(|pending| pending.call != *call)
        {
            return Poll::Ready(Err(AiError::PolicyViolation(
                "another account operation is pending".into(),
            )));
        }
        let mut pending = if let Some(pending) = self.pending_account_call.take() {
            pending
        } else {
            let args: serde_json::Value = match serde_json::from_str(&call.arguments_json) {
                Ok(args) => args,
                Err(_) => {
                    return Poll::Ready(Err(AiError::InvalidToolArguments(
                        "invalid connection arguments".into(),
                    )))
                }
            };
            let Some(app) = args["app"].as_str() else {
                return Poll::Ready(Err(AiError::InvalidToolArguments(
                    "missing remote app ID".into(),
                )));
            };
            let endpoint = match self.account_binding(app) {
                Ok(endpoint) => endpoint,
                Err(error) => return Poll::Ready(Err(error)),
            };
            if call.name == "mcp_connection_status" {
                return Poll::Ready(self.account_output(call, app, None));
            }
            if call.name == "mcp_connect" {
                if let Err(error) = self.accounts.borrow_mut().prepare_connection(app) {
                    return Poll::Ready(Err(AiError::InvalidToolArguments(error.to_string())));
                }
            }
            let scopes: Vec<String> = match args.get("scopes") {
                None => Vec::new(),
                Some(serde_json::Value::Array(scopes)) => match scopes
                    .iter()
                    .map(|scope| scope.as_str().map(ToString::to_string))
                    .collect::<Option<Vec<_>>>()
                {
                    Some(scopes) if alloyos_mcp::account::valid_scopes(&scopes) => scopes,
                    _ => {
                        return Poll::Ready(Err(AiError::InvalidToolArguments(
                            "invalid OAuth scopes".into(),
                        )))
                    }
                },
                _ => {
                    return Poll::Ready(Err(AiError::InvalidToolArguments(
                        "scopes must be an array".into(),
                    )))
                }
            };
            PendingAccountCall {
                call: call.clone(),
                app: app.into(),
                endpoint,
                scopes,
                recovering: call.name == "mcp_connect",
                stage: match call.name.as_str() {
                    "mcp_disconnect" => Stage::Disconnect,
                    "mcp_refresh_connection" => Stage::Refresh,
                    _ => Stage::Recover,
                },
            }
        };
        loop {
            let operation = match pending.stage {
                Stage::Recover => self.accounts.borrow_mut().recover_credentials(&pending.app),
                Stage::Probe | Stage::Verify => {
                    let now = self.sys_status.wall_clock_ms.unwrap_or(0) / 1000;
                    let reference = self.accounts.borrow_mut().credential_for_transport(
                        &pending.app,
                        &pending.endpoint,
                        now,
                    );
                    let bearer = match &reference {
                        Some(reference) => self.accounts.borrow_mut().bearer_for_transport(&pending.app, &pending.endpoint, reference),
                        None => Ok(None),
                    };
                    let bearer = match bearer {
                        Ok(bearer) => bearer,
                        Err(error) => return Poll::Ready(self.account_output(call, &pending.app, Some(error))),
                    };
                    if self.mcp_registry.borrow_mut().set_remote_trusted_bearer(&pending.app, bearer).is_err() {
                        return Poll::Ready(self.account_output(call, &pending.app, Some(AccountError::TransportFailed)));
                    }
                    let attach_failed = self
                        .mcp_registry
                        .borrow_mut()
                        .set_remote_credential_reference(
                            &pending.app,
                            reference.map(|reference| reference.for_transport().into()),
                        )
                        .is_err();
                    if attach_failed {
                        return Poll::Ready(self.account_output(
                            call,
                            &pending.app,
                            Some(AccountError::TransportFailed),
                        ));
                    }
                    let result = self
                        .mcp_registry
                        .borrow_mut()
                        .poll_connect_remote(&pending.app);
                    if result.is_pending() {
                        self.pending_account_call = Some(pending);
                        return Poll::Pending;
                    }
                    let status = self
                        .mcp_registry
                        .borrow()
                        .remote_connection_status(&pending.app);
                    match status {
                        Some(RemoteConnectionStatus::Ready) => {
                            let _ = self.accounts.borrow_mut().observe_transport(
                                &pending.app,
                                &pending.endpoint,
                                true,
                                false,
                            );
                            return Poll::Ready(self.account_output(call, &pending.app, None));
                        }
                        Some(RemoteConnectionStatus::AuthorizationRequired { challenge })
                            if matches!(pending.stage, Stage::Probe) =>
                        {
                            let noted = self.accounts.borrow_mut().note_challenge(&pending.endpoint, &challenge);
                            if let Err(error) = noted {
                                return Poll::Ready(self.account_output(call, &pending.app, Some(error)));
                            }
                            let _ = self.accounts.borrow_mut().observe_transport(
                                &pending.app,
                                &pending.endpoint,
                                false,
                                true,
                            );
                            pending.stage = Stage::StartAuthorization;
                            continue;
                        }
                        Some(RemoteConnectionStatus::AuthorizationRequired { .. }) => {
                            let _ = self.accounts.borrow_mut().observe_transport(
                                &pending.app,
                                &pending.endpoint,
                                false,
                                true,
                            );
                            return Poll::Ready(self.account_output(
                                call,
                                &pending.app,
                                Some(AccountError::CredentialUnavailable),
                            ));
                        }
                        _ => {
                            self.accounts.borrow_mut().transport_failed(&pending.app);
                            return Poll::Ready(self.account_output(
                                call,
                                &pending.app,
                                Some(AccountError::TransportFailed),
                            ));
                        }
                    }
                }
                Stage::StartAuthorization => self
                    .accounts
                    .borrow_mut()
                    .begin_authorization(&pending.app, pending.scopes.clone()),
                Stage::Broker => self.accounts.borrow_mut().poll(),
                Stage::Disconnect => {
                    let _ = self.mcp_registry.borrow_mut().set_remote_trusted_bearer(&pending.app, None);
                    let _ = self
                        .mcp_registry
                        .borrow_mut()
                        .set_remote_credential_reference(&pending.app, None);
                    self.accounts.borrow_mut().disconnect(&pending.app)
                }
                Stage::Refresh => self.accounts.borrow_mut().refresh(&pending.app),
            };
            match operation {
                Poll::Pending => {
                    pending.stage = Stage::Broker;
                    self.pending_account_call = Some(pending);
                    return Poll::Pending;
                }
                Poll::Ready(Err(error))
                    if pending.recovering
                        && matches!(
                            error,
                            AccountError::CredentialUnavailable
                                | AccountError::HostUnavailable
                                | AccountError::RefreshFailed
                        ) =>
                {
                    pending.recovering = false;
                    pending.stage = Stage::Probe;
                }
                Poll::Ready(Err(error)) => {
                    return Poll::Ready(self.account_output(call, &pending.app, Some(error)))
                }
                Poll::Ready(Ok(record)) if record.phase == ConnectionPhase::Authorizing => {
                    pending.stage = Stage::Broker;
                    self.pending_account_call = Some(pending);
                    return Poll::Pending;
                }
                Poll::Ready(Ok(record)) if record.phase == ConnectionPhase::Disconnected => {
                    return Poll::Ready(self.account_output(call, &pending.app, None))
                }
                Poll::Ready(Ok(record)) => {
                    let now = self.sys_status.wall_clock_ms.unwrap_or(0) / 1000;
                    if record.expires_at.is_some_and(|expiry| expiry <= now) {
                        pending.stage = Stage::Refresh;
                    } else {
                        pending.recovering = false;
                        pending.stage = Stage::Verify;
                    }
                }
            }
        }
    }
}
