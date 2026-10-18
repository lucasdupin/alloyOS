use alloyos_mcp::{AppCapability, AppManifest, McpApp, McpValue, RemoteMcpApp, RemoteConnectionStatus};
use alloyos_mcp::transport::{HttpClient,HttpRequest,HttpResponse,HttpTransport};
use std::{cell::RefCell,collections::BTreeMap,rc::Rc,task::Poll};

#[derive(Default)]
struct Trace { requests:Vec<HttpRequest>, cancelled:usize, unauthorized:bool, expire_call:bool, modern:bool }
struct Cooperative { trace:Rc<RefCell<Trace>>, pending:Option<HttpResponse>, ticks:usize }
impl HttpTransport for Cooperative {
    fn request(&mut self,_:&HttpRequest)->Result<HttpResponse,String> {panic!("remote protocol must never block")}
    fn start_request(&mut self,request:&HttpRequest)->Poll<Result<HttpResponse,String>> {
        assert!(self.pending.is_none());
        let rpc:serde_json::Value=serde_json::from_str(&request.body).unwrap();
        let method=rpc["method"].as_str().unwrap();
        let mut trace=self.trace.borrow_mut();trace.requests.push(request.clone());
        let response=if trace.unauthorized {
            HttpResponse {status:401,headers:vec![("WWW-Authenticate".into(),"Bearer resource_metadata=\"https://example.com/.well-known/oauth-protected-resource\"".into())],body:String::new()}
        } else if !trace.modern && request.headers.iter().any(|(name,value)|name=="MCP-Protocol-Version" && value=="2026-07-28") {
            HttpResponse {status:400,headers:vec![],body:String::new()}
        } else if method=="notifications/initialized" {
            assert!(rpc.get("id").is_none());
            HttpResponse {status:202,headers:vec![],body:String::new()}
        } else if method=="tools/call" && trace.expire_call {
            HttpResponse {status:404,headers:vec![],body:String::new()}
        } else {
            let result=match method {
                "initialize"=>serde_json::json!({"protocolVersion":"2025-03-26","capabilities":{"tools":{}},"serverInfo":{"name":"test","version":"1"}}),
                "tools/list"=>serde_json::json!({"tools":[{"name":"echo","description":"Echo","inputSchema":{"type":"object","properties":{}}}]}),
                "tools/call"=>serde_json::json!({"content":[{"type":"text","text":"done"}]}),
                _=>panic!("unexpected method {method}"),
            };
            HttpResponse {status:200,headers:vec![("Mcp-Session-Id".into(),"bounded-session".into())],
                body:serde_json::json!({"jsonrpc":"2.0","id":rpc["id"],"result":result}).to_string()}
        };
        self.pending=Some(response);self.ticks=0;Poll::Pending
    }
    fn poll_request(&mut self)->Poll<Result<HttpResponse,String>> {
        self.ticks+=1;if self.ticks<2 {return Poll::Pending;}
        Poll::Ready(self.pending.take().ok_or("no request".into()))
    }
    fn cancel_request(&mut self) {self.pending=None;self.trace.borrow_mut().cancelled+=1;}
}
fn app(trace:Rc<RefCell<Trace>>)->RemoteMcpApp {
    let mut client=HttpClient::new(Rc::new(RefCell::new(BTreeMap::new())));
    client.set_transport(Box::new(Cooperative{trace,pending:None,ticks:0}));
    RemoteMcpApp::new(AppManifest::new_remote("remote","Remote","1",
        vec![AppCapability::ExposeMcpTools,AppCapability::ScopedNetwork],"https://example.com/mcp",Some("streamable-http".into())),
        vec![],vec![],vec![],vec![]).unwrap().with_http_client(Rc::new(RefCell::new(client)))
}
fn call(app:&mut RemoteMcpApp)->Result<McpValue,alloyos_mcp::McpError> {
    for _ in 0..32 {if let Poll::Ready(result)=app.poll_call_tool("remote.echo",&BTreeMap::new()) {return result;}}
    panic!("poll budget exhausted")
}
#[test]
fn real_poll_path_initializes_discovers_calls_and_reuses_negotiated_session() {
    let trace=Rc::new(RefCell::new(Trace::default()));let mut app=app(trace.clone());
    assert!(call(&mut app).is_ok());assert_eq!(app.connection_status(),RemoteConnectionStatus::Ready);
    assert_eq!(app.tools()[0].name,"echo");assert!(call(&mut app).is_ok());
    let trace=trace.borrow();
    let methods:Vec<_>=trace.requests.iter().map(|r|serde_json::from_str::<serde_json::Value>(&r.body).unwrap()["method"].as_str().unwrap().to_string()).collect();
    assert_eq!(methods,["tools/list","initialize","notifications/initialized","tools/list","tools/call","tools/call"]);
    for request in trace.requests.iter().skip(2) {
        assert!(request.headers.contains(&("MCP-Protocol-Version".into(),"2025-03-26".into())));
        assert!(request.headers.contains(&("Mcp-Session-Id".into(),"bounded-session".into())));
    }
}
#[test]
fn authorization_challenge_is_preserved_without_claiming_readiness() {
    let trace=Rc::new(RefCell::new(Trace{unauthorized:true,..Default::default()}));let mut app=app(trace.clone());
    assert!(call(&mut app).is_err());
    assert!(matches!(app.connection_status(),RemoteConnectionStatus::AuthorizationRequired{challenge} if challenge.contains("resource_metadata=")));
    assert_eq!(trace.borrow().requests.len(),1);
}
#[test]
fn expired_call_is_not_retried_and_reconnect_starts_without_stale_session() {
    let trace=Rc::new(RefCell::new(Trace{expire_call:true,..Default::default()}));let mut app=app(trace.clone());
    assert!(call(&mut app).is_err());
    assert_eq!(trace.borrow().requests.len(),5,"ambiguous call must never be replayed");
    trace.borrow_mut().expire_call=false;assert!(call(&mut app).is_ok());
    let trace=trace.borrow();let new_init=&trace.requests[5];
    assert!(!new_init.headers.iter().any(|(name,_)|name.eq_ignore_ascii_case("mcp-session-id")));
}
#[test]
fn cancellation_releases_pending_transport_and_binds_credential_to_app() {
    let trace=Rc::new(RefCell::new(Trace::default()));let mut app=app(trace.clone());
    app.set_credential_reference(Some("opaque_ref".into())).unwrap();
    assert!(app.poll_connect().is_pending());
    app.set_credential_reference(Some("opaque_ref".into())).unwrap();
    assert_eq!(trace.borrow().cancelled,0,"reattaching same reference must not cancel progress");
    app.cancel_pending_tool();assert_eq!(trace.borrow().cancelled,1);
    assert_eq!(app.connection_status(),RemoteConnectionStatus::Disconnected);
    assert!(app.poll_connect().is_pending());
    let trace=trace.borrow();let headers=&trace.requests.last().unwrap().headers;
    assert!(headers.contains(&("X-AlloyOS-App-Id".into(),"remote".into())));
    assert!(headers.contains(&("X-AlloyOS-Credential-Ref".into(),"opaque_ref".into())));
    assert!(!headers.iter().any(|(name,_)|name.eq_ignore_ascii_case("authorization")));
}

