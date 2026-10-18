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

