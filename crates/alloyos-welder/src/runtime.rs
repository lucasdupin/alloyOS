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
    pinned_tile_ids: Vec<String>,
    turn_tool_calls: usize,
    turn_tool_outputs: Vec<ToolExecutionOutput>,
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
    pub const DASHBOARD_CONFIG_PATH: &'static str = "/etc/dashboard.conf";

    pub const CONFIG_PATH: &'static str = "/etc/ai.conf";

    pub fn new() -> Self {
        let mut runtime = Self {
            gateway: ModelGateway::new(),
            pinned_tile_ids: Vec::new(),
            turn_tool_calls: 0,
            turn_tool_outputs: Vec::new(),
            history: vec![Message::system(
                "You are the alloyOS Ferrofluid AI Agent running on bare-metal ARM64. \
                 POSIX shells (sh, bash) do not exist for the agent; use structured OS tools, \
                 local JavaScript scripts, and headless MCP apps. Only you can synthesize UI.",
            )],
            tool_registry: NativeToolRegistry::new(),
            ui_synthesizer: AgentUiSynthesizer::new(),
        };
        runtime.persist_config_to_vfs();
        runtime.restore_dashboard();
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
            pinned_tile_ids: Vec::new(),
            turn_tool_calls: 0,
            turn_tool_outputs: Vec::new(),
            history: vec![Message::system(
                "You are the alloyOS Ferrofluid AI Agent running on bare-metal ARM64. \
                 POSIX shells (sh, bash) do not exist for the agent; use structured OS tools, \
                 local JavaScript scripts, and headless MCP apps. Only you can synthesize UI.",
            )],
            tool_registry: NativeToolRegistry::with_vfs_backend(backend),
            ui_synthesizer: AgentUiSynthesizer::new(),
        };
        if let Some(conf) = runtime
            .tool_registry
            .vfs_read(Self::CONFIG_PATH)
            .map(|s| s.to_string())
        {
            runtime.apply_knowledge_config_str(&conf);
            let _ = runtime.gateway.load_config_str(&conf);
        } else {
            runtime.persist_config_to_vfs();
        }
        runtime.restore_dashboard();
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
        if let Some(conf) = self
            .tool_registry
            .vfs_read(Self::CONFIG_PATH)
            .map(|s| s.to_string())
        {
            self.apply_knowledge_config_str(&conf);
            let _ = self.gateway.load_config_str(&conf);
        } else {
            self.persist_config_to_vfs();
        }
        self.restore_dashboard();
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
        let mut conf = self.gateway.serialize_config();
        conf.push_str(&format!(
            "knowledge_learning_paused={}\n",
            self.tool_registry.is_knowledge_learning_paused()
        ));
        conf
    }
    pub fn load_config_str(&mut self, text: &str) -> Result<(), AiError> {
        self.gateway.load_config_str(text)?;
        self.apply_knowledge_config_str(text);
        self.persist_config_to_vfs();
        Ok(())
    }

    fn apply_knowledge_config_str(&mut self, text: &str) {
        for raw_line in text.lines() {
            let line = raw_line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            if let Some((k, v)) = line.split_once('=') {
                if k.trim() == "knowledge_learning_paused" {
                    let paused = matches!(
                        v.trim().to_ascii_lowercase().as_str(),
                        "true" | "1" | "yes" | "paused"
                    );
                    self.tool_registry.set_knowledge_learning_paused(paused);
                }
            }
        }
    }

    /// Whether `/knowledge/` profile learning is currently paused by the user.
    pub fn is_learning_paused(&self) -> bool {
        self.tool_registry.is_knowledge_learning_paused()
    }

    /// Pause or resume automatic `/knowledge/` profile learning and persist to `/etc/ai.conf`.
    pub fn set_learning_paused(&mut self, paused: bool) {
        self.tool_registry.set_knowledge_learning_paused(paused);
        self.persist_config_to_vfs();
        self.refresh_knowledge_tile_if_present();
    }

    /// Storage durability classification of the `/knowledge/` repository (`DurableCobaltFs` vs `VolatileRamFs`).
    pub fn knowledge_storage_kind(&self) -> crate::knowledge::KnowledgeStorageKind {
        self.tool_registry.knowledge_storage_kind()
    }

    fn refresh_knowledge_tile_if_present(&mut self) {
        let has_knowledge_tile = self
            .ui_synthesizer
            .views()
            .iter()
            .any(|v| v.tile.as_ref().map(|t| t.tile_id.as_str()) == Some("knowledge.profile"));
        if has_knowledge_tile {
            let call = alloyos_forge::provider::ToolCall::new(
                "knowledge_refresh",
                "knowledge_search",
                r#"{"query":"*"}"#,
            );
            let _ = self
                .tool_registry
                .execute_tool(&call, &mut self.ui_synthesizer);
        }
    }

    fn messages_with_knowledge_context(&mut self, user_turn: &str) -> Vec<Message> {
        let mut messages = self.history.clone();
        if let Some(ctx) = self.tool_registry.build_turn_knowledge_context(user_turn) {
            let insert_idx = messages.len().saturating_sub(1);
            messages.insert(insert_idx, Message::system(ctx));
        }
        messages
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

    fn handle_knowledge_command(
        &mut self,
        rest: &str,
        on_token: &mut dyn FnMut(&str),
    ) -> Result<ConsoleTurnOutput, AiError> {
        let target = rest.trim();
        let (subcmd, arg) = match target.split_once(char::is_whitespace) {
            Some((cmd, remainder)) => (cmd.trim(), remainder.trim()),
            None => (target, ""),
        };

        match subcmd {
            "" | "inspect" | "status" => {
                let call = alloyos_forge::provider::ToolCall::new(
                    "knowledge_inspect",
                    "knowledge_search",
                    r#"{"query":"*"}"#,
                );
                let out = self
                    .tool_registry
                    .execute_tool(&call, &mut self.ui_synthesizer)?;
                let entries = self.tool_registry.all_knowledge_entries();
                let storage = self.tool_registry.knowledge_storage_kind();
                let status = if self.tool_registry.is_knowledge_learning_paused() {
                    "paused"
                } else {
                    "active"
                };
                let mut msg = format!(
                    "User Knowledge (/knowledge): {} record(s), learning={}, storage={}.",
                    entries.len(),
                    status,
                    storage.durability_notice()
                );
                for entry in &entries {
                    msg.push_str(&format!(
                        "\n- [{}] {}: {} (basis: {}, scope: {})",
                        entry.category.path(),
                        entry.title,
                        entry.value,
                        entry.basis.as_code(),
                        entry.scope
                    ));
                }
                on_token(&msg);
                Ok(ConsoleTurnOutput {
                    active_provider: self.active_provider_name(),
                    active_model: self.active_model_id().to_string(),
                    response_text: msg,
                    tool_outputs: vec![out],
                    config_persisted: false,
                })
            }
            "search" => {
                let q = if arg.is_empty() { "*" } else { arg };
                let call = alloyos_forge::provider::ToolCall::new(
                    "knowledge_search_cmd",
                    "knowledge_search",
                    format!(
                        r#"{{"query":"{}"}}"#,
                        alloyos_forge::provider::json_escape(q)
                    ),
                );
                let out = self
                    .tool_registry
                    .execute_tool(&call, &mut self.ui_synthesizer)?;
                let matches = self.tool_registry.knowledge_search(q, None, None, 32);
                let mut msg = format!(
                    "Found {} knowledge record(s) matching '{}'.",
                    matches.len(),
                    q
                );
                for m in &matches {
                    msg.push_str(&format!(
                        "\n- [{}] {}: {} (scope: {})",
                        m.entry.category.path(),
                        m.entry.title,
                        m.entry.value,
                        m.entry.scope
                    ));
                }
                on_token(&msg);
                Ok(ConsoleTurnOutput {
                    active_provider: self.active_provider_name(),
                    active_model: self.active_model_id().to_string(),
                    response_text: msg,
                    tool_outputs: vec![out],
                    config_persisted: false,
                })
            }
            "pause" => {
                self.tool_registry.set_knowledge_learning_paused(true);
                self.persist_config_to_vfs();
                let call = alloyos_forge::provider::ToolCall::new(
                    "knowledge_pause_cmd",
                    "knowledge_search",
                    r#"{"query":"*"}"#,
                );
                let out = self
                    .tool_registry
                    .execute_tool(&call, &mut self.ui_synthesizer)?;
                let msg = "Knowledge profile learning paused and persisted to /etc/ai.conf."
                    .to_string();
                on_token(&msg);
                Ok(ConsoleTurnOutput {
                    active_provider: self.active_provider_name(),
                    active_model: self.active_model_id().to_string(),
                    response_text: msg,
                    tool_outputs: vec![out],
                    config_persisted: true,
                })
            }
            "resume" => {
                self.tool_registry.set_knowledge_learning_paused(false);
                self.persist_config_to_vfs();
                let call = alloyos_forge::provider::ToolCall::new(
                    "knowledge_resume_cmd",
                    "knowledge_search",
                    r#"{"query":"*"}"#,
                );
                let out = self
                    .tool_registry
                    .execute_tool(&call, &mut self.ui_synthesizer)?;
                let msg = "Knowledge profile learning resumed and persisted to /etc/ai.conf."
                    .to_string();
                on_token(&msg);
                Ok(ConsoleTurnOutput {
                    active_provider: self.active_provider_name(),
                    active_model: self.active_model_id().to_string(),
                    response_text: msg,
                    tool_outputs: vec![out],
                    config_persisted: true,
                })
            }
            "forget" => {
                if arg.is_empty() {
                    return Err(AiError::InvalidToolArguments(
                        "Usage: /knowledge forget <query>".into(),
                    ));
                }
                let call = alloyos_forge::provider::ToolCall::new(
                    "knowledge_forget_cmd",
                    "knowledge_forget",
                    format!(
                        r#"{{"query":"{}"}}"#,
                        alloyos_forge::provider::json_escape(arg)
                    ),
                );
                let out = self
                    .tool_registry
                    .execute_tool(&call, &mut self.ui_synthesizer)?;
                let msg = format!("Forgot knowledge entries matching '{}'.", arg);
                on_token(&msg);
                Ok(ConsoleTurnOutput {
                    active_provider: self.active_provider_name(),
                    active_model: self.active_model_id().to_string(),
                    response_text: msg,
                    tool_outputs: vec![out],
                    config_persisted: false,
                })
            }
            "correct" => {
                let Some((lhs, rhs)) = arg.split_once('=') else {
                    return Err(AiError::InvalidToolArguments(
                        "Usage: /knowledge correct [category:]<title> = <value>".into(),
                    ));
                };
                let lhs = lhs.trim();
                let value = rhs.trim();
                let (category, title) = match lhs.split_once(':') {
                    Some((cat, t)) => (cat.trim(), t.trim()),
                    None => ("profile", lhs),
                };
                let call = alloyos_forge::provider::ToolCall::new(
                    "knowledge_correct_cmd",
                    "knowledge_upsert",
                    format!(
                        r#"{{"category":"{}","title":"{}","value":"{}","basis":"explicit","source":"User correction (/knowledge correct)","scope":"global"}}"#,
                        alloyos_forge::provider::json_escape(category),
                        alloyos_forge::provider::json_escape(title),
                        alloyos_forge::provider::json_escape(value),
                    ),
                );
                let out = self
                    .tool_registry
                    .execute_tool(&call, &mut self.ui_synthesizer)?;
                let msg = format!(
                    "Corrected knowledge entry '{}' in '{}' to '{}'.",
                    title, category, value
                );
                on_token(&msg);
                Ok(ConsoleTurnOutput {
                    active_provider: self.active_provider_name(),
                    active_model: self.active_model_id().to_string(),
                    response_text: msg,
                    tool_outputs: vec![out],
                    config_persisted: false,
                })
            }
            other => Err(AiError::InvalidToolArguments(format!(
                "Unknown /knowledge subcommand '{}'. Supported: inspect, search, pause, resume, forget, correct",
                other
            ))),
        }
    }

    fn handle_apps_command(
        &mut self,
        rest: &str,
        on_token: &mut dyn FnMut(&str),
    ) -> Result<ConsoleTurnOutput, AiError> {
        let filter = rest.trim();
        let filter_app = if filter.is_empty() || filter == "list" || filter == "status" {
            None
        } else if let Some(app) = filter.strip_prefix("inspect ") {
            Some(app.trim())
        } else if let Some(app) = filter.strip_prefix("status ") {
            Some(app.trim())
        } else {
            Some(filter)
        };

        let packages_json = self.tool_registry.vfs_read("/proc/app-packages.json");
        let mut msg = String::new();

        if let Some(ref json_str) = packages_json {
            if let Ok(value) = serde_json::from_str::<serde_json::Value>(json_str) {
                if let Some(pkgs) = value.get("packages").and_then(|v| v.as_array()) {
                    let total = pkgs.len();
                    let ready_count = pkgs
                        .iter()
                        .filter(|p| p.get("ready").and_then(|r| r.as_bool()).unwrap_or(false))
                        .count();

                    msg.push_str(&format!(
                        "Installed Application Packages (/apps): {} package(s), {} ready.",
                        total, ready_count
                    ));

                    for pkg in pkgs {
                        let app_id = pkg.get("app_id").and_then(|v| v.as_str()).unwrap_or("unknown");
                        if let Some(target) = filter_app {
                            if app_id != target {
                                continue;
                            }
                        }

                        let ver = pkg.get("version").and_then(|v| v.as_str()).unwrap_or("unknown");
                        let ready = pkg.get("ready").and_then(|v| v.as_bool()).unwrap_or(false);
                        let rev = pkg.get("revision").and_then(|v| v.as_u64()).unwrap_or(0);
                        let val_rev = pkg.get("validated_revision").and_then(|v| v.as_u64());

                        let status_label = if ready {
                            "Ready".to_string()
                        } else if val_rev.is_none() {
                            "Validation Pending".to_string()
                        } else if val_rev.unwrap() < rev {
                            "Stale Revision (Revalidation Pending)".to_string()
                        } else {
                            "Validation Error".to_string()
                        };

                        msg.push_str(&format!(
                            "\n- [{}] v{} · Status: {} (rev: {}, validated_rev: {})",
                            app_id,
                            ver,
                            status_label,
                            rev,
                            val_rev.map(|r| r.to_string()).unwrap_or_else(|| "none".to_string())
                        ));

                        if let Some(diags) = pkg.get("diagnostics").and_then(|v| v.as_array()) {
                            for diag in diags {
                                let path = diag.get("path").and_then(|v| v.as_str()).unwrap_or("");
                                let err_msg = diag.get("message").and_then(|v| v.as_str()).unwrap_or("");
                                msg.push_str(&format!("\n    * Error in {}: {}", path, err_msg));
                            }
                        }
                    }
                }
            }
        }

        if msg.is_empty() {
            let reg = self.tool_registry.mcp_registry();
            let manifests = reg.manifests();
            msg = format!(
                "Installed Application Packages (/apps): {} package(s).\n",
                manifests.len()
            );
            for m in manifests {
                if let Some(target) = filter_app {
                    if m.app_id != target {
                        continue;
                    }
                }
                msg.push_str(&format!(
                    "\n- [{}] {} v{} · Ready",
                    m.app_id, m.name, m.version
                ));
            }
        }

        on_token(&msg);
        Ok(ConsoleTurnOutput {
            active_provider: self.active_provider_name(),
            active_model: self.active_model_id().to_string(),
            response_text: msg,
            tool_outputs: Vec::new(),
            config_persisted: false,
        })
    }

    /// Preferred name of the active user retrieved from `/knowledge/profile.md`, if known.
    pub fn user_preferred_name(&mut self) -> Option<String> {
        let entries = self
            .tool_registry
            .knowledge_entries_for(&crate::knowledge::KnowledgeCategory::Profile);
        entries
            .into_iter()
            .find(|e| {
                let norm = crate::knowledge::normalize_topic_key(&e.title);
                norm == "preferred name" || norm == "name"
            })
            .map(|e| e.value)
    }

    /// Processes a user input line from the Ferrofluid AI Console:
    /// - Rejects `!` shell escapes and `sh`/`bash` commands with `AiError::ShellAccessForbidden`
    /// - Handles `/model gemini`, `/model claude`, `/model chatgpt` hot-swapping and persists `/etc/ai.conf`
    /// - Handles `/knowledge` inspection, search, pause/resume, forget, and correction commands
    /// - Handles `/apps` and `/packages` installed package status and diagnostics inspection
    /// - Automatically extracts and merges durable user knowledge from conversational turns and injects
    ///   bounded profile + relevant retrieved context into the model request.
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

        if let Some(rest) = trimmed.strip_prefix("/knowledge") {
            if rest.is_empty() || rest.starts_with(char::is_whitespace) {
                return self.handle_knowledge_command(rest, on_token);
            }
        }

        if let Some(rest) = trimmed
            .strip_prefix("/apps")
            .or_else(|| trimmed.strip_prefix("/packages"))
        {
            if rest.is_empty() || rest.starts_with(char::is_whitespace) {
                return self.handle_apps_command(rest, on_token);
            }
        }

        let report = self.tool_registry.learn_from_conversation_turn(trimmed);
        if report.has_changes() {
            self.refresh_knowledge_tile_if_present();
        }

        self.history.push(Message::user(trimmed));
        let messages = self.messages_with_knowledge_context(trimmed);
        let tools_snapshot = self.tool_registry.specs().to_vec();

        self.turn_tool_calls = 0;
        self.turn_tool_outputs.clear();
        let mut messages = messages;
        loop {
            let response = self.gateway.chat(&messages, &tools_snapshot, on_token)?;
            if response.tool_calls.is_empty() { return self.apply_response(response); }
            self.execute_response_tools(&response)?;
            messages = self.messages_with_knowledge_context(trimmed);
        }
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
        if trimmed.starts_with("/model")
            || trimmed == "/knowledge"
            || trimmed.starts_with("/knowledge ")
            || trimmed == "/apps"
            || trimmed.starts_with("/apps ")
            || trimmed == "/packages"
            || trimmed.starts_with("/packages ")
        {
            return self.handle_console_input(input, &mut |_| {}).map(Some);
        }

        let report = self.tool_registry.learn_from_conversation_turn(trimmed);
        if report.has_changes() {
            self.refresh_knowledge_tile_if_present();
        }

        self.history.push(Message::user(trimmed));
        let messages = self.messages_with_knowledge_context(trimmed);
        self.turn_tool_calls = 0;
        self.turn_tool_outputs.clear();
        let tools = self.tool_registry.specs().to_vec();
        if let Err(error) = self.gateway.start_chat_with_tools(&messages, &tools) {
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
                let response = match result { Ok(response) => response, Err(error) => return Poll::Ready(Err(error)) };
                if response.tool_calls.is_empty() { return Poll::Ready(self.apply_response(response)); }
                if let Err(error) = self.execute_response_tools(&response) { return Poll::Ready(Err(error)); }
                let tools = self.tool_registry.specs().to_vec();
                let user = self.history.iter().rev().find(|m| m.role == alloyos_forge::provider::Role::User)
                    .map(|m| m.content.clone()).unwrap_or_default();
                let messages = self.messages_with_knowledge_context(&user);
                match self.gateway.start_chat_with_tools(&messages, &tools) {
                    Ok(()) => Poll::Pending,
                    Err(error) => Poll::Ready(Err(error)),
                }
            }
        }
    }

    fn execute_response_tools(&mut self, response: &Response) -> Result<(), AiError> {
        if self.turn_tool_calls + response.tool_calls.len() > 8 {
            return Err(AiError::PolicyViolation("Tool call budget exhausted".into()));
        }
        self.turn_tool_calls += response.tool_calls.len();
        self.history.push(Message::assistant_with_tools(&response.text, response.tool_calls.clone()));
        for call in &response.tool_calls {
            let output = match self.tool_registry.execute_tool(call, &mut self.ui_synthesizer) {
                Ok(output) => output,
                Err(error @ (AiError::InvalidToolArguments(_) | AiError::UnknownTool(_))) => {
                    // Complete the tool exchange so the model can repair a bad
                    // call and the next user turn never inherits a dangling call.
                    ToolExecutionOutput {
                        tool_name: call.name.clone(),
                        output_json: serde_json::json!({"ok":false,"error":error.to_string()}).to_string(),
                        synthesized_view: None,
                    }
                }
                Err(error) => return Err(error),
            };
            let model_output = if call.name == "mcp_list_tools" {
                // Discovery needs schemas, not every rendered tile and full skill body.
                // Keep the complete result for the UI, and a compact inventory for the model.
                let value: serde_json::Value = serde_json::from_str(&output.output_json)
                    .map_err(|_| AiError::ParseError("Invalid app inventory".into()))?;
                format!("{{\"ok\":true,\"headless_mcp_apps\":{},\"tools\":{},\"resources\":{}}}",
                    value["headless_mcp_apps"], value["tools"], value["resources"])
            } else { output.output_json.clone() };
            self.history.push(Message::tool_result(&call.id, model_output));
            self.turn_tool_outputs.push(output);
        }
        Ok(())
    }
    fn apply_response(&mut self, response: Response) -> Result<ConsoleTurnOutput, AiError> {
        if !response.tool_calls.is_empty() { self.execute_response_tools(&response)?; }
        else { self.history.push(Message::assistant(&response.text)); }
        Ok(ConsoleTurnOutput {
            active_provider: self.active_provider_name(),
            active_model: response.model_name,
            response_text: response.text,
            tool_outputs: core::mem::take(&mut self.turn_tool_outputs),
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
    /// The native tool registry keeps MCP and knowledge orchestration under Welder ownership.
    pub fn execute_tile_action(
        &mut self,
        action: &alloyos_mcp::TileAction,
    ) -> Result<ToolExecutionOutput, AiError> {
        let tiles = self.synthesized_tiles();
        let matching: Vec<_> = tiles.iter().filter(|tile| tile.actions.contains(action)).collect();
        if matching.len() != 1 {
            return Err(AiError::PolicyViolation("tile action is unexposed or ambiguous".into()));
        }
        self.execute_tile_action_for(&matching[0].tile_id, action)
    }

    /// Authorize against the current descriptor of the explicitly selected tile.
    pub fn execute_tile_action_for(
        &mut self,
        tile_id: &str,
        action: &alloyos_mcp::TileAction,
    ) -> Result<ToolExecutionOutput, AiError> {
        let tiles = self.synthesized_tiles();
        let tile = tiles.iter().find(|tile| tile.tile_id == tile_id && tile.actions.contains(action))
            .ok_or_else(|| AiError::PolicyViolation("tile action is not currently exposed".into()))?;
        let app = tile.folder()
            .ok_or_else(|| AiError::InvalidToolArguments("tile has no app namespace".into()))?;

        if app == "knowledge" {
            match action.tool_name.as_str() {
                "knowledge.pause" => {
                    self.tool_registry.set_knowledge_learning_paused(true);
                    self.persist_config_to_vfs();
                    let call = alloyos_forge::provider::ToolCall::new(
                        "tile_action",
                        "knowledge_search",
                        r#"{"query":"*"}"#,
                    );
                    return self
                        .tool_registry
                        .execute_tool(&call, &mut self.ui_synthesizer);
                }
                "knowledge.resume" => {
                    self.tool_registry.set_knowledge_learning_paused(false);
                    self.persist_config_to_vfs();
                    let call = alloyos_forge::provider::ToolCall::new(
                        "tile_action",
                        "knowledge_search",
                        r#"{"query":"*"}"#,
                    );
                    return self
                        .tool_registry
                        .execute_tool(&call, &mut self.ui_synthesizer);
                }
                "knowledge_search" | "knowledge_upsert" | "knowledge_forget" => {
                    let args = alloyos_mcp::McpValue::Map(action.default_args.clone()).to_json();
                    let call = alloyos_forge::provider::ToolCall::new(
                        "tile_action",
                        &action.tool_name,
                        args,
                    );
                    return self
                        .tool_registry
                        .execute_tool(&call, &mut self.ui_synthesizer);
                }
                _ => {
                    return Err(AiError::PolicyViolation(
                        "unsupported knowledge tile action".into(),
                    ))
                }
            }
        }

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

    /// Refresh only the resource URI exposed by the selected current tile.
    pub fn refresh_tile(&mut self, tile_id: &str) -> Result<ToolExecutionOutput, AiError> {
        let tile = self.synthesized_tiles().into_iter().find(|tile| tile.tile_id == tile_id)
            .ok_or_else(|| AiError::PolicyViolation("tile is not currently exposed".into()))?;
        let app = tile.folder().ok_or_else(|| AiError::InvalidToolArguments("tile has no app namespace".into()))?.to_string();
        if app == "knowledge" {
            let call = alloyos_forge::provider::ToolCall::new(
                "tile_refresh",
                "knowledge_search",
                r#"{"query":"*"}"#,
            );
            return self
                .tool_registry
                .execute_tool(&call, &mut self.ui_synthesizer);
        }
        let uri = tile.refresh_resource_uri.ok_or_else(|| AiError::PolicyViolation("tile exposes no refresh resource".into()))?;
        let arguments = format!("{{\"app\":\"{}\",\"uri\":\"{}\"}}",
            alloyos_forge::provider::json_escape(&app), alloyos_forge::provider::json_escape(&uri));
        let call = alloyos_forge::provider::ToolCall::new("tile_refresh", "mcp_read_resource", arguments);
        // Resource dispatch must not publish a different tile from the same app.
        let mut synthesis = self.ui_synthesizer.clone();
        let mut output = self.tool_registry.execute_tool(&call, &mut synthesis)?;
        let refreshed = self.tool_registry.mcp_registry().render_tile(tile_id)
            .map_err(|error| AiError::InvalidToolArguments(error.message))?;
        let view = self.ui_synthesizer.synthesize_from_mcp_output_with_tile(
            &crate::ui_synth::UiCallerOrigin::AiAgent, &app, &uri, &output.output_json, Some(refreshed))?;
        output.synthesized_view = Some(view);
        Ok(output)
    }

    pub fn pinned_tile_ids(&self) -> &[String] {
        &self.pinned_tile_ids
    }

    /// Persist pin order. Unavailable apps retain their pins for later restoration.
    pub fn set_pinned_tile_ids(&mut self, ids: &[String]) -> Result<(), AiError> {
        let mut unique = Vec::new();
        for id in ids {
            if id.is_empty() || id.contains('\n') || id.contains('\r') || !id.contains('.') {
                return Err(AiError::InvalidToolArguments("invalid pinned tile ID".into()));
            }
            if !unique.contains(id) { unique.push(id.clone()); }
        }
        self.tool_registry.persist_ui_config(Self::DASHBOARD_CONFIG_PATH, &unique.join("\n"))
            .map_err(AiError::ConfigError)?;
        self.pinned_tile_ids = unique;
        Ok(())
    }

    /// Populate filesystem-discovered app cards without a model request.
    pub fn initialize_dashboard_tiles(&mut self) -> Result<(), AiError> {
        let descriptors = {
            let registry = self.tool_registry.mcp_registry();
            registry.list_tiles().into_iter().filter_map(|tile| registry.render_tile(&tile.tile_id).ok()).collect::<Vec<_>>()
        };
        for tile in descriptors {
            let app = tile.folder().unwrap_or("").to_string();
            let id = tile.tile_id.clone();
            self.ui_synthesizer.synthesize_from_mcp_output_with_tile(
                &crate::ui_synth::UiCallerOrigin::AiAgent, &app, &id, "{}", Some(tile))?;
        }
        Ok(())
    }

    fn restore_dashboard(&mut self) {
        if self.tool_registry.vfs_read(Self::DASHBOARD_CONFIG_PATH).is_none() {
            self.pinned_tile_ids = Vec::new();
            return;
        }
        let saved = self.tool_registry.vfs_read(Self::DASHBOARD_CONFIG_PATH)
            .unwrap_or("").to_string();
        self.pinned_tile_ids.clear();
        for id in saved.lines().filter(|id| !id.is_empty() && id.contains('.')) {
            if !self.pinned_tile_ids.iter().any(|existing| existing == id) {
                self.pinned_tile_ids.push(id.to_string());
            }
        }
    }

    /// Returns all UI views synthesized by the AI agent for Ferrofluid (`ferrofluidd`).
    pub fn synthesized_views(&self) -> &[SynthesizedUiView] {
        self.ui_synthesizer.views()
    }
}

#[cfg(test)]
#[path = "../../../tests/support/apps.rs"]
mod fixture_apps;

#[cfg(test)]
mod tests {
    use super::fixture_apps;
    fn runtime_with_apps() -> AgentRuntime {
        let backend = crate::SharedMapVfsBackend::shared();
        backend.borrow_mut().files.extend(fixture_apps::files());
        backend.borrow_mut().files.insert("/etc/ai.conf".into(), "provider=gemini\nmodel=gemini-3.8-flash\n".into());
        AgentRuntime::with_vfs_backend(backend)
    }

    use super::*;
    use crate::{ProviderToolCall as ToolCall, ToolSpec, UiCallerOrigin, ViewKind};
    use alloyos_forge::provider::TokenUsage;
    #[test]
    fn invalid_generated_script_is_returned_to_model_and_next_turn_can_continue() {
        use alloc::boxed::Box;
        use alloyos_forge::{
            ForgeError, Generation, GenerationRequest, InferenceBackend, DEFAULT_MODEL,
        };
        struct Model {
            step: usize,
        }
        impl InferenceBackend for Model {
            fn resident_bytes(&self) -> u64 {
                0
            }
            fn generate(&mut self, request: &GenerationRequest) -> Result<Generation, ForgeError> {
                let text = match self.step {
                    0 => serde_json::json!({"kind":"tools","calls":[{"name":"script_create","arguments":{"path":"/tmp/get_time.js","source":"console.log(new Date());"}}]}).to_string(),
                    1 => {
                        assert!(request.prompt.contains("JavaScript syntax error"));
                        assert!(request.prompt.contains("tool: {\"error\":"));
                        assert!(request.prompt.contains("restricted subset"));
                        r#"{"kind":"final","text":"Wall-clock time is unavailable."}"#.into()
                    }
                    2 => {
                        assert!(request.prompt.contains("Is Gmail MCP available?"));
                        r#"{"kind":"final","text":"Need to check the app registry."}"#.into()
                    }
                    _ => panic!("unexpected additional generation"),
                };
                self.step += 1;
                Ok(Generation {
                    model: DEFAULT_MODEL.into(),
                    text,
                    generated_tokens: 20,
                })
            }
        }
        let mut runtime = AgentRuntime::new();
        let forge = Rc::new(RefCell::new(Forge::new(0)));
        forge
            .borrow_mut()
            .attach(DEFAULT_MODEL, Box::new(Model { step: 0 }))
            .unwrap();
        runtime.gateway.set_forge(forge);
        runtime.switch_provider("local").unwrap();
        assert!(runtime
            .start_console_input("What time is it?")
            .unwrap()
            .is_none());
        let output = loop {
            if let Poll::Ready(output) = runtime.poll_console_reply() {
                break output.unwrap();
            }
        };
        assert_eq!(output.response_text, "Wall-clock time is unavailable.");
        assert_eq!(output.tool_outputs.len(), 1);
        assert!(output.tool_outputs[0].output_json.contains("\"ok\":false"));
        assert!(runtime.tool_registry.vfs_read("/tmp/get_time.js").is_none());
        let output = runtime
            .handle_console_input("Is Gmail MCP available?", &mut |_| {})
            .unwrap();
        assert_eq!(output.response_text, "Need to check the app registry.");
    }

    #[test]
    fn console_local_tool_loop_discovers_filesystem_apps_before_invoking_them() {
        use alloc::boxed::Box;
        use alloyos_forge::{InferenceBackend, Generation, GenerationRequest, ForgeError, DEFAULT_MODEL};
        struct Model { step: usize, app: String, tool: String }
        impl InferenceBackend for Model {
            fn resident_bytes(&self) -> u64 { 0 }
            fn generate(&mut self, request: &GenerationRequest) -> Result<Generation, ForgeError> {
                assert!(request.prompt.contains("mcp_list_tools:"));
                let text = match self.step {
                    0 => r#"{"kind":"tools","calls":[{"name":"mcp_list_tools","arguments":{}}]}"#.into(),
                    1 => {
                        assert!(request.prompt.contains("tool: {\"ok\":true,\"headless_mcp_apps\":"));
                        assert!(request.prompt.contains(&self.tool));
                        serde_json::json!({"kind":"tools","calls":[{"name":"mcp_call_tool","arguments":{"app":self.app,"tool":self.tool,"args":"{}"}}]}).to_string()
                    }
                    2 => {
                        assert!(request.prompt.contains("requested tool mcp_call_tool"));
                        assert_eq!(request.prompt.lines().filter(|line| line.starts_with("tool: ")).count(), 2);
                        r#"{"kind":"final","text":"Answer based on the app result."}"#.into()
                    }
                    _ => panic!("unbounded model loop"),
                };
                self.step += 1;
                Ok(Generation { model: DEFAULT_MODEL.into(), text, generated_tokens: 20 })
            }
        }
        for asynchronous in [false, true] {
            let mut runtime = runtime_with_apps();
            let tool = runtime.tool_registry.mcp_registry().list_tools().into_iter()
                .find(|tool| tool.parameters.is_empty() && tool.name.matches('.').count() == 1).unwrap();
            let app = tool.name.split('.').next().unwrap().to_string();
            let forge = Rc::new(RefCell::new(Forge::new(0)));
            forge.borrow_mut().attach(DEFAULT_MODEL, Box::new(Model { step: 0, app, tool: tool.name })).unwrap();
            runtime.gateway.set_forge(forge);
            runtime.switch_provider("forge").unwrap();
            let mut displayed = String::new();
            let output = if asynchronous {
                assert!(runtime.start_console_input("Read a live value from an available app.").unwrap().is_none());
                loop {
                    assert!(runtime.partial_console_reply().is_none(), "tool protocol must not leak into chat");
                    match runtime.poll_console_reply() {
                        Poll::Pending => {},
                        Poll::Ready(result) => break result.unwrap(),
                    }
                }
            } else {
                runtime.handle_console_input("Read a live value from an available app.", &mut |token| displayed.push_str(token)).unwrap()
            };
            assert_eq!(output.response_text, "Answer based on the app result.");
            assert_eq!(output.tool_outputs.len(), 2);
            if !asynchronous { assert_eq!(displayed, output.response_text); }
            assert_eq!(runtime.history.last().unwrap().content, output.response_text);
        }
    }

    #[test]
    fn app_inventory_comes_only_from_the_filesystem() {
        let backend = crate::SharedMapVfsBackend::shared();
        let mut runtime = AgentRuntime::with_vfs_backend(backend.clone());
        assert_eq!(runtime.tool_registry.mcp_registry().app_count(), 0);
        backend.borrow_mut().files.extend(fixture_apps::files());
        assert_eq!(runtime.tool_registry.refresh_app_packages().unwrap(), 3);
        backend.borrow_mut().files.retain(|path, _| !path.starts_with("/apps/"));
        assert_eq!(runtime.tool_registry.refresh_app_packages().unwrap(), 0);
        assert_eq!(runtime.tool_registry.mcp_registry().app_count(), 0);
        assert!(backend.borrow().files["/proc/app-packages.json"].contains("[]"));
    }

    #[test]
    fn shared_action_requires_tile_identity_and_stale_actions_are_rejected() {
        let mut runtime = runtime_with_apps();
        runtime.initialize_dashboard_tiles().unwrap();
        let mut sample_timer = runtime.synthesized_tiles().into_iter().find(|tile| tile.tile_id == "sample_timer.now").unwrap();
        let action = sample_timer.actions[0].clone();
        sample_timer.tile_id = "sample_timer.copy".into();
        runtime.ui_synthesizer.synthesize_from_mcp_output_with_tile(
            &crate::ui_synth::UiCallerOrigin::AiAgent, "sample_timer", "copy", "{}", Some(sample_timer.clone())).unwrap();
        assert!(matches!(runtime.execute_tile_action(&action), Err(AiError::PolicyViolation(_))));
        runtime.execute_tile_action_for("sample_timer.now", &action).unwrap();
        sample_timer.actions.clear();
        runtime.ui_synthesizer.synthesize_from_mcp_output_with_tile(
            &crate::ui_synth::UiCallerOrigin::AiAgent, "sample_timer", "copy", "{}", Some(sample_timer)).unwrap();
        assert!(matches!(runtime.execute_tile_action_for("sample_timer.copy", &action), Err(AiError::PolicyViolation(_))));
    }

    #[test]
    fn dashboard_pin_order_restores_and_empty_is_intentional() {
        let backend = Rc::new(RefCell::new(crate::tools::SharedMapVfsBackend::new()));
        let mut runtime = AgentRuntime::with_vfs_backend(backend.clone());
        assert!(runtime.pinned_tile_ids().is_empty());
        runtime.set_pinned_tile_ids(&["sample_scheduler.board".into(), "sample_timer.now".into(), "sample_scheduler.board".into()]).unwrap();
        let mut restored = AgentRuntime::with_vfs_backend(backend.clone());
        assert_eq!(restored.pinned_tile_ids(), &["sample_scheduler.board", "sample_timer.now"]);
        restored.set_pinned_tile_ids(&[]).unwrap();
        assert!(AgentRuntime::with_vfs_backend(backend).pinned_tile_ids().is_empty());
    }

    #[test]
    fn resource_refresh_is_scoped_to_current_tile() {
        let mut runtime = runtime_with_apps();
        runtime.initialize_dashboard_tiles().unwrap();
        runtime.tick_scheduler(2000, None);
        let output = runtime.refresh_tile("sample_timer.now").unwrap();
        assert_eq!(output.synthesized_view.unwrap().tile.unwrap().tile_id, "sample_timer.now");
        assert!(runtime.synthesized_tiles().iter().any(|tile| tile.tile_id == "sample_timer.now" && tile.items[0].title == "00:00:02"));
        assert!(matches!(runtime.refresh_tile("sample_timer.unknown"), Err(AiError::PolicyViolation(_))));
        let action = runtime.synthesized_tiles().iter().find(|tile| tile.tile_id == "sample_timer.now").unwrap().actions[0].clone();
        assert!(matches!(runtime.execute_tile_action_for("sample_scheduler.board", &action), Err(AiError::PolicyViolation(_))));
        let mut stale = action;
        stale.label.push_str("stale");
        assert!(matches!(runtime.execute_tile_action_for("sample_timer.now", &stale), Err(AiError::PolicyViolation(_))));
    }

    #[test]
    fn app_tile_refresh_uses_live_host_uptime() {
        let mut runtime = runtime_with_apps();
        runtime.tick_scheduler(45_296_000, None);
        let call = ToolCall::new("sample_timer", "mcp_call_tool",
            "{\"app\":\"sample_timer\",\"tool\":\"sample_timer.now\",\"args\":\"{}\"}");
        runtime.tool_registry.execute_tool(&call, &mut runtime.ui_synthesizer).unwrap();
        let tile = runtime.synthesized_tiles().remove(0);
        assert_eq!(tile.tile_id, "sample_timer.now");
        assert_eq!(tile.items[0].title, "12:34:56");
        runtime.tick_scheduler(45_297_000, None);
        let refreshed = runtime.execute_tile_action(&tile.actions[0]).unwrap();
        assert!(refreshed.output_json.contains("45297000"));
        assert_eq!(runtime.synthesized_tiles()[0].items[0].title, "12:34:57");
        assert_eq!(runtime.synthesized_tiles().len(), 1);
    }

    #[test]
    fn tile_refresh_routes_through_welder_and_replaces_latest_descriptor() {
        let mut runtime = runtime_with_apps();
        let call = ToolCall::new("initial", "mcp_call_tool",
            "{\"app\":\"sample_scheduler\",\"tool\":\"sample_scheduler.list\",\"args\":\"{}\"}");
        runtime.tool_registry.execute_tool(&call, &mut runtime.ui_synthesizer).unwrap();
        let initial = runtime.synthesized_tiles();
        assert_eq!(initial.len(), 1);
        let action = initial[0].actions.iter().find(|a| a.tool_name == "sample_scheduler.list").unwrap().clone();
        let output = runtime.execute_tile_action(&action).unwrap();
        assert_eq!(output.tool_name, "mcp_call_tool");
        assert_eq!(runtime.synthesized_tiles().len(), 1);
        assert_eq!(output.synthesized_view.unwrap().tile.unwrap(), runtime.synthesized_tiles()[0]);
        let mut forged = action;
        forged.tool_name = "sample_scheduler.delete".into();
        assert!(matches!(runtime.execute_tile_action(&forged), Err(AiError::PolicyViolation(_))));
    }

    #[test]
    fn runtime_hot_swapping_and_etc_ai_conf_persistence() {
        let mut runtime = runtime_with_apps();
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
        let mut restored = runtime_with_apps();
        restored
            .load_config_str("provider=claude\nmodel=claude-sonnet-4-6\n")
            .expect("config should load");
        assert_eq!(restored.active_provider_name(), "claude");
        assert_eq!(restored.active_model_id(), "claude-sonnet-4-6");
    }

    #[test]
    fn shell_commands_and_tools_are_strictly_blocked() {
        let mut runtime = runtime_with_apps();

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
        let mut runtime = runtime_with_apps();
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

        // AI Agent invokes Sample Scheduler MCP tool and synthesizes ViewKind::McpAppView with declarative AppTile
        let turn = runtime
            .apply_response(Response::new(
                "Listing scheduled sample_scheduler via Sample Scheduler app",
                vec![ToolCall::new(
                    "mcp-1",
                    "mcp_call_tool",
                    "{\"app\":\"sample_scheduler\",\"tool\":\"sample_scheduler.list\",\"args\":\"{}\"}",
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
        assert_eq!(mcp_view.source_app.as_deref(), Some("sample_scheduler"));
        assert_eq!(mcp_view.title, "Sample Scheduler");
        assert!(mcp_view.tile.is_some());
        assert_eq!(mcp_view.tile.as_ref().unwrap().tile_id, "sample_scheduler.board");

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

        // AI Agent schedules a JS script (routed through the scheduler service) and synthesizes ViewKind::ScriptScheduleList
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
        assert_eq!(sched_view.source_app.as_deref(), None);
        assert!(sched_view.tile.is_none());
    }
    #[test]
    fn cloud_is_default_and_explicit_local_never_silently_uses_cloud() {
        let mut ai = runtime_with_apps();
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
        let mut runtime = runtime_with_apps();
        let mut ui = AgentUiSynthesizer::new();

        // 1. Create and immediately run a script via Welder tools
        let create_call = ToolCall::new(
            "c1",
            "script_create",
            r#"{"path":"/scripts/compute.js","code":"let s = AlloyOS.sys.status(); AlloyOS.fs.write('/var/computed.txt', 'sample_scheduler=' + s.activeTasks); AlloyOS.ui.notify('Compute: Done'); s.activeTasks * 10;"}"#,
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
            Some("sample_scheduler=6")
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

        // Advancing the scheduler sample_timer executes the restored `/scripts/tick_counter.js`
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
    fn filesystem_app_crud_tiles_and_cross_app_isolation() {
        let mut runtime = runtime_with_apps();
        let mut ui = AgentUiSynthesizer::new();

        // 1. Discover apps, tools, SKILLs, and Tiles via mcp_list_tools
        let list_out = runtime
            .tool_registry
            .execute_tool(&ToolCall::new("l1", "mcp_list_tools", "{}"), &mut ui)
            .expect("mcp_list_tools must succeed");
        assert!(list_out.output_json.contains(r#""headless_mcp_apps":["sample-inventory","sample_scheduler","sample_timer"]"#));
        assert!(list_out.output_json.contains(r#""sample_scheduler.list""#));
        assert!(list_out.output_json.contains(r#""sample_scheduler.create""#));
        assert!(list_out.output_json.contains(r#""sample_scheduler.edit""#));
        assert!(list_out.output_json.contains(r#""sample_scheduler.delete""#));
        assert!(list_out.output_json.contains(r#""/apps/sample_scheduler/SKILL.md""#));
        assert!(list_out.output_json.contains(r#""sample_scheduler.board""#));

        // 2. Create a scheduled task through Sample Scheduler app via Welder
        let create_task = runtime
            .tool_registry
            .execute_tool(
                &ToolCall::new(
                    "t1",
                    "mcp_call_tool",
                    r#"{"app":"sample_scheduler","tool":"sample_scheduler.create","id":"daily-check","path":"/scripts/heartbeat.js","trigger":"every:60s"}"#,
                ),
                &mut ui,
            )
            .expect("sample_scheduler.create must succeed");
        assert!(create_task.output_json.contains(r#""status":"created""#));
        assert!(create_task.output_json.contains(r#""id":"daily-check""#));
        let tile = create_task
            .synthesized_view
            .as_ref()
            .and_then(|v| v.tile.as_ref())
            .expect("sample_scheduler.create must synthesize view with sample_scheduler.board AppTile");
        assert_eq!(tile.tile_id, "sample_scheduler.board");
        assert_eq!(tile.items.len(), 1);
        assert_eq!(tile.items[0].id, "daily-check");
        assert_eq!(tile.items[0].badge, "interval:60000");



        // 3. Edit the scheduled task through Sample Scheduler app via Welder
        let edit_task = runtime
            .tool_registry
            .execute_tool(
                &ToolCall::new(
                    "t2",
                    "mcp_call_tool",
                    r#"{"app":"sample_scheduler","tool":"sample_scheduler.edit","id":"daily-check","trigger":"cron:*/5 * * * *"}"#,
                ),
                &mut ui,
            )
            .expect("sample_scheduler.edit must succeed");
        assert!(edit_task.output_json.contains(r#""status":"edited""#));
        assert!(edit_task.output_json.contains(r#""cron:*/5 * * * *""#));

        assert!(runtime
            .tool_registry
            .vfs_read("/etc/schedules.conf")
            .unwrap()
            .contains("cron:*/5 * * * *"));

        // 4. Reject any cross-app invocation (another app trying to call Sample Scheduler)
        let cross_app_err = runtime
            .tool_registry
            .execute_tool(
                &ToolCall::new(
                    "t3",
                    "mcp_call_tool",
                    r#"{"caller_app":"other_app","app":"sample_scheduler","tool":"sample_scheduler.list"}"#,
                ),
                &mut ui,
            )
            .expect_err("cross-app invocation must be rejected");
        assert!(matches!(cross_app_err, AiError::PolicyViolation(_)));
    }

    #[test]
    fn user_knowledge_framework_learns_as_conversations_happen_and_injects_context() {
        use crate::knowledge::KnowledgeStorageKind;
        use alloc::boxed::Box;
        use alloyos_forge::{CloudModel, CloudTransport, ForgeError, Generation, GenerationRequest};

        struct CapturingTransport;
        impl CloudTransport for CapturingTransport {
            fn generate(
                &mut self,
                model: &CloudModel,
                request: &GenerationRequest,
            ) -> Result<Generation, ForgeError> {
                let sys_slice = request
                    .prompt
                    .lines()
                    .filter(|l| {
                        !l.starts_with("assistant:")
                            && (l.contains("Stored user knowledge context")
                                || l.contains("Wagner Zaniboni")
                                || l.contains("concise technical explanations"))
                    })
                    .collect::<Vec<_>>()
                    .join(" | ");
                Ok(Generation {
                    model: model.id.clone(),
                    text: format!("ack: {}", sys_slice),
                    generated_tokens: 4,
                })
            }
        }

        let forge = Rc::new(RefCell::new(Forge::new(0)));
        let cloud = CloudModel::new(
            "gemini-3.8-flash",
            "gemini",
            "gemini-3.8-flash",
            "test-credential",
        )
        .unwrap();
        forge.borrow_mut().register_cloud(&cloud).unwrap();
        forge
            .borrow_mut()
            .attach_cloud(cloud, Box::new(CapturingTransport))
            .unwrap();

        let mut runtime = AgentRuntime::with_forge(forge);
        assert_eq!(
            runtime.knowledge_storage_kind(),
            KnowledgeStorageKind::VolatileRamFs
        );
        let _ = runtime.set_vfs_backend(crate::tools::SharedMapVfsBackend::shared());
        runtime
            .switch_provider("gemini")
            .expect("switch to gemini-3.8-flash");
        assert_eq!(
            runtime.knowledge_storage_kind(),
            KnowledgeStorageKind::DurableCobaltFs
        );

        // 1. Conversation turn reveals explicit profile fact, global preference, and a component-scoped boot-screen preference
        runtime
            .handle_console_input(
                "My name is Wagner. I prefer concise technical explanations. Use a minimal black-and-white boot screen for alloyOS.",
                &mut |_| {},
            )
            .expect("conversation turn 1 should succeed");

        let profile_md = runtime
            .tool_registry
            .vfs_read("/knowledge/profile.md")
            .expect("/knowledge/profile.md must exist");
        assert!(profile_md.contains("### Preferred name"));
        assert!(profile_md.contains("- Fact: Wagner"));
        assert!(profile_md.contains("- Basis: Explicit user statement."));
        assert!(profile_md.contains("- Scope: global"));

        let prefs_md = runtime
            .tool_registry
            .vfs_read("/knowledge/preferences.md")
            .expect("/knowledge/preferences.md must exist");
        assert!(prefs_md.contains("### Communication style"));
        assert!(prefs_md.contains("- Preference: concise technical explanations"));
        assert!(prefs_md.contains("### Visual style"));
        assert!(prefs_md.contains("- Scope: alloyOS boot screen"));

        // 2. Tentative observation goes to observations.md and never overwrites explicit facts
        runtime
            .handle_console_input(
                "Maybe I prefer dark terminal borders. Also maybe my name is Bob.",
                &mut |_| {},
            )
            .expect("conversation turn 2 should succeed");

        let obs_md = runtime
            .tool_registry
            .vfs_read("/knowledge/observations.md")
            .expect("/knowledge/observations.md must exist");
        assert!(obs_md.contains("- Basis: Inferred observation."));
        let profile_after_tentative = runtime
            .tool_registry
            .vfs_read("/knowledge/profile.md")
            .unwrap();
        assert!(
            profile_after_tentative.contains("- Fact: Wagner")
                && !profile_after_tentative.contains("Bob"),
            "Inferred observation must never overwrite explicit Preferred name"
        );

        // 3. Explicit user correction immediately supersedes earlier entry
        let turn3_out = runtime
            .handle_console_input(
                "Correction: my name is Wagner Zaniboni. My goal is building an AI-native ARM64 operating system.",
                &mut |_| {},
            )
            .expect("conversation turn 3 should succeed");

        let profile_after_correction = runtime
            .tool_registry
            .vfs_read("/knowledge/profile.md")
            .unwrap();
        assert!(profile_after_correction.contains("- Fact: Wagner Zaniboni"));

        let interests_md = runtime
            .tool_registry
            .vfs_read("/knowledge/interests.md")
            .unwrap();
        assert!(interests_md.contains("building an AI-native ARM64 operating system"));

        // 4. Verify bounded profile summary + relevant retrieved context is injected into model prompt
        let last_prompt = &turn3_out.response_text;
        assert!(last_prompt.contains("Stored user knowledge context"));
        assert!(last_prompt.contains("Wagner Zaniboni"));
        assert!(last_prompt.contains("concise technical explanations"));

        // 5. Secrets / API keys are strictly filtered from both conversational turns and knowledge_upsert
        runtime
            .handle_console_input(
                "My API key is sk-1234567890abcdef1234567890abcdef and my password is hunter2.",
                &mut |_| {},
            )
            .unwrap();
        for path in [
            "/knowledge/profile.md",
            "/knowledge/preferences.md",
            "/knowledge/interests.md",
            "/knowledge/observations.md",
        ] {
            let text = runtime.tool_registry.vfs_read(path).unwrap();
            assert!(!text.contains("sk-123456"));
            assert!(!text.contains("hunter2"));
        }

        let mut ui = AgentUiSynthesizer::new();
        let secret_upsert = runtime.tool_registry.execute_tool(
            &ToolCall::new(
                "k_sec",
                "knowledge_upsert",
                r#"{"category":"profile","title":"Cloud token","value":"Bearer eyJhbGciOiJIUzI1NiJ9"}"#,
            ),
            &mut ui,
        );
        assert!(matches!(secret_upsert, Err(AiError::PolicyViolation(_))));

        // Top-level /skills/ is rejected
        let skills_upsert = runtime.tool_registry.execute_tool(
            &ToolCall::new(
                "k_skills",
                "knowledge_upsert",
                r#"{"category":"/skills/coding.md","title":"Skill","value":"Rust"}"#,
            ),
            &mut ui,
        );
        assert!(matches!(skills_upsert, Err(AiError::InvalidToolArguments(_))));

        // 6. Pause learning via /knowledge pause or tile action, verify no new facts learned while paused
        let pause_out = runtime
            .handle_console_input("/knowledge pause", &mut |_| {})
            .expect("/knowledge pause should succeed");
        assert!(pause_out.config_persisted);
        assert!(runtime.is_learning_paused());
        assert!(runtime
            .tool_registry
            .vfs_read("/etc/ai.conf")
            .unwrap()
            .contains("knowledge_learning_paused=true"));

        runtime
            .handle_console_input("My timezone is UTC-3.", &mut |_| {})
            .unwrap();
        assert!(!runtime
            .tool_registry
            .vfs_read("/knowledge/profile.md")
            .unwrap()
            .contains("UTC-3"));

        // Resume learning via knowledge.profile AppTile action
        let k_tile = runtime
            .synthesized_tiles()
            .into_iter()
            .find(|t| t.tile_id == "knowledge.profile")
            .expect("knowledge.profile tile must be synthesized");
        let resume_action = k_tile
            .actions
            .iter()
            .find(|a| a.tool_name == "knowledge.resume")
            .expect("resume action must be present while paused")
            .clone();
        runtime
            .execute_tile_action_for("knowledge.profile", &resume_action)
            .expect("executing knowledge.resume tile action must succeed");
        assert!(!runtime.is_learning_paused());

        // Now learning is active again
        runtime
            .handle_console_input("My timezone is UTC-3.", &mut |_| {})
            .unwrap();
        assert!(runtime
            .tool_registry
            .vfs_read("/knowledge/profile.md")
            .unwrap()
            .contains("UTC-3"));

        // 7. Forget entries via natural language and /knowledge forget
        runtime
            .handle_console_input("Please forget my timezone.", &mut |_| {})
            .unwrap();
        assert!(!runtime
            .tool_registry
            .vfs_read("/knowledge/profile.md")
            .unwrap()
            .contains("UTC-3"));

        runtime
            .handle_console_input("/knowledge forget boot screen", &mut |_| {})
            .unwrap();
        assert!(!runtime
            .tool_registry
            .vfs_read("/knowledge/preferences.md")
            .unwrap()
            .contains("alloyOS boot screen"));

        // 8. Volatile RamFS backend explicitly reports non-persistent status
        let volatile_vfs = crate::tools::SharedMapVfsBackend::shared_volatile();
        let volatile_runtime = AgentRuntime::with_vfs_backend(volatile_vfs);
        assert_eq!(
            volatile_runtime.knowledge_storage_kind(),
            KnowledgeStorageKind::VolatileRamFs
        );
    }

    #[test]
    fn apps_and_packages_command_inspects_inventory_and_diagnostics() {
        let mut runtime = runtime_with_apps();

        // 1. Slash command /apps lists installed packages from registry fallback when /proc/app-packages.json is not present
        let mut tokens = Vec::new();
        let out = runtime
            .handle_console_input("/apps", &mut |t| tokens.push(t.to_string()))
            .expect("/apps should succeed");
        assert!(out.response_text.contains("Installed Application Packages (/apps)"));
        assert!(out.response_text.contains("sample_scheduler"));
        assert!(out.response_text.contains("sample-inventory"));

        // 2. Set mock /proc/app-packages.json with valid and errored packages
        let proc_json = serde_json::json!({
            "packages": [
                {
                    "app_id": "sample_scheduler",
                    "version": "1.0.0",
                    "revision": 2,
                    "validated_revision": 2,
                    "ready": true,
                    "diagnostics": []
                },
                {
                    "app_id": "broken-app",
                    "version": "0.2.0",
                    "revision": 5,
                    "validated_revision": 4,
                    "ready": false,
                    "diagnostics": [
                        {
                            "path": "/apps/broken-app/main.js",
                            "message": "ReferenceError: callTool is not defined"
                        }
                    ]
                }
            ]
        });
        runtime
            .tool_registry
            .vfs_write("/proc/app-packages.json", proc_json.to_string());

        // 3. Command /packages inspects status, readiness, revisions, and diagnostics
        let out_packages = runtime
            .handle_console_input("/packages", &mut |_| {})
            .expect("/packages should succeed");
        assert!(out_packages.response_text.contains("2 package(s), 1 ready"));
        assert!(out_packages.response_text.contains("[sample_scheduler] v1.0.0 · Status: Ready (rev: 2, validated_rev: 2)"));
        assert!(out_packages.response_text.contains("[broken-app] v0.2.0 · Status: Stale Revision (Revalidation Pending)"));
        assert!(out_packages.response_text.contains("Error in /apps/broken-app/main.js: ReferenceError: callTool is not defined"));

        // 4. Command /apps inspect <app_id> filters to the requested app
        let out_single = runtime
            .handle_console_input("/apps inspect broken-app", &mut |_| {})
            .expect("/apps inspect broken-app should succeed");
        assert!(out_single.response_text.contains("[broken-app]"));
        assert!(!out_single.response_text.contains("[sample_scheduler]"));
    }

    #[test]
    fn smithery_search_install_remote_tool_call_and_uninstall_lifecycle() {
        let mut runtime = AgentRuntime::with_vfs_backend(crate::SharedMapVfsBackend::shared());
        let mut ui = AgentUiSynthesizer::new();

        runtime.tool_registry.register_http_response(
            "https://api.smithery.ai/servers?q=github",
            r#"{"servers":[{"qualifiedName":"smithery-ai/github","displayName":"GitHub MCP"}]}"#,
        );
        runtime.tool_registry.register_http_response(
            "https://api.smithery.ai/servers/smithery-ai%2Fgithub",
            r#"{"qualifiedName":"smithery-ai/github","displayName":"GitHub MCP","remote":true,"deploymentUrl":"https://github.example/mcp","tools":[{"name":"create_issue","inputSchema":{"type":"object","properties":{"title":{"type":"string"}},"required":["title"]}}]}"#,
        );
        runtime.tool_registry.register_http_response(
            "POST https://github.example/mcp initialize",
            r#"{"jsonrpc":"2.0","id":1,"result":{"protocolVersion":"2025-06-18","capabilities":{"tools":{}},"serverInfo":{"name":"fixture","version":"1"}}}"#,
        );
        runtime.tool_registry.register_http_response("POST https://github.example/mcp notifications/initialized", "");
        // 1. Search Smithery registry for 'github'
        let search_call = ToolCall::new("s1", "smithery_search", r#"{"query":"github"}"#);
        let search_out = runtime
            .tool_registry
            .execute_tool(&search_call, &mut ui)
            .expect("smithery_search must succeed");
        assert!(search_out.output_json.contains("smithery-ai/github"));
        assert!(search_out.output_json.contains("GitHub MCP"));

        // 2. Register mock HTTP response for the remote Smithery endpoint
        runtime.tool_registry.register_http_response(
            "POST https://github.example/mcp tools/call",
            r#"{"jsonrpc":"2.0","id":2,"result":{"content":[{"type":"text","text":"Issue #101 created: Support Smithery MCP"}]}}"#,
        );

        // 3. Install the 3P MCP server from Smithery
        let install_call = ToolCall::new(
            "i1",
            "smithery_install",
            r#"{"server":"@smithery-ai/github","app_id":"github"}"#,
        );
        let install_out = runtime
            .tool_registry
            .execute_tool(&install_call, &mut ui)
            .expect("smithery_install must succeed");
        assert!(install_out.output_json.contains(r#""ok":true"#));
        assert!(install_out.output_json.contains("/apps/github/"));

        // Verify files were created on CobaltFS
        assert!(runtime.tool_registry.vfs_read("/apps/github/github.mcp").is_some());
        assert!(runtime.tool_registry.vfs_read("/apps/github/tools.json").is_some());
        assert!(runtime.tool_registry.vfs_read("/apps/github/SKILL.md").is_some());

        // 4. mcp_list_tools now discovers the remote 3P app
        let list_call = ToolCall::new("l1", "mcp_list_tools", "{}");
        let list_out = runtime
            .tool_registry
            .execute_tool(&list_call, &mut ui)
            .expect("mcp_list_tools must succeed");
        assert!(list_out.output_json.contains("github.create_issue"));

        // 5. Call a tool on the remote 3P app via mcp_call_tool
        let call_tool = ToolCall::new(
            "c1",
            "mcp_call_tool",
            r#"{"app":"github","tool":"create_issue","args":"{\"title\":\"Support Smithery MCP\"}"}"#,
        );
        let call_out = runtime
            .tool_registry
            .execute_tool(&call_tool, &mut ui)
            .expect("mcp_call_tool on remote 3P app must succeed");
        assert!(call_out.output_json.contains("Issue #101 created: Support Smithery MCP"));

        // 6. Uninstall the 3P app
        let uninstall_call = ToolCall::new(
            "u1",
            "smithery_uninstall",
            r#"{"app_id":"github"}"#,
        );
        let uninstall_out = runtime
            .tool_registry
            .execute_tool(&uninstall_call, &mut ui)
            .expect("smithery_uninstall must succeed");
        assert!(uninstall_out.output_json.contains(r#""ok":true"#));

        // 7. Verify mcp_list_tools no longer lists github
        let list_out2 = runtime
            .tool_registry
            .execute_tool(&list_call, &mut ui)
            .expect("mcp_list_tools must succeed");
        assert!(!list_out2.output_json.contains("github.create_issue"));
    }

}
