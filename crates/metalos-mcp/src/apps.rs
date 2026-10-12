use alloc::boxed::Box;
use alloc::collections::BTreeMap;
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec;
use alloc::vec::Vec;

use crate::protocol::{
    mcp_map, McpError, McpParamSpec, McpPrompt, McpResource, McpResourceContent, McpTool, McpValue,
};
use crate::registry::{McpApp, McpRegistry};
use crate::sandbox::{AppCapability, AppManifest, McpSandboxError};

/// `/apps/notes.mcp` manifest definition for `NotesVaultApp`.
pub const NOTES_MANIFEST_MCP: &str = "\
# metalOS Headless MCP App Manifest (/apps/notes.mcp)
app_id = notes
name = Notes Vault
version = 1.0.0
capabilities = ExposeMcpTools, ExposeMcpResources, ExposeMcpPrompts, ScopedStorage(/var/notes)
";

/// `/apps/tasks.mcp` manifest definition for `TaskBoardApp`.
pub const TASKS_MANIFEST_MCP: &str = "\
# metalOS Headless MCP App Manifest (/apps/tasks.mcp)
app_id = tasks
name = Task Board
version = 1.0.0
capabilities = ExposeMcpTools, ExposeMcpResources, ExposeMcpPrompts, ScopedStorage(/var/tasks)
";

/// `/apps/syspulse.mcp` manifest definition for `SysPulseApp`.
pub const SYSPULSE_MANIFEST_MCP: &str = "\
# metalOS Headless MCP App Manifest (/apps/syspulse.mcp)
app_id = syspulse
name = SysPulse Telemetry
version = 1.0.0
capabilities = ExposeMcpTools, ExposeMcpResources, ExposeMcpPrompts, ScopedStorage(/var/syspulse)
";

/// Register all 3 built-in headless developer MCP apps into an `McpRegistry`.
pub fn register_builtin_apps(registry: &mut McpRegistry) -> Result<(), McpSandboxError> {
    registry.register_app(Box::new(NotesVaultApp::new()))?;
    registry.register_app(Box::new(TaskBoardApp::new()))?;
    registry.register_app(Box::new(SysPulseApp::new()))?;
    Ok(())
}

// ============================================================================
// 1. NotesVaultApp (app_id: "notes")
// ============================================================================

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NoteEntry {
    pub id: u64,
    pub title: String,
    pub body: String,
    pub tags: Vec<String>,
}

impl NoteEntry {
    pub fn to_mcp_value(&self) -> McpValue {
        McpValue::Map(mcp_map(&[
            ("id", McpValue::Int(self.id as i64)),
            ("title", McpValue::Text(self.title.clone())),
            ("body", McpValue::Text(self.body.clone())),
            (
                "tags",
                McpValue::Array(
                    self.tags
                        .iter()
                        .map(|t| McpValue::Text(t.clone()))
                        .collect(),
                ),
            ),
        ]))
    }
}

/// Headless note-taking and knowledge base MCP application (`app_id: "notes"`).
///
/// Exposes zero UI of its own; the AI agent queries and mutates notes via MCP tools
/// (`notes.create`, `notes.search`, `notes.delete`) and resources (`mcp://notes/all`,
/// `mcp://notes/recent`) and renders visual cards on its exclusive canvas.
pub struct NotesVaultApp {
    manifest: AppManifest,
    notes: Vec<NoteEntry>,
    next_id: u64,
}

impl Default for NotesVaultApp {
    fn default() -> Self {
        Self::new()
    }
}

impl NotesVaultApp {
    pub fn new() -> Self {
        let manifest = AppManifest::parse(NOTES_MANIFEST_MCP).unwrap_or_else(|_| {
            AppManifest::new(
                "notes",
                "Notes Vault",
                "1.0.0",
                vec![
                    AppCapability::ExposeMcpTools,
                    AppCapability::ExposeMcpResources,
                    AppCapability::ExposeMcpPrompts,
                    AppCapability::ScopedStorage("/var/notes".to_string()),
                ],
            )
        });
        Self {
            manifest,
            notes: Vec::new(),
            next_id: 1,
        }
    }

