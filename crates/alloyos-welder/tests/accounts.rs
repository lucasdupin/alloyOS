use alloyos_mcp::account::*;
use alloyos_welder::accounts::{AccountBroker, AccountService, ConnectionPhase};
use std::{cell::RefCell, collections::VecDeque, rc::Rc, task::Poll};
const ENDPOINT: &str = "https://mail.example/mcp";
const REFERENCE: &str = "secretcapabilityreference1234567890";
struct Broker {
    outcomes: Rc<RefCell<VecDeque<AccountOutcome>>>,
    requests: Rc<RefCell<Vec<AccountRequest>>>,
    pending: Option<AccountRequest>,
    delayed: bool,
}
impl AccountBroker for Broker {
    fn start(&mut self, request: &AccountRequest) -> Poll<Result<AccountResponse, AccountError>> {
        self.requests.borrow_mut().push(request.clone());
        self.pending = Some(request.clone());
        if self.delayed {
            Poll::Pending
        } else {
            self.poll()
        }
    }
    fn poll(&mut self) -> Poll<Result<AccountResponse, AccountError>> {
        let request = self.pending.take().unwrap();
        let Some(outcome) = self.outcomes.borrow_mut().pop_front() else {
            return Poll::Ready(Err(AccountError::HostUnavailable));
        };
        Poll::Ready(Ok(AccountResponse {
            request_id: request.request_id,
            app_id: request.app_id,
            endpoint: request.endpoint,
            outcome,
        }))
    }
    fn cancel(&mut self) {
        self.pending = None;
    }
}
fn authorized() -> AccountOutcome {
    AccountOutcome::Authorized {
        credential: CredentialReference::new(REFERENCE.into()).unwrap(),
        expires_at: Some(2000),
        scopes: vec!["mail.read".into()],
        storage: SecretStorage::Volatile,
    }
}
fn service(
    outcomes: Vec<AccountOutcome>,
    delayed: bool,
) -> (AccountService, Rc<RefCell<Vec<AccountRequest>>>) {
    let requests = Rc::new(RefCell::new(Vec::new()));
    let mut service = AccountService::new();
    service
        .set_broker(Box::new(Broker {
            outcomes: Rc::new(RefCell::new(outcomes.into())),
            requests: requests.clone(),
            pending: None,
            delayed,
        }))
        .unwrap();
    service.register_installed("mail", ENDPOINT).unwrap();
    (service, requests)
}
fn ready<T>(result: Poll<Result<T, AccountError>>) -> T {
    match result {
        Poll::Ready(Ok(result)) => result,
        _ => panic!("operation did not succeed"),
    }
}
#[test]
fn oauth_authorized_is_not_ready_until_verified_mcp_and_refs_never_persist() {
    let (mut service, requests) = service(
        vec![
            AccountOutcome::Authorizing {
                attempt: "attempt123456789012345".into(),
            },
            authorized(),
        ],
        false,
    );
    assert_eq!(
        service.status("mail").unwrap().phase,
        ConnectionPhase::Installed
    );
    assert_eq!(
        ready(service.begin_authorization("mail", vec!["mail.read".into()])).phase,
        ConnectionPhase::Authorizing
    );
    assert!(service
        .credential_for_transport("mail", ENDPOINT, 1000)
        .is_none());
    assert_eq!(ready(service.poll()).phase, ConnectionPhase::Connecting);
    assert_eq!(requests.borrow().len(), 2);
    assert!(service
        .credential_for_transport("other", ENDPOINT, 1000)
        .is_none());
    assert!(service
        .credential_for_transport("mail", "https://other.example/mcp", 1000)
        .is_none());
    assert_eq!(
        service
            .credential_for_transport("mail", ENDPOINT, 1000)
            .unwrap()
            .for_transport(),
        REFERENCE
    );
    service
        .observe_transport("mail", ENDPOINT, true, false)
        .unwrap();
    assert_eq!(
        service.status("mail").unwrap().phase,
        ConnectionPhase::Ready
    );
    let state = service.public_json().unwrap();
    assert!(
        !state.contains(REFERENCE)
            && !state.contains("credential_reference")
            && !state.contains("access_token")
    );
    assert!(!format!("{service:?}").contains(REFERENCE));
    assert!(
        !format!("{:?}", CredentialReference::new(REFERENCE.into()).unwrap()).contains(REFERENCE)
    );
    let mut rebooted = AccountService::new();
    rebooted.restore_public_json(&state).unwrap();
    assert_eq!(
        rebooted.status("mail").unwrap().phase,
        ConnectionPhase::Installed
    );
    assert!(rebooted
        .credential_for_transport("mail", ENDPOINT, 1000)
        .is_none());
}
#[test]
fn expiry_requires_refresh_and_disconnect_cuts_access_before_revocation_finishes() {
    let (mut service, _) = service(
        vec![
            authorized(),
            authorized(),
            AccountOutcome::Disconnected {
                remote_revoked: false,
            },
        ],
        true,
    );
    assert!(service.recover_credentials("mail").is_pending());
    ready(service.poll());
    assert!(service
        .credential_for_transport("mail", ENDPOINT, 2000)
        .is_none());
    assert_eq!(
        service.status("mail").unwrap().phase,
        ConnectionPhase::NeedsAuth
    );
    assert!(service.refresh("mail").is_pending());
    ready(service.poll());
    assert!(service
        .credential_for_transport("mail", ENDPOINT, 1000)
        .is_some());
    assert!(service.disconnect("mail").is_pending());
    assert!(service
        .credential_for_transport("mail", ENDPOINT, 1000)
        .is_none());
    assert_eq!(
        service.status("mail").unwrap().phase,
        ConnectionPhase::Disconnected
    );
    let disconnected = ready(service.poll());
    assert_eq!(disconnected.remote_revocation_confirmed, Some(false));
    assert_eq!(
        disconnected.last_error,
        Some(AccountError::RevocationFailed)
    );
}
#[test]
fn provider_confirmed_revocation_is_reported_separately_from_local_removal() {
    let (mut service, _) = service(
        vec![
            authorized(),
            AccountOutcome::Disconnected {
                remote_revoked: true,
            },
        ],
        false,
    );
    ready(service.recover_credentials("mail"));
    let status = ready(service.disconnect("mail"));
    assert_eq!(status.phase, ConnectionPhase::Disconnected);
    assert_eq!(status.remote_revocation_confirmed, Some(true));
    assert_eq!(status.last_error, None);
}
#[test]
fn denied_or_unavailable_authorization_never_marks_ready() {
    let (mut service, _) = service(
        vec![AccountOutcome::Error {
            error: AccountError::AuthorizationDenied,
        }],
        false,
    );
    assert!(matches!(
        service.begin_authorization("mail", vec![]),
        Poll::Ready(Err(AccountError::AuthorizationDenied))
    ));
    assert_eq!(
        service.status("mail").unwrap().phase,
        ConnectionPhase::NeedsAuth
    );
    let mut unavailable = AccountService::new();
    unavailable.register_installed("mail", ENDPOINT).unwrap();
    assert!(matches!(
        unavailable.begin_authorization("mail", vec![]),
        Poll::Ready(Err(AccountError::HostUnavailable))
    ));
    assert_eq!(
        unavailable.status("mail").unwrap().phase,
        ConnectionPhase::Error
    );
}
#[test]
fn endpoint_update_drops_old_capability_and_duplicate_or_oversized_state_is_rejected() {
    let (mut service, _) = service(vec![authorized()], false);
    ready(service.recover_credentials("mail"));
    service
        .register_installed("mail", "https://new.example/mcp")
        .unwrap();
    assert!(service
        .credential_for_transport("mail", ENDPOINT, 1000)
        .is_none());
    assert!(service
        .credential_for_transport("mail", "https://new.example/mcp", 1000)
        .is_none());
    let record = service.inventory()[0].clone();
    assert!(service
        .restore_public_json(&serde_json::to_string(&vec![record.clone(), record]).unwrap())
        .is_err());
    assert!(service.restore_public_json(&"x".repeat(262145)).is_err());
    assert_eq!(
        service.status("mail").unwrap().endpoint,
        "https://new.example/mcp"
    );
}
#[test]
fn broker_protocol_rejects_cross_app_response_and_invalid_references() {
    let request = AccountRequest {
        request_id: 1,
        app_id: "mail".into(),
        endpoint: ENDPOINT.into(),
        operation: AccountOperation::Status,
    };
    let response = AccountResponse {
        request_id: 1,
        app_id: "other".into(),
        endpoint: ENDPOINT.into(),
        outcome: authorized(),
    };
    assert_eq!(
        response.validate_for(&request),
        Err(AccountError::InvalidBrokerResponse)
    );
    for value in ["", "short", "raw token with spaces raw token with spaces"] {
        assert!(CredentialReference::new(value.into()).is_err());
    }
}

