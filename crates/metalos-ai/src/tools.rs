use alloc::collections::BTreeMap;
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec;
use alloc::vec::Vec;

use crate::provider::{
    extract_json_string_field, json_escape, AiError, ToolCall, ToolParameter, ToolSpec,
};
use crate::ui_synth::{
    AgentUiSynthesizer, SynthesizedUiView, UiCallerOrigin, UiMetric, UiScheduleEntry, ViewKind,
};

/// Canonical explanation returned whenever a user or model attempts a POSIX shell escape or tool.
pub const SHELL_FREE_POLICY_MESSAGE: &str =
    "metalOS is a shell-free, AI-first operating system: POSIX shell access (`sh`, `bash`, `sh_exec`, `!cmd`) is strictly forbidden. Use structured native OS tools (`fs_*`, `sys_status`), local JavaScript automation (`script_create`, `script_run`, `script_schedule`), and headless MCP apps (`mcp_*`) instead.";

/// Returns `true` if a tool identifier corresponds to a forbidden shell/process execution primitive.
pub fn is_forbidden_shell_tool(tool_name: &str) -> bool {
    let normalized = tool_name.trim().to_ascii_lowercase();
    if normalized.starts_with('!')
        || normalized.starts_with("sh_")
        || normalized.starts_with("bash_")
        || normalized.starts_with("shell_")
    {
        return true;
    }

    matches!(
        normalized.as_str(),
        "sh"
            | "bash"
            | "zsh"
            | "fish"
            | "csh"
            | "ksh"
            | "dash"
            | "sh_exec"
            | "bash_exec"
            | "exec"
            | "execve"
            | "cmd"
            | "cmd_exec"
            | "shell"
            | "run_shell"
            | "run_terminal_cmd"
            | "system"
            | "popen"
            | "spawn_shell"
            | "/bin/sh"
            | "/bin/bash"
    )
}

/// Validates raw user or console input, rejecting `!` shell escapes and direct shell invocations.
pub fn validate_console_input_no_shell(input: &str) -> Result<(), AiError> {
    let trimmed = input.trim();
    if trimmed.starts_with('!') {
        return Err(AiError::ShellAccessForbidden(format!(
            "Rejected shell escape '{}'. {}",
            trimmed, SHELL_FREE_POLICY_MESSAGE
        )));
    }

    let first_token = trimmed.split_whitespace().next().unwrap_or("");
    if is_forbidden_shell_tool(first_token) {
        return Err(AiError::ShellAccessForbidden(format!(
            "Rejected shell command '{}'. {}",
            first_token, SHELL_FREE_POLICY_MESSAGE
        )));
    }

    Ok(())
}

/// Output produced by executing a structured native OS tool.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolExecutionOutput {
    pub tool_name: String,
    pub output_json: String,
    pub synthesized_view: Option<SynthesizedUiView>,
}

/// Registry of safe, structured AI-first OS capabilities in metalOS.
///
/// The AI agent has zero access to `sh` or POSIX shell execution; all OS interaction goes through
/// strongly-typed tools for CobaltFS, system telemetry, local JavaScript automation, headless MCP
/// apps, and Mercury UI synthesis.
#[derive(Debug, Clone)]
pub struct NativeToolRegistry {
    tools: Vec<ToolSpec>,
    vfs_files: BTreeMap<String, String>,
    js_schedules: Vec<(String, String)>,
    mcp_apps: BTreeMap<String, String>,
}