    pub fn notes(&self) -> &[NoteEntry] {
        &self.notes
    }
}

impl McpApp for NotesVaultApp {
    fn manifest(&self) -> &AppManifest {
        &self.manifest
    }

    fn tools(&self) -> Vec<McpTool> {
        vec![
            McpTool::new(
                "notes.create",
                "Create a new note in the headless Notes Vault knowledge base",
                vec![
                    McpParamSpec::new("title", "string", true, "Title of the note"),
                    McpParamSpec::new("body", "string", true, "Markdown body of the note"),
                    McpParamSpec::new(
                        "tag",
                        "string",
                        false,
                        "Optional primary tag for categorizing the note",
                    ),
                ],
            ),
            McpTool::new(
                "notes.search",
                "Search notes by substring match across title, body, and tags",
                vec![McpParamSpec::new(
                    "query",
                    "string",
                    true,
                    "Case-insensitive search query",
                )],
            ),
            McpTool::new(
                "notes.delete",
                "Delete a note by its numeric ID",
                vec![McpParamSpec::new(
                    "id",
                    "integer",
                    true,
                    "Unique note identifier to delete",
                )],
            ),
        ]
    }

    fn resources(&self) -> Vec<McpResource> {
        vec![
            McpResource::new(
                "mcp://notes/all",
                "All Notes",
                "application/json",
                "Complete JSON array of all notes stored in Notes Vault",
            ),
            McpResource::new(
                "mcp://notes/recent",
                "Recent Notes",
                "application/json",
                "Most recently created notes in Notes Vault",
            ),
        ]
    }

    fn prompts(&self) -> Vec<McpPrompt> {
        vec![McpPrompt::new(
            "notes.summarize",
            "Generate an agent synthesis prompt over stored notes for a topic",
            vec!["topic".to_string()],
        )]
    }

    fn call_tool(
        &mut self,
        tool_name: &str,
        args: &BTreeMap<String, McpValue>,
    ) -> Result<McpValue, McpError> {
        let short = tool_name.strip_prefix("notes.").unwrap_or(tool_name);
        match short {
            "create" => {
                let title = args
                    .get("title")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| McpError::invalid_params("missing required string 'title'"))?;
                let body = args
                    .get("body")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| McpError::invalid_params("missing required string 'body'"))?;

                let mut tags = Vec::new();
                if let Some(tag) = args.get("tag").and_then(|v| v.as_str()) {
                    if !tag.is_empty() {
                        tags.push(tag.to_string());
                    }
                }
                if let Some(arr) = args.get("tags").and_then(|v| v.as_array()) {
                    for item in arr {
                        if let Some(t) = item.as_str() {
                            tags.push(t.to_string());
                        }
                    }
                }

                let id = self.next_id;
                self.next_id += 1;
                let entry = NoteEntry {
                    id,
                    title: title.to_string(),
                    body: body.to_string(),
                    tags,
                };
                let value = entry.to_mcp_value();
                self.notes.push(entry);

                Ok(McpValue::Map(mcp_map(&[
                    ("status", McpValue::Text("created".to_string())),
                    ("id", McpValue::Int(id as i64)),
                    ("note", value),
                ])))
            }
            "search" => {
                let query = args
                    .get("query")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| McpError::invalid_params("missing required string 'query'"))?;
                let q_lower = query.to_ascii_lowercase();

                let matches: Vec<McpValue> = self
                    .notes
                    .iter()
                    .filter(|n| {
                        n.title.to_ascii_lowercase().contains(&q_lower)
                            || n.body.to_ascii_lowercase().contains(&q_lower)
                            || n.tags
                                .iter()
                                .any(|t| t.to_ascii_lowercase().contains(&q_lower))
                    })
                    .map(|n| n.to_mcp_value())
                    .collect();

