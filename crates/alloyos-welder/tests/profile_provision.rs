use alloyos_forge::provider::ToolCall;
use alloyos_mcp::{account::*, transport::*};
use alloyos_welder::{
    accounts::AccountBroker, AgentUiSynthesizer, NativeToolRegistry, SharedMapVfsBackend,
};
use std::{cell::RefCell, rc::Rc, task::Poll};
const ENDPOINT: &str = "https://mail.example/mcp";
struct Broker(Rc<RefCell<Vec<(String, ClientProfile)>>>);
impl AccountBroker for Broker {
    fn start(&mut self, request: &AccountRequest) -> Poll<Result<AccountResponse, AccountError>> {
        Poll::Ready(Err(
            if matches!(request.operation, AccountOperation::Status) {
                AccountError::CredentialUnavailable
            } else {
                AccountError::ClientRegistrationRequired
            },
        ))
    }
    fn poll(&mut self) -> Poll<Result<AccountResponse, AccountError>> {
        panic!("immediate fixture")
    }
    fn cancel(&mut self) {}
    fn provision_profile(
        &mut self,
        app: &str,
        profile: &ClientProfile,
    ) -> Result<SecretStorage, AccountError> {
        self.0.borrow_mut().push((app.into(), profile.clone()));
        Ok(SecretStorage::OsPrivateStore)
    }
}
struct Unauthorized;
impl HttpTransport for Unauthorized {
    fn request(&mut self, _: &HttpRequest) -> Result<HttpResponse, String> {
        Ok(HttpResponse {
            status: 401,
            headers: vec![("WWW-Authenticate".into(), "Bearer".into())],
            body: String::new(),
        })
    }
}
fn registry() -> (
    NativeToolRegistry,
    Rc<RefCell<SharedMapVfsBackend>>,
    Rc<RefCell<Vec<(String, ClientProfile)>>>,
) {
    let backend = SharedMapVfsBackend::shared();
    backend.borrow_mut().files.insert("/apps/mail/mail.mcp".into(),format!("name=Mail\nversion=1\ncapabilities=ExposeMcpTools,ScopedNetwork\nendpoint={ENDPOINT}\ntransport=streamable-http\n"));
    backend
        .borrow_mut()
        .files
        .insert("/apps/mail/tools.json".into(), "[]".into());
    let mut registry = NativeToolRegistry::with_vfs_backend(backend.clone());
    let calls = Rc::new(RefCell::new(vec![]));
    registry
        .set_account_broker(Box::new(Broker(calls.clone())))
        .unwrap();
    registry.set_http_transport(Box::new(Unauthorized));
    (registry, backend, calls)
}
fn request_setup(registry: &mut NativeToolRegistry) -> PendingClientProfileSetup {
    let call = ToolCall::new("connect", "mcp_connect", r#"{"app":"mail"}"#);
    let mut ui = AgentUiSynthesizer::new();
    for _ in 0..30 {
        if let Poll::Ready(result) = registry.poll_execute_tool(&call, &mut ui) {
            let output = result.unwrap();
            assert!(!output.output_json.contains("private-client-secret"));
            let state: serde_json::Value = serde_json::from_str(&output.output_json).unwrap();
            assert_eq!(
                state["connection"]["last_error"],
                "client_registration_required"
            );
            return registry.pending_client_profile_setup().unwrap();
        }
    }
    panic!("fixture poll limit")
}
fn profile() -> ClientProfile {
    ClientProfile {
        endpoint: ENDPOINT.into(),
        issuer: "https://auth.example".into(),
        client_id: "registered-native-client".into(),
        allowed_scopes: vec!["mail.read".into()],
        loopback_port: 0,
        client_secret: Some("private-client-secret".into()),
        authorization_options: Default::default(),
    }
}
#[test]
fn only_native_review_saves_profile_and_never_connects_or_echoes_secret() {
    let (mut registry, backend, calls) = registry();
    let review = request_setup(&mut registry);
    assert!(calls.borrow().is_empty());
    assert_eq!(review.endpoint, ENDPOINT);
    assert!(!serde_json::to_string(&review)
        .unwrap()
        .contains("private-client-secret"));
    let result = registry
        .provision_client_profile(&review.id, &profile())
        .unwrap();
    assert_eq!(result.app_id, "mail");
    assert_eq!(calls.borrow().len(), 1);
    assert_eq!(
        calls.borrow()[0].1.client_secret.as_deref(),
        Some("private-client-secret")
    );
    assert!(registry.pending_client_profile_setup().is_none());
    assert!(!serde_json::to_string(&result)
        .unwrap()
        .contains("private-client-secret"));
    assert!(backend
        .borrow()
        .files
        .values()
        .all(|content| !content.contains("private-client-secret")));
    assert!(registry
        .provision_client_profile(&review.id, &profile())
        .is_err());
}
#[test]
fn cancel_stale_ids_and_wrong_endpoint_never_save() {
    let (mut registry, _, calls) = registry();
    let review = request_setup(&mut registry);
    assert!(registry
        .provision_client_profile("stale", &profile())
        .is_err());
    let mut wrong = profile();
    wrong.endpoint = "https://other.example/mcp".into();
    assert!(registry
        .provision_client_profile(&review.id, &wrong)
        .is_err());
    assert_eq!(
        registry.pending_client_profile_setup().unwrap().id,
        review.id
    );
    registry.dismiss_client_profile_setup(&review.id).unwrap();
    assert!(registry.pending_client_profile_setup().is_none());
    assert!(calls.borrow().is_empty());
}
#[test]
fn changed_package_or_volume_invalidates_review_before_any_profile_write() {
    let (mut registry, backend, calls) = registry();
    let review = request_setup(&mut registry);
    backend.borrow_mut().files.insert(
        "/apps/mail/tools.json".into(),
        "[{\"name\":\"changed\"}]".into(),
    );
    assert!(registry
        .provision_client_profile(&review.id, &profile())
        .is_err());
    assert!(calls.borrow().is_empty());
    registry.set_vfs_backend(SharedMapVfsBackend::shared());
    assert!(registry.pending_client_profile_setup().is_none());
}
#[test]
fn model_cannot_submit_or_approve_private_configuration() {
    let (mut registry, _, calls) = registry();
    request_setup(&mut registry);
    let call = ToolCall::new(
        "fake",
        "provision_client_profile",
        serde_json::to_string(&profile()).unwrap(),
    );
    assert!(registry
        .execute_tool(&call, &mut AgentUiSynthesizer::new())
        .is_err());
    assert!(calls.borrow().is_empty());
}
#[test]
fn profile_validation_rejects_local_issuer_controls_and_overlong_metadata() {
    for mutate in [0, 1, 2, 3, 4] {
        let mut value = profile();
        match mutate {
            0 => value.issuer = "https://localhost/oauth".into(),
            1 => value.issuer = "https://auth.example/?issuer=other".into(),
            2 => value.client_id = "client\nother".into(),
            3 => value.allowed_scopes = vec!["read write".into()],
            _ => value.client_secret = Some("x".repeat(4001)),
        };
        assert_eq!(value.validate(), Err(AccountError::InvalidRequest));
    }
    assert!(!format!("{:?}", profile()).contains("private-client-secret"));
}
