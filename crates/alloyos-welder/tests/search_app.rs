use alloyos_forge::provider::ToolCall;
use alloyos_mcp::transport::{HttpRequest, HttpResponse, HttpTransport};
use alloyos_welder::{AgentUiSynthesizer, NativeToolRegistry, SharedMapVfsBackend};
use std::{cell::RefCell, rc::Rc, task::Poll};

fn registry() -> (NativeToolRegistry, Rc<RefCell<SharedMapVfsBackend>>) {
    let backend = SharedMapVfsBackend::shared();
    for name in ["search.mcp", "main.js", "tools.json", "resources.json", "SKILL.md"] {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../apps/search").join(name);
        backend.borrow_mut().files.insert(format!("/apps/search/{name}"), std::fs::read_to_string(path).unwrap().replacen("\"__GEMINI_API_KEY__\"", "\"fixture-key\"", 1));
    }
    let mut registry = NativeToolRegistry::with_vfs_backend(backend.clone());
    registry.refresh_app_packages().unwrap();
    (registry, backend)
}
const GEMINI_URL: &str = "https://generativelanguage.googleapis.com/v1beta/models/gemini-3.8-flash:generateContent";
const GEMINI_RESPONSE: &str = r#"{
    "candidates": [
        {
            "content": {
                "parts": [{"text": "Rust"}],
                "role": "model"
            },
            "finishReason": "STOP",
            "groundingMetadata": {
                "groundingChunks": [
                    {
                        "web": {
                            "uri": "https://rust-lang.org/",
                            "title": "Rust",
                            "snippet": "A language."
                        }
                    }
                ],
                "webSearchQueries": ["rust"]
            }
        }
    ]
}"#;

struct Delayed;
impl HttpTransport for Delayed {
    fn request(&mut self, _: &HttpRequest) -> Result<HttpResponse, String> { panic!("must poll") }
    fn start_request(&mut self, request: &HttpRequest) -> Poll<Result<HttpResponse, String>> {
        assert_eq!(request.method, "POST");
        assert!(request.headers.contains(&("x-goog-api-key".into(), "fixture-key".into())));
        assert!(request.body.contains("google_search"));
        assert_eq!(request.url, GEMINI_URL);
        Poll::Pending
    }
    fn poll_request(&mut self) -> Poll<Result<HttpResponse, String>> {
        Poll::Ready(Ok(HttpResponse {status:200, headers:vec![], body: GEMINI_RESPONSE.into()}))
    }
}
#[test]
fn search_completes_between_frames_persists_results_and_creates_no_cards() {
    let (mut registry, backend) = registry();
    registry.set_http_transport(Box::new(Delayed));
    let mut ui = AgentUiSynthesizer::new();
    let call = ToolCall::new("s1", "mcp_call_tool", r#"{"app":"search","tool":"web","args":{"query":"rust"}}"#);
    assert!(registry.poll_execute_tool(&call, &mut ui).is_pending());
    assert!(!backend.borrow().files.contains_key("/var/search/latest.json"));
    let Poll::Ready(Ok(output)) = registry.poll_execute_tool(&call, &mut ui) else { panic!("search did not complete") };
    assert!(output.synthesized_view.is_none());
    let result: serde_json::Value = serde_json::from_str(&output.output_json).unwrap();
    assert_eq!(result["results"][0]["title"], "Rust");
    assert!(backend.borrow().files.contains_key("/var/search/latest.json"));
    let read = ToolCall::new("r1", "mcp_read_resource", r#"{"app":"search","uri":"mcp://search/latest"}"#);
    let output = registry.execute_tool(&read, &mut ui).unwrap();
    assert!(output.synthesized_view.is_none());
    assert_eq!(serde_json::from_str::<serde_json::Value>(&output.output_json).unwrap()["cached"], true);
    let clear = ToolCall::new("c1", "mcp_call_tool", r#"{"app":"search","tool":"clear","args":{}}"#);
    assert!(registry.execute_tool(&clear, &mut ui).unwrap().synthesized_view.is_none());
    assert!(!backend.borrow().files.contains_key("/var/search/latest.json"));
}