                let count = matches.len() as i64;
                Ok(McpValue::Map(mcp_map(&[
                    ("count", McpValue::Int(count)),
                    ("matches", McpValue::Array(matches)),
                ])))
            }
            "delete" => {
                let id = args
                    .get("id")
                    .and_then(|v| v.as_i64())
                    .and_then(|v| u64::try_from(v).ok())
                    .ok_or_else(|| McpError::invalid_params("missing required integer 'id'"))?;

                let before = self.notes.len();
                self.notes.retain(|n| n.id != id);
                let deleted = self.notes.len() < before;

                Ok(McpValue::Map(mcp_map(&[
                    ("deleted", McpValue::Bool(deleted)),
                    ("id", McpValue::Int(id as i64)),
                    ("remaining", McpValue::Int(self.notes.len() as i64)),
                ])))
            }
            other => Err(McpError::method_not_found(format!(
                "unknown tool 'notes.{}'",
                other
            ))),
        }
    }

    fn read_resource(&self, uri: &str) -> Result<McpResourceContent, McpError> {
        match uri {
            "mcp://notes/all" => {
                let list: Vec<McpValue> = self.notes.iter().map(|n| n.to_mcp_value()).collect();
                let payload = McpValue::Map(mcp_map(&[
                    ("total", McpValue::Int(self.notes.len() as i64)),
                    ("notes", McpValue::Array(list)),
                ]));
                Ok(McpResourceContent::new(
                    uri,
                    "application/json",
                    payload.to_json(),
                ))
            }
            "mcp://notes/recent" => {
                let recent: Vec<McpValue> = self
                    .notes
                    .iter()
                    .rev()
                    .take(5)
                    .map(|n| n.to_mcp_value())
                    .collect();
                let payload = McpValue::Map(mcp_map(&[
                    ("count", McpValue::Int(recent.len() as i64)),
                    ("notes", McpValue::Array(recent)),
                ]));
                Ok(McpResourceContent::new(
                    uri,
                    "application/json",
                    payload.to_json(),
                ))
            }
            _ => Err(McpError::resource_not_found(format!(
                "NotesVaultApp has no resource at '{}'",
                uri
            ))),
        }
    }

    fn get_prompt(
        &self,
        name: &str,
        args: &BTreeMap<String, String>,
    ) -> Result<String, McpError> {
        let short = name.strip_prefix("notes.").unwrap_or(name);
        match short {
            "summarize" => {
                let topic = args
                    .get("topic")
                    .map(|s| s.as_str())
                    .unwrap_or("all topics");
                Ok(format!(
                    "Synthesize a visual summary card for topic '{}' across {} stored notes in Notes Vault.",
                    topic,
                    self.notes.len()
                ))
            }
            _ => Err(McpError::method_not_found(format!(
                "unknown prompt '{}' in NotesVaultApp",
                name
            ))),
        }
    }
}

// ============================================================================
// 2. TaskBoardApp (app_id: "tasks")
// ============================================================================

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskItem {
    pub id: u64,
    pub title: String,
    pub priority: String,
    pub completed: bool,
}

impl TaskItem {
    pub fn to_mcp_value(&self) -> McpValue {
        McpValue::Map(mcp_map(&[
            ("id", McpValue::Int(self.id as i64)),
            ("title", McpValue::Text(self.title.clone())),
            ("priority", McpValue::Text(self.priority.clone())),
            ("completed", McpValue::Bool(self.completed)),
        ]))
    }
}

/// Headless project/task tracker MCP application (`app_id: "tasks"`).
///
/// Exposes `tasks.add`, `tasks.complete`, `tasks.list`, and resource `mcp://tasks/summary`.
pub struct TaskBoardApp {
    manifest: AppManifest,
    tasks: Vec<TaskItem>,
    next_id: u64,
}

impl Default for TaskBoardApp {
    fn default() -> Self {
        Self::new()
    }
}

impl TaskBoardApp {
    pub fn new() -> Self {
        let manifest = AppManifest::parse(TASKS_MANIFEST_MCP).unwrap_or_else(|_| {
            AppManifest::new(
                "tasks",
                "Task Board",
                "1.0.0",
                vec![
                    AppCapability::ExposeMcpTools,
                    AppCapability::ExposeMcpResources,
                    AppCapability::ExposeMcpPrompts,
                    AppCapability::ScopedStorage("/var/tasks".to_string()),
                ],
            )
        });
        Self {
            manifest,
            tasks: Vec::new(),
            next_id: 1,
        }
    }

