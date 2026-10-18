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
    fn profile(&mut self, e: &str) -> Result<Option<ClientProfile>, AccountError> {
        Ok(Some(ClientProfile {
            endpoint: e.into(),
            issuer: ISSUER.into(),
            client_id: "public-native-client".into(),
            allowed_scopes: vec!["mail.read".into()],
            client_secret: None,
            loopback_port: 0,
        }))
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
            BrowserOperation::Open { authorization_url } => {
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
                        if let BrowserOperation::Open { authorization_url } = &r.operation {
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
        let body = if r.url.contains("oauth-protected-resource")
            || r.url.ends_with("/custom-metadata")
        {
            format!(r#"{{"resource":"{RESOURCE}","authorization_servers":["{ISSUER}"]}}"#)
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
            assert!(r.body.contains("resource=https%3A%2F%2Fmail.example%2Fmcp"));
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
    let call = alloyos_forge::provider::ToolCall::new(
        tool,
        tool,
        if tool == "mcp_connect" {
            "{\"app\":\"mail\",\"scopes\":[\"mail.read\"]}"
        } else {
            "{\"app\":\"mail\"}"
        },
    );
    let mut ui = alloyos_welder::AgentUiSynthesizer::new();
    for _ in 0..100 {
        match registry.poll_execute_tool(&call, &mut ui) {
            Poll::Pending => {}
            Poll::Ready(Ok(output)) => {
                assert!(!output.output_json.contains("private-access"));
                assert!(!output.output_json.contains("private-refresh"));
                assert!(!output.output_json.contains("test-code"));
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
        AccountError::InvalidRequest
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
