use alloyos_forge::{
    cloud::{ApiProvider, HttpRequest, HttpResponse, HttpsClient},
    provider::{AiError, Message},
    Forge, ForgeError, GenerationRequest, ModelGateway,
};
use serde_json::{json, Value};
use std::{
    cell::RefCell,
    rc::Rc,
    sync::{Arc, Mutex},
};
struct FixtureHttp {
    status: u16,
    response: String,
    requests: Arc<Mutex<Vec<(String, Value)>>>,
    provider: ApiProvider,
}
impl HttpsClient for FixtureHttp {
    fn post(&mut self, request: &HttpRequest) -> Result<HttpResponse, ForgeError> {
        let body: Value = serde_json::from_str(&request.body).unwrap();
        let (auth, name) = match self.provider {
            ApiProvider::Claude => ("fixture-key", "x-api-key"),
            ApiProvider::Gemini => ("fixture-key", "x-goog-api-key"),
            ApiProvider::OpenAi => ("Bearer fixture-key", "Authorization"),
        };
        assert!(request.headers.iter().any(|(n, v)| n == name && v == auth));
        assert!(!request.url.contains("fixture-key"));
        self.requests
            .lock()
            .unwrap()
            .push((request.url.clone(), body));
        Ok(HttpResponse {
            status: self.status,
            body: self.response.clone(),
        })
    }
}
fn reply(provider: ApiProvider) -> Value {
    match provider {
        ApiProvider::Claude => {
            json!({"type":"message","content":[{"type":"text","text":"Hello 東京"}],"usage":{"output_tokens":4}})
        }
        ApiProvider::Gemini => {
            json!({"candidates":[{"content":{"parts":[{"thought":true,"text":"private reasoning"},{"text":"Hello 東京"}]}}],"usageMetadata":{"candidatesTokenCount":4}})
        }
        ApiProvider::OpenAi => {
            json!({"status":"completed","output":[{"type":"reasoning"},{"type":"message","role":"assistant","content":[{"type":"output_text","text":"Hello 東京"}]}],"usage":{"output_tokens":4}})
        }
    }
}
#[test]
fn all_apis_execute_through_forge_and_gateway_with_explicit_selection() {
    let forge = Rc::new(RefCell::new(Forge::new(0)));
    let captured = Arc::new(Mutex::new(Vec::new()));
    for provider in [
        ApiProvider::Claude,
        ApiProvider::Gemini,
        ApiProvider::OpenAi,
    ] {
        forge
            .borrow_mut()
            .connect_api(
                provider,
                provider.default_model(),
                "fixture-key",
                Box::new(FixtureHttp {
                    status: 200,
                    response: reply(provider).to_string(),
                    requests: captured.clone(),
                    provider,
                }),
            )
            .unwrap();
    }
    assert_eq!(forge.borrow().default_model(), "gemma-2-2b-it");
    assert_eq!(
        forge
            .borrow()
            .models()
            .iter()
            .filter(|m| m.state == "ready")
            .count(),
        3
    );
    let mut gateway = ModelGateway::new();
    gateway.set_forge(forge);
    gateway.switch_provider("local").unwrap();
    assert!(matches!(
        gateway.chat(&[Message::user("hello")], &[], &mut |_| {}),
        Err(AiError::LocalInferenceError(_))
    ));
    assert!(captured.lock().unwrap().is_empty());
    for provider in [
        ApiProvider::Claude,
        ApiProvider::Gemini,
        ApiProvider::OpenAi,
    ] {
        gateway.switch_provider(provider.name()).unwrap();
        let mut streamed = String::new();
        let response = gateway
            .chat(
                &[Message::user("Quotes \" and 東京\nnew line")],
                &[],
                &mut |t| streamed.push_str(t),
            )
            .unwrap();
        assert_eq!(response.text, "Hello 東京");
        assert_eq!(response.text, streamed);
        assert_eq!(response.model_name, provider.default_model());
        assert_eq!(response.usage.completion_tokens, 4);
    }
    let requests = captured.lock().unwrap();
    assert_eq!(requests[0].0, "https://api.anthropic.com/v1/messages");
    assert_eq!(requests[0].1["max_tokens"], 128);
    assert!(requests[0].1["messages"][0]["content"]
        .as_str()
        .unwrap()
        .contains("東京"));
    assert!(requests[1].0.ends_with(":generateContent"));
    assert_eq!(requests[1].1["generationConfig"]["maxOutputTokens"], 128);
    assert_eq!(requests[2].0, "https://api.openai.com/v1/responses");
    assert_eq!(requests[2].1["max_output_tokens"], 128);
    assert_eq!(requests[2].1["store"], false);
}
#[test]
fn api_failures_never_turn_into_canned_text_or_leak_response_details() {
    for provider in [
        ApiProvider::Claude,
        ApiProvider::Gemini,
        ApiProvider::OpenAi,
    ] {
        for (status, body, expected) in [
            (401, "fixture-secret", ForgeError::AuthenticationFailed),
            (403, "fixture-secret", ForgeError::AuthenticationFailed),
            (429, "fixture-secret", ForgeError::RateLimited),
            (503, "fixture-secret", ForgeError::HttpStatus(503)),
            (200, "not JSON fixture-secret", ForgeError::InvalidResponse),
            (200, "{}", ForgeError::InvalidResponse),
        ] {
            let mut forge = Forge::new(0);
            forge
                .connect_api(
                    provider,
                    provider.default_model(),
                    "fixture-key",
                    Box::new(FixtureHttp {
                        status,
                        response: body.into(),
                        requests: Arc::new(Mutex::new(Vec::new())),
                        provider,
                    }),
                )
                .unwrap();
            let error = forge
                .generate(&GenerationRequest {
                    model: Some(provider.default_model().into()),
                    prompt: "hello".into(),
                    max_tokens: 128,
                })
                .unwrap_err();
            assert_eq!(error, expected);
            assert!(!error.to_string().contains("fixture-secret"));
        }
    }
}
#[test]
fn rejects_bad_credentials_and_impossible_usage_counts() {
    for key in ["", "secret\r\nInjected: yes"] {
        let mut forge = Forge::new(0);
        let provider = ApiProvider::OpenAi;
        assert_eq!(
            forge.connect_api(
                provider,
                provider.default_model(),
                key,
                Box::new(FixtureHttp {
                    status: 200,
                    response: reply(provider).to_string(),
                    requests: Arc::new(Mutex::new(Vec::new())),
                    provider
                })
            ),
            Err(ForgeError::InvalidRequest)
        );
        assert_eq!(forge.models().len(), 1);
    }
    let mut forge = Forge::new(0);
    let provider = ApiProvider::OpenAi;
    let mut body = reply(provider);
    body["usage"]["output_tokens"] = json!(999);
    forge
        .connect_api(
            provider,
            provider.default_model(),
            "fixture-key",
            Box::new(FixtureHttp {
                status: 200,
                response: body.to_string(),
                requests: Arc::new(Mutex::new(Vec::new())),
                provider,
            }),
        )
        .unwrap();
    assert_eq!(
        forge.generate(&GenerationRequest {
            model: Some(provider.default_model().into()),
            prompt: "hello".into(),
            max_tokens: 128
        }),
        Err(ForgeError::GenerationFailed)
    );
}