    pub fn tasks(&self) -> &[TaskItem] {
        &self.tasks
    }
}

impl McpApp for TaskBoardApp {
    fn manifest(&self) -> &AppManifest {
        &self.manifest
    }

    fn tools(&self) -> Vec<McpTool> {
        vec![
            McpTool::new(
                "tasks.add",
                "Add a new task to the headless Task Board",
                vec![
                    McpParamSpec::new("title", "string", true, "Task description"),
                    McpParamSpec::new(
                        "priority",
                        "string",
                        false,
                        "Priority level (low, normal, high, critical)",
                    ),
                ],
            ),
            McpTool::new(
                "tasks.complete",
                "Mark an existing task as completed by ID",
                vec![McpParamSpec::new("id", "integer", true, "Task ID")],
            ),
            McpTool::new(
                "tasks.list",
                "List tasks, optionally filtering by completion state",
                vec![McpParamSpec::new(
                    "include_completed",
                    "boolean",
                    false,
                    "Whether to include completed tasks (default true)",
                )],
            ),
        ]
    }

    fn resources(&self) -> Vec<McpResource> {
        vec![McpResource::new(
            "mcp://tasks/summary",
            "Task Board Summary",
            "application/json",
            "Aggregated task counts (total, pending, completed) and full task list",
        )]
    }

    fn prompts(&self) -> Vec<McpPrompt> {
        vec![McpPrompt::new(
            "tasks.prioritize",
            "Generate an agent triage prompt for pending tasks",
            vec!["focus".to_string()],
        )]
    }

    fn call_tool(
        &mut self,
        tool_name: &str,
        args: &BTreeMap<String, McpValue>,
    ) -> Result<McpValue, McpError> {
        let short = tool_name.strip_prefix("tasks.").unwrap_or(tool_name);
        match short {
            "add" => {
                let title = args
                    .get("title")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| McpError::invalid_params("missing required string 'title'"))?;
                let priority = args
                    .get("priority")
                    .and_then(|v| v.as_str())
                    .unwrap_or("normal");

                let id = self.next_id;
                self.next_id += 1;
                let item = TaskItem {
                    id,
                    title: title.to_string(),
                    priority: priority.to_string(),
                    completed: false,
                };
                let val = item.to_mcp_value();
                self.tasks.push(item);

                Ok(McpValue::Map(mcp_map(&[
                    ("status", McpValue::Text("added".to_string())),
                    ("id", McpValue::Int(id as i64)),
                    ("task", val),
                ])))
            }
            "complete" => {
                let id = args
                    .get("id")
                    .and_then(|v| v.as_i64())
                    .and_then(|v| u64::try_from(v).ok())
                    .ok_or_else(|| McpError::invalid_params("missing required integer 'id'"))?;

                let task = self
                    .tasks
                    .iter_mut()
                    .find(|t| t.id == id)
                    .ok_or_else(|| {
                        McpError::invalid_params(format!("task with id {} not found", id))
                    })?;
                task.completed = true;
                let val = task.to_mcp_value();

                Ok(McpValue::Map(mcp_map(&[
                    ("status", McpValue::Text("completed".to_string())),
                    ("id", McpValue::Int(id as i64)),
                    ("task", val),
                ])))
            }
            "list" => {
                let include_completed = args
                    .get("include_completed")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(true);

                let items: Vec<McpValue> = self
                    .tasks
                    .iter()
                    .filter(|t| include_completed || !t.completed)
                    .map(|t| t.to_mcp_value())
                    .collect();

                Ok(McpValue::Map(mcp_map(&[
                    ("count", McpValue::Int(items.len() as i64)),
                    ("tasks", McpValue::Array(items)),
                ])))
            }
            other => Err(McpError::method_not_found(format!(
                "unknown tool 'tasks.{}'",
                other
            ))),
        }
    }

    fn read_resource(&self, uri: &str) -> Result<McpResourceContent, McpError> {
        match uri {
            "mcp://tasks/summary" => {
                let total = self.tasks.len() as i64;
                let completed = self.tasks.iter().filter(|t| t.completed).count() as i64;
                let pending = total - completed;
                let items: Vec<McpValue> = self.tasks.iter().map(|t| t.to_mcp_value()).collect();

                let summary = McpValue::Map(mcp_map(&[
                    ("total", McpValue::Int(total)),
                    ("pending", McpValue::Int(pending)),
                    ("completed", McpValue::Int(completed)),
                    ("tasks", McpValue::Array(items)),
                ]));
                Ok(McpResourceContent::new(
                    uri,
                    "application/json",
                    summary.to_json(),
                ))
            }
            _ => Err(McpError::resource_not_found(format!(
                "TaskBoardApp has no resource at '{}'",
                uri
            ))),
        }
    }

    fn get_prompt(
        &self,
        name: &str,
        args: &BTreeMap<String, String>,
    ) -> Result<String, McpError> {
        let short = name.strip_prefix("tasks.").unwrap_or(name);
        match short {
            "prioritize" => {
                let focus = args
                    .get("focus")
                    .map(|s| s.as_str())
                    .unwrap_or("general execution");
                let pending = self.tasks.iter().filter(|t| !t.completed).count();
                Ok(format!(
                    "Prioritize {} pending tasks on the agent canvas with focus '{}'.",
                    pending, focus
                ))
            }
            _ => Err(McpError::method_not_found(format!(
                "unknown prompt '{}' in TaskBoardApp",
                name
            ))),
        }
    }
}