impl Default for NativeToolRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl NativeToolRegistry {
    pub fn new() -> Self {
        let mut vfs_files = BTreeMap::new();
        vfs_files.insert(
            "/etc/ai.conf".to_string(),
            "provider=gemini\nmodel=gemini-2.5-pro\n".to_string(),
        );
        vfs_files.insert(
            "/etc/schedules.conf".to_string(),
            "# metalOS local JavaScript schedule registry\n".to_string(),
        );
        vfs_files.insert(
            "/scripts/heartbeat.js".to_string(),
            "export function main() { return MetalOS.sys.status(); }\n".to_string(),
        );

        let mut mcp_apps = BTreeMap::new();
        mcp_apps.insert(
            "telemetry.mcp".to_string(),
            "{\"title\":\"Cluster Telemetry\",\"summary\":\"All 8 M4 cores nominal\",\"cpu_usage\":\"14\",\"memory_mb\":\"512\",\"rows\":[[\"core0\",\"active\"],[\"core1\",\"active\"]]}".to_string(),
        );
        mcp_apps.insert(
            "notes.mcp".to_string(),
            "{\"title\":\"Cobalt Notes\",\"summary\":\"2 indexed workspace notes\",\"value\":\"2\",\"label\":\"Notes\",\"unit\":\"docs\"}".to_string(),
        );

        let tools = Self::default_tool_specs();
        Self {
            tools,
            vfs_files,
            js_schedules: Vec::new(),
            mcp_apps,
        }
    }

    fn default_tool_specs() -> Vec<ToolSpec> {
        vec![
            ToolSpec::new(
                "fs_read",
                "Read UTF-8 file contents from CobaltFS or VFS path.",
                vec![ToolParameter::new(
                    "path",
                    "string",
                    "Absolute file path on CobaltFS (e.g. /etc/ai.conf)",
                    true,
                )],
            ),
            ToolSpec::new(
                "fs_write",
                "Write UTF-8 file contents to CobaltFS or VFS path.",
                vec![
                    ToolParameter::new("path", "string", "Absolute file path on CobaltFS", true),
                    ToolParameter::new("content", "string", "File contents to persist", true),
                ],
            ),
            ToolSpec::new(
                "fs_list",
                "List entries under a directory prefix on CobaltFS or VFS.",
                vec![ToolParameter::new(
                    "path",
                    "string",
                    "Directory path prefix (e.g. / or /scripts)",
                    true,
                )],
            ),
            ToolSpec::new(
                "sys_status",
                "Inspect metalOS kernel memory allocation, active tasks, uptime, and VirtIO-Net status.",
                Vec::new(),
            ),
            ToolSpec::new(
                "script_create",
                "Author and persist a local JavaScript program on CobaltFS (/scripts/*.js).",
                vec![
                    ToolParameter::new(
                        "path",
                        "string",
                        "Target script path under /scripts/ (e.g. /scripts/cleanup.js)",
                        true,
                    ),
                    ToolParameter::new(
                        "source",
                        "string",
                        "JavaScript ES module source code using MetalOS.* host bindings",
                        true,
                    ),
                ],
            ),
            ToolSpec::new(
                "script_run",
                "Execute a local JavaScript script inside the embedded metalOS JS runtime.",
                vec![ToolParameter::new(
                    "path",
                    "string",
                    "Path to the JavaScript file on CobaltFS",
                    true,
                )],
            ),
            ToolSpec::new(
                "script_schedule",
                "Schedule a local JavaScript script to run on a time trigger (cron/interval) or OS event.",
                vec![
                    ToolParameter::new("path", "string", "Path to the JavaScript file", true),
                    ToolParameter::new(
                        "trigger",
                        "string",
                        "Time or event trigger (e.g. 'every:60s', 'cron:*/5 * * * *', 'fs:modify:/etc')",
                        true,
                    ),
                ],
            ),
            ToolSpec::new(
                "mcp_list_tools",
                "List available tools and resources exposed by headless MCP developer applications.",
                Vec::new(),
            ),
            ToolSpec::new(
                "mcp_call_tool",
                "Invoke a structured tool on a headless MCP developer app and synthesize Mercury UI if needed.",
                vec![
                    ToolParameter::new("app", "string", "Headless MCP server identifier", true),
                    ToolParameter::new("tool", "string", "MCP tool name to invoke", true),
                    ToolParameter::new(
                        "args",
                        "string",
                        "JSON-serialized arguments for the MCP tool",
                        false,
                    ),
                ],
            ),
            ToolSpec::new(
                "mcp_read_resource",
                "Read a structured resource URI from a headless MCP developer application.",
                vec![
                    ToolParameter::new("app", "string", "Headless MCP server identifier", true),
                    ToolParameter::new("uri", "string", "Resource URI to read", true),
                ],
            ),
            ToolSpec::new(
                "ui_render_view",
                "Agent-exclusive tool to synthesize or update a generative UI view in Mercury (mercuryd).",
                vec![
                    ToolParameter::new(
                        "kind",
                        "string",
                        "View archetype: console, metric_dashboard, table, script_schedule_list, mcp_app_view",
                        true,
                    ),
                    ToolParameter::new("title", "string", "Window or panel title", true),
                    ToolParameter::new("summary", "string", "Concise summary for the view", true),
                ],
            ),
        ]
    }