struct DelayedHttp {
    polls: usize,
    status: u16,
    starts: Arc<Mutex<usize>>,
}
impl HttpsClient for DelayedHttp {
    fn post(&mut self, _: &HttpRequest) -> Result<HttpResponse, ForgeError> {
        panic!("cooperative calls must never use blocking post")
    }
    fn start_post(&mut self, _: &HttpRequest) -> std::task::Poll<Result<HttpResponse, ForgeError>> {
        *self.starts.lock().unwrap() += 1;
        std::task::Poll::Pending
    }
    fn poll_post(&mut self) -> std::task::Poll<Result<HttpResponse, ForgeError>> {
        self.polls += 1;
        if self.polls < 120 {
            return std::task::Poll::Pending;
        }
        std::task::Poll::Ready(Ok(HttpResponse {
            status: self.status,
            body: reply(ApiProvider::OpenAi).to_string(),
        }))
    }
}

#[test]
fn delayed_cloud_jobs_yield_without_resending_and_release_on_success_or_failure() {
    use std::task::Poll;
    for status in [200, 429] {
        let starts = Arc::new(Mutex::new(0));
        let mut forge = Forge::new(0);
        forge
            .connect_api(
                ApiProvider::OpenAi,
                ApiProvider::OpenAi.default_model(),
                "fixture",
                Box::new(DelayedHttp {
                    polls: 0,
                    status,
                    starts: starts.clone(),
                }),
            )
            .unwrap();
        let request = GenerationRequest {
            model: Some(ApiProvider::OpenAi.default_model().into()),
            prompt: "Hello".into(),
            max_tokens: 32,
        };
        let job = forge.start_generation(&request).unwrap();
        assert!(matches!(
            forge.start_generation(&request),
            Err(ForgeError::Busy)
        ));
        assert!(matches!(
            forge.poll_generation(job + 1),
            Poll::Ready(Err(ForgeError::InvalidRequest))
        ));
        assert!(forge.has_pending_generation());
        for _ in 0..119 {
            assert!(forge.poll_generation(job).is_pending());
        }
        let Poll::Ready(result) = forge.poll_generation(job) else {
            panic!("reply missing");
        };
        if status == 200 {
            assert_eq!(result.unwrap().text, "Hello 東京");
        } else {
            assert!(matches!(result, Err(ForgeError::RateLimited)));
        }
        assert!(!forge.has_pending_generation());
        assert_eq!(*starts.lock().unwrap(), 1);
        assert!(forge.start_generation(&request).is_ok());
    }
}

