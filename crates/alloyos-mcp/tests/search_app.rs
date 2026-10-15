use alloyos_mcp::{McpCaller, McpRegistry, McpValue, PackageSnapshot};
use alloyos_mcp::transport::{HttpRequest, HttpResponse, HttpTransport};
use std::{cell::RefCell, collections::BTreeMap, rc::Rc, task::Poll};

const GEMINI_URL: &str = "https://generativelanguage.googleapis.com/v1beta/models/gemini-3.8-flash:generateContent";
const GEMINI_RESPONSE: &str = r#"{
    "candidates": [
        {
            "content": {
                "parts": [{"text": "Found results for query."}],
                "role": "model"
            },
            "finishReason": "STOP",
            "groundingMetadata": {
                "groundingChunks": [
                    {
                        "web": {
                            "uri": "https://example.org/?a=1&b=2",
                            "title": "Rust & Café",
                            "snippet": "A useful \"guide\"."
                        }
                    },
                    {
                        "web": {
                            "uri": "https://example.net/",
                            "title": "Second result",
                            "snippet": "Another source."
                        }
                    },
                    {
                        "web": {
                            "uri": "https://example.net/",
                            "title": "Duplicate",
                            "snippet": "Duplicate source."
                        }
                    }
                ],
                "webSearchQueries": ["rust & café"]
            }
        }
    ]
}"#;

fn snapshot() -> PackageSnapshot {
    let mut result = PackageSnapshot::default();
    result.directories.push("/apps/search".into());
    for name in ["search.mcp", "main.js", "tools.json", "resources.json", "SKILL.md"] {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../apps/search").join(name);
        result.files.insert(format!("/apps/search/{name}"), std::fs::read_to_string(path).unwrap().replacen("\"__GEMINI_API_KEY__\"", "\"fixture-key\"", 1));
    }
    result
}
fn registry() -> McpRegistry {
    let mut registry = McpRegistry::new();
    let installed = registry.discover_and_register_packages(&snapshot(), None).unwrap();
    let mut service = alloyos_mcp::PackageService::new(); service.scan_startup(snapshot());
    assert_eq!(installed, 1, "{:?}", service.inventory());
    registry
}
fn args() -> BTreeMap<String, McpValue> {
    BTreeMap::from([("query".into(), McpValue::Text("rust & café".into()))])
}
fn json(value: McpValue) -> serde_json::Value { serde_json::from_str(&value.to_json()).unwrap() }
fn call(registry: &mut McpRegistry, name: &str, args: &BTreeMap<String, McpValue>) -> serde_json::Value {
    json(registry.call_tool(name, args).unwrap())
}

