use alloyos_mcp::{account::*, browser::*, transport::*};
use alloyos_welder::{accounts::AccountBroker, oauth::*};
use std::{cell::RefCell, rc::Rc, task::Poll};
const RESOURCE: &str = "https://mail.example/mcp";
const ISSUER: &str = "https://auth.example";
#[derive(Default)]
struct State {
    record: Option<PrivateAccount>,
    requests: Vec<(String, String)>,
    browser: Vec<BrowserRequest>,
    forget: bool,
    deny_entropy: bool,
    bad_state: bool,
    clock: Option<Option<u64>>,
    token_json: Option<String>,
    revoke_fails: bool,
    configured_profile: Option<ClientProfile>,
    profile_write_fails: bool,
    profile_delete_fails: bool,
    profile_removed: bool,
    prm_json: Option<String>,
    metadata_404: Vec<String>,
    token_resource: Option<String>,
}
struct Store(Rc<RefCell<State>>);
impl CredentialStore for Store {
    fn load(&mut self, a: &str, e: &str) -> Result<Option<PrivateAccount>, AccountError> {
        Ok(self
            .0
            .borrow()
            .record
            .clone()
            .filter(|r| r.app_id == a && r.endpoint == e))
    }
    fn save(&mut self, r: &PrivateAccount) -> Result<SecretStorage, AccountError> {
        self.0.borrow_mut().record = Some(r.clone());
        Ok(SecretStorage::OsPrivateStore)
    }
    fn forget(&mut self, _: &str, _: &str) -> Result<(), AccountError> {
        let mut s = self.0.borrow_mut();
        s.record = None;
        s.forget = true;
        Ok(())
    }
    fn profile(&mut self, _: &str, e: &str) -> Result<Option<ClientProfile>, AccountError> {
        if self.0.borrow().profile_removed {
            return Ok(None);
        }
        if let Some(profile) = &self.0.borrow().configured_profile {
            return Ok(Some(profile.clone()));
        }
        Ok(Some(ClientProfile {
            endpoint: e.into(),
            issuer: ISSUER.into(),
            client_id: "public-native-client".into(),
            allowed_scopes: vec!["mail.read".into()],
            client_secret: None,
            loopback_port: 0,
        }))
    }
    fn save_profile(
        &mut self,
        _: &str,
        profile: &ClientProfile,
    ) -> Result<SecretStorage, AccountError> {
        if self.0.borrow().profile_write_fails {
            return Err(AccountError::StorageUnavailable);
        }
        self.0.borrow_mut().configured_profile = Some(profile.clone());
        self.0.borrow_mut().profile_removed = false;
        Ok(SecretStorage::OsPrivateStore)
    }
    fn forget_profile(&mut self, _: &str, _: &str) -> Result<(), AccountError> {
        let mut state = self.0.borrow_mut();
        assert!(
            state.record.is_none(),
            "forget token authority before private profile"
        );
        if state.profile_delete_fails {
            return Err(AccountError::StorageUnavailable);
        }
        state.configured_profile = None;
        state.profile_removed = true;
        Ok(())
    }
}
struct Crypto(Rc<RefCell<State>>, u8);
impl OAuthCrypto for Crypto {
    fn random(&mut self, out: &mut [u8]) -> Result<(), AccountError> {
        if self.0.borrow().deny_entropy {
            return Err(AccountError::CredentialUnavailable);
        }
        self.1 += 1;
        out.fill(self.1);
        Ok(())
    }
    fn sha256(&self, _: &[u8]) -> [u8; 32] {
        [7; 32]
    }
    fn unix_seconds(&self) -> Option<u64> {
        self.0.borrow().clock.unwrap_or(Some(1000))
    }
}
struct Browser(Rc<RefCell<State>>);
fn decode_url(value: &str) -> String {
    let mut out = Vec::new();
    let bytes = value.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            out.push(u8::from_str_radix(&value[i + 1..i + 3], 16).unwrap());
            i += 3
        } else {
            out.push(bytes[i]);
            i += 1
        }
    }
    String::from_utf8(out).unwrap()
}
impl BrowserAssistant for Browser {
    fn start(&mut self, r: &BrowserRequest) -> Poll<Result<BrowserResponse, AccountError>> {
        let outcome = match &r.operation {
            BrowserOperation::Prepare { .. } => BrowserOutcome::Prepared {
                redirect_uri: "http://127.0.0.1:3000/callback".into(),
            },
            BrowserOperation::Open {
                authorization_url, ..
            } => {
                assert!(authorization_url.contains("code_challenge_method=S256"));
                assert!(!authorization_url.contains("access_token"));
                BrowserOutcome::Opened
            }
            BrowserOperation::Poll => {
                let s = self.0.borrow();
                let open = s
                    .browser
                    .iter()
                    .find_map(|r| {
                        if let BrowserOperation::Open {
                            authorization_url, ..
                        } = &r.operation
                        {
                            Some(authorization_url)
                        } else {
                            None
                        }
                    })
                    .unwrap();
                let state = open
                    .split('&')
                    .find_map(|p| p.strip_prefix("state="))
                    .unwrap();
                BrowserOutcome::Callback {
                    query: format!(
                        "code=test-code&state={}&iss={}",
                        if s.bad_state { "wrong" } else { state },
                        form_encode(ISSUER)
                    ),
                }
            }
            _ => BrowserOutcome::Cancelled,
        };
        self.0.borrow_mut().browser.push(r.clone());
        Poll::Ready(Ok(BrowserResponse {
            request_id: r.request_id,
            app_id: r.app_id.clone(),
            endpoint: r.endpoint.clone(),
            attempt: r.attempt.clone(),
            outcome,
        }))
    }
    fn poll(&mut self) -> Poll<Result<BrowserResponse, AccountError>> {
        panic!("fixture immediate")
    }
    fn cancel(&mut self) {}
}
struct Network(Rc<RefCell<State>>);
impl OAuthNetwork for Network {
    fn start(&mut self, r: &HttpRequest) -> Poll<Result<HttpResponse, AccountError>> {
        self.0
            .borrow_mut()
            .requests
            .push((r.url.clone(), r.body.clone()));
        if r.url.ends_with("/revoke") && self.0.borrow().revoke_fails {
            return Poll::Ready(Err(AccountError::TransportFailed));
        }
        if self.0.borrow().metadata_404.contains(&r.url) {
            return Poll::Ready(Ok(HttpResponse {
                status: 404,
                headers: vec![],
                body: String::new(),
            }));
        }
        let body = if r.url.contains("oauth-protected-resource")
            || r.url.ends_with("/custom-metadata")
        {
            self.0.borrow().prm_json.clone().unwrap_or_else(|| {
                format!(r#"{{"resource":"{RESOURCE}","authorization_servers":["{ISSUER}"]}}"#)
            })
        } else if r.url.contains("oauth-authorization-server") {
            format!(
                r#"{{"issuer":"{ISSUER}","authorization_endpoint":"{ISSUER}/authorize","token_endpoint":"{ISSUER}/token","revocation_endpoint":"{ISSUER}/revoke","code_challenge_methods_supported":["S256"],"authorization_response_iss_parameter_supported":true}}"#
            )
        } else if r.url.ends_with("/revoke") {
            assert!(
                self.0.borrow().record.is_none(),
                "local authority forgotten before revocation"
            );
            "{}".into()
        } else {
            let resource = self
                .0
                .borrow()
                .token_resource
                .clone()
                .unwrap_or_else(|| RESOURCE.into());
            assert!(r
                .body
                .contains(&format!("resource={}", form_encode(&resource))));
            if r.body.contains("authorization_code") {
                assert!(r.body.contains("code_verifier="));
                assert!(r.body.contains("code=test-code"));
            }
            self.0.borrow().token_json.clone().unwrap_or_else(|| format!(
                r#"{{"token_type":"Bearer","access_token":"{}","refresh_token":"private-refresh","expires_in":100,"scope":"mail.read"}}"#,
                if r.body.contains("grant_type=refresh_token") {
                    "refreshed-token"
                } else {
                    "private-access"
                }
            ))
        };
        Poll::Ready(Ok(HttpResponse {
            status: 200,
            headers: vec![],
            body,
        }))
    }
    fn poll(&mut self) -> Poll<Result<HttpResponse, AccountError>> {
        panic!("fixture immediate")
    }
    fn cancel(&mut self) {}
}
fn broker(s: &Rc<RefCell<State>>) -> OsAccountBroker {
    OsAccountBroker::new(
        Box::new(Store(s.clone())),
        Box::new(Browser(s.clone())),
        Box::new(Network(s.clone())),
        Box::new(Crypto(s.clone(), 0)),
    )
}
fn req(op: AccountOperation) -> AccountRequest {
    AccountRequest {
        request_id: 1,
        app_id: "mail".into(),
        endpoint: RESOURCE.into(),
        operation: op,
    }
}
fn ready(
    value: Poll<Result<AccountResponse, AccountError>>,
) -> Result<AccountResponse, AccountError> {
    match value {
        Poll::Ready(result) => result,
        Poll::Pending => panic!("unexpected pending"),
    }
}
fn authorize(b: &mut OsAccountBroker) -> CredentialReference {
    let response = ready(b.start(&req(AccountOperation::Begin {
        scopes: vec!["mail.read".into()],
    })))
    .unwrap();
    let AccountOutcome::Authorizing { attempt } = response.outcome else {
        panic!()
    };
    let response = ready(b.start(&req(AccountOperation::Poll { attempt }))).unwrap();
    let AccountOutcome::Authorized {
        credential,
        storage: SecretStorage::OsPrivateStore,
        ..
    } = response.outcome
    else {
        panic!()
    };
    credential
}
#[test]
fn os_owns_exchange_recovery_refresh_and_local_revocation() {
    let s = Rc::new(RefCell::new(State::default()));
    let mut b = broker(&s);
    let reference = authorize(&mut b);
    assert_eq!(
        b.bearer("mail", RESOURCE, &reference).unwrap(),
        "private-access"
    );
    assert!(b.bearer("other", RESOURCE, &reference).is_err());
    assert!(b
        .bearer("mail", "https://mail.example/other", &reference)
        .is_err());
    let mut reboot = broker(&s);
    let r = ready(reboot.start(&req(AccountOperation::Status))).unwrap();
    assert!(matches!(r.outcome, AccountOutcome::Authorized { .. }));
    ready(reboot.start(&req(AccountOperation::Refresh {
        credential: reference.clone(),
    })))
    .unwrap();
    assert_eq!(
        reboot.bearer("mail", RESOURCE, &reference).unwrap(),
        "refreshed-token"
    );
    let r = ready(reboot.start(&req(AccountOperation::Disconnect {
        credential: Some(reference.clone()),
    })))
    .unwrap();
    assert!(matches!(
        r.outcome,
        AccountOutcome::Disconnected {
            remote_revoked: true
        }
    ));
    assert!(broker(&s).bearer("mail", RESOURCE, &reference).is_err());
    assert!(s.borrow().forget);
}
#[test]
fn no_entropy_no_browser_or_network() {
    let s = Rc::new(RefCell::new(State {
        deny_entropy: true,
        ..State::default()
    }));
    let mut b = broker(&s);
    assert!(ready(b.start(&req(AccountOperation::Begin { scopes: vec![] }))).is_err());
    assert!(s.borrow().browser.is_empty());
    assert!(s.borrow().requests.is_empty())
}
#[test]
fn callback_mismatch_never_exchanges_or_saves() {
    let s = Rc::new(RefCell::new(State {
        bad_state: true,
        ..State::default()
    }));
    let mut b = broker(&s);
    let r = ready(b.start(&req(AccountOperation::Begin {
        scopes: vec!["mail.read".into()],
    })))
    .unwrap();
    let AccountOutcome::Authorizing { attempt } = r.outcome else {
        panic!()
    };
    assert_eq!(
        ready(b.start(&req(AccountOperation::Poll { attempt }))).unwrap_err(),
        AccountError::CallbackMismatch
    );
    assert!(s.borrow().record.is_none());
    assert!(!s
        .borrow()
        .requests
        .iter()
        .any(|(url, _)| url.ends_with("/token")))
}
#[test]
fn callback_rejects_duplicate_encoded_fields_and_wrong_issuer() {
    assert_eq!(
        callback_code("state=s&iss=i&code=c&%63ode=d", "s", "i"),
        Err(AccountError::CallbackMismatch)
    );
    assert_eq!(
        callback_code("state=s&iss=evil&code=c", "s", "i"),
        Err(AccountError::CallbackMismatch)
    );
    assert_eq!(
        callback_code("state=s&iss=i&error=denied", "s", "i"),
        Err(AccountError::AuthorizationDenied)
    );
    assert_eq!(decode_url(&form_encode("hello /+")), "hello /+")
}

struct McpWire {
    fail: bool,
    seen: Rc<RefCell<Vec<String>>>,
}
impl HttpTransport for McpWire {
    fn request(&mut self, r: &HttpRequest) -> Result<HttpResponse, String> {
        if !r.headers.iter().any(|(name, value)| {
            name == "X-AlloyOS-Trusted-Bearer"
                && matches!(value.as_str(), "private-access" | "refreshed-token")
        }) {
            return Ok(HttpResponse {
                status: 401,
                headers: vec![("WWW-Authenticate".into(), "Bearer".into())],
                body: String::new(),
            });
        }
        let rpc: serde_json::Value = serde_json::from_str(&r.body).unwrap();
        self.seen
            .borrow_mut()
            .push(rpc["method"].as_str().unwrap().into());
        if self.fail {
            return Ok(HttpResponse {
                status: 503,
                headers: vec![],
                body: String::new(),
            });
        }
        Ok(HttpResponse{status:200,headers:vec![],body:serde_json::json!({"jsonrpc":"2.0","id":rpc["id"],"result":{"tools":[{"name":"list_mail","inputSchema":{"type":"object","properties":{}}}]}}).to_string()})
    }
}
fn drive(registry: &mut alloyos_welder::NativeToolRegistry, tool: &str) -> serde_json::Value {
    drive_arguments(
        registry,
        tool,
        if tool == "mcp_connect" {
            "{\"app\":\"mail\",\"scopes\":[\"mail.read\"]}"
        } else {
            "{\"app\":\"mail\"}"
        },
    )
}
fn drive_arguments(
    registry: &mut alloyos_welder::NativeToolRegistry,
    tool: &str,
    arguments: &str,
) -> serde_json::Value {
    let call = alloyos_forge::provider::ToolCall::new(tool, tool, arguments);
    let mut ui = alloyos_welder::AgentUiSynthesizer::new();
    for _ in 0..100 {
        match registry.poll_execute_tool(&call, &mut ui) {
            Poll::Pending => {}
            Poll::Ready(Ok(output)) => {
                assert!(!output.output_json.contains("private-access"));
                assert!(!output.output_json.contains("private-refresh"));
                assert!(!output.output_json.contains("test-code"));
                assert!(!output.output_json.contains("revoked-token"));
                assert!(!output.output_json.contains("still-rejected"));
                return serde_json::from_str(&output.output_json).unwrap();
            }
            Poll::Ready(Err(e)) => panic!("{e}"),
        }
    }
    panic!("fixture exceeded poll budget")
}
fn native_registry(
    s: &Rc<RefCell<State>>,
    fail: bool,
) -> (
    alloyos_welder::NativeToolRegistry,
    Rc<RefCell<alloyos_welder::SharedMapVfsBackend>>,
    Rc<RefCell<Vec<String>>>,
) {
    let backend = alloyos_welder::SharedMapVfsBackend::shared();
    backend.borrow_mut().files.insert("/apps/mail/mail.mcp".into(),format!("name=Mail\nversion=1\ncapabilities=ExposeMcpTools,ScopedNetwork\nendpoint={RESOURCE}\ntransport=streamable-http\n"));
    backend
        .borrow_mut()
        .files
        .insert("/apps/mail/tools.json".into(), "[]".into());
    let mut registry = alloyos_welder::NativeToolRegistry::with_vfs_backend(backend.clone());
    let seen = Rc::new(RefCell::new(vec![]));
    registry.set_http_transport(Box::new(McpWire {
        fail,
        seen: seen.clone(),
    }));
    registry.set_account_broker(Box::new(broker(s))).unwrap();
    (registry, backend, seen)
}
#[test]
fn complete_native_connect_requires_mcp_verification_and_reconnects_os_store() {
    let s = Rc::new(RefCell::new(State::default()));
    let (mut registry, backend, seen) = native_registry(&s, false);
    let result = drive(&mut registry, "mcp_connect");
    assert_eq!(result["connection"]["phase"], "ready", "{result}");
    assert!(seen.borrow().iter().any(|method| method == "tools/list"));
    assert!(backend
        .borrow()
        .files
        .values()
        .all(|v| !v.contains("private-access") && !v.contains("private-refresh")));
    let (mut reboot, _, _) = native_registry(&s, false);
    assert_eq!(
        drive(&mut reboot, "mcp_connect")["connection"]["phase"],
        "ready"
    );
    assert_eq!(
        drive(&mut reboot, "mcp_refresh_connection")["connection"]["phase"],
        "ready"
    );
    assert_eq!(
        drive(&mut reboot, "mcp_disconnect")["connection"]["phase"],
        "disconnected"
    );
    assert!(s.borrow().record.is_none());
}
#[test]
fn oauth_authorized_does_not_make_failed_mcp_ready() {
    let s = Rc::new(RefCell::new(State::default()));
    let (mut registry, _, _) = native_registry(&s, true);
    let result = drive(&mut registry, "mcp_connect");
    assert_ne!(result["connection"]["phase"], "ready");
    assert_eq!(result["ok"], false)
}
#[test]
fn private_store_is_absent_from_model_cache_and_ancestor_listing() {
    use alloyos_forge::provider::ToolCall;
    use alloyos_welder::{AgentUiSynthesizer, NativeToolRegistry, SharedMapVfsBackend};
    let backend = SharedMapVfsBackend::shared();
    backend
        .borrow_mut()
        .files
        .insert("/system/accounts/token".into(), "secret-token".into());
    let mut registry = NativeToolRegistry::with_vfs_backend(backend);
    assert!(registry.vfs_read("/system/accounts/token").is_none());
    for (tool, args) in [
        ("fs_read", r#"{"path":"/tmp/../system/accounts/token"}"#),
        ("fs_list", r#"{"path":"/"}"#),
        ("fs_list", r#"{"path":"/system"}"#),
    ] {
        let result = registry.execute_tool(
            &ToolCall::new(tool, tool, args),
            &mut AgentUiSynthesizer::new(),
        );
        if let Ok(output) = result {
            assert!(!output.output_json.contains("secret-token"));
            if tool == "fs_list" {
                assert!(!output.output_json.contains("accounts/token"));
            }
        }
    }
}

#[test]
fn challenge_metadata_is_bound_and_scopes_cannot_bypass_profile() {
    let s = Rc::new(RefCell::new(State::default()));
    let mut b = broker(&s);
    b.note_challenge(
        RESOURCE,
        "Bearer resource_metadata=\"https://mail.example/custom-metadata\", scope=\"mail.read\"",
    )
    .unwrap();
    let r = ready(b.start(&req(AccountOperation::Begin { scopes: vec![] }))).unwrap();
    assert!(matches!(r.outcome, AccountOutcome::Authorizing { .. }));
    assert_eq!(
        s.borrow().requests[0].0,
        "https://mail.example/custom-metadata"
    );
    b.cancel();
    assert!(b
        .note_challenge(RESOURCE, "Bearer resource_metadata=\"http://localhost/no\"")
        .is_err());
    assert!(b.note_challenge(RESOURCE,"Bearer resource_metadata=\"https://mail.example/one\", resource_metadata=\"https://mail.example/two\"").is_err());
    b.note_challenge(RESOURCE, "Bearer scope=\"mail.write\"")
        .unwrap();
    assert_eq!(
        ready(b.start(&req(AccountOperation::Begin { scopes: vec![] }))).unwrap_err(),
        AccountError::ClientRegistrationRequired
    );
}
#[test]
fn invalid_token_expiry_and_duplicate_members_never_create_credentials() {
    for token in [
        r#"{"token_type":"Bearer","access_token":"one","access_token":"two"}"#.to_string(),
        r#"{"token_type":"Bearer","access_token":"one","expires_in":0}"#.into(),
        r#"{"token_type":"Bearer","access_token":"one","expires_in":-1}"#.into(),
        r#"{"token_type":"Bearer","access_token":"one","expires_in":18446744073709551615}"#.into(),
        format!(
            r#"{{"token_type":"Bearer","access_token":"{}"}}"#,
            "x".repeat(MAX_BEARER_BYTES + 1)
        ),
    ] {
        let s = Rc::new(RefCell::new(State {
            token_json: Some(token),
            ..State::default()
        }));
        let mut b = broker(&s);
        let r = ready(b.start(&req(AccountOperation::Begin {
            scopes: vec!["mail.read".into()],
        })))
        .unwrap();
        let AccountOutcome::Authorizing { attempt } = r.outcome else {
            panic!()
        };
        assert!(ready(b.start(&req(AccountOperation::Poll { attempt }))).is_err());
        assert!(s.borrow().record.is_none());
    }
}
#[test]
fn restored_credentials_fail_closed_without_clock_or_after_clock_rollback() {
    let s = Rc::new(RefCell::new(State::default()));
    let mut b = broker(&s);
    let reference = authorize(&mut b);
    for clock in [None, Some(900), Some(1100)] {
        s.borrow_mut().clock = Some(clock);
        assert!(broker(&s).bearer("mail", RESOURCE, &reference).is_err());
    }
    s.borrow_mut().clock = Some(Some(900));
    assert!(ready(broker(&s).start(&req(AccountOperation::Status))).is_err());
    assert!(ready(broker(&s).start(&req(AccountOperation::Refresh {
        credential: reference
    })))
    .is_err());
}
#[test]
fn provider_network_failure_does_not_revive_locally_disconnected_credentials() {
    let s = Rc::new(RefCell::new(State::default()));
    let mut b = broker(&s);
    let reference = authorize(&mut b);
    s.borrow_mut().revoke_fails = true;
    let r = ready(b.start(&req(AccountOperation::Disconnect {
        credential: Some(reference.clone()),
    })))
    .unwrap();
    assert!(matches!(
        r.outcome,
        AccountOutcome::Disconnected {
            remote_revoked: false
        }
    ));
    assert!(broker(&s).bearer("mail", RESOURCE, &reference).is_err());
}

struct DelayedBrowser {
    inner: Browser,
    result: Option<Result<BrowserResponse, AccountError>>,
}
impl BrowserAssistant for DelayedBrowser {
    fn start(&mut self, r: &BrowserRequest) -> Poll<Result<BrowserResponse, AccountError>> {
        let Poll::Ready(result) = self.inner.start(r) else {
            panic!()
        };
        self.result = Some(result);
        Poll::Pending
    }
    fn poll(&mut self) -> Poll<Result<BrowserResponse, AccountError>> {
        Poll::Ready(self.result.take().unwrap())
    }
    fn cancel(&mut self) {
        self.result = None
    }
}
struct DelayedNetwork {
    inner: Network,
    result: Option<Result<HttpResponse, AccountError>>,
}
impl OAuthNetwork for DelayedNetwork {
    fn start(&mut self, r: &HttpRequest) -> Poll<Result<HttpResponse, AccountError>> {
        let Poll::Ready(result) = self.inner.start(r) else {
            panic!()
        };
        self.result = Some(result);
        Poll::Pending
    }
    fn poll(&mut self) -> Poll<Result<HttpResponse, AccountError>> {
        Poll::Ready(self.result.take().unwrap())
    }
    fn cancel(&mut self) {
        self.result = None
    }
}
#[test]
fn asynchronous_network_and_browser_complete_between_polls() {
    let s = Rc::new(RefCell::new(State::default()));
    let mut b = OsAccountBroker::new(
        Box::new(Store(s.clone())),
        Box::new(DelayedBrowser {
            inner: Browser(s.clone()),
            result: None,
        }),
        Box::new(DelayedNetwork {
            inner: Network(s.clone()),
            result: None,
        }),
        Box::new(Crypto(s.clone(), 0)),
    );
    fn drive(b: &mut OsAccountBroker, op: AccountOperation) -> AccountOutcome {
        let mut result = b.start(&req(op));
        for _ in 0..30 {
            if let Poll::Ready(r) = result {
                return r.unwrap().outcome;
            }
            result = b.poll()
        }
        panic!("poll budget")
    }
    let AccountOutcome::Authorizing { attempt } = drive(
        &mut b,
        AccountOperation::Begin {
            scopes: vec!["mail.read".into()],
        },
    ) else {
        panic!()
    };
    let AccountOutcome::Authorized { credential, .. } =
        drive(&mut b, AccountOperation::Poll { attempt })
    else {
        panic!()
    };
    assert_eq!(
        b.bearer("mail", RESOURCE, &credential).unwrap(),
        "private-access"
    );
}

#[test]
fn issuer_callback_validation_follows_all_specification_rows() {
    for advertised in [true, false] {
        assert_eq!(
            callback_code_for_metadata("state=s&iss=i&code=c", "s", "i", advertised),
            Ok("c".into())
        );
        assert_eq!(
            callback_code_for_metadata("state=s&iss=other&code=c", "s", "i", advertised),
            Err(AccountError::CallbackMismatch)
        );
        assert_eq!(
            callback_code_for_metadata("state=s&iss=other&error=denied", "s", "i", advertised),
            Err(AccountError::CallbackMismatch)
        );
    }
    assert_eq!(
        callback_code_for_metadata("state=s&code=c", "s", "i", true),
        Err(AccountError::CallbackMismatch)
    );
    assert_eq!(
        callback_code_for_metadata("state=s&code=c", "s", "i", false),
        Ok("c".into())
    );
    assert_eq!(
        callback_code_for_metadata("state=s&error=denied", "s", "i", false),
        Err(AccountError::AuthorizationDenied)
    );
}

#[test]
fn changing_client_profile_drops_old_authority_even_if_profile_write_fails() {
    for fail in [false, true] {
        let s = Rc::new(RefCell::new(State::default()));
        let mut b = broker(&s);
        let reference = authorize(&mut b);
        s.borrow_mut().profile_write_fails = fail;
        let profile = ClientProfile {
            endpoint: RESOURCE.into(),
            issuer: ISSUER.into(),
            client_id: "changed-client".into(),
            allowed_scopes: vec![],
            loopback_port: 0,
            client_secret: None,
        };
        let result = b.provision_profile("mail", &profile);
        assert_eq!(result.is_err(), fail);
        assert!(s.borrow().record.is_none());
        assert!(broker(&s).bearer("mail", RESOURCE, &reference).is_err());
    }
}

#[test]
fn approved_account_removal_forgets_private_profile_while_disconnect_retains_it() {
    let s = Rc::new(RefCell::new(State::default()));
    let mut b = broker(&s);
    let reference = authorize(&mut b);
    let mut private_profile = Store(s.clone()).profile("mail", RESOURCE).unwrap().unwrap();
    private_profile.client_secret = Some("private-client-secret".into());
    Store(s.clone())
        .save_profile("mail", &private_profile)
        .unwrap();
    ready(b.start(&req(AccountOperation::Disconnect {
        credential: Some(reference),
    })))
    .unwrap();
    assert!(Store(s.clone())
        .profile("mail", RESOURCE)
        .unwrap()
        .is_some());
    b.forget_profile("mail", RESOURCE).unwrap();
    assert!(Store(s.clone())
        .profile("mail", RESOURCE)
        .unwrap()
        .is_none());
    assert!(ready(broker(&s).start(&req(AccountOperation::Status))).is_err());
}
#[test]
fn account_removal_profile_failure_never_reports_complete_cleanup() {
    let s = Rc::new(RefCell::new(State::default()));
    let mut b = broker(&s);
    authorize(&mut b);
    s.borrow_mut().profile_delete_fails = true;
    assert_eq!(
        b.forget_profile("mail", RESOURCE),
        Err(AccountError::StorageUnavailable)
    );
    assert!(s.borrow().record.is_none());
    assert!(Store(s.clone())
        .profile("mail", RESOURCE)
        .unwrap()
        .is_some());
}

fn resource_metadata(scopes: serde_json::Value) -> String {
    serde_json::json!({"resource":RESOURCE,"authorization_servers":[ISSUER],"scopes_supported":scopes}).to_string()
}
fn authorization_url(s: &Rc<RefCell<State>>) -> String {
    s.borrow()
        .browser
        .iter()
        .find_map(|request| {
            if let BrowserOperation::Open {
                authorization_url, ..
            } = &request.operation
            {
                Some(authorization_url.clone())
            } else {
                None
            }
        })
        .unwrap()
}
#[test]
fn resource_scope_fallback_respects_native_approval_and_omits_empty_scope() {
    for scopes in [
        None,
        Some(serde_json::json!([])),
        Some(serde_json::json!(["mail.read"])),
    ] {
        let s = Rc::new(RefCell::new(State {
            prm_json: scopes.clone().map(resource_metadata),
            ..State::default()
        }));
        let mut b = broker(&s);
        let response = ready(b.start(&req(AccountOperation::Begin { scopes: vec![] }))).unwrap();
        assert!(matches!(
            response.outcome,
            AccountOutcome::Authorizing { .. }
        ));
        let url = authorization_url(&s);
        let scope = url.split('&').find_map(|part| part.strip_prefix("scope="));
        assert_eq!(
            scope,
            if scopes == Some(serde_json::json!(["mail.read"])) {
                Some("mail.read")
            } else {
                None
            }
        );
    }
    let s = Rc::new(RefCell::new(State {
        prm_json: Some(resource_metadata(serde_json::json!(["mail.write"]))),
        ..State::default()
    }));
    let mut b = broker(&s);
    assert_eq!(
        ready(b.start(&req(AccountOperation::Begin { scopes: vec![] }))).unwrap_err(),
        AccountError::ClientRegistrationRequired
    );
    assert!(s.borrow().browser.is_empty());
    assert!(s.borrow().record.is_none());
}
#[test]
fn challenge_and_explicit_scopes_take_precedence_over_resource_scope_metadata() {
    for challenged in [false, true] {
        let s = Rc::new(RefCell::new(State {
            prm_json: Some(resource_metadata(serde_json::json!(["mail.write"]))),
            ..State::default()
        }));
        let mut b = broker(&s);
        if challenged {
            b.note_challenge(RESOURCE, "Bearer scope=\"mail.read\"")
                .unwrap();
        }
        let scopes = if challenged {
            vec![]
        } else {
            vec!["mail.read".into()]
        };
        ready(b.start(&req(AccountOperation::Begin { scopes }))).unwrap();
        let url = authorization_url(&s);
        assert!(url.contains("scope=mail.read&"));
        assert!(!url.contains("mail.write"));
    }
}
#[test]
fn malformed_ambiguous_or_unbounded_resource_scopes_fail_before_browser() {
    let mut values = vec![
        resource_metadata(serde_json::Value::Null),
        resource_metadata(serde_json::json!("mail.read")),
        resource_metadata(serde_json::json!(["mail.read", 7])),
        resource_metadata(serde_json::json!(["mail.read", "mail.read"])),
        resource_metadata(serde_json::json!(["invalid scope"])),
        resource_metadata(serde_json::json!(vec!["x"; 33])),
        resource_metadata(serde_json::json!(["x".repeat(257)])),
    ];
    values.push(format!(r#"{{"resource":"{RESOURCE}","authorization_servers":["{ISSUER}"],"scopes_supported":[],"scopes_supported":["mail.read"]}}"#));
    for metadata in values {
        let s = Rc::new(RefCell::new(State {
            prm_json: Some(metadata),
            ..State::default()
        }));
        let mut b = broker(&s);
        assert!(ready(b.start(&req(AccountOperation::Begin { scopes: vec![] }))).is_err());
        assert!(s.borrow().browser.is_empty());
        assert!(s.borrow().record.is_none());
    }
}

fn metadata_for_resource(resource: &str) -> String {
    serde_json::json!({"resource":resource,"authorization_servers":[ISSUER]}).to_string()
}
fn authorize_endpoint(b: &mut OsAccountBroker, endpoint: &str) -> CredentialReference {
    let mut request = req(AccountOperation::Begin {
        scopes: vec!["mail.read".into()],
    });
    request.endpoint = endpoint.into();
    let response = ready(b.start(&request)).unwrap();
    let AccountOutcome::Authorizing { attempt } = response.outcome else {
        panic!()
    };
    request.operation = AccountOperation::Poll { attempt };
    let response = ready(b.start(&request)).unwrap();
    let AccountOutcome::Authorized { credential, .. } = response.outcome else {
        panic!()
    };
    credential
}
#[test]
fn root_fallback_audience_is_persisted_but_transport_app_and_reference_remain_exact() {
    let origin = "https://mail.example";
    let s = Rc::new(RefCell::new(State {
        metadata_404: vec!["https://mail.example/.well-known/oauth-protected-resource/mcp".into()],
        prm_json: Some(metadata_for_resource(origin)),
        token_resource: Some(origin.into()),
        ..State::default()
    }));
    let mut b = broker(&s);
    let reference = authorize_endpoint(&mut b, RESOURCE);
    let record = s.borrow().record.clone().unwrap();
    assert_eq!(record.endpoint, RESOURCE);
    assert_eq!(record.oauth_resource.as_deref(), Some(origin));
    let wire = serde_json::to_vec(&record).unwrap();
    s.borrow_mut().record = Some(serde_json::from_slice(&wire).unwrap());
    let open = s
        .borrow()
        .browser
        .iter()
        .find_map(|r| {
            if let BrowserOperation::Open {
                authorization_url,
                resource,
            } = &r.operation
            {
                Some((
                    r.endpoint.clone(),
                    authorization_url.clone(),
                    resource.clone(),
                ))
            } else {
                None
            }
        })
        .unwrap();
    assert_eq!(open.0, RESOURCE);
    assert_eq!(open.2.as_deref(), Some(origin));
    assert!(open
        .1
        .contains(&format!("resource={}", form_encode(origin))));
    let mut restored = broker(&s);
    assert_eq!(
        restored.bearer("mail", RESOURCE, &reference).unwrap(),
        "private-access"
    );
    for (app, endpoint) in [
        ("other", RESOURCE),
        ("mail", "https://mail.example/other"),
        ("mail", "https://mail.example/mcp?tenant=other"),
        ("mail", "https://mail.example"),
    ] {
        assert!(restored.bearer(app, endpoint, &reference).is_err())
    }
    ready(restored.start(&req(AccountOperation::Refresh {
        credential: reference,
    })))
    .unwrap();
    assert_eq!(
        s.borrow()
            .record
            .as_ref()
            .unwrap()
            .oauth_resource
            .as_deref(),
        Some(origin)
    );
}
#[test]
fn path_and_query_resources_match_exactly_and_preserve_discovery_query() {
    for endpoint in [
        RESOURCE,
        "https://mail.example/tenant-one/mcp",
        "https://mail.example/mcp?tenant=one",
        "https://mail.example?tenant=one",
    ] {
        let s = Rc::new(RefCell::new(State {
            prm_json: Some(metadata_for_resource(endpoint)),
            token_resource: Some(endpoint.into()),
            ..State::default()
        }));
        let mut b = broker(&s);
        authorize_endpoint(&mut b, endpoint);
        assert_eq!(
            s.borrow()
                .record
                .as_ref()
                .unwrap()
                .oauth_resource
                .as_deref(),
            Some(endpoint)
        );
        let expected = format!(
            "https://mail.example/.well-known/oauth-protected-resource{}",
            &endpoint["https://mail.example".len()..]
        );
        assert_eq!(s.borrow().requests[0].0, expected);
    }
}
#[test]
fn header_metadata_requires_actual_transport_resource_and_never_falls_back_on_failure() {
    let metadata = "https://metadata.example/custom-metadata";
    for resource in [
        RESOURCE,
        "https://mail.example",
        "https://mail.example/tenant-other",
        "https://other.example/mcp",
    ] {
        let s = Rc::new(RefCell::new(State {
            prm_json: Some(metadata_for_resource(resource)),
            ..State::default()
        }));
        let mut b = broker(&s);
        b.note_challenge(
            RESOURCE,
            &format!("Bearer resource_metadata=\"{metadata}\""),
        )
        .unwrap();
        let result = ready(b.start(&req(AccountOperation::Begin {
            scopes: vec!["mail.read".into()],
        })));
        assert_eq!(result.is_ok(), resource == RESOURCE);
        assert_eq!(s.borrow().requests[0].0, metadata);
        if resource != RESOURCE {
            assert!(s.borrow().browser.is_empty())
        }
    }
    let s = Rc::new(RefCell::new(State {
        metadata_404: vec![metadata.into()],
        ..State::default()
    }));
    let mut b = broker(&s);
    b.note_challenge(
        RESOURCE,
        &format!("Bearer resource_metadata=\"{metadata}\""),
    )
    .unwrap();
    assert!(ready(b.start(&req(AccountOperation::Begin { scopes: vec![] }))).is_err());
    assert_eq!(s.borrow().requests.len(), 1);
}
#[test]
fn constructed_metadata_rejects_same_origin_parent_tenant_query_and_cross_origin_confusion() {
    for (endpoint, returned, root) in [
        (
            "https://mail.example/tenant-one/mcp",
            "https://mail.example/tenant-one",
            false,
        ),
        (RESOURCE, "https://mail.example", false),
        (RESOURCE, "https://other.example/mcp", false),
        (
            "https://mail.example/mcp?tenant=one",
            "https://mail.example/mcp?tenant=two",
            false,
        ),
        ("https://mail.example/mcp?tenant=one", RESOURCE, false),
        (RESOURCE, RESOURCE, true),
        (RESOURCE, "https://mail.example/tenant-other", true),
    ] {
        let path = format!(
            "https://mail.example/.well-known/oauth-protected-resource{}",
            &endpoint["https://mail.example".len()..]
        );
        let s = Rc::new(RefCell::new(State {
            prm_json: Some(metadata_for_resource(returned)),
            metadata_404: if root { vec![path] } else { vec![] },
            ..State::default()
        }));
        let mut b = broker(&s);
        let mut request = req(AccountOperation::Begin { scopes: vec![] });
        request.endpoint = endpoint.into();
        assert!(ready(b.start(&request)).is_err());
        assert!(s.borrow().browser.is_empty());
        assert!(s.borrow().record.is_none());
    }
}
#[test]
fn old_or_replaced_challenges_do_not_change_another_resource_candidate() {
    let other = "https://mail.example/tenant-other/mcp";
    let s = Rc::new(RefCell::new(State {
        prm_json: Some(metadata_for_resource(other)),
        ..State::default()
    }));
    let mut b = broker(&s);
    b.note_challenge(
        RESOURCE,
        "Bearer resource_metadata=\"https://metadata.example/custom-metadata\"",
    )
    .unwrap();
    let mut request = req(AccountOperation::Begin { scopes: vec![] });
    request.endpoint = other.into();
    ready(b.start(&request)).unwrap();
    assert_eq!(
        s.borrow().requests[0].0,
        "https://mail.example/.well-known/oauth-protected-resource/tenant-other/mcp"
    );
    b.cancel();
    s.borrow_mut().prm_json = Some(metadata_for_resource(RESOURCE));
    s.borrow_mut().requests.clear();
    b.note_challenge(RESOURCE, "Bearer").unwrap();
    ready(b.start(&req(AccountOperation::Begin { scopes: vec![] }))).unwrap();
    assert_eq!(
        s.borrow().requests[0].0,
        "https://mail.example/.well-known/oauth-protected-resource/mcp"
    );
}
#[test]
fn legacy_records_default_only_to_exact_endpoint_and_invalid_persisted_audience_is_rejected() {
    let s = Rc::new(RefCell::new(State::default()));
    let mut b = broker(&s);
    let reference = authorize(&mut b);
    let mut json = serde_json::to_value(s.borrow().record.as_ref().unwrap()).unwrap();
    json.as_object_mut().unwrap().remove("oauth_resource");
    s.borrow_mut().record = Some(serde_json::from_value(json).unwrap());
    assert!(broker(&s).bearer("mail", RESOURCE, &reference).is_ok());
    for resource in [
        "https://mail.example/other",
        "https://other.example",
        "https://mail.example?aud=other",
    ] {
        s.borrow_mut().record.as_mut().unwrap().oauth_resource = Some(resource.into());
        assert!(broker(&s).bearer("mail", RESOURCE, &reference).is_err());
        assert!(ready(broker(&s).start(&req(AccountOperation::Status))).is_err());
        assert!(ready(broker(&s).start(&req(AccountOperation::Refresh {
            credential: reference.clone()
        })))
        .is_err())
    }
}

fn revoked_account() -> Rc<RefCell<State>> {
    let state = Rc::new(RefCell::new(State::default()));
    authorize(&mut broker(&state));
    let mut value = state.borrow_mut();
    value.record.as_mut().unwrap().access_token = "revoked-token".into();
    value.browser.clear();
    value.requests.clear();
    drop(value);
    state
}
fn browser_opens(state: &Rc<RefCell<State>>) -> usize {
    state
        .borrow()
        .browser
        .iter()
        .filter(|request| matches!(request.operation, BrowserOperation::Open { .. }))
        .count()
}
#[test]
fn explicit_connect_reauthorizes_revoked_saved_token_once_then_verifies_actual_mcp() {
    let state = revoked_account();
    let (mut registry, _, seen) = native_registry(&state, false);
    let output = drive_arguments(&mut registry, "mcp_connect", r#"{"app":"mail"}"#);
    assert_eq!(output["connection"]["phase"], "ready");
    assert_eq!(browser_opens(&state), 1);
    assert_eq!(
        output["connection"]["scopes"],
        serde_json::json!(["mail.read"])
    );
    assert!(seen.borrow().iter().any(|method| method == "tools/list"));
    assert!(!seen.borrow().iter().any(|method| method == "tools/call"));
    assert_eq!(
        state
            .borrow()
            .requests
            .iter()
            .filter(|(url, _)| url.ends_with("/token"))
            .count(),
        1
    );
}
#[test]
fn rejected_reauthorization_exits_needs_auth_without_an_infinite_browser_loop() {
    let state = revoked_account();
    state.borrow_mut().token_json=Some(r#"{"token_type":"Bearer","access_token":"still-rejected","expires_in":100,"scope":"mail.read"}"#.into());
    let (mut registry, _, _) = native_registry(&state, false);
    let output = drive_arguments(&mut registry, "mcp_connect", r#"{"app":"mail"}"#);
    assert_eq!(output["ok"], false);
    assert_eq!(output["connection"]["phase"], "needs_auth");
    assert_eq!(browser_opens(&state), 1);
    assert_eq!(
        state
            .borrow()
            .requests
            .iter()
            .filter(|(url, _)| url.ends_with("/token"))
            .count(),
        1
    );
}
struct AlwaysUnauthorized {
    challenge: String,
}
impl HttpTransport for AlwaysUnauthorized {
    fn request(&mut self, _: &HttpRequest) -> Result<HttpResponse, String> {
        Ok(HttpResponse {
            status: 401,
            headers: vec![("WWW-Authenticate".into(), self.challenge.clone())],
            body: String::new(),
        })
    }
}
#[test]
fn status_and_refresh_do_not_turn_rejected_credentials_into_browser_authorization() {
    let state = Rc::new(RefCell::new(State::default()));
    let (mut registry, _, _) = native_registry(&state, false);
    drive(&mut registry, "mcp_connect");
    state.borrow_mut().browser.clear();
    registry.set_http_transport(Box::new(AlwaysUnauthorized {
        challenge: "Bearer".into(),
    }));
    drive(&mut registry, "mcp_connection_status");
    let output = drive(&mut registry, "mcp_refresh_connection");
    assert_eq!(output["ok"], false);
    assert_eq!(output["connection"]["phase"], "needs_auth");
    assert_eq!(browser_opens(&state), 0);
}
#[test]
fn reauthorization_preserves_profile_limits_and_rejects_untrusted_challenge_metadata() {
    for invalid_metadata in 0..3 {
        let state = revoked_account();
        let (mut registry, _, _) = native_registry(&state, false);
        if invalid_metadata == 1 {
            registry.set_http_transport(Box::new(AlwaysUnauthorized {
                challenge: "Bearer resource_metadata=\"http://localhost/metadata\"".into(),
            }));
        }
        if invalid_metadata == 2 {
            state.borrow_mut().prm_json=Some(serde_json::json!({"resource":RESOURCE,"authorization_servers":["https://attacker.example"]}).to_string());
            registry.set_http_transport(Box::new(AlwaysUnauthorized {
                challenge: "Bearer resource_metadata=\"https://metadata.example/custom-metadata\""
                    .into(),
            }));
        }
        let args = if invalid_metadata != 0 {
            r#"{"app":"mail"}"#
        } else {
            r#"{"app":"mail","scopes":["mail.write"]}"#
        };
        let output = drive_arguments(&mut registry, "mcp_connect", args);
        assert_eq!(output["ok"], false);
        assert_ne!(output["connection"]["phase"], "ready");
        assert_eq!(browser_opens(&state), 0);
        assert!(!state
            .borrow()
            .requests
            .iter()
            .any(|(url, _)| url.ends_with("/token")));
        assert_eq!(
            state.borrow().record.as_ref().unwrap().access_token,
            "revoked-token"
        );
        assert_eq!(state.borrow().record.as_ref().unwrap().issuer, ISSUER);
    }
}

#[test]
fn challenged_scope_union_preserves_previous_grants_without_expanding_private_profile() {
    let state=Rc::new(RefCell::new(State::default()));
    let mut profile=Store(state.clone()).profile("mail",RESOURCE).unwrap().unwrap();
    profile.allowed_scopes.push("mail.write".into());
    state.borrow_mut().configured_profile=Some(profile);
    let mut broker=broker(&state);
    broker.note_challenge(RESOURCE,r#"Bearer error="insufficient_scope", scope="mail.write""#).unwrap();
    let response=ready(broker.start(&req(AccountOperation::Begin{scopes:vec!["mail.read".into()]}))).unwrap();
    assert!(matches!(response.outcome,AccountOutcome::Authorizing{..}));
    let url=state.borrow().browser.iter().find_map(|request|match &request.operation {
        BrowserOperation::Open{authorization_url,..}=>Some(authorization_url.clone()),_=>None
    }).unwrap();
    assert!(url.contains("scope=mail.read%20mail.write"));
    broker.cancel();
    state.borrow_mut().browser.clear();
    broker.note_challenge(RESOURCE,r#"Bearer error="insufficient_scope", scope="mail.admin""#).unwrap();
    assert_eq!(ready(broker.start(&req(AccountOperation::Begin{scopes:vec!["mail.read".into()]}))).unwrap_err(),AccountError::ClientRegistrationRequired);
    assert_eq!(browser_opens(&state),0);
    assert!(!state.borrow().configured_profile.as_ref().unwrap().allowed_scopes.contains(&"mail.admin".into()));
}

struct ScopeUpgrade(McpWire);
impl HttpTransport for ScopeUpgrade {
    fn request(&mut self, request: &HttpRequest) -> Result<HttpResponse,String> {
        if request.headers.iter().any(|(name,value)|name=="X-AlloyOS-Trusted-Bearer" && value=="revoked-token") {
            return Ok(HttpResponse{status:403,headers:vec![("WWW-Authenticate".into(),r#"Bearer error="insufficient_scope", scope="mail.write""#.into())],body:String::new()});
        }
        self.0.request(request)
    }
}
#[test]
fn explicit_connect_steps_up_once_within_profile_or_requests_native_configuration() {
    for approved in [true,false] {
        let state=revoked_account();
        if approved {
            let mut profile=Store(state.clone()).profile("mail",RESOURCE).unwrap().unwrap();
            profile.allowed_scopes.push("mail.write".into());
            state.borrow_mut().configured_profile=Some(profile);
            state.borrow_mut().token_json=Some(r#"{"token_type":"Bearer","access_token":"private-access","expires_in":100,"scope":"mail.read mail.write"}"#.into());
        }
        let (mut registry,_,seen)=native_registry(&state,false);
        registry.set_http_transport(Box::new(ScopeUpgrade(McpWire{fail:false,seen:seen.clone()})));
        let output=drive_arguments(&mut registry,"mcp_connect",r#"{"app":"mail"}"#);
        assert_eq!(output["ok"],approved);
        assert_eq!(browser_opens(&state),usize::from(approved));
        if approved {
            assert_eq!(output["connection"]["phase"],"ready");
            assert_eq!(output["connection"]["scopes"],serde_json::json!(["mail.read","mail.write"]));
            assert!(seen.borrow().iter().any(|method|method=="tools/list"));
        } else {
            assert!(registry.pending_client_profile_setup().is_some());
            assert_ne!(output["connection"]["phase"],"ready");
            assert!(state.borrow().record.as_ref().unwrap().scopes==vec!["mail.read"]);
        }
        assert!(!seen.borrow().iter().any(|method|method=="tools/call"));
    }
}