struct StreamingSseHttp {
    chunks: Vec<String>,
    cursor: usize,
    accumulated: String,
}

impl HttpsClient for StreamingSseHttp {
    fn post(&mut self, _: &HttpRequest) -> Result<HttpResponse, ForgeError> {
        panic!("streaming test must use cooperative start_post/poll_post")
    }
    fn start_post(&mut self, _: &HttpRequest) -> std::task::Poll<Result<HttpResponse, ForgeError>> {
        self.cursor = 0;
        self.accumulated.clear();
        std::task::Poll::Pending
    }
    fn poll_post(&mut self) -> std::task::Poll<Result<HttpResponse, ForgeError>> {
        if self.cursor < self.chunks.len() {
            self.accumulated.push_str(&self.chunks[self.cursor]);
            self.cursor += 1;
            if self.cursor < self.chunks.len() {
                return std::task::Poll::Pending;
            }
        }
        std::task::Poll::Ready(Ok(HttpResponse {
            status: 200,
            body: self.accumulated.clone(),
        }))
    }
    fn partial_response_body(&self) -> Option<&str> {
        if self.accumulated.is_empty() {
            None
        } else {
            Some(&self.accumulated)
        }
    }
}

fn sse_chunks(provider: ApiProvider) -> Vec<String> {
    match provider {
        ApiProvider::Claude => vec![
            "event: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"delta\":{\"text\":\"Hello\"}}\n\n".into(),
            "event: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"delta\":{\"text\":\" alloyOS\"}}\n\n".into(),
            "event: message_delta\ndata: {\"type\":\"message_delta\",\"delta\":{\"text\":\"!\"},\"usage\":{\"output_tokens\":3}}\n\n".into(),
        ],
        ApiProvider::Gemini => vec![
            "data: {\"candidates\":[{\"content\":{\"parts\":[{\"thought\":true,\"text\":\"hidden\"},{\"text\":\"Hello\"}]}}]}\n\n".into(),
            "data: {\"candidates\":[{\"content\":{\"parts\":[{\"text\":\" alloyOS\"}]}}]}\n\n".into(),
            "data: {\"candidates\":[{\"content\":{\"parts\":[{\"text\":\"!\"}]}}],\"usageMetadata\":{\"candidatesTokenCount\":3}}\n\n".into(),
        ],
        ApiProvider::OpenAi => vec![
            "data: {\"type\":\"response.output_text.delta\",\"delta\":\"Hello\"}\n\n".into(),
            "data: {\"type\":\"response.output_text.delta\",\"delta\":\" alloyOS\"}\n\n".into(),
            "data: {\"type\":\"response.output_text.delta\",\"delta\":\"!\",\"usage\":{\"output_tokens\":3}}\n\ndata: [DONE]\n\n".into(),
        ],
    }
}