    /// Returns the slice of registered safe OS tools.
    pub fn specs(&self) -> &[ToolSpec] {
        &self.tools
    }

    /// Looks up a registered tool specification by name.
    pub fn get_spec(&self, name: &str) -> Option<&ToolSpec> {
        self.tools.iter().find(|t| t.name == name)
    }

    /// Registers an additional structured tool (e.g., discovered from an MCP server) after
    /// verifying that it does not expose shell execution.
    pub fn register_tool(&mut self, spec: ToolSpec) -> Result<(), AiError> {
        if is_forbidden_shell_tool(&spec.name) {
            return Err(AiError::ShellAccessForbidden(format!(
                "Cannot register forbidden shell tool '{}'. {}",
                spec.name, SHELL_FREE_POLICY_MESSAGE
            )));
        }
        if let Some(existing) = self.tools.iter_mut().find(|t| t.name == spec.name) {
            *existing = spec;
        } else {
            self.tools.push(spec);
        }
        Ok(())
    }

    /// Reads a file directly from the virtualized CobaltFS state.
    pub fn vfs_read(&self, path: &str) -> Option<&str> {
        self.vfs_files.get(path).map(|s| s.as_str())
    }

    /// Writes a file directly into the virtualized CobaltFS state.
    pub fn vfs_write(&mut self, path: impl Into<String>, content: impl Into<String>) {
        self.vfs_files.insert(path.into(), content.into());
    }