#[derive(Default)]
struct Wire {
    requests: Vec<alloyos_mcp::transport::HttpRequest>,
    fail_discovery: bool,
}
struct McpTransport(Rc<RefCell<Wire>>);
impl alloyos_mcp::transport::HttpTransport for McpTransport {
    fn request(
        &mut self,
        request: &alloyos_mcp::transport::HttpRequest,
    ) -> Result<alloyos_mcp::transport::HttpResponse, String> {
        use alloyos_mcp::transport::HttpResponse;
        let mut wire = self.0.borrow_mut();
        wire.requests.push(request.clone());
        assert!(!request
            .headers
            .iter()
            .any(|(name, _)| name.eq_ignore_ascii_case("Authorization")));
        if !request
            .headers
            .iter()
            .any(|(name, value)| name == "X-AlloyOS-Credential-Ref" && value == REFERENCE)
        {
            return Ok(HttpResponse { status:401, headers:vec![("WWW-Authenticate".into(), "Bearer resource_metadata=\"https://mail.example/.well-known/oauth-protected-resource\"".into())], body:String::new() });
        }
        let rpc: serde_json::Value = serde_json::from_str(&request.body).unwrap();
        let method = rpc["method"].as_str().unwrap();
        if method == "notifications/initialized" {
            return Ok(HttpResponse {
                status: 202,
                headers: vec![],
                body: String::new(),
            });
        }
        let result = match method {
            "initialize" => {
                serde_json::json!({"protocolVersion":"2025-06-18","capabilities":{"tools":{}},"serverInfo":{"name":"mail","version":"1"}})
            }
            "tools/list" => {
                if wire.fail_discovery {
                    return Ok(HttpResponse {
                        status: 503,
                        headers: vec![],
                        body: String::new(),
                    });
                }
                serde_json::json!({"tools":[{"name":"read_mail","inputSchema":{"type":"object","properties":{}}}]})
            }
            _ => panic!("unexpected method {method}"),
        };
        Ok(HttpResponse {
            status: 200,
            headers: vec![],
            body: serde_json::json!({"jsonrpc":"2.0","id":rpc["id"],"result":result}).to_string(),
        })
    }
}
fn tool_registry(
    fail_discovery: bool,
) -> (
    alloyos_welder::NativeToolRegistry,
    Rc<RefCell<alloyos_welder::SharedMapVfsBackend>>,
    Rc<RefCell<Wire>>,
) {
    let backend = alloyos_welder::SharedMapVfsBackend::shared();
    backend.borrow_mut().files.insert("/apps/mail/mail.mcp".into(), "name=Mail\nversion=1\ncapabilities=ExposeMcpTools,ScopedNetwork\nendpoint=https://mail.example/mcp\ntransport=streamable-http\n".into());
    backend
        .borrow_mut()
        .files
        .insert("/apps/mail/tools.json".into(), "[]".into());
    let mut registry = alloyos_welder::NativeToolRegistry::with_vfs_backend(backend.clone());
    let wire = Rc::new(RefCell::new(Wire {
        fail_discovery,
        ..Wire::default()
    }));
    registry.set_http_transport(Box::new(McpTransport(wire.clone())));
    registry
        .set_account_broker(Box::new(Broker {
            outcomes: Rc::new(RefCell::new(
                vec![
                    AccountOutcome::Error {
                        error: AccountError::CredentialUnavailable,
                    },
                    AccountOutcome::Authorizing {
                        attempt: "attempt123456789012345".into(),
                    },
                    authorized(),
                    AccountOutcome::Disconnected {
                        remote_revoked: false,
                    },
                ]
                .into(),
            )),
            requests: Rc::new(RefCell::new(Vec::new())),
            pending: None,
            delayed: true,
        }))
        .unwrap();
    (registry, backend, wire)
}
fn drive_tool(registry: &mut alloyos_welder::NativeToolRegistry, name: &str) -> serde_json::Value {
    let call = alloyos_forge::provider::ToolCall::new(name, name, "{\"app\":\"mail\"}");
    let mut ui = alloyos_welder::AgentUiSynthesizer::new();
    for _ in 0..100 {
        match registry.poll_execute_tool(&call, &mut ui) {
            Poll::Pending => {}
            Poll::Ready(Ok(output)) => {
                assert!(!output.output_json.contains(REFERENCE));
                return serde_json::from_str(&output.output_json).unwrap();
            }
            Poll::Ready(Err(error)) => panic!("{error}"),
        }
    }
    panic!("connection did not complete within bounded fixture polls")
}
#[test]
fn user_connection_tools_dispatch_broker_then_verify_mcp_and_never_expose_capabilities() {
    let (mut registry, backend, wire) = tool_registry(false);
    let initial = drive_tool(&mut registry, "mcp_connection_status");
    assert_eq!(initial["connection"]["phase"], "installed");
    let connected = drive_tool(&mut registry, "mcp_connect");
    assert_eq!(connected["ok"], true);
    assert_eq!(connected["connection"]["phase"], "ready");
    assert!(wire
        .borrow()
        .requests
        .iter()
        .any(|request| request.body.contains("tools/list")));
    assert!(backend
        .borrow()
        .files
        .values()
        .all(|content| !content.contains(REFERENCE)));
    let disconnected = drive_tool(&mut registry, "mcp_disconnect");
    assert_eq!(disconnected["connection"]["phase"], "disconnected");
    assert_eq!(
        disconnected["connection"]["remote_revocation_confirmed"],
        false
    );
    assert_eq!(
        disconnected["connection"]["last_error"],
        "revocation_failed"
    );
    let restored = alloyos_welder::NativeToolRegistry::with_vfs_backend(backend);
    assert!(!format!("{restored:?}").contains(REFERENCE));
}
#[test]
fn successful_oauth_with_failed_mcp_discovery_never_reports_connection_ready() {
    let (mut registry, _, _) = tool_registry(true);
    let connected = drive_tool(&mut registry, "mcp_connect");
    assert_eq!(connected["ok"], false);
    assert_eq!(connected["connection"]["phase"], "error");
    assert_eq!(connected["connection"]["last_error"], "transport_failed");
}
