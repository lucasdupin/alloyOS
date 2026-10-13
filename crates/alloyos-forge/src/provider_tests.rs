use crate::provider::*;
use crate::providers::*;
use alloc::{string::String, vec, vec::Vec};
#[test]
fn gemini_provider_request_and_sse_streaming_with_tool_call() {
    let mut gemini = GeminiProvider::new();
    assert_eq!(gemini.name(), "gemini");
    assert_eq!(gemini.model, "gemini-3.8-flash");
    assert!(gemini
        .endpoint_url()
        .contains("models/gemini-3.8-flash:streamGenerateContent?alt=sse"));

    let specs = vec![
        ToolSpec::new("fs_read", "Read file", Vec::new()),
        ToolSpec::new("script_run", "Run script", Vec::new()),
        ToolSpec::new("script_schedule", "Schedule script", Vec::new()),
    ];
    let messages = vec![
        Message::system("System prompt for alloyOS"),
        Message::user("Read /etc/ai.conf and summarize it"),
    ];

    let req_json = gemini
        .build_request_json(&messages, &specs)
        .expect("should build Gemini JSON request");
    assert!(req_json.contains("\"systemInstruction\""));
    assert!(req_json.contains("\"functionDeclarations\""));
    assert!(req_json.contains("\"fs_read\""));
    assert!(!req_json.contains("\"sh_exec\""));

    let sse_payload = concat!(
            "data: {\"candidates\":[{\"content\":{\"parts\":[{\"text\":\"Reading \"}]}}]}\n",
            "data: {\"candidates\":[{\"content\":{\"parts\":[{\"text\":\"config...\"}]}}]}\n",
            "data: {\"candidates\":[{\"content\":{\"parts\":[{\"functionCall\":{\"name\":\"fs_read\",\"args\":{\"path\":\"/etc/ai.conf\"}}}]}}]}\n"
        );
    gemini.queue_sse_stream(sse_payload);

    let mut streamed_tokens = Vec::new();
    let response = gemini
        .stream_chat(&messages, &specs, &mut |tok| {
            streamed_tokens.push(String::from(tok));
        })
        .expect("Gemini SSE stream should parse");

    assert_eq!(streamed_tokens, vec!["Reading ", "config..."]);
    assert_eq!(response.text, "Reading config...");
    assert_eq!(response.model_name, "gemini-3.8-flash");
    assert_eq!(response.tool_calls.len(), 1);
    assert_eq!(response.tool_calls[0].name, "fs_read");
    assert_eq!(
        response.tool_calls[0].arguments_json,
        "{\"path\":\"/etc/ai.conf\"}"
    );
}

#[test]
fn claude_provider_request_and_sse_streaming_with_tool_use() {
    let mut claude = ClaudeProvider::new();
    assert_eq!(claude.name(), "claude");
    assert_eq!(claude.model, "claude-sonnet-4-6");
    assert_eq!(
        claude.endpoint_url(),
        "https://api.anthropic.com/v1/messages"
    );

    let specs = vec![
        ToolSpec::new("fs_read", "Read file", Vec::new()),
        ToolSpec::new("script_run", "Run script", Vec::new()),
        ToolSpec::new("script_schedule", "Schedule script", Vec::new()),
    ];
    let messages = vec![
        Message::system("AlloyOS system context"),
        Message::user("Run heartbeat script"),
    ];

    let req_json = claude
        .build_request_json(&messages, &specs)
        .expect("should build Claude JSON request");
    assert!(req_json.contains("\"model\":\"claude-sonnet-4-6\""));
    assert!(req_json.contains("\"stream\":true"));
    assert!(req_json.contains("\"input_schema\""));
    assert!(req_json.contains("\"script_run\""));

    let sse_payload = concat!(
            "event: content_block_delta\n",
            "data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"Launching \"}}\n",
            "event: content_block_delta\n",
            "data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"heartbeat.js\"}}\n",
            "event: content_block_start\n",
            "data: {\"type\":\"content_block_start\",\"index\":1,\"content_block\":{\"type\":\"tool_use\",\"id\":\"toolu_99\",\"name\":\"script_run\",\"input\":{}}}\n",
            "event: content_block_delta\n",
            "data: {\"type\":\"content_block_delta\",\"index\":1,\"delta\":{\"type\":\"input_json_delta\",\"partial_json\":\"{\\\"path\\\":\\\"/scripts/\"}}\n",
            "event: content_block_delta\n",
            "data: {\"type\":\"content_block_delta\",\"index\":1,\"delta\":{\"type\":\"input_json_delta\",\"partial_json\":\"heartbeat.js\\\"}\"}}\n"
        );
    claude.queue_sse_stream(sse_payload);

    let mut tokens = Vec::new();
    let response = claude
        .stream_chat(&messages, &specs, &mut |tok| {
            tokens.push(String::from(tok));
        })
        .expect("Claude SSE stream should parse");

    assert_eq!(tokens, vec!["Launching ", "heartbeat.js"]);
    assert_eq!(response.text, "Launching heartbeat.js");
    assert_eq!(response.tool_calls.len(), 1);
    assert_eq!(response.tool_calls[0].id, "toolu_99");
    assert_eq!(response.tool_calls[0].name, "script_run");
    assert_eq!(
        response.tool_calls[0].arguments_json,
        "{\"path\":\"/scripts/heartbeat.js\"}"
    );
}