#[test]
fn current_revision_uses_per_request_metadata_without_legacy_session_handshake() {
    let trace=Rc::new(RefCell::new(Trace{modern:true,..Default::default()}));let mut app=app(trace.clone());
    assert!(call(&mut app).is_ok());
    let trace=trace.borrow();assert_eq!(trace.requests.len(),2);
    for (index,request) in trace.requests.iter().enumerate() {
        let rpc:serde_json::Value=serde_json::from_str(&request.body).unwrap();
        assert_eq!(rpc["params"]["_meta"]["io.modelcontextprotocol/protocolVersion"],"2026-07-28");
        assert!(rpc["params"]["_meta"]["io.modelcontextprotocol/clientCapabilities"].is_object());
        assert!(!request.headers.iter().any(|(name,_)|name.eq_ignore_ascii_case("mcp-session-id")));
        assert!(request.headers.contains(&("Mcp-Method".into(),if index==0 {"tools/list".into()}else{"tools/call".into()})));
    }
}

#[test]
fn trusted_os_bearer_is_transient_idempotent_and_removed_on_disconnect() {
    let trace=Rc::new(RefCell::new(Trace{modern:true,..Default::default()}));let mut app=app(trace.clone());
    assert!(app.set_trusted_bearer(Some("bad\r\ninjection".into())).is_err());
    app.set_trusted_bearer(Some("private-fixture-token".into())).unwrap();
    assert!(app.poll_connect().is_pending());
    app.set_trusted_bearer(Some("private-fixture-token".into())).unwrap();
    assert_eq!(trace.borrow().cancelled,0);
    let request=trace.borrow().requests.last().unwrap().clone();
    assert!(request.headers.contains(&("X-AlloyOS-Trusted-Bearer".into(),"private-fixture-token".into())));
    assert!(!request.body.contains("private-fixture-token"));
    assert!(!app.manifest().to_mcp_config().contains("private-fixture-token"));
    app.set_trusted_bearer(None).unwrap();
    assert_eq!(trace.borrow().cancelled,1);
    assert!(app.poll_connect().is_pending());
    assert!(!trace.borrow().requests.last().unwrap().headers.iter().any(|(n,_)|n.eq_ignore_ascii_case("x-alloyos-trusted-bearer")));
}

#[test]
fn oauth_route_requires_os_marker_tls_and_form_content_type() {
    use alloyos_mcp::transport::allowed_oauth_http;
    let mut request=HttpRequest{method:"POST",url:"https://accounts.example.com/token".into(),headers:vec![("Content-Type".into(),"application/x-www-form-urlencoded".into())],body:"grant_type=authorization_code&code=private".into()};
    assert!(!allowed_oauth_http(&request));
    request.headers.push(("X-AlloyOS-OAuth".into(),"1".into()));
    assert!(allowed_oauth_http(&request));
    request.headers.push(("Authorization".into(),"Bearer untrusted".into()));
    assert!(!allowed_oauth_http(&request));request.headers.pop();
    request.url="http://accounts.example.com/token".into();assert!(!allowed_oauth_http(&request));
    request.url="https://127.0.0.1/token".into();assert!(!allowed_oauth_http(&request));
}
