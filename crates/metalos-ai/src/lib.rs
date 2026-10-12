#![cfg_attr(not(test), no_std)]

#[cfg(test)]
extern crate std;

extern crate alloc;

pub mod provider;
pub mod providers;
pub mod tools;
pub mod ui_synth;

pub use provider::{
    AiError, AiModelProvider, Message, Response, Role, TokenUsage, ToolCall, ToolParameter,
    ToolSpec,
};
pub use providers::{
    ActiveProvider, AiRuntime, ChatGptProvider, ClaudeProvider, ConsoleTurnOutput, GeminiProvider,
};
pub use tools::{
    is_forbidden_shell_tool, validate_console_input_no_shell, NativeToolRegistry,
    ToolExecutionOutput, SHELL_FREE_POLICY_MESSAGE,
};
pub use ui_synth::{
    AgentUiSynthesizer, SynthesizedUiView, UiCallerOrigin, UiMetric, UiScheduleEntry, UiTable,
    ViewKind,
};

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::string::String;
    use alloc::vec;
    use alloc::vec::Vec;

    #[test]
    fn gemini_provider_request_and_sse_streaming_with_tool_call() {
        let mut gemini = GeminiProvider::new();
        assert_eq!(gemini.name(), "gemini");
        assert_eq!(gemini.model, "gemini-2.5-pro");
        assert!(gemini
            .endpoint_url()
            .contains("models/gemini-2.5-pro:streamGenerateContent?alt=sse"));

        let registry = NativeToolRegistry::new();
        let messages = vec![
            Message::system("System prompt for metalOS"),
            Message::user("Read /etc/ai.conf and summarize it"),
        ];

        let req_json = gemini
            .build_request_json(&messages, registry.specs())
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
            .stream_chat(&messages, registry.specs(), &mut |tok| {
                streamed_tokens.push(String::from(tok));
            })
            .expect("Gemini SSE stream should parse");

        assert_eq!(streamed_tokens, vec!["Reading ", "config..."]);
        assert_eq!(response.text, "Reading config...");
        assert_eq!(response.model_name, "gemini-2.5-pro");
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
        assert_eq!(claude.model, "claude-sonnet-4-20250514");
        assert_eq!(claude.endpoint_url(), "https://api.anthropic.com/v1/messages");

        let registry = NativeToolRegistry::new();
        let messages = vec![
            Message::system("MetalOS system context"),
            Message::user("Run heartbeat script"),
        ];

        let req_json = claude
            .build_request_json(&messages, registry.specs())
            .expect("should build Claude JSON request");
        assert!(req_json.contains("\"model\":\"claude-sonnet-4-20250514\""));
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
            .stream_chat(&messages, registry.specs(), &mut |tok| {
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

        let registry = NativeToolRegistry::new();
        let messages = vec![Message::user("Schedule cleanup script")];

        let req_json = chatgpt
            .build_request_json(&messages, registry.specs())
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
            .stream_chat(&messages, registry.specs(), &mut |tok| {
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

    #[test]
    fn runtime_hot_swapping_and_etc_ai_conf_persistence() {
        let mut runtime = AiRuntime::new();
        assert_eq!(runtime.active_provider_name(), "gemini");
        assert_eq!(runtime.active_model_id(), "gemini-2.5-pro");

        // Switch to Claude via slash command
        let mut streamed = String::new();
        let out_claude = runtime
            .handle_console_input("/model claude", &mut |t| streamed.push_str(t))
            .expect("should switch to claude");
        assert_eq!(out_claude.active_provider, "claude");
        assert_eq!(out_claude.active_model, "claude-sonnet-4-20250514");
        assert!(out_claude.config_persisted);

        let saved_conf = runtime
            .tool_registry
            .vfs_read("/etc/ai.conf")
            .expect("/etc/ai.conf must exist");
        assert!(saved_conf.contains("provider=claude"));
        assert!(saved_conf.contains("model=claude-sonnet-4-20250514"));

        // Switch to ChatGPT via slash command
        let out_gpt = runtime
            .handle_console_input("/model chatgpt", &mut |_| {})
            .expect("should switch to chatgpt");
        assert_eq!(out_gpt.active_provider, "chatgpt");
        assert_eq!(out_gpt.active_model, "gpt-4.1");

        // Switch back to Gemini via slash command
        let out_gemini = runtime
            .handle_console_input("/model gemini", &mut |_| {})
            .expect("should switch to gemini");
        assert_eq!(out_gemini.active_provider, "gemini");
        assert_eq!(out_gemini.active_model, "gemini-2.5-pro");

        // Test loading a custom /etc/ai.conf into a fresh runtime
        let mut restored = AiRuntime::new();
        restored
            .load_config_str("provider=claude\nmodel=claude-sonnet-4-20250514\n")
            .expect("config should load");
        assert_eq!(restored.active_provider_name(), "claude");
        assert_eq!(restored.active_model_id(), "claude-sonnet-4-20250514");
    }

    #[test]
    fn shell_commands_and_tools_are_strictly_blocked() {
        let mut runtime = AiRuntime::new();

        // 1. Reject `!ls` and `!sh` shell escapes in console input
        for forbidden_input in ["!ls", "!sh", "!cat /etc/ai.conf", "sh", "bash -c ls", "sh_exec"]
        {
            let err = runtime
                .handle_console_input(forbidden_input, &mut |_| {})
                .expect_err("shell input must be rejected");
            match err {
                AiError::ShellAccessForbidden(msg) => {
                    assert!(msg.contains("shell-free"));
                }
                other => panic!("expected ShellAccessForbidden, got {:?}", other),
            }
        }

        // 2. Reject registering shell tools in NativeToolRegistry
        for forbidden_tool in ["sh", "sh_exec", "bash", "exec", "cmd", "spawn_shell"] {
            let err = runtime
                .tool_registry
                .register_tool(ToolSpec::new(
                    forbidden_tool,
                    "Attempted shell execution tool",
                    Vec::new(),
                ))
                .expect_err("registering shell tool must fail");
            assert!(matches!(err, AiError::ShellAccessForbidden(_)));
        }

        // 3. Reject executing shell tools even if constructed manually in a ToolCall
        let mut ui_synth = AgentUiSynthesizer::new();
        for forbidden_tool in ["sh", "sh_exec", "bash", "exec", "cmd"] {
            let call = ToolCall::new("call_bad", forbidden_tool, "{\"cmd\":\"ls\"}");
            let err = runtime
                .tool_registry
                .execute_tool(&call, &mut ui_synth)
                .expect_err("executing shell tool must fail");
            assert!(matches!(err, AiError::ShellAccessForbidden(_)));
        }
    }

    #[test]
    fn agent_exclusive_ui_synthesis_from_mcp_and_os_tools() {
        let mut runtime = AiRuntime::new();
        // Initial boot view is ViewKind::Console
        assert_eq!(runtime.synthesized_views().len(), 1);
        assert_eq!(runtime.synthesized_views()[0].kind, ViewKind::Console);

        // Third-party MCP apps cannot directly create UI surfaces
        let direct_attempt = runtime.ui_synthesizer.synthesize_from_mcp_output(
            &UiCallerOrigin::ThirdPartyMcpApp(String::from("rogue_app.mcp")),
            "rogue_app.mcp",
            "open_window",
            "{\"title\":\"Unauthorized Window\"}",
        );
        assert!(matches!(direct_attempt, Err(AiError::PolicyViolation(_))));

        // AI Agent invokes MCP tool and synthesizes ViewKind::McpAppView
        let mut streamed = String::new();
        let turn = runtime
            .handle_console_input("Query mcp cluster health", &mut |tok| {
                streamed.push_str(tok)
            })
            .expect("agent MCP turn should succeed");
        assert!(!streamed.is_empty());
        assert_eq!(turn.tool_outputs.len(), 1);
        let mcp_view = turn.tool_outputs[0]
            .synthesized_view
            .as_ref()
            .expect("MCP tool call should synthesize a UI view");
        assert_eq!(mcp_view.kind, ViewKind::McpAppView);
        assert_eq!(mcp_view.source_app.as_deref(), Some("telemetry.mcp"));
        assert_eq!(mcp_view.title, "Cluster Telemetry");
        assert!(!mcp_view.metrics.is_empty());

        // AI Agent invokes sys_status and synthesizes ViewKind::MetricDashboard
        let sys_turn = runtime
            .handle_console_input("Show system status and memory telemetry", &mut |_| {})
            .expect("sys_status turn should succeed");
        let sys_view = sys_turn.tool_outputs[0]
            .synthesized_view
            .as_ref()
            .expect("sys_status should synthesize MetricDashboard");
        assert_eq!(sys_view.kind, ViewKind::MetricDashboard);

        // AI Agent schedules a JS script and synthesizes ViewKind::ScriptScheduleList
        let sched_turn = runtime
            .handle_console_input("Schedule heartbeat script every 60s", &mut |_| {})
            .expect("script_schedule turn should succeed");
        let sched_view = sched_turn.tool_outputs[0]
            .synthesized_view
            .as_ref()
            .expect("script_schedule should synthesize ScriptScheduleList");
        assert_eq!(sched_view.kind, ViewKind::ScriptScheduleList);
        assert_eq!(sched_view.schedules.len(), 1);
    }
}