    /// Executes a `ToolCall` against the native OS tool registry and updates `ui_synth` when applicable.
    pub fn execute_tool(
        &mut self,
        call: &ToolCall,
        ui_synth: &mut AgentUiSynthesizer,
    ) -> Result<ToolExecutionOutput, AiError> {
        if is_forbidden_shell_tool(&call.name) {
            return Err(AiError::ShellAccessForbidden(format!(
                "Attempted to invoke forbidden shell tool '{}'. {}",
                call.name, SHELL_FREE_POLICY_MESSAGE
            )));
        }

        match call.name.as_str() {
            "fs_read" => {
                let path = extract_json_string_field(&call.arguments_json, "path")
                    .ok_or_else(|| AiError::InvalidToolArguments("missing 'path'".to_string()))?;
                match self.vfs_files.get(&path) {
                    Some(content) => Ok(ToolExecutionOutput {
                        tool_name: call.name.clone(),
                        output_json: format!(
                            "{{\"ok\":true,\"path\":\"{}\",\"content\":\"{}\"}}",
                            json_escape(&path),
                            json_escape(content)
                        ),
                        synthesized_view: None,
                    }),
                    None => Ok(ToolExecutionOutput {
                        tool_name: call.name.clone(),
                        output_json: format!(
                            "{{\"ok\":false,\"error\":\"file not found\",\"path\":\"{}\"}}",
                            json_escape(&path)
                        ),
                        synthesized_view: None,
                    }),
                }
            }
            "fs_write" => {
                let path = extract_json_string_field(&call.arguments_json, "path")
                    .ok_or_else(|| AiError::InvalidToolArguments("missing 'path'".to_string()))?;
                let content = extract_json_string_field(&call.arguments_json, "content")
                    .ok_or_else(|| {
                        AiError::InvalidToolArguments("missing 'content'".to_string())
                    })?;
                let bytes = content.len();
                self.vfs_files.insert(path.clone(), content);
                Ok(ToolExecutionOutput {
                    tool_name: call.name.clone(),
                    output_json: format!(
                        "{{\"ok\":true,\"path\":\"{}\",\"bytes_written\":{}}}",
                        json_escape(&path),
                        bytes
                    ),
                    synthesized_view: None,
                })
            }
            "fs_list" => {
                let prefix = extract_json_string_field(&call.arguments_json, "path")
                    .unwrap_or_else(|| "/".to_string());
                let entries: Vec<String> = self
                    .vfs_files
                    .keys()
                    .filter(|k| k.starts_with(&prefix))
                    .cloned()
                    .collect();
                let joined = entries
                    .iter()
                    .map(|e| format!("\"{}\"", json_escape(e)))
                    .collect::<Vec<_>>()
                    .join(",");
                Ok(ToolExecutionOutput {
                    tool_name: call.name.clone(),
                    output_json: format!(
                        "{{\"ok\":true,\"prefix\":\"{}\",\"entries\":[{}]}}",
                        json_escape(&prefix),
                        joined
                    ),
                    synthesized_view: None,
                })
            }
            "sys_status" => {
                let view = ui_synth.synthesize_metric_dashboard(
                    "metalOS System Telemetry",
                    "ARM64 Apple M4 HVF kernel & Mercury runtime metrics",
                    vec![
                        UiMetric::new("Arch", "aarch64", "EL1"),
                        UiMetric::new("Heap Used", "12", "MB"),
                        UiMetric::new("Active Tasks", "6", "tasks"),
                        UiMetric::new("Net Stack", "smoltcp+tls13", "up"),
                    ],
                );
                Ok(ToolExecutionOutput {
                    tool_name: call.name.clone(),
                    output_json: "{\"ok\":true,\"arch\":\"aarch64\",\"heap_used_mb\":12,\"active_tasks\":6,\"net\":\"up\",\"shell\":\"disabled\"}".to_string(),
                    synthesized_view: Some(view),
                })
            }
            "script_create" => {
                let path = extract_json_string_field(&call.arguments_json, "path")
                    .ok_or_else(|| AiError::InvalidToolArguments("missing 'path'".to_string()))?;
                let source = extract_json_string_field(&call.arguments_json, "source")
                    .ok_or_else(|| AiError::InvalidToolArguments("missing 'source'".to_string()))?;
                self.vfs_files.insert(path.clone(), source);
                Ok(ToolExecutionOutput {
                    tool_name: call.name.clone(),
                    output_json: format!(
                        "{{\"ok\":true,\"script_path\":\"{}\",\"status\":\"created\"}}",
                        json_escape(&path)
                    ),
                    synthesized_view: None,
                })
            }
            "script_run" => {
                let path = extract_json_string_field(&call.arguments_json, "path")
                    .ok_or_else(|| AiError::InvalidToolArguments("missing 'path'".to_string()))?;
                let exists = self.vfs_files.contains_key(&path);
                Ok(ToolExecutionOutput {
                    tool_name: call.name.clone(),
                    output_json: format!(
                        "{{\"ok\":{},\"script_path\":\"{}\",\"engine\":\"metalos-js\",\"result\":\"executed_ok\"}}",
                        exists,
                        json_escape(&path)
                    ),
                    synthesized_view: None,
                })
            }
            "script_schedule" => {
                let path = extract_json_string_field(&call.arguments_json, "path")
                    .ok_or_else(|| AiError::InvalidToolArguments("missing 'path'".to_string()))?;
                let trigger = extract_json_string_field(&call.arguments_json, "trigger")
                    .ok_or_else(|| {
                        AiError::InvalidToolArguments("missing 'trigger'".to_string())
                    })?;
                self.js_schedules.push((path.clone(), trigger.clone()));
                let mut conf = self
                    .vfs_files
                    .get("/etc/schedules.conf")
                    .cloned()
                    .unwrap_or_default();
                conf.push_str(&format!("{} -> {}\n", path, trigger));
                self.vfs_files.insert("/etc/schedules.conf".to_string(), conf);

                let entries = self
                    .js_schedules
                    .iter()
                    .map(|(p, t)| UiScheduleEntry::new(p.clone(), t.clone(), "scheduled"))
                    .collect();
                let view = ui_synth.synthesize_schedule_list("Scheduled JS Automations", entries);
                Ok(ToolExecutionOutput {
                    tool_name: call.name.clone(),
                    output_json: format!(
                        "{{\"ok\":true,\"script_path\":\"{}\",\"trigger\":\"{}\"}}",
                        json_escape(&path),
                        json_escape(&trigger)
                    ),
                    synthesized_view: Some(view),
                })
            }
            "mcp_list_tools" => {
                let apps: Vec<String> = self
                    .mcp_apps
                    .keys()
                    .map(|k| format!("\"{}\"", json_escape(k)))
                    .collect();
                Ok(ToolExecutionOutput {
                    tool_name: call.name.clone(),
                    output_json: format!(
                        "{{\"ok\":true,\"headless_mcp_apps\":[{}],\"ui_policy\":\"agent_exclusive\"}}",
                        apps.join(",")
                    ),
                    synthesized_view: None,
                })
            }
            "mcp_call_tool" => {
                let app = extract_json_string_field(&call.arguments_json, "app")
                    .unwrap_or_else(|| "telemetry.mcp".to_string());
                let tool = extract_json_string_field(&call.arguments_json, "tool")
                    .unwrap_or_else(|| "inspect".to_string());
                let payload = self
                    .mcp_apps
                    .get(&app)
                    .cloned()
                    .unwrap_or_else(|| {
                        format!(
                            "{{\"title\":\"{}\",\"summary\":\"Executed MCP tool {}\",\"value\":\"ok\"}}",
                            json_escape(&app),
                            json_escape(&tool)
                        )
                    });
                let view = ui_synth.synthesize_from_mcp_output(
                    &UiCallerOrigin::AiAgent,
                    &app,
                    &tool,
                    &payload,
                )?;
                Ok(ToolExecutionOutput {
                    tool_name: call.name.clone(),
                    output_json: payload,
                    synthesized_view: Some(view),
                })
            }
            "mcp_read_resource" => {
                let app = extract_json_string_field(&call.arguments_json, "app")
                    .unwrap_or_else(|| "notes.mcp".to_string());
                let uri = extract_json_string_field(&call.arguments_json, "uri")
                    .unwrap_or_else(|| "mcp://notes/index".to_string());
                let payload = self
                    .mcp_apps
                    .get(&app)
                    .cloned()
                    .unwrap_or_else(|| {
                        format!(
                            "{{\"title\":\"{}\",\"summary\":\"Resource {}\"}}",
                            json_escape(&app),
                            json_escape(&uri)
                        )
                    });
                let view = ui_synth.synthesize_from_mcp_output(
                    &UiCallerOrigin::AiAgent,
                    &app,
                    &uri,
                    &payload,
                )?;
                Ok(ToolExecutionOutput {
                    tool_name: call.name.clone(),
                    output_json: payload,
                    synthesized_view: Some(view),
                })
            }
            "ui_render_view" => {
                let kind_str = extract_json_string_field(&call.arguments_json, "kind")
                    .unwrap_or_else(|| "mcp_app_view".to_string());
                let kind = ViewKind::from_str(&kind_str).unwrap_or(ViewKind::McpAppView);
                let title = extract_json_string_field(&call.arguments_json, "title")
                    .unwrap_or_else(|| "Synthesized View".to_string());
                let summary = extract_json_string_field(&call.arguments_json, "summary")
                    .unwrap_or_else(|| "Agent-rendered Mercury surface".to_string());
                let source_app = extract_json_string_field(&call.arguments_json, "source_app");

                let view = ui_synth.synthesize_custom_view(
                    kind,
                    title,
                    summary,
                    source_app,
                    call.arguments_json.clone(),
                );
                let descriptor = view.to_mercury_descriptor();
                Ok(ToolExecutionOutput {
                    tool_name: call.name.clone(),
                    output_json: descriptor,
                    synthesized_view: Some(view),
                })
            }
            other => Err(AiError::UnknownTool(other.to_string())),
        }
    }
}