#[test]
fn search_parses_links_snippets_and_retains_only_latest_success() {
    let mut registry = registry();
    assert!(registry.list_tiles().is_empty());
    registry.register_http_response(GEMINI_URL, GEMINI_RESPONSE);
    let result = call(&mut registry, "search.web", &args());
    assert_eq!(result["ok"], true);
    assert_eq!(result["presentation"], "text");
    assert_eq!(result["provider"], "Google");
    assert_eq!(result["cached"], false);
    assert_eq!(result["webSearchQueries"][0], "rust & café");
    assert_eq!(result["results"].as_array().unwrap().len(), 2);
    assert_eq!(result["results"][0]["url"], "https://example.org/?a=1&b=2");
    assert_eq!(result["results"][0]["title"], "Rust & Café");
    assert_eq!(result["results"][0]["snippet"], "A useful \"guide\".");
    assert_eq!(result["results"][1]["snippet"], "Another source.");
    let cached = call(&mut registry, "search.latest", &BTreeMap::new());
    assert_eq!(cached["cached"], true);
    assert_eq!(cached["results"], result["results"]);
    let resource = registry.read_resource("mcp://search/latest").unwrap();
    assert_eq!(serde_json::from_str::<serde_json::Value>(&resource.text).unwrap()["cached"], true);
    registry.register_http_response(GEMINI_URL, r#"{"error":{"message":"API quota exceeded"}}"#);
    assert_eq!(call(&mut registry, "search.web", &args())["status"], "api_error");
    assert_eq!(call(&mut registry, "search.latest", &BTreeMap::new())["results"], result["results"]);
    call(&mut registry, "search.clear", &BTreeMap::new());
    assert_eq!(call(&mut registry, "search.latest", &BTreeMap::new())["status"], "no_search");
}

#[test]
fn search_validates_arguments_distinguishes_empty_and_malformed_responses() {
    let mut registry = registry();
    for query in [McpValue::Text("  ".into()), McpValue::Text("x".repeat(501)), McpValue::Int(4)] {
        let invalid = BTreeMap::from([("query".into(), query)]);
        assert_eq!(call(&mut registry, "search.web", &invalid)["status"], "invalid_arguments");
    }
    for limit in [McpValue::Int(0), McpValue::Int(11), McpValue::Text("5".into())] {
        let mut invalid = args(); invalid.insert("limit".into(), limit);
        assert_eq!(call(&mut registry, "search.web", &invalid)["status"], "invalid_arguments");
    }
    registry.register_http_response(GEMINI_URL, "not valid json");
    assert_eq!(call(&mut registry, "search.web", &args())["status"], "parse_error");
    registry.register_http_response(GEMINI_URL, r#"{"candidates":[]}"#);
    let empty = call(&mut registry, "search.web", &args());
    assert_eq!(empty["status"], "empty"); assert_eq!(empty["ok"], true);
    registry.register_http_response(GEMINI_URL, GEMINI_RESPONSE);
    let mut one = args(); one.insert("limit".into(), McpValue::Int(1));
    assert_eq!(call(&mut registry, "search.web", &one)["results"].as_array().unwrap().len(), 1);
}

#[derive(Default)]
struct State { starts: usize, polls: usize, cancels: usize, fail: bool }
struct Delayed(Rc<RefCell<State>>);
impl HttpTransport for Delayed {
    fn request(&mut self, _: &HttpRequest) -> Result<HttpResponse, String> { panic!("must not block") }
    fn start_request(&mut self, request: &HttpRequest) -> Poll<Result<HttpResponse, String>> {
        assert_eq!(request.url, GEMINI_URL);
        assert_eq!(request.method, "POST");
        assert!(request.headers.contains(&("x-goog-api-key".into(), "fixture-key".into())));
        assert!(request.body.contains("google_search"));
        self.0.borrow_mut().starts += 1; Poll::Pending
    }
    fn poll_request(&mut self) -> Poll<Result<HttpResponse, String>> {
        let mut state = self.0.borrow_mut(); state.polls += 1;
        if state.fail { Poll::Ready(Err("offline".into())) }
        else { Poll::Ready(Ok(HttpResponse {status: 200, headers: vec![], body: GEMINI_RESPONSE.into()})) }
    }
    fn cancel_request(&mut self) { self.0.borrow_mut().cancels += 1; }
}

#[test]
fn live_app_posts_are_cooperative_fresh_cancellable_and_fail_without_stale_fallback() {
    let mut registry = registry();
    let state = Rc::new(RefCell::new(State::default()));
    registry.set_http_transport(Box::new(Delayed(state.clone())));
    let who = McpCaller::Welder;
    assert!(registry.poll_call_tool_as(&who, "search.web", &args()).is_pending());
    assert_eq!(call(&mut registry, "search.latest", &BTreeMap::new())["status"], "no_search");
    assert!(registry.poll_call_tool_as(&McpCaller::App("other".into()), "search.web", &args()).is_ready());
    let Poll::Ready(Ok(result)) = registry.poll_call_tool_as(&who, "search.web", &args()) else { panic!("search not completed") };
    assert_eq!(json(result)["results"][0]["title"], "Rust & Café");
    assert!(registry.poll_call_tool_as(&who, "search.web", &args()).is_pending());
    registry.cancel_pending_tools();
    assert_eq!(state.borrow().cancels, 1);
    assert!(registry.poll_call_tool_as(&who, "search.web", &args()).is_pending());
    state.borrow_mut().fail = true;
    let Poll::Ready(Err(error)) = registry.poll_call_tool_as(&who, "search.web", &args()) else { panic!("expected network error") };
    assert!(error.message.contains("offline"));
    assert_eq!(state.borrow().starts, 3);
    assert_eq!(call(&mut registry, "search.latest", &BTreeMap::new())["cached"], true);
}

#[test]
fn public_search_relay_rejects_arbitrary_destinations_and_extra_parameters() {
    use alloyos_mcp::transport::{allowed_public_get, allowed_public_post};
    assert!(allowed_public_get("https://registry.modelcontextprotocol.io/v0.1/servers?search=rust"));
    assert!(allowed_public_post(GEMINI_URL));
    for url in [
        "http://html.duckduckgo.com/html/?q=rust",
        "https://html.duckduckgo.com.evil.test/html/?q=rust",
        "https://html.duckduckgo.com/html/?q=x&api_key=secret",
        "https://generativelanguage.googleapis.com/v1beta/models/../evil:generateContent",
    ] {
        assert!(!allowed_public_get(url) && !allowed_public_post(url), "accepted {url}");
    }
}

#[test]
fn oversized_post_is_rejected_before_starting_transport() {
    let state = Rc::new(RefCell::new(State::default()));
    let mut client = alloyos_mcp::transport::HttpClient::new(Rc::new(RefCell::new(BTreeMap::new())));
    client.set_transport(Box::new(Delayed(state.clone())));
    let body = "x".repeat(alloyos_mcp::transport::MAX_HTTP_BODY_BYTES + 1);
    let Poll::Ready(Err(error)) = client.start_post(GEMINI_URL, &body, &[]) else {
        panic!("oversized request must fail immediately");
    };
    assert!(error.message.contains("body exceeds limit"));
    assert_eq!(state.borrow().starts, 0);
}

#[test]
fn missing_build_key_fails_without_network_or_cached_fallback() {
    let mut package = snapshot();
    let source = package.files.get_mut("/apps/search/main.js").unwrap();
    *source = source.replacen("\"fixture-key\"", "\"\"", 1);
    let mut registry = McpRegistry::new();
    registry.discover_and_register_packages(&package, None).unwrap();
    let state = Rc::new(RefCell::new(State::default()));
    registry.set_http_transport(Box::new(Delayed(state.clone())));
    let Poll::Ready(Ok(result)) = registry.poll_call_tool_as(&McpCaller::Welder, "search.web", &args()) else {
        panic!("missing key must fail immediately");
    };
    assert_eq!(json(result)["status"], "not_configured");
    assert_eq!(state.borrow().starts, 0);
}