#[test]
fn cloud_sse_streams_deliver_incremental_partial_text_and_gateway_deltas() {
    use std::task::Poll;
    for provider in [
        ApiProvider::Claude,
        ApiProvider::Gemini,
        ApiProvider::OpenAi,
    ] {
        let mut forge = Forge::new(0);
        forge
            .connect_api(
                provider,
                provider.default_model(),
                "fixture-key",
                Box::new(StreamingSseHttp {
                    chunks: sse_chunks(provider),
                    cursor: 0,
                    accumulated: String::new(),
                }),
            )
            .unwrap();
        let req = GenerationRequest {
            model: Some(provider.default_model().into()),
            prompt: "Stream test".into(),
            max_tokens: 32,
        };
        let job = forge.start_generation(&req).unwrap();
        assert_eq!(forge.partial_text(job), None);

        assert!(forge.poll_generation(job).is_pending());
        assert_eq!(forge.partial_text(job), Some("Hello"));

        assert!(forge.poll_generation(job).is_pending());
        assert_eq!(forge.partial_text(job), Some("Hello alloyOS"));

        let Poll::Ready(Ok(done)) = forge.poll_generation(job) else {
            panic!("expected third poll to finish SSE stream");
        };
        assert_eq!(done.text, "Hello alloyOS!");
        assert_eq!(done.generated_tokens, 3);

        // Also verify ModelGateway::chat emits live incremental token deltas via on_token
        let shared_forge = Rc::new(RefCell::new(forge));
        let mut gateway = ModelGateway::new();
        gateway.set_forge(shared_forge);
        gateway.switch_provider(provider.name()).unwrap();

        let mut deltas = Vec::new();
        let response = gateway
            .chat(&[Message::user("Stream via gateway")], &[], &mut |delta| {
                deltas.push(delta.to_string())
            })
            .unwrap();
        assert_eq!(
            deltas,
            vec!["Hello".to_string(), " alloyOS".to_string(), "!".to_string()]
        );
        assert_eq!(response.text, "Hello alloyOS!");
        assert_eq!(response.usage.completion_tokens, 3);
    }
}

#[test]
fn api_error_messages_reach_gateway_and_protocol_with_provider_context() {
    for provider in [
        ApiProvider::Gemini,
        ApiProvider::OpenAi,
        ApiProvider::Claude,
    ] {
        let forge = Rc::new(RefCell::new(Forge::new(0)));
        forge.borrow_mut().connect_api(provider, provider.default_model(), "fixture-key", Box::new(FixtureHttp {
            status: 429,
            response: json!({"error": {"message": "Quota exhausted; retry in 30s. fixture-key", "code": "insufficient_quota"}}).to_string(),
            requests: Arc::new(Mutex::new(Vec::new())),
            provider,
        })).unwrap();
        let error = forge
            .borrow_mut()
            .generate(&GenerationRequest {
                model: Some(provider.default_model().into()),
                prompt: "hello".into(),
                max_tokens: 128,
            })
            .unwrap_err();
        assert_eq!(error.code(), "rate_limited");
        let message = error.to_string();
        assert!(message.contains(provider.name()));
        assert!(message.contains(provider.default_model()));
        assert!(message.contains("HTTP 429: Quota exhausted; retry in 30s."));
        assert!(!message.contains("fixture-key"));
        let wire = alloyos_forge::protocol::response(1, Err(error));
        let payload = wire.trim_end().split('\t').nth(4).unwrap();
        assert_eq!(
            alloyos_forge::protocol::decode_hex(payload).unwrap(),
            message
        );
        let mut gateway = ModelGateway::new();
        gateway.set_forge(forge);
        gateway.switch_provider(provider.name()).unwrap();
        let error = gateway
            .chat(&[Message::user("hello")], &[], &mut |_| {})
            .unwrap_err();
        assert!(format!("{error:?}").contains(&message));
    }
}
