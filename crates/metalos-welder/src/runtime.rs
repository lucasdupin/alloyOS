//! Welder's conversational console, persistent configuration, tools and agent-owned UI.
use crate::tools::{validate_console_input_no_shell, NativeToolRegistry, ToolExecutionOutput};
use crate::ui_synth::{AgentUiSynthesizer, SynthesizedUiView};
use alloc::{
    format,
    rc::Rc,
    string::{String, ToString},
    vec,
    vec::Vec,
};
use core::{cell::RefCell, task::Poll};
use metalos_forge::provider::{AiError, Message, Response};
use metalos_forge::{ActiveProvider, Forge, ModelGateway};

/// Result of handling a single conversational or slash-command turn in the Ferrofluid AI Console.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConsoleTurnOutput {
    pub active_provider: &'static str,
    pub active_model: String,
    pub response_text: String,
    pub tool_outputs: Vec<ToolExecutionOutput>,
    pub config_persisted: bool,
}

/// Pluggable AI runtime routing through Forge (Gemma by default) or cloud providers
/// (`GeminiProvider`, `ClaudeProvider`, `ChatGptProvider`), with `/etc/ai.conf` persistence, conversation history,
/// shell-free native OS tools, and agent-exclusive Ferrofluid UI synthesis.
#[derive(Clone, Debug)]
pub struct AgentRuntime {
    gateway: ModelGateway,
    pub history: Vec<Message>,
    pub tool_registry: NativeToolRegistry,
    pub ui_synthesizer: AgentUiSynthesizer,
}

impl Default for AgentRuntime {
    fn default() -> Self {
        Self::new()
    }
}

impl AgentRuntime {
    pub const CONFIG_PATH: &'static str = "/etc/ai.conf";

    pub fn new() -> Self {
        let mut runtime = Self {
            gateway: ModelGateway::new(),
            history: vec![Message::system(
                "You are the metalOS Ferrofluid AI Agent running on bare-metal ARM64. \
                 POSIX shells (sh, bash) do not exist for the agent; use structured OS tools, \
                 local JavaScript scripts, and headless MCP apps. Only you can synthesize UI.",
            )],
            tool_registry: NativeToolRegistry::new(),
            ui_synthesizer: AgentUiSynthesizer::new(),
        };
        runtime.persist_config_to_vfs();
        runtime
    }