#[test]
fn chatgpt_provider_request_and_sse_streaming_with_done() {
    let mut chatgpt = ChatGptProvider::new();
    assert_eq!(chatgpt.name(), "chatgpt");
    assert_eq!(chatgpt.model, "gpt-4.1");
    assert_eq!(
        chatgpt.endpoint_url(),
        "https://api.openai.com/v1/chat/completions"
    );

    let specs = vec![
        ToolSpec::new("fs_read", "Read file", Vec::new()),
        ToolSpec::new("script_run", "Run script", Vec::new()),
        ToolSpec::new("script_schedule", "Schedule script", Vec::new()),
    ];
    let messages = vec![Message::user("Schedule cleanup script")];

    let req_json = chatgpt
        .build_request_json(&messages, &specs)
        .expect("should build ChatGPT JSON request");
    assert!(req_json.contains("\"model\":\"gpt-4.1\""));
    assert!(req_json.contains("\"stream\":true"));
    assert!(req_json.contains("\"type\":\"function\""));

    let sse_payload = concat!(
            "data: {\"choices\":[{\"delta\":{\"content\":\"Scheduling \"}}]}\n",
            "data: {\"choices\":[{\"delta\":{\"content\":\"task.\"}}]}\n",
            "data: {\"choices\":[{\"delta\":{\"tool_calls\":[{\"id\":\"call_abc\",\"function\":{\"name\":\"script_schedule\",\"arguments\":\"{\\\"path\\\":\\\"/scripts/heartbeat.js\\\",\"}}]}}]}\n",
            "data: {\"choices\":[{\"delta\":{\"tool_calls\":[{\"function\":{\"arguments\":\"\\\"trigger\\\":\\\"every:60s\\\"}\"}}]}}]}\n",
            "data: [DONE]\n",
            "data: {\"choices\":[{\"delta\":{\"content\":\"IGNORED_AFTER_DONE\"}}]}\n"
        );
    chatgpt.queue_sse_stream(sse_payload);

    let mut tokens = Vec::new();
    let response = chatgpt
        .stream_chat(&messages, &specs, &mut |tok| {
            tokens.push(String::from(tok));
        })
        .expect("ChatGPT SSE stream should parse");

    assert_eq!(tokens, vec!["Scheduling ", "task."]);
    assert_eq!(response.text, "Scheduling task.");
    assert_eq!(response.tool_calls.len(), 1);
    assert_eq!(response.tool_calls[0].id, "call_abc");
    assert_eq!(response.tool_calls[0].name, "script_schedule");
    assert_eq!(
        response.tool_calls[0].arguments_json,
        "{\"path\":\"/scripts/heartbeat.js\",\"trigger\":\"every:60s\"}"
    );
}