// ============================================================================
// 3. SysPulseApp (app_id: "syspulse")
// ============================================================================

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TelemetrySample {
    pub metric: String,
    pub value: i64,
    pub unit: String,
}

impl TelemetrySample {
    pub fn to_mcp_value(&self) -> McpValue {
        McpValue::Map(mcp_map(&[
            ("metric", McpValue::Text(self.metric.clone())),
            ("value", McpValue::Int(self.value)),
            ("unit", McpValue::Text(self.unit.clone())),
        ]))
    }
}

/// Headless hardware & sensor telemetry MCP application (`app_id: "syspulse"`).
///
/// Exposes `syspulse.snapshot`, `syspulse.record_metric`, and resource `mcp://syspulse/metrics`.
pub struct SysPulseApp {
    manifest: AppManifest,
    samples: Vec<TelemetrySample>,
}

impl Default for SysPulseApp {
    fn default() -> Self {
        Self::new()
    }
}

impl SysPulseApp {
    pub fn new() -> Self {
        let manifest = AppManifest::parse(SYSPULSE_MANIFEST_MCP).unwrap_or_else(|_| {
            AppManifest::new(
                "syspulse",
                "SysPulse Telemetry",
                "1.0.0",
                vec![
                    AppCapability::ExposeMcpTools,
                    AppCapability::ExposeMcpResources,
                    AppCapability::ExposeMcpPrompts,
                    AppCapability::ScopedStorage("/var/syspulse".to_string()),
                ],
            )
        });

        let samples = vec![
            TelemetrySample {
                metric: "cpu_load_pct".to_string(),
                value: 14,
                unit: "%".to_string(),
            },
            TelemetrySample {
                metric: "mem_used_mib".to_string(),
                value: 96,
                unit: "MiB".to_string(),
            },
            TelemetrySample {
                metric: "gpu_frame_us".to_string(),
                value: 2400,
                unit: "us".to_string(),
            },
        ];

        Self { manifest, samples }
    }

    pub fn samples(&self) -> &[TelemetrySample] {
        &self.samples
    }
}

impl McpApp for SysPulseApp {
    fn manifest(&self) -> &AppManifest {
        &self.manifest
    }

    fn tools(&self) -> Vec<McpTool> {
        vec![
            McpTool::new(
                "syspulse.snapshot",
                "Capture a live snapshot of hardware and kernel telemetry metrics",
                vec![],
            ),
            McpTool::new(
                "syspulse.record_metric",
                "Record or update a named hardware/sensor telemetry metric",
                vec![
                    McpParamSpec::new("metric", "string", true, "Metric identifier"),
                    McpParamSpec::new("value", "integer", true, "Numeric sensor value"),
                    McpParamSpec::new("unit", "string", false, "Measurement unit (e.g. %, MiB, us)"),
                ],
            ),
        ]
    }