    /// Connect the long-lived resident device service to the default local provider.
    pub fn set_forge(&mut self, forge: Rc<RefCell<Forge>>) {
        self.gateway.set_forge(forge);
        self.persist_config_to_vfs();
    }
    pub fn active_provider_name(&self) -> &'static str {
        self.gateway.active_provider_name()
    }
    pub fn active_model_id(&self) -> &str {
        self.gateway.active_model_id()
    }
    pub fn switch_provider(&mut self, name: &str) -> Result<ActiveProvider, AiError> {
        let provider = self.gateway.switch_provider(name)?;
        self.persist_config_to_vfs();
        Ok(provider)
    }
    pub fn cycle_model(&mut self) -> Result<(), AiError> {
        self.gateway.cycle_model()?;
        self.persist_config_to_vfs();
        Ok(())
    }
    pub fn serialize_config(&self) -> String {
        self.gateway.serialize_config()
    }
    pub fn load_config_str(&mut self, text: &str) -> Result<(), AiError> {
        self.gateway.load_config_str(text)?;
        self.persist_config_to_vfs();
        Ok(())
    }

    /// Reloads `/etc/ai.conf` directly from the virtualized CobaltFS store.
    pub fn reload_config_from_vfs(&mut self) -> Result<(), AiError> {
        let conf = self
            .tool_registry
            .vfs_read(Self::CONFIG_PATH)
            .ok_or_else(|| AiError::ConfigError("/etc/ai.conf not found in VFS".to_string()))?
            .to_string();
        self.load_config_str(&conf)
    }

    fn persist_config_to_vfs(&mut self) {
        let serialized = self.serialize_config();
        self.tool_registry.vfs_write(Self::CONFIG_PATH, serialized);
    }

    /// Processes a user input line from the Ferrofluid AI Console:
    /// - Rejects `!` shell escapes and `sh`/`bash` commands with `AiError::ShellAccessForbidden`
    /// - Handles `/model gemini`, `/model claude`, `/model chatgpt` hot-swapping and persists `/etc/ai.conf`
    /// - Streams tokens from the active `AiModelProvider` and executes any emitted native OS `ToolCall`s.
    pub fn handle_console_input(
        &mut self,
        input: &str,
        on_token: &mut dyn FnMut(&str),
    ) -> Result<ConsoleTurnOutput, AiError> {
        let trimmed = input.trim();
        validate_console_input_no_shell(trimmed)?;

        if let Some(rest) = trimmed.strip_prefix("/model") {
            let target = rest.trim();
            if target.is_empty() {
                let status_msg = format!(
                    "Active provider: {} (model: {})",
                    self.active_provider_name(),
                    self.active_model_id()
                );
                on_token(&status_msg);
                return Ok(ConsoleTurnOutput {
                    active_provider: self.active_provider_name(),
                    active_model: self.active_model_id().to_string(),
                    response_text: status_msg,
                    tool_outputs: Vec::new(),
                    config_persisted: false,
                });
            }

            let mut parts = target.split_whitespace();
            let provider_arg = parts.next().unwrap_or("");
            let custom_model_arg = parts.next();

            self.switch_provider(provider_arg)?;
            if let Some(custom_model) = custom_model_arg {
                self.gateway.set_model(custom_model)?;
                self.persist_config_to_vfs();
            }

            let msg = format!(
                "Switched active AI model provider to '{}' ({}) and persisted to /etc/ai.conf.",
                self.active_provider_name(),
                self.active_model_id()
            );
            on_token(&msg);
            return Ok(ConsoleTurnOutput {
                active_provider: self.active_provider_name(),
                active_model: self.active_model_id().to_string(),
                response_text: msg,
                tool_outputs: Vec::new(),
                config_persisted: true,
            });
        }

        self.history.push(Message::user(trimmed));
        let tools_snapshot = self.tool_registry.specs().to_vec();

        let response = self
            .gateway
            .chat(&self.history, &tools_snapshot, on_token)?;
        self.apply_response(response)
    }

    /// Console UI entry point: return immediately after starting cloud work.
    pub fn start_console_input(
        &mut self,
        input: &str,
    ) -> Result<Option<ConsoleTurnOutput>, AiError> {
        if self.gateway.is_pending() {
            return Err(AiError::PolicyViolation(
                "A reply is already pending".into(),
            ));
        }
        let trimmed = input.trim();
        validate_console_input_no_shell(trimmed)?;
        if trimmed.starts_with("/model") {
            return self.handle_console_input(input, &mut |_| {}).map(Some);
        }
        self.history.push(Message::user(trimmed));
        if let Err(error) = self.gateway.start_chat(&self.history) {
            self.history.pop();
            return Err(error);
        }
        Ok(None)
    }
    pub fn partial_console_reply(&self) -> Option<String> {
        self.gateway.partial_reply()
    }
    pub fn poll_console_reply(&mut self) -> Poll<Result<ConsoleTurnOutput, AiError>> {
        match self.gateway.poll_chat() {
            Poll::Pending => Poll::Pending,
            Poll::Ready(result) => {
                Poll::Ready(result.and_then(|response| self.apply_response(response)))
            }
        }
    }

    fn apply_response(&mut self, response: Response) -> Result<ConsoleTurnOutput, AiError> {
        let mut tool_outputs = Vec::new();
        for call in &response.tool_calls {
            let output = self
                .tool_registry
                .execute_tool(call, &mut self.ui_synthesizer)?;
            self.history
                .push(Message::tool_result(&call.id, &output.output_json));
            tool_outputs.push(output);
        }

        self.history.push(Message::assistant_with_tools(
            &response.text,
            response.tool_calls.clone(),
        ));

        Ok(ConsoleTurnOutput {
            active_provider: self.active_provider_name(),
            active_model: response.model_name,
            response_text: response.text,
            tool_outputs,
            config_persisted: false,
        })
    }

    /// Returns all UI views synthesized by the AI agent for Ferrofluid (`ferrofluidd`).
    pub fn synthesized_views(&self) -> &[SynthesizedUiView] {
        self.ui_synthesizer.views()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ProviderToolCall as ToolCall, ToolSpec, UiCallerOrigin, ViewKind};
    use metalos_forge::provider::TokenUsage;
    #[test]
    fn runtime_hot_swapping_and_etc_ai_conf_persistence() {
        let mut runtime = AgentRuntime::new();
        assert_eq!(runtime.active_provider_name(), "forge");
        assert_eq!(runtime.active_model_id(), "gemma-2-2b-it");

        // Switch to Claude via slash command
        let mut streamed = String::new();
        let out_claude = runtime
            .handle_console_input("/model claude", &mut |t| streamed.push_str(t))
            .expect("should switch to claude");
        assert_eq!(out_claude.active_provider, "claude");
        assert_eq!(out_claude.active_model, "claude-sonnet-4-6");
        assert!(out_claude.config_persisted);

        let saved_conf = runtime
            .tool_registry
            .vfs_read("/etc/ai.conf")
            .expect("/etc/ai.conf must exist");
        assert!(saved_conf.contains("provider=claude"));
        assert!(saved_conf.contains("model=claude-sonnet-4-6"));

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
        assert_eq!(out_gemini.active_model, "gemini-3.8-flash");

        // Test loading a custom /etc/ai.conf into a fresh runtime
        let mut restored = AgentRuntime::new();
        restored
            .load_config_str("provider=claude\nmodel=claude-sonnet-4-6\n")
            .expect("config should load");
        assert_eq!(restored.active_provider_name(), "claude");
        assert_eq!(restored.active_model_id(), "claude-sonnet-4-6");
    }

    #[test]
    fn shell_commands_and_tools_are_strictly_blocked() {
        let mut runtime = AgentRuntime::new();

        // 1. Reject `!ls` and `!sh` shell escapes in console input
        for forbidden_input in [
            "!ls",
            "!sh",
            "!cat /etc/ai.conf",
            "sh",
            "bash -c ls",
            "sh_exec",
        ] {
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
        let mut runtime = AgentRuntime::new();
        runtime.switch_provider("gemini").unwrap();
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
        let turn = runtime
            .apply_response(Response::new(
                "Querying cluster",
                vec![ToolCall::new(
                    "mcp-1",
                    "mcp_call_tool",
                    "{\"app\":\"telemetry.mcp\",\"tool\":\"cluster_health\",\"args\":\"{}\"}",
                )],
                "fixture",
                TokenUsage::new(0, 0),
            ))
            .expect("agent MCP turn should succeed");
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
            .apply_response(Response::new(
                "System status",
                vec![ToolCall::new("sys-1", "sys_status", "{}")],
                "fixture",
                TokenUsage::new(0, 0),
            ))
            .expect("sys_status turn should succeed");
        let sys_view = sys_turn.tool_outputs[0]
            .synthesized_view
            .as_ref()
            .expect("sys_status should synthesize MetricDashboard");
        assert_eq!(sys_view.kind, ViewKind::MetricDashboard);

        // AI Agent schedules a JS script and synthesizes ViewKind::ScriptScheduleList
        let sched_turn = runtime
            .apply_response(Response::new(
                "Scheduling",
                vec![ToolCall::new(
                    "schedule-1",
                    "script_schedule",
                    "{\"path\":\"/scripts/heartbeat.js\",\"trigger\":\"every:60s\"}",
                )],
                "fixture",
                TokenUsage::new(0, 0),
            ))
            .expect("script_schedule turn should succeed");
        let sched_view = sched_turn.tool_outputs[0]
            .synthesized_view
            .as_ref()
            .expect("script_schedule should synthesize ScriptScheduleList");
        assert_eq!(sched_view.kind, ViewKind::ScriptScheduleList);
        assert_eq!(sched_view.schedules.len(), 1);
    }
    #[test]
    fn default_local_inference_never_silently_uses_cloud() {
        let mut ai = AgentRuntime::new();
        assert_eq!(ai.active_provider_name(), "forge");
        assert_eq!(ai.active_model_id(), "gemma-2-2b-it");
        let mut streamed = false;
        assert!(matches!(
            ai.handle_console_input("hello", &mut |_| streamed = true),
            Err(AiError::LocalInferenceError(_))
        ));
        assert!(!streamed);
        let config = ai.serialize_config();
        assert!(config.contains("provider=forge"));
        ai.switch_provider("claude").unwrap();
        assert_eq!(ai.active_provider_name(), "claude");
        ai.switch_provider("local").unwrap();
        assert_eq!(ai.active_provider_name(), "forge");
        ai.load_config_str("provider=forge\nmodel=gemma-2-2b-it\n")
            .unwrap();
        assert_eq!(ai.active_provider_name(), "forge");
    }
}
