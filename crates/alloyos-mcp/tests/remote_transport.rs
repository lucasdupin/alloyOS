use alloyos_mcp::transport::{HttpRequest, HttpResponse, HttpTransport};
use alloyos_mcp::{
    AppCapability, AppManifest, McpRegistry, McpValue, PackageService, PackageSnapshot,
    RemoteMcpApp,
};
use std::{cell::RefCell, collections::BTreeMap, rc::Rc};

#[derive(Default)]
struct Requests {
    bodies: Vec<McpValue>,
    headers: Vec<Vec<(String, String)>>,
    fail_id: bool,
    sse: bool,
}
struct RecordingTransport(Rc<RefCell<Requests>>);
impl HttpTransport for RecordingTransport {
    fn request(&mut self, request: &HttpRequest) -> Result<HttpResponse, String> {
        assert_eq!(request.method, "POST");
        assert_eq!(request.url, "https://example.com/mcp");
        let body = McpValue::from_json(&request.body).unwrap();
        let map = body.as_map().unwrap();
        let method = map["method"].as_str().unwrap();
        let mut state = self.0.borrow_mut();
        state.bodies.push(body.clone());
        state.headers.push(request.headers.clone());
        if method == "notifications/initialized" {
            assert!(!map.contains_key("id"));
            return Ok(HttpResponse {
                status: 202,
                headers: Vec::new(),
                body: String::new(),
            });
        }
        let id = map["id"].as_i64().unwrap();
        let result = if method == "initialize" {
            r#"{"protocolVersion":"2025-06-18","capabilities":{"tools":{}},"serverInfo":{"name":"fixture","version":"1"}}"#
        } else if method == "resources/read" {
            assert_eq!(
                map["params"].as_map().unwrap()["uri"].as_str(),
                Some("file:///remote/data.txt")
            );
            r#"{"contents":[{"uri":"file:///remote/data.txt","mimeType":"text/plain","text":"remote data"}]}"#
        } else if method == "prompts/get" {
            let params = map["params"].as_map().unwrap();
            assert_eq!(params["name"].as_str(), Some("summarize"));
            assert_eq!(
                params["arguments"].as_map().unwrap()["text"].as_str(),
                Some("hello")
            );
            r#"{"messages":[{"role":"user","content":{"type":"text","text":"summary"}}]}"#
        } else {
            r#"{"content":[{"type":"text","text":"first"},{"type":"text","text":"second"}],"structuredContent":{"count":2,"decimal":1.25,"large":18446744073709551616,"emoji":"\ud83d\ude00"},"isError":true}"#
        };
        let id = if state.fail_id && method != "initialize" {
            id + 1
        } else {
            id
        };
        let response = format!(r#"{{"jsonrpc":"2.0","id":{id},"result":{result}}}"#);
        let body = if state.sse && method != "initialize" {
            format!("event: message\r\ndata: {{\"jsonrpc\":\"2.0\",\"method\":\"notifications/message\"}}\r\n\r\ndata: {response}\r\n\r\n")
        } else {
            response
        };
        Ok(HttpResponse {
            status: 200,
            headers: vec![("Mcp-Session-Id".into(), "session-123".into())],
            body,
        })
    }
}

fn snapshot() -> PackageSnapshot {
    PackageSnapshot { files: BTreeMap::from([
        ("/apps/remote/remote.mcp".into(), "name=Remote\nversion=1\nendpoint=https://example.com/mcp\ntransport=streamable-http\ncapabilities=ExposeMcpTools,ScopedNetwork\n".into()),
        ("/apps/remote/tools.json".into(), r#"[{"name":"echo","inputSchema":{"type":"object","properties":{"payload":{"type":"object","properties":{"title":{"type":"string"}}}},"required":["payload"]}}]"#.into()),
    ]), ..PackageSnapshot::default() }
}

#[test]
fn actual_requests_keep_args_sessions_and_complete_results() {
    let state = Rc::new(RefCell::new(Requests::default()));
    let mut registry = McpRegistry::new();
    registry.set_http_transport(Box::new(RecordingTransport(state.clone())));
    let snapshot = snapshot();
    registry
        .discover_and_register_packages(&snapshot, None)
        .unwrap();
    assert!(registry.list_tools()[0].input_schema.is_some());
    assert!(registry.list_tools()[0].parameters[0].required);
    let args = McpValue::from_json(r#"{"payload":{"title":"quoted \"title\" 雪","value":1.25}}"#)
        .unwrap()
        .as_map()
        .unwrap()
        .clone();
    let result = registry.call_tool("remote.echo", &args).unwrap();
    assert_eq!(
        result.as_map().unwrap()["content"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
    assert_eq!(result.as_map().unwrap()["isError"].as_bool(), Some(true));
    let structured = result.as_map().unwrap()["structuredContent"]
        .as_map()
        .unwrap();
    assert_eq!(structured["decimal"].to_json(), "1.25");
    assert_eq!(structured["large"].to_json(), "18446744073709551616");
    assert_eq!(structured["emoji"].as_str(), Some("😀"));
    registry
        .discover_and_register_packages(&snapshot, None)
        .unwrap();
    registry.call_tool("remote.echo", &args).unwrap();
    let state = state.borrow();
    assert_eq!(
        state.bodies.len(),
        4,
        "an unchanged package must reuse its initialized session"
    );
    let request = state.bodies[2].as_map().unwrap();
    let params = request["params"].as_map().unwrap();
    assert_eq!(params["name"].as_str(), Some("echo"));
    assert_eq!(params["arguments"], McpValue::Map(args));
    assert_eq!(state.bodies[3].as_map().unwrap()["id"].as_i64(), Some(3));
    for headers in &state.headers[1..] {
        assert!(headers.contains(&("Mcp-Session-Id".into(), "session-123".into())));
        assert!(headers.contains(&("MCP-Protocol-Version".into(), "2025-06-18".into())));
    }
    drop(state);
    assert!(registry
        .call_tool("remote.unknown", &BTreeMap::new())
        .is_err());
}

#[test]
fn sse_is_supported_and_mismatched_response_ids_are_errors() {
    for fail_id in [false, true] {
        let state = Rc::new(RefCell::new(Requests {
            sse: true,
            fail_id,
            ..Requests::default()
        }));
        let mut registry = McpRegistry::new();
        registry.set_http_transport(Box::new(RecordingTransport(state)));
        registry
            .discover_and_register_packages(&snapshot(), None)
            .unwrap();
        assert_eq!(
            registry.call_tool("remote.echo", &BTreeMap::new()).is_err(),
            fail_id
        );
    }
}

#[test]
fn malformed_metadata_and_endpoints_never_become_ready() {
    for tools in [
        "{}",
        "not-json",
        r#"[{"name":"missing_schema"}]"#,
        r#"[{"name":"x","inputSchema":null}]"#,
    ] {
        let mut snapshot = snapshot();
        snapshot
            .files
            .insert("/apps/remote/tools.json".into(), tools.into());
        let mut service = PackageService::new();
        service.apply_snapshot(snapshot);
        assert!(!service.status("remote").unwrap().ready);
    }
    for endpoint in [
        "https://",
        "https://example.com\ncapabilities=ShellExecution",
        "https://user:secret@example.com",
        "https://example.com:bad/mcp",
        "https://example.com/#fragment",
    ] {
        let manifest = AppManifest::new_remote(
            "remote",
            "Remote",
            "1",
            vec![AppCapability::ExposeMcpTools, AppCapability::ScopedNetwork],
            endpoint,
            Some("remote".into()),
        );
        assert!(
            RemoteMcpApp::new(manifest, Vec::new(), Vec::new(), Vec::new(), Vec::new()).is_err()
        );
    }
}

#[test]
fn remote_resource_aliases_and_prompt_names_are_translated() {
    let mut snapshot = snapshot();
    snapshot
        .files
        .get_mut("/apps/remote/remote.mcp")
        .unwrap()
        .push_str(
            "capabilities=ExposeMcpTools,ExposeMcpResources,ExposeMcpPrompts,ScopedNetwork\n",
        );
    snapshot.files.insert(
        "/apps/remote/resources.json".into(),
        r#"[{"uri":"file:///remote/data.txt","name":"Data","mimeType":"text/plain"}]"#.into(),
    );
    snapshot.files.insert(
        "/apps/remote/prompts.json".into(),
        r#"[{"name":"summarize","arguments":[{"name":"text","required":true}]}]"#.into(),
    );
    let state = Rc::new(RefCell::new(Requests::default()));
    let mut registry = McpRegistry::new();
    registry.set_http_transport(Box::new(RecordingTransport(state)));
    registry
        .discover_and_register_packages(&snapshot, None)
        .unwrap();
    let resource = &registry.list_resources()[0];
    assert_eq!(resource.uri, "mcp://remote/resources/0");
    let content = registry.read_resource(&resource.uri).unwrap();
    assert_eq!(content.uri, resource.uri);
    assert_eq!(content.text, "remote data");
    let prompt = registry
        .get_prompt(
            "remote.summarize",
            &BTreeMap::from([("text".into(), "hello".into())]),
        )
        .unwrap();
    assert!(prompt.contains("summary"));
    assert!(registry
        .read_resource("mcp://remote/resources/100")
        .is_err());
    assert!(registry
        .get_prompt("remote.unknown", &BTreeMap::new())
        .is_err());
}