    fn resources(&self) -> Vec<McpResource> {
        vec![McpResource::new(
            "mcp://syspulse/metrics",
            "Hardware & Sensor Telemetry Metrics",
            "application/json",
            "Live stream snapshot of recorded hardware and kernel metrics",
        )]
    }

    fn prompts(&self) -> Vec<McpPrompt> {
        vec![McpPrompt::new(
            "syspulse.diagnose",
            "Generate an agent diagnostic prompt for hardware telemetry",
            vec!["subsystem".to_string()],
        )]
    }

    fn call_tool(
        &mut self,
        tool_name: &str,
        args: &BTreeMap<String, McpValue>,
    ) -> Result<McpValue, McpError> {
        let short = tool_name.strip_prefix("syspulse.").unwrap_or(tool_name);
        match short {
            "snapshot" => {
                let list: Vec<McpValue> = self.samples.iter().map(|s| s.to_mcp_value()).collect();
                Ok(McpValue::Map(mcp_map(&[
                    ("status", McpValue::Text("ok".to_string())),
                    ("metric_count", McpValue::Int(self.samples.len() as i64)),
                    ("metrics", McpValue::Array(list)),
                ])))
            }
            "record_metric" => {
                let metric = args
                    .get("metric")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| McpError::invalid_params("missing required string 'metric'"))?;
                let value = args
                    .get("value")
                    .and_then(|v| v.as_i64())
                    .ok_or_else(|| McpError::invalid_params("missing required integer 'value'"))?;
                let unit = args
                    .get("unit")
                    .and_then(|v| v.as_str())
                    .unwrap_or("count");

                if let Some(existing) = self.samples.iter_mut().find(|s| s.metric == metric) {
                    existing.value = value;
                    existing.unit = unit.to_string();
                } else {
                    self.samples.push(TelemetrySample {
                        metric: metric.to_string(),
                        value,
                        unit: unit.to_string(),
                    });
                }

                Ok(McpValue::Map(mcp_map(&[
                    ("status", McpValue::Text("recorded".to_string())),
                    ("metric", McpValue::Text(metric.to_string())),
                    ("value", McpValue::Int(value)),
                    ("unit", McpValue::Text(unit.to_string())),
                ])))
            }
            other => Err(McpError::method_not_found(format!(
                "unknown tool 'syspulse.{}'",
                other
            ))),
        }
    }

    fn read_resource(&self, uri: &str) -> Result<McpResourceContent, McpError> {
        match uri {
            "mcp://syspulse/metrics" => {
                let list: Vec<McpValue> = self.samples.iter().map(|s| s.to_mcp_value()).collect();
                let payload = McpValue::Map(mcp_map(&[
                    ("count", McpValue::Int(self.samples.len() as i64)),
                    ("metrics", McpValue::Array(list)),
                ]));
                Ok(McpResourceContent::new(
                    uri,
                    "application/json",
                    payload.to_json(),
                ))
            }
            _ => Err(McpError::resource_not_found(format!(
                "SysPulseApp has no resource at '{}'",
                uri
            ))),
        }
    }

    fn get_prompt(
        &self,
        name: &str,
        args: &BTreeMap<String, String>,
    ) -> Result<String, McpError> {
        let short = name.strip_prefix("syspulse.").unwrap_or(name);
        match short {
            "diagnose" => {
                let subsystem = args
                    .get("subsystem")
                    .map(|s| s.as_str())
                    .unwrap_or("system");
                Ok(format!(
                    "Analyze {} telemetry metrics for subsystem '{}' and render a health gauge widget.",
                    self.samples.len(),
                    subsystem
                ))
            }
            _ => Err(McpError::method_not_found(format!(
                "unknown prompt '{}' in SysPulseApp",
                name
            ))),
        }
    }
}
