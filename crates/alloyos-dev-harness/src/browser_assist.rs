//! Optional QEMU development helper: launch a browser and return one bound
//! loopback callback. It has no credential vault or OAuth/token exchange code.
use alloyos_mcp::browser::{
    BrowserError as Error, BrowserOperation, BrowserOutcome, BrowserRequest, BrowserResponse,
};
use std::{
    collections::BTreeMap,
    io::{Read, Write},
    net::{TcpListener, TcpStream},
    process::{Command, Stdio},
    sync::{Arc, Mutex, OnceLock, Weak},
    time::{Duration, Instant},
};
use url::Url;
struct Flow {
    app_id: String,
    endpoint: String,
    listener: TcpListener,
    redirect: String,
    started: Instant,
    opened: bool,
    resource: Option<String>,
}
#[derive(Default)]
struct Assistant {
    flows: BTreeMap<String, Flow>,
}
const FLOW_LIFETIME: Duration = Duration::from_secs(300);
static ASSISTANT: OnceLock<Result<Arc<Mutex<Assistant>>, Error>> = OnceLock::new();
fn expiry_worker(assistant: Weak<Mutex<Assistant>>, interval: Duration) -> Result<(), Error> {
    std::thread::Builder::new().name("browser-lease-expiry".into()).spawn(move || loop {
        std::thread::sleep(interval);
        let Some(assistant) = assistant.upgrade() else { break; };
        let Ok(mut state) = assistant.lock() else { break; };
        state.expire();
    }).map(|_| ()).map_err(|_| Error::HostUnavailable)
}
pub fn handle_request(request: &BrowserRequest) -> BrowserResponse {
    let result = request
        .validate()
        .and_then(|_| hosted_url(&request.endpoint).map(|_| ()))
        .and_then(|_| {
            ASSISTANT
                .get_or_init(|| {
                    let assistant = Arc::new(Mutex::new(Assistant::default()));
                    expiry_worker(Arc::downgrade(&assistant), Duration::from_secs(1))?;
                    Ok(assistant)
                })
                .as_ref().map_err(|error| *error)?
                .lock()
                .map_err(|_| Error::HostUnavailable)?
                .handle(request)
        });
    BrowserResponse {
        request_id: request.request_id,
        app_id: request.app_id.clone(),
        endpoint: request.endpoint.clone(),
        attempt: request.attempt.clone(),
        outcome: result.unwrap_or_else(|error| BrowserOutcome::Error { error }),
    }
}
impl Assistant {
    fn expire(&mut self) {
        self.flows.retain(|_, flow| flow.started.elapsed() < FLOW_LIFETIME);
    }
    fn handle(&mut self, request: &BrowserRequest) -> Result<BrowserOutcome, Error> {
        self.expire();
        if let Some(flow) = self.flows.get(&request.attempt) {
            if flow.app_id != request.app_id || flow.endpoint != request.endpoint {
                return Err(Error::InvalidRequest);
            }
        }
        match &request.operation {
            BrowserOperation::Prepare { port } => {
                if let Some(flow) = self.flows.get(&request.attempt) {
                    return Ok(BrowserOutcome::Prepared {
                        redirect_uri: flow.redirect.clone(),
                    });
                }
                if self.flows.len() >= 4 {
                    return Err(Error::Busy);
                }
                let listener = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, *port))
                    .map_err(|_| Error::HostUnavailable)?;
                listener
                    .set_nonblocking(true)
                    .map_err(|_| Error::HostUnavailable)?;
                let redirect = format!(
                    "http://127.0.0.1:{}/oauth/callback",
                    listener
                        .local_addr()
                        .map_err(|_| Error::HostUnavailable)?
                        .port()
                );
                self.flows.insert(
                    request.attempt.clone(),
                    Flow {
                        app_id: request.app_id.clone(),
                        endpoint: request.endpoint.clone(),
                        listener,
                        redirect: redirect.clone(),
                        started: Instant::now(),
                        opened: false,
                        resource: None,
                    },
                );
                Ok(BrowserOutcome::Prepared {
                    redirect_uri: redirect,
                })
            }
            BrowserOperation::Open { authorization_url, resource } => {
                let flow = self.flows.get_mut(&request.attempt).ok_or(Error::Expired)?;
                let resource=resource.as_deref().unwrap_or(&flow.endpoint);
                if !alloyos_mcp::account::valid_oauth_resource_binding(&flow.endpoint,resource){return Err(Error::InvalidRequest)}
                if flow.resource.as_ref().is_some_and(|previous|previous!=resource){return Err(Error::InvalidRequest)}
                validate_authorization_url(authorization_url, &flow.redirect, resource)?;
                if !flow.opened {
                    open_browser(authorization_url)?;
                    flow.opened = true;
                    flow.resource=Some(resource.to_string());
                }
                Ok(BrowserOutcome::Opened)
            }
            BrowserOperation::Poll => {
                let flow = self.flows.get(&request.attempt).ok_or(Error::Expired)?;
                if !flow.opened {
                    return Ok(BrowserOutcome::Pending);
                }
                let (mut socket, peer) = match flow.listener.accept() {
                    Ok(pair) => pair,
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        return Ok(BrowserOutcome::Pending)
                    }
                    Err(_) => return Err(Error::CallbackRejected),
                };
                if !peer.ip().is_loopback() {
                    return Err(Error::CallbackRejected);
                }
                let query = read_callback(&mut socket, &flow.redirect);
                let text = if query.is_ok() {
                    "Callback delivered. Return to alloyOS to finish connecting."
                } else {
                    "Callback rejected. Return to alloyOS and retry."
                };
                let _=write!(socket,"HTTP/1.1 {}\r\nContent-Type: text/plain\r\nCache-Control: no-store\r\nConnection: close\r\nContent-Length: {}\r\n\r\n{}",if query.is_ok(){"200 OK"}else{"400 Bad Request"},text.len(),text);
                let query = query?;
                self.flows.remove(&request.attempt);
                Ok(BrowserOutcome::Callback { query })
            }
            BrowserOperation::Cancel => {
                self.flows.remove(&request.attempt);
                Ok(BrowserOutcome::Cancelled)
            }
        }
    }
}
fn hosted_url(value: &str) -> Result<Url, Error> {
    let url = Url::parse(value).map_err(|_| Error::InvalidRequest)?;
    let host = url
        .host_str()
        .ok_or(Error::InvalidRequest)?
        .trim_end_matches('.')
        .to_ascii_lowercase();
    if url.scheme() != "https"
        || !url.username().is_empty()
        || url.password().is_some()
        || url.fragment().is_some()
        || value
            .bytes()
            .any(|b| b.is_ascii_control() || b.is_ascii_whitespace())
        || !host.contains('.')
        || [
            "local",
            "localhost",
            "localdomain",
            "internal",
            "lan",
            "home",
        ]
        .iter()
        .any(|suffix| host == *suffix || host.ends_with(&format!(".{suffix}")))
        || host
            .parse::<std::net::IpAddr>()
            .is_ok_and(|ip| !crate::https::public_image_address(ip))
    {
        return Err(Error::InvalidRequest);
    }
    Ok(url)
}
fn validate_authorization_url(value: &str, redirect: &str, resource: &str) -> Result<(), Error> {
    let url = hosted_url(value)?;
    let pairs: Vec<_> = url.query_pairs().collect();
    for (name, expected) in [
        ("redirect_uri", redirect),
        ("resource", resource),
        ("response_type", "code"),
        ("code_challenge_method", "S256"),
    ] {
        let values: Vec<_> = pairs.iter().filter(|(key, _)| key == name).collect();
        if values.len() != 1 || values[0].1 != expected {
            return Err(Error::InvalidRequest);
        }
    }
    for name in ["state", "code_challenge", "client_id"] {
        let values: Vec<_> = pairs.iter().filter(|(key, _)| key == name).collect();
        if values.len() != 1 || values[0].1.is_empty() {
            return Err(Error::InvalidRequest);
        }
    }
    if pairs.iter().any(|(key, _)| {
        matches!(
            key.as_ref(),
            "access_token" | "refresh_token" | "client_secret" | "code_verifier"
        )
    }) {
        return Err(Error::InvalidRequest);
    }
    Ok(())
}
fn read_callback(socket: &mut TcpStream, redirect: &str) -> Result<String, Error> {
    // Accepted sockets may inherit the nonblocking listener flag on macOS.
    socket.set_nonblocking(false).map_err(|_| Error::CallbackRejected)?;
    socket
        .set_read_timeout(Some(Duration::from_millis(250)))
        .map_err(|_| Error::CallbackRejected)?;
    let mut bytes = Vec::new();
    let mut buffer = [0u8; 1024];
    while !bytes.ends_with(b"\r\n\r\n") && bytes.len() < 8192 {
        let n = socket
            .read(&mut buffer)
            .map_err(|_| Error::CallbackRejected)?;
        if n == 0 {
            break;
        }
        bytes.extend_from_slice(&buffer[..n]);
    }
    let result = (|| {
        if bytes.len() >= 8192 || !bytes.ends_with(b"\r\n\r\n") {
            return Err(Error::CallbackRejected);
        }
        let request = std::str::from_utf8(&bytes).map_err(|_| Error::CallbackRejected)?;
        let mut lines = request.split("\r\n");
        let mut first = lines
            .next()
            .ok_or(Error::CallbackRejected)?
            .split_whitespace();
        if first.next() != Some("GET") {
            return Err(Error::CallbackRejected);
        }
        let target = first.next().ok_or(Error::CallbackRejected)?;
        if !target.starts_with("/oauth/callback?") || target.contains('#') {
            return Err(Error::CallbackRejected);
        }
        let redirect = Url::parse(redirect).map_err(|_| Error::CallbackRejected)?;
        let expected_host = format!(
            "127.0.0.1:{}",
            redirect.port().ok_or(Error::CallbackRejected)?
        );
        let hosts: Vec<_> = lines
            .filter_map(|line| line.split_once(':'))
            .filter(|(name, _)| name.eq_ignore_ascii_case("Host"))
            .collect();
        if hosts.len() != 1 || hosts[0].1.trim() != expected_host {
            return Err(Error::CallbackRejected);
        }
        let query = target
            .strip_prefix("/oauth/callback?")
            .ok_or(Error::CallbackRejected)?;
        if query.is_empty() || query.bytes().any(|b| b.is_ascii_control()) {
            return Err(Error::CallbackRejected);
        }
        Ok(query.to_owned())
    })();
    bytes.fill(0);
    result
}
fn open_browser(url: &str) -> Result<(), Error> {
    #[cfg(test)]
    if TEST_BROWSER.with(|slot| {
        if let Some(urls) = slot.borrow_mut().as_mut() {
            urls.push(url.into());
            true
        } else {
            false
        }
    }) {
        return Ok(());
    }
    #[cfg(target_os = "macos")]
    let launcher = "/usr/bin/open";
    #[cfg(target_os = "linux")]
    let launcher = "/usr/bin/xdg-open";
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    return {
        let _ = url;
        Err(Error::BrowserUnavailable)
    };
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    {
        let mut command = Command::new(launcher);
        command
            .arg(url)
            .env_clear()
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        for key in [
            "HOME",
            "USER",
            "LOGNAME",
            "DISPLAY",
            "WAYLAND_DISPLAY",
            "XDG_RUNTIME_DIR",
            "DBUS_SESSION_BUS_ADDRESS",
            "LANG",
            "LC_ALL",
            "TMPDIR",
        ] {
            if let Some(value) = std::env::var_os(key) {
                command.env(key, value);
            }
        }
        command.env("PATH", "/usr/bin:/bin:/usr/sbin:/sbin");
        let mut child = command.spawn().map_err(|_| Error::BrowserUnavailable)?;
        let started = Instant::now();
        loop {
            match child.try_wait() {
                Ok(Some(status)) => {
                    return if status.success() {
                        Ok(())
                    } else {
                        Err(Error::BrowserUnavailable)
                    }
                }
                Err(_) => return Err(Error::BrowserUnavailable),
                _ => {}
            }
            if started.elapsed() > Duration::from_secs(5) {
                let _ = child.kill();
                let _ = child.wait();
                return Err(Error::BrowserUnavailable);
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }
}
#[cfg(test)]
thread_local! {static TEST_BROWSER:std::cell::RefCell<Option<Vec<String>>>=const{std::cell::RefCell::new(None)};}

/// Private QEMU channel; caller must forward replies only to the guest UART.
/// In particular callback queries are not ordinary serial/debug log messages.
pub fn relay_line(line: &str) -> Option<String> {
    use alloyos_forge::protocol::{decode_hex, encode_hex};
    use alloyos_mcp::browser::MAX_BROWSER_FRAME_BYTES;
    if !line.starts_with("BROWSER1\t") {
        return None;
    }
    let mut fields = line.trim_end_matches(['\r', '\n']).split('\t');
    fields.next();
    let id = fields.next()?.parse::<u64>().ok()?;
    if id == 0 {
        return None;
    }
    let result = (|| {
        let payload = fields.next().ok_or(Error::InvalidRequest)?;
        if fields.next().is_some() || payload.len() > MAX_BROWSER_FRAME_BYTES * 2 {
            return Err(Error::InvalidRequest);
        }
        let mut decoded = decode_hex(payload).map_err(|_| Error::InvalidRequest)?;
        let parsed = serde_json::from_str::<BrowserRequest>(&decoded);
        unsafe { decoded.as_bytes_mut().fill(0) }
        let request = parsed.map_err(|_| Error::InvalidRequest)?;
        if request.request_id != id {
            return Err(Error::InvalidRequest);
        }
        request.validate()?;
        Ok(handle_request(&request))
    })();
    match result {
        Ok(response) => Some(format!(
            "BROWSER1\t{id}\tok\t{}\n",
            encode_hex(&serde_json::to_string(&response).ok()?)
        )),
        Err(_) => Some(format!("BROWSER1\t{id}\terror\tinvalid_browser_request\n")),
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    const ENDPOINT: &str = "https://mail.example/mcp";
    fn request(operation: BrowserOperation) -> BrowserRequest {
        BrowserRequest {
            request_id: 1,
            app_id: "mail".into(),
            endpoint: ENDPOINT.into(),
            attempt: "attempt-0123456789abcdef".into(),
            operation,
        }
    }
    struct MockBrowser;
    impl MockBrowser {
        fn new() -> Self {
            TEST_BROWSER.with(|slot| *slot.borrow_mut() = Some(Vec::new()));
            Self
        }
    }
    impl Drop for MockBrowser {
        fn drop(&mut self) {
            TEST_BROWSER.with(|slot| *slot.borrow_mut() = None);
        }
    }
    fn authorization_url(redirect: &str) -> String {
        authorization_url_for_resource(redirect,ENDPOINT)
    }
    fn authorization_url_for_resource(redirect: &str, resource: &str) -> String {
        let mut url = Url::parse("https://identity.example/authorize").unwrap();
        url.query_pairs_mut().extend_pairs([
            ("redirect_uri", redirect),
            ("resource", resource),
            ("response_type", "code"),
            ("code_challenge_method", "S256"),
            ("state", "guest-owned-state"),
            ("code_challenge", "guest-owned-public-challenge"),
            ("client_id", "guest-client"),
        ]);
        url.to_string()
    }
    #[test]
    fn prepare_open_callback_cancel_without_host_credentials() {
        let _browser = MockBrowser::new();
        let mut assistant = Assistant::default();
        let prepared = assistant
            .handle(&request(BrowserOperation::Prepare { port: 0 }))
            .unwrap();
        let BrowserOutcome::Prepared { redirect_uri } = prepared else {
            panic!("expected bound redirect")
        };
        assert!(
            TEST_BROWSER.with(|slot| slot.borrow().as_ref().unwrap().is_empty()),
            "Prepare never opens the browser"
        );
        let authorization_url = authorization_url(&redirect_uri);
        assert!(matches!(
            assistant
                .handle(&request(BrowserOperation::Open {
                    resource: None,
                    authorization_url: authorization_url.clone()
                }))
                .unwrap(),
            BrowserOutcome::Opened
        ));
        assistant
            .handle(&request(BrowserOperation::Open { authorization_url, resource: None }))
            .unwrap();
        assert_eq!(
            TEST_BROWSER.with(|slot| slot.borrow().as_ref().unwrap().len()),
            1,
            "retrying Open does not launch twice"
        );
        assert!(matches!(
            assistant.handle(&request(BrowserOperation::Poll)).unwrap(),
            BrowserOutcome::Pending
        ));
        let url = Url::parse(&redirect_uri).unwrap();
        let address = format!("127.0.0.1:{}", url.port().unwrap());
        let mut browser = TcpStream::connect(&address).unwrap();
        // The helper forwards the query without accepting OAuth state/issuer:
        // those are checked by the OS against its own private flow record.
        write!(browser,"GET /oauth/callback?code=synthetic-code&state=wrong-state&iss=https%3A%2F%2Fidentity.example HTTP/1.1\r\nHost: {address}\r\n\r\n").unwrap();
        let response = assistant.handle(&request(BrowserOperation::Poll)).unwrap();
        let BrowserOutcome::Callback { query } = response else {
            panic!("expected transient callback")
        };
        assert!(query.contains("state=wrong-state"));
        assert!(!assistant.flows.contains_key("attempt-0123456789abcdef"));
        assert!(
            matches!(
                assistant.handle(&request(BrowserOperation::Poll)),
                Err(Error::Expired)
            ),
            "a callback can be delivered only once"
        );
        assistant
            .handle(&request(BrowserOperation::Prepare { port: 0 }))
            .unwrap();
        assert!(matches!(
            assistant
                .handle(&request(BrowserOperation::Cancel))
                .unwrap(),
            BrowserOutcome::Cancelled
        ));
        assert!(assistant.flows.is_empty());
    }
    #[test]
    fn exact_binding_and_redirect_gates_browser_launch() {
        let _browser = MockBrowser::new();
        let mut assistant = Assistant::default();
        let BrowserOutcome::Prepared { redirect_uri } = assistant
            .handle(&request(BrowserOperation::Prepare { port: 0 }))
            .unwrap()
        else {
            panic!()
        };
        let mut other = request(BrowserOperation::Cancel);
        other.app_id = "other".into();
        assert!(matches!(
            assistant.handle(&other),
            Err(Error::InvalidRequest)
        ));
        other.app_id = "mail".into();
        other.endpoint = "https://mail.example/other".into();
        assert!(matches!(
            assistant.handle(&other),
            Err(Error::InvalidRequest)
        ));
        for mut url in [
            authorization_url("http://127.0.0.1:1/oauth/callback"),
            authorization_url(&redirect_uri),
        ] {
            if url == authorization_url(&redirect_uri) {
                url.push_str("&code_verifier=must-never-enter-browser-url")
            }
            assert!(matches!(
                assistant.handle(&request(BrowserOperation::Open {
                    resource: None,
                    authorization_url: url
                })),
                Err(Error::InvalidRequest)
            ));
        }
        assert!(TEST_BROWSER.with(|slot| slot.borrow().as_ref().unwrap().is_empty()));
    }
    #[test]
    fn cancellation_releases_only_the_bound_listener_and_reclaims_capacity() {
        let mut assistant = Assistant::default();
        let mut requests = Vec::new();
        for index in 0..4 {
            let mut req = request(BrowserOperation::Prepare { port: 0 });
            req.attempt = format!("synthetic-attempt-{index:016}");
            let BrowserOutcome::Prepared { redirect_uri } = assistant.handle(&req).unwrap() else { panic!() };
            let port = Url::parse(&redirect_uri).unwrap().port().unwrap();
            requests.push((req, port));
        }
        assert!(matches!(assistant.handle(&request(BrowserOperation::Prepare { port: 0 })), Err(Error::Busy)));
        let (mut cancel, port) = requests[0].clone();
        cancel.operation = BrowserOperation::Cancel;
        for wrong in ["app", "endpoint"] {
            let mut mismatched = cancel.clone();
            if wrong == "app" { mismatched.app_id = "another-app".into(); }
            else { mismatched.endpoint = "https://other.example/mcp".into(); }
            assert!(matches!(assistant.handle(&mismatched), Err(Error::InvalidRequest)));
            assert!(TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, port)).is_err());
        }
        assert!(matches!(assistant.handle(&cancel), Ok(BrowserOutcome::Cancelled)));
        let released_port = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, port)).expect("Cancel releases the actual socket");
        assert!(matches!(assistant.handle(&cancel), Ok(BrowserOutcome::Cancelled)), "Cancel is idempotent");
        assert!(matches!(assistant.handle(&request(BrowserOperation::Cancel)), Ok(BrowserOutcome::Cancelled)), "unknown flow cancellation is harmless");
        for (_, other_port) in &requests[1..] {
            assert!(TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, *other_port)).is_err(), "other flows stay open");
        }
        assert!(matches!(assistant.handle(&request(BrowserOperation::Prepare { port: 0 })), Ok(BrowserOutcome::Prepared { .. })), "Cancel reclaims a flow slot");
        drop(released_port);
    }
    #[test]
    fn idle_lease_expiry_closes_socket_without_another_browser_request() {
        let assistant = Arc::new(Mutex::new(Assistant::default()));
        let port = {
            let mut state = assistant.lock().unwrap();
            let BrowserOutcome::Prepared { redirect_uri } = state.handle(&request(BrowserOperation::Prepare { port: 0 })).unwrap() else { panic!() };
            let flow = state.flows.values_mut().next().unwrap();
            flow.started = Instant::now() - FLOW_LIFETIME;
            Url::parse(&redirect_uri).unwrap().port().unwrap()
        };
        expiry_worker(Arc::downgrade(&assistant), Duration::from_millis(5)).unwrap();
        let deadline = Instant::now() + Duration::from_secs(1);
        loop {
            // No Assistant::handle call: the expiry worker must release the port.
            if let Ok(listener) = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, port)) { drop(listener); break; }
            assert!(Instant::now() < deadline, "idle browser listener did not expire");
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(assistant.lock().unwrap().flows.is_empty());
    }
    #[test]
    fn wrong_callback_host_is_rejected_and_public_debug_is_redacted() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let mut stream = TcpStream::connect(address).unwrap();
        write!(
            stream,
            "GET /oauth/callback?code=synthetic HTTP/1.1\r\nHost: evil.example\r\n\r\n"
        )
        .unwrap();
        let (mut server, _) = listener.accept().unwrap();
        assert!(matches!(
            read_callback(&mut server, &format!("http://{address}/oauth/callback")),
            Err(Error::CallbackRejected)
        ));
        let request = request(BrowserOperation::Poll);
        let response = BrowserResponse {
            request_id: 1,
            app_id: request.app_id.clone(),
            endpoint: ENDPOINT.into(),
            attempt: request.attempt.clone(),
            outcome: BrowserOutcome::Callback {
                query: "code=synthetic-secret".into(),
            },
        };
        assert!(!format!("{response:?}").contains("synthetic-secret"));
        assert!(response.validate_for(&request).is_ok());
        let line = relay_line("BROWSER1\t9\tzz\n").unwrap();
        assert_eq!(line, "BROWSER1\t9\terror\tinvalid_browser_request\n");
    }
    #[test]
    fn local_authorization_urls_and_embedded_secrets_are_never_launched() {
        for url in [
            "http://example.com/auth",
            "https://localhost/auth",
            "https://127.0.0.1/auth",
            "https://10.0.0.1/auth",
            "https://host.local/auth",
            "https://user:secret@example.com/auth",
        ] {
            assert!(hosted_url(url).is_err(), "{url}");
        }
    }
    #[test]
    fn explicit_origin_resource_keeps_transport_binding_and_rejects_changed_audience() {
        let _browser=MockBrowser::new();let mut assistant=Assistant::default();
        let BrowserOutcome::Prepared{redirect_uri}=assistant.handle(&request(BrowserOperation::Prepare{port:0})).unwrap()else{panic!()};
        let origin="https://mail.example";
        let open=BrowserOperation::Open{authorization_url:authorization_url_for_resource(&redirect_uri,origin),resource:Some(origin.into())};
        assert!(matches!(assistant.handle(&request(open)).unwrap(),BrowserOutcome::Opened));
        assert!(assistant.handle(&request(BrowserOperation::Open{authorization_url:authorization_url(&redirect_uri),resource:None})).is_err());
        for resource in ["https://mail.example/tenant-other","https://other.example","https://mail.example?audience=other"] {
            assert!(assistant.handle(&request(BrowserOperation::Open{authorization_url:authorization_url_for_resource(&redirect_uri,resource),resource:Some(resource.into())})).is_err());
        }
        TEST_BROWSER.with(|slot|assert_eq!(slot.borrow().as_ref().unwrap().len(),1));
    }
    #[test]
    fn legacy_open_omission_defaults_to_transport_and_explicit_resource_must_match_url() {
        let legacy:BrowserRequest=serde_json::from_value(serde_json::json!({"request_id":1,"app_id":"mail","endpoint":ENDPOINT,"attempt":"attempt-0123456789abcdef","operation":{"op":"open","authorization_url":"https://identity.example/authorize"}})).unwrap();
        assert!(matches!(legacy.operation,BrowserOperation::Open{resource:None,..}));
        let _browser=MockBrowser::new();let mut assistant=Assistant::default();
        let BrowserOutcome::Prepared{redirect_uri}=assistant.handle(&request(BrowserOperation::Prepare{port:0})).unwrap()else{panic!()};
        assert!(assistant.handle(&request(BrowserOperation::Open{authorization_url:authorization_url(&redirect_uri),resource:Some("https://mail.example".into())})).is_err());
        TEST_BROWSER.with(|slot|assert!(slot.borrow().as_ref().unwrap().is_empty()));
    }

}
