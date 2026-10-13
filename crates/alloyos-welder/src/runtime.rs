//! Welder's conversational console, persistent configuration, tools and agent-owned UI.
use crate::tools::{
    validate_console_input_no_shell, NativeToolRegistry, ToolExecutionOutput, ToolVfsBackend,
};
use crate::ui_synth::{AgentUiSynthesizer, SynthesizedUiView};
use alloc::{
    format,
    rc::Rc,
    string::{String, ToString},
    vec,
    vec::Vec,
};
use core::{cell::RefCell, task::Poll};
use alloyos_forge::provider::{AiError, Message, Response};
use alloyos_forge::{ActiveProvider, Forge, ModelGateway};
use alloyos_js::{CalendarTime, OsEvent, ScriptExecutionReport};

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
                "You are the alloyOS Ferrofluid AI Agent running on bare-metal ARM64. \
                 POSIX shells (sh, bash) do not exist for the agent; use structured OS tools, \
                 local JavaScript scripts, and headless MCP apps. Only you can synthesize UI.",
            )],
            tool_registry: NativeToolRegistry::new(),
            ui_synthesizer: AgentUiSynthesizer::new(),
        };
        runtime.persist_config_to_vfs();
        runtime
    }

    /// Construct an `AgentRuntime` pre-wired with a shared `Forge` instance.
    pub fn with_forge(forge: Rc<RefCell<Forge>>) -> Self {
        let mut runtime = Self::new();
        runtime.set_forge(forge);
        runtime
    }

    /// Construct an `AgentRuntime` backed by a durable VFS backend (e.g. CobaltFS),
    /// restoring any persisted `/etc/ai.conf`, `/scripts/*.js`, and `/etc/schedules.conf`.
    pub fn with_vfs_backend(backend: Rc<RefCell<dyn ToolVfsBackend>>) -> Self {
        let mut runtime = Self {
            gateway: ModelGateway::new(),
            history: vec![Message::system(
                "You are the alloyOS Ferrofluid AI Agent running on bare-metal ARM64. \
                 POSIX shells (sh, bash) do not exist for the agent; use structured OS tools, \
                 local JavaScript scripts, and headless MCP apps. Only you can synthesize UI.",
            )],
            tool_registry: NativeToolRegistry::with_vfs_backend(backend),
            ui_synthesizer: AgentUiSynthesizer::new(),
        };
        if let Some(conf) = runtime.tool_registry.vfs_read(Self::CONFIG_PATH) {
            let _ = runtime.gateway.load_config_str(&conf);
        } else {
            runtime.persist_config_to_vfs();
        }
        runtime
    }

    /// Attach a custom VFS backend and restore persisted configuration, scripts, and schedules.
    pub fn set_vfs_backend(
        &mut self,
        backend: Rc<RefCell<dyn ToolVfsBackend>>,
    ) -> Result<usize, AiError> {
        self.tool_registry.set_vfs_backend(backend);
        self.restore_from_vfs()
    }

    /// Reloads `/etc/ai.conf` (if present) and restores `/scripts/*.js` and `/etc/schedules.conf`
    /// from the attached VFS backend.
    pub fn restore_from_vfs(&mut self) -> Result<usize, AiError> {
        if let Some(conf) = self.tool_registry.vfs_read(Self::CONFIG_PATH) {
            let _ = self.gateway.load_config_str(&conf);
        } else {
            self.persist_config_to_vfs();
        }
        self.tool_registry
            .restore_automations_from_vfs()
            .map_err(AiError::ConfigError)
    }

    /// Connect the long-lived resident device service to the default local provider
    /// and make it available to `AlloyOS.forge(...)` in capability-authorized JS scripts.
    pub fn set_forge(&mut self, forge: Rc<RefCell<Forge>>) {
        self.tool_registry.set_forge(forge.clone());
        self.gateway.set_forge(forge);
        self.persist_config_to_vfs();
    }

    /// Advance the JS automation scheduler to `now_ms` (and optional `calendar`) and return execution reports.
    pub fn tick_scheduler(
        &mut self,
        now_ms: u64,
        calendar: Option<CalendarTime>,
    ) -> Vec<ScriptExecutionReport> {
        self.tool_registry.tick_scheduler(now_ms, calendar)
    }

    /// Dispatch an OS event to all matching scheduled JS scripts.
    pub fn dispatch_os_event(&mut self, event: &OsEvent) -> Vec<ScriptExecutionReport> {
        self.tool_registry.dispatch_os_event(event)
    }

    /// Trigger all scripts scheduled for system boot (`SystemBoot` / `boot`).
    pub fn notify_system_boot(&mut self) -> Vec<ScriptExecutionReport> {
        self.tool_registry.notify_system_boot()
    }

    /// Trigger all scripts scheduled for file creation on `path`.
    pub fn notify_file_created(&mut self, path: &str) -> Vec<ScriptExecutionReport> {
        self.tool_registry.notify_file_created(path)
    }

    /// Trigger all scripts scheduled for file modifications on `path`.
    pub fn notify_file_modified(&mut self, path: &str) -> Vec<ScriptExecutionReport> {
        self.tool_registry.notify_file_modified(path)
    }

    /// Trigger all scripts scheduled for file deletion on `path`.
    pub fn notify_file_deleted(&mut self, path: &str) -> Vec<ScriptExecutionReport> {
        self.tool_registry.notify_file_deleted(path)
    }

    /// Trigger all scripts scheduled for network interface coming online.
    pub fn notify_network_online(&mut self) -> Vec<ScriptExecutionReport> {
        self.tool_registry.notify_network_online()
    }

    /// Trigger all scripts scheduled for incoming network packets/requests on `port`.
    pub fn notify_network_request(
        &mut self,
        port: u16,
        payload: &str,
    ) -> Vec<ScriptExecutionReport> {
        self.tool_registry.notify_network_request(port, payload)
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

    /// Latest agent-owned descriptor for each tile, replacing stale refresh results.
    pub fn synthesized_tiles(&self) -> Vec<alloyos_mcp::AppTile> {
        let mut tiles: Vec<alloyos_mcp::AppTile> = Vec::new();
        for view in self.synthesized_views() {
            if let Some(tile) = &view.tile {
                if let Some(existing) = tiles.iter_mut().find(|t| t.tile_id == tile.tile_id) {
                    *existing = tile.clone();
                } else {
                    tiles.push(tile.clone());
                }
            }
        }
        tiles
    }

    /// Execute only actions currently exposed by an agent-synthesized tile.
    /// The native tool registry keeps MCP orchestration under Welder ownership.
    pub fn execute_tile_action(
        &mut self,
        action: &alloyos_mcp::TileAction,
    ) -> Result<ToolExecutionOutput, AiError> {
        let tiles = self.synthesized_tiles();
        let tile = tiles.iter().find(|tile| tile.actions.contains(action))
            .ok_or_else(|| AiError::PolicyViolation("tile action is not currently exposed".into()))?;
        let app = tile.folder()
            .ok_or_else(|| AiError::InvalidToolArguments("tile has no app namespace".into()))?;
        let args = alloyos_mcp::McpValue::Map(action.default_args.clone()).to_json();
        let arguments = format!(
            "{{\"app\":\"{}\",\"tool\":\"{}\",\"args\":\"{}\"}}",
            alloyos_forge::provider::json_escape(app),
            alloyos_forge::provider::json_escape(&action.tool_name),
            alloyos_forge::provider::json_escape(&args),
        );
        let call = alloyos_forge::provider::ToolCall::new(
            "tile_action", "mcp_call_tool", arguments,
        );
        self.tool_registry.execute_tool(&call, &mut self.ui_synthesizer)
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
    use alloyos_forge::provider::TokenUsage;
    #[test]
    fn clock_tile_refresh_uses_live_host_uptime() {
        let mut runtime = AgentRuntime::new();
        runtime.tick_scheduler(45_296_000, None);
        let call = ToolCall::new("clock", "mcp_call_tool",
            "{\"app\":\"clock\",\"tool\":\"clock.now\",\"args\":\"{}\"}");
        runtime.tool_registry.execute_tool(&call, &mut runtime.ui_synthesizer).unwrap();
        let tile = runtime.synthesized_tiles().remove(0);
        assert_eq!(tile.tile_id, "clock.now");
        assert_eq!(tile.items[0].title, "12:34:56");
        runtime.tick_scheduler(45_297_000, None);
        let refreshed = runtime.execute_tile_action(&tile.actions[0]).unwrap();
        assert!(refreshed.output_json.contains("45297000"));
        assert_eq!(runtime.synthesized_tiles()[0].items[0].title, "12:34:57");
        assert_eq!(runtime.synthesized_tiles().len(), 1);
    }

    #[test]
    fn tile_refresh_routes_through_welder_and_replaces_latest_descriptor() {
        let mut runtime = AgentRuntime::new();
        let call = ToolCall::new("initial", "mcp_call_tool",
            "{\"app\":\"tasks\",\"tool\":\"tasks.list\",\"args\":\"{}\"}");
        runtime.tool_registry.execute_tool(&call, &mut runtime.ui_synthesizer).unwrap();
        let initial = runtime.synthesized_tiles();
        assert_eq!(initial.len(), 1);
        let action = initial[0].actions.iter().find(|a| a.tool_name == "tasks.list").unwrap().clone();
        let output = runtime.execute_tile_action(&action).unwrap();
        assert_eq!(output.tool_name, "mcp_call_tool");
        assert_eq!(runtime.synthesized_tiles().len(), 1);
        assert_eq!(output.synthesized_view.unwrap().tile.unwrap(), runtime.synthesized_tiles()[0]);
        let mut forged = action;
        forged.tool_name = "tasks.delete".into();
        assert!(matches!(runtime.execute_tile_action(&forged), Err(AiError::PolicyViolation(_))));
    }

    #[test]
    fn runtime_hot_swapping_and_etc_ai_conf_persistence() {
        let mut runtime = AgentRuntime::new();
        assert_eq!(runtime.active_provider_name(), "gemini");
        assert_eq!(runtime.active_model_id(), "gemini-3.8-flash");

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

        // AI Agent invokes Tasks MCP tool and synthesizes ViewKind::McpAppView with declarative AppTile
        let turn = runtime
            .apply_response(Response::new(
                "Listing scheduled tasks via Tasks app",
                vec![ToolCall::new(
                    "mcp-1",
                    "mcp_call_tool",
                    "{\"app\":\"tasks\",\"tool\":\"tasks.list\",\"args\":\"{}\"}",
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
        assert_eq!(mcp_view.source_app.as_deref(), Some("tasks"));
        assert_eq!(mcp_view.title, "Tasks");
        assert!(mcp_view.tile.is_some());
        assert_eq!(mcp_view.tile.as_ref().unwrap().tile_id, "tasks.board");

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

        // AI Agent schedules a JS script (routed through TasksApp) and synthesizes ViewKind::ScriptScheduleList
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
        assert_eq!(sched_view.source_app.as_deref(), Some("tasks"));
        assert!(sched_view.tile.is_some());
    }
    #[test]
    fn cloud_is_default_and_explicit_local_never_silently_uses_cloud() {
        let mut ai = AgentRuntime::new();
        assert_eq!(ai.active_provider_name(), "gemini");
        assert_eq!(ai.active_model_id(), "gemini-3.8-flash");
        assert!(ai.serialize_config().contains("provider=gemini"));
        ai.switch_provider("local").unwrap();
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

    #[test]
    fn live_script_create_run_schedule_and_event_triggers() {
        let mut runtime = AgentRuntime::new();
        let mut ui = AgentUiSynthesizer::new();

        // 1. Create and immediately run a script via Welder tools
        let create_call = ToolCall::new(
            "c1",
            "script_create",
            r#"{"path":"/scripts/compute.js","code":"let s = AlloyOS.sys.status(); AlloyOS.fs.write('/var/computed.txt', 'tasks=' + s.activeTasks); AlloyOS.ui.notify('Compute: Done'); s.activeTasks * 10;"}"#,
        );
        runtime
            .tool_registry
            .execute_tool(&create_call, &mut ui)
            .expect("script_create must succeed");

        let run_call = ToolCall::new("r1", "script_run", r#"{"path":"/scripts/compute.js"}"#);
        let run_out = runtime
            .tool_registry
            .execute_tool(&run_call, &mut ui)
            .expect("script_run must execute via alloyos-js");
        assert!(run_out.output_json.contains(r#""ok":true"#));
        assert!(run_out.output_json.contains(r#""value":60"#));
        assert_eq!(
            runtime.tool_registry.vfs_read("/var/computed.txt").as_deref(),
            Some("tasks=6")
        );
        assert_eq!(
            runtime.tool_registry.notifications(),
            &[String::from("Compute: Done")]
        );

        // 2. Schedule an inbox watcher triggered by `fs:create:/inbox/*`
        let watcher_create = ToolCall::new(
            "c2",
            "script_create",
            r#"{"path":"/scripts/inbox_watcher.js","code":"let raw = AlloyOS.fs.read(__event.path); AlloyOS.fs.write('/processed/latest.txt', 'seen:' + raw); raw.length;"}"#,
        );
        runtime
            .tool_registry
            .execute_tool(&watcher_create, &mut ui)
            .unwrap();

        let watcher_sched = ToolCall::new(
            "s1",
            "script_schedule",
            r#"{"path":"/scripts/inbox_watcher.js","trigger":"fs:create:/inbox/*"}"#,
        );
        runtime
            .tool_registry
            .execute_tool(&watcher_sched, &mut ui)
            .unwrap();

        // Writing a new file under /inbox via `fs_write` must automatically fire the scheduled script
        let write_inbox = ToolCall::new(
            "w1",
            "fs_write",
            r#"{"path":"/inbox/job1.txt","content":"payload-42"}"#,
        );
        let write_out = runtime
            .tool_registry
            .execute_tool(&write_inbox, &mut ui)
            .unwrap();
        assert!(write_out.output_json.contains(r#""triggered_scripts":1"#));
        assert_eq!(
            runtime
                .tool_registry
                .vfs_read("/processed/latest.txt")
                .as_deref(),
            Some("seen:payload-42")
        );

        // 3. Schedule a network request handler and a timer interval script
        let net_create = ToolCall::new(
            "c3",
            "script_create",
            r#"{"path":"/scripts/net_hook.js","code":"AlloyOS.fs.write('/var/net_last.txt', __event.port + ':' + __event.payload); true;"}"#,
        );
        runtime
            .tool_registry
            .execute_tool(&net_create, &mut ui)
            .unwrap();
        let net_sched = ToolCall::new(
            "s2",
            "script_schedule",
            r#"{"path":"/scripts/net_hook.js","trigger":"net:request:8080"}"#,
        );
        runtime
            .tool_registry
            .execute_tool(&net_sched, &mut ui)
            .unwrap();

        let net_reports = runtime.notify_network_request(8080, "GET /health");
        assert_eq!(net_reports.len(), 1);
        assert_eq!(
            runtime.tool_registry.vfs_read("/var/net_last.txt").as_deref(),
            Some("8080:GET /health")
        );
    }

    #[test]
    fn durable_scripts_and_schedules_restore_across_reboot() {
        let shared_vfs = Rc::new(RefCell::new(crate::tools::SharedMapVfsBackend::new()));

        // Boot 1: Create scripts and register schedules
        {
            let mut runtime = AgentRuntime::with_vfs_backend(shared_vfs.clone());
            let mut ui = AgentUiSynthesizer::new();

            runtime
                .tool_registry
                .execute_tool(
                    &ToolCall::new(
                        "c1",
                        "script_create",
                        r#"{"path":"/scripts/on_boot.js","code":"AlloyOS.fs.write('/var/boot_marker.txt', 'booted-ok'); 'booted';"}"#,
                    ),
                    &mut ui,
                )
                .unwrap();
            runtime
                .tool_registry
                .execute_tool(
                    &ToolCall::new(
                        "s1",
                        "script_schedule",
                        r#"{"path":"/scripts/on_boot.js","trigger":"boot"}"#,
                    ),
                    &mut ui,
                )
                .unwrap();

            runtime
                .tool_registry
                .execute_tool(
                    &ToolCall::new(
                        "c2",
                        "script_create",
                        r#"{"path":"/scripts/tick_counter.js","code":"let prev = AlloyOS.fs.read('/var/ticks.txt'); let next = '1'; if (prev !== null) { next = prev + '1'; } AlloyOS.fs.write('/var/ticks.txt', next); next;"}"#,
                    ),
                    &mut ui,
                )
                .unwrap();
            runtime
                .tool_registry
                .execute_tool(
                    &ToolCall::new(
                        "s2",
                        "script_schedule",
                        r#"{"path":"/scripts/tick_counter.js","trigger":"every:10s"}"#,
                    ),
                    &mut ui,
                )
                .unwrap();
        }

        // Verify `/etc/schedules.conf` was persisted into the shared VFS backing store
        let conf_text = shared_vfs
            .borrow_mut()
            .read_file("/etc/schedules.conf")
            .expect("/etc/schedules.conf must be persisted");
        assert!(conf_text.contains("/scripts/on_boot.js"));
        assert!(conf_text.contains("/scripts/tick_counter.js"));

        // Boot 2: Construct a fresh AgentRuntime attached to the same durable VFS backend
        let mut rebooted = AgentRuntime::with_vfs_backend(shared_vfs.clone());
        assert_eq!(rebooted.tool_registry.scheduler().schedules().len(), 2);

        // Firing SystemBoot on the rebooted runtime executes the restored `/scripts/on_boot.js`
        let boot_reports = rebooted.notify_system_boot();
        assert_eq!(boot_reports.len(), 1);
        assert_eq!(
            rebooted
                .tool_registry
                .vfs_read("/var/boot_marker.txt")
                .as_deref(),
            Some("booted-ok")
        );

        // Advancing the scheduler clock executes the restored `/scripts/tick_counter.js`
        let tick1 = rebooted.tick_scheduler(10_000, None);
        assert_eq!(tick1.len(), 1);
        assert_eq!(
            rebooted.tool_registry.vfs_read("/var/ticks.txt").as_deref(),
            Some("1")
        );

        let tick2 = rebooted.tick_scheduler(20_000, None);
        assert_eq!(tick2.len(), 1);
        assert_eq!(
            rebooted.tool_registry.vfs_read("/var/ticks.txt").as_deref(),
            Some("11")
        );
    }

    #[test]
    fn tasks_app_crud_tiles_and_cross_app_isolation() {
        let mut runtime = AgentRuntime::new();
        let mut ui = AgentUiSynthesizer::new();

        // 1. Discover apps, tools, SKILLs, and Tiles via mcp_list_tools
        let list_out = runtime
            .tool_registry
            .execute_tool(&ToolCall::new("l1", "mcp_list_tools", "{}"), &mut ui)
            .expect("mcp_list_tools must succeed");
        assert!(list_out.output_json.contains(r#""headless_mcp_apps":["clock","tasks"]"#));
        assert!(list_out.output_json.contains(r#""tasks.list""#));
        assert!(list_out.output_json.contains(r#""tasks.create""#));
        assert!(list_out.output_json.contains(r#""tasks.edit""#));
        assert!(list_out.output_json.contains(r#""tasks.delete""#));
        assert!(list_out.output_json.contains(r#""/apps/tasks/SKILL.md""#));
        assert!(list_out.output_json.contains(r#""tasks.board""#));

        // 2. Create a scheduled task through Tasks app via Welder
        let create_task = runtime
            .tool_registry
            .execute_tool(
                &ToolCall::new(
                    "t1",
                    "mcp_call_tool",
                    r#"{"app":"tasks","tool":"tasks.create","id":"daily-check","path":"/scripts/heartbeat.js","trigger":"every:60s"}"#,
                ),
                &mut ui,
            )
            .expect("tasks.create must succeed");
        assert!(create_task.output_json.contains(r#""status":"created""#));
        assert!(create_task.output_json.contains(r#""id":"daily-check""#));
        let tile = create_task
            .synthesized_view
            .as_ref()
            .and_then(|v| v.tile.as_ref())
            .expect("tasks.create must synthesize view with tasks.board AppTile");
        assert_eq!(tile.tile_id, "tasks.board");
        assert_eq!(tile.items.len(), 1);
        assert_eq!(tile.items[0].id, "daily-check");
        assert_eq!(tile.items[0].badge, "interval:60000");



        // 3. Edit the scheduled task through Tasks app via Welder
        let edit_task = runtime
            .tool_registry
            .execute_tool(
                &ToolCall::new(
                    "t2",
                    "mcp_call_tool",
                    r#"{"app":"tasks","tool":"tasks.edit","id":"daily-check","trigger":"cron:*/5 * * * *"}"#,
                ),
                &mut ui,
            )
            .expect("tasks.edit must succeed");
        assert!(edit_task.output_json.contains(r#""status":"edited""#));
        assert!(edit_task.output_json.contains(r#""cron:*/5 * * * *""#));

        assert!(runtime
            .tool_registry
            .vfs_read("/etc/schedules.conf")
            .unwrap()
            .contains("cron:*/5 * * * *"));

        // 4. Reject any cross-app invocation (another app trying to call Tasks)
        let cross_app_err = runtime
            .tool_registry
            .execute_tool(
                &ToolCall::new(
                    "t3",
                    "mcp_call_tool",
                    r#"{"caller_app":"other_app","app":"tasks","tool":"tasks.list"}"#,
                ),
                &mut ui,
            )
            .expect_err("cross-app invocation must be rejected");
        assert!(matches!(cross_app_err, AiError::PolicyViolation(_)));
    }
}
