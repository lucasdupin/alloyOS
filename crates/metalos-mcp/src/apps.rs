use alloc::boxed::Box;
use alloc::collections::BTreeMap;
use alloc::format;
use alloc::rc::Rc;
use alloc::string::{String, ToString};
use alloc::vec;
use alloc::vec::Vec;
use core::cell::RefCell;

use metalos_js::{parse_program, ScheduleTrigger, ScriptCapabilities, ScriptScheduler};

use crate::protocol::{
    mcp_map, AppSkill, AppTile, McpError, McpParamSpec, McpPrompt, McpResource,
    McpResourceContent, McpTool, McpValue, TileAction, TileItem,
};
use crate::registry::{McpApp, McpRegistry};
use crate::sandbox::{
    AppCapability, AppManifest, McpSandboxError, RuntimeCapabilityGuard, TASKS_APP_ID,
};

/// `/apps/tasks/tasks.mcp` manifest definition for `TasksApp`.
pub const TASKS_MANIFEST_MCP: &str = include_str!("../../../apps/tasks/tasks.mcp");
pub const TASKS_MAIN_JS: &str = include_str!("../../../apps/tasks/main.js");
pub const TASKS_MAIN_PATH: &str = "/apps/tasks/main.js";

/// `/apps/tasks/SKILL.md` skill text file for `TasksApp`.
pub const TASKS_SKILL_MD: &str = include_str!("../../../apps/tasks/SKILL.md");

/// Canonical VFS path for the `Tasks` app manifest.
pub const TASKS_MANIFEST_PATH: &str = "/apps/tasks/tasks.mcp";

/// Canonical VFS path for the `Tasks` app skill text file.
pub const TASKS_SKILL_PATH: &str = "/apps/tasks/SKILL.md";

/// Register the canonical `Tasks` app into an `McpRegistry` with a new `ScriptScheduler`.
pub fn register_builtin_apps(registry: &mut McpRegistry) -> Result<(), McpSandboxError> {
    registry.register_app(Box::new(TasksApp::new()))?;
    registry.register_app(Box::new(ClockApp::new()))?;
    Ok(())
}

/// Register the canonical `Tasks` app into an `McpRegistry` bound to a shared `ScriptScheduler`.
pub fn register_builtin_apps_with_scheduler(
    registry: &mut McpRegistry,
    scheduler: Rc<RefCell<ScriptScheduler>>,
) -> Result<(), McpSandboxError> {
    registry.register_app(Box::new(TasksApp::with_scheduler(scheduler)))?;
    registry.register_app(Box::new(ClockApp::new()))?;
    Ok(())
}

/// Representation of a scheduled task managed by `TasksApp` and backed by `ScriptScheduler`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScheduledTask {
    pub id: String,
    pub title: String,
    pub script_path: String,
    pub trigger: String,
    pub enabled: bool,
    pub status: String,
    pub source: Option<String>,
}

impl ScheduledTask {
    pub fn to_mcp_value(&self) -> McpValue {
        McpValue::Map(mcp_map(&[
            ("id", McpValue::Text(self.id.clone())),
            ("title", McpValue::Text(self.title.clone())),
            ("script_path", McpValue::Text(self.script_path.clone())),
            ("trigger", McpValue::Text(self.trigger.clone())),
            ("enabled", McpValue::Bool(self.enabled)),
            ("status", McpValue::Text(self.status.clone())),
            (
                "source",
                match &self.source {
                    Some(src) => McpValue::Text(src.clone()),
                    None => McpValue::Null,
                },
            ),
        ]))
    }

    pub fn to_tile_item(&self) -> TileItem {
        TileItem::new(
            self.id.clone(),
            self.title.clone(),
            self.script_path.clone(),
            self.trigger.clone(),
            self.status.clone(),
        )
    }
}

#[derive(Debug, Clone)]
struct TaskMetadata {
    title: String,
    enabled: bool,
    paused_entry: Option<(String, ScheduleTrigger)>,
}

/// Canonical `Tasks` application (`app_id: "tasks"`).
///
/// `TasksApp` is the **only** application in metalOS permitted to hold
/// `AppCapability::ScriptSchedulerAccess` and manage `ScriptScheduler` (`/etc/schedules.conf`
/// and `/scripts/*.js`). Welder routes all scheduled-task listing, creation, editing, and
/// deletion through `TasksApp`.
///
/// Exposes:
/// - **MCP Tools**: `tasks.list`, `tasks.create`, `tasks.edit`, `tasks.delete`
/// - **MCP Resources**: `mcp://tasks/schedules`, `mcp://tasks/tiles`, `mcp://tasks/skill`
/// - **MCP Prompts**: `tasks.manage`
/// - **SKILL**: `/apps/tasks/SKILL.md`
/// - **Tiles**: `tasks.board` (embeddable in chat stream or pinnable to the Dashboard)
pub struct TasksApp {
    manifest: AppManifest,
    guard: RuntimeCapabilityGuard,
    scheduler: Rc<RefCell<ScriptScheduler>>,
    metadata: BTreeMap<String, TaskMetadata>,
    next_seq: u64,
}

impl Default for TasksApp {
    fn default() -> Self {
        Self::new()
    }
}

impl TasksApp {
    pub fn new() -> Self {
        Self::with_scheduler(Rc::new(RefCell::new(ScriptScheduler::new())))
    }

    pub fn with_scheduler(scheduler: Rc<RefCell<ScriptScheduler>>) -> Self {
        let manifest = AppManifest::parse(TASKS_MANIFEST_MCP)
            .expect("valid built-in Tasks manifest");
        let guard = RuntimeCapabilityGuard::new(manifest.clone())
            .expect("TasksApp manifest must satisfy sandbox rules");
        Self {
            manifest,
            guard,
            scheduler,
            metadata: BTreeMap::new(),
            next_seq: 1,
        }
    }

    /// Returns a reference to the shared `ScriptScheduler` exclusively managed by `TasksApp`.
    pub fn scheduler(&self) -> &Rc<RefCell<ScriptScheduler>> {
        &self.scheduler
    }

    fn default_title_for(id: &str, script_path: &str) -> String {
        let stem = script_path
            .rsplit('/')
            .next()
            .unwrap_or(id)
            .trim_end_matches(".js")
            .replace('_', " ")
            .replace('-', " ");
        if stem.is_empty() {
            id.to_string()
        } else {
            stem
        }
    }

    fn normalize_script_path(raw: &str) -> String {
        let trimmed = raw.trim();
        if trimmed.starts_with('/') {
            ScriptCapabilities::normalize_path(trimmed)
        } else {
            ScriptCapabilities::normalize_path(&format!("/scripts/{}", trimmed))
        }
    }

    /// Collects all scheduled tasks from the live `ScriptScheduler` plus any paused tasks.
    pub fn scheduled_tasks(&self, include_disabled: bool) -> Vec<ScheduledTask> {
        let sched = self.scheduler.borrow();
        let mut out = Vec::new();
        let mut seen_ids = Vec::new();

        for entry in sched.schedules() {
            seen_ids.push(entry.id.clone());
            let title = self
                .metadata
                .get(&entry.id)
                .map(|m| m.title.clone())
                .unwrap_or_else(|| Self::default_title_for(&entry.id, &entry.script_path));
            let status = if entry.once_fired {
                "completed".to_string()
            } else {
                "active".to_string()
            };
            let source = sched.store().get_script(&entry.script_path).map(String::from);
            out.push(ScheduledTask {
                id: entry.id.clone(),
                title,
                script_path: entry.script_path.clone(),
                trigger: entry.trigger.serialize(),
                enabled: true,
                status,
                source,
            });
        }

        if include_disabled {
            for (id, meta) in &self.metadata {
                if !meta.enabled && !seen_ids.contains(id) {
                    if let Some((ref script_path, ref trigger)) = meta.paused_entry {
                        let source = sched.store().get_script(script_path).map(String::from);
                        out.push(ScheduledTask {
                            id: id.clone(),
                            title: meta.title.clone(),
                            script_path: script_path.clone(),
                            trigger: trigger.serialize(),
                            enabled: false,
                            status: "paused".to_string(),
                            source,
                        });
                    }
                }
            }
        }

        out
    }

    /// Builds the live declarative `tasks.board` `AppTile` for chat-stream embedding or Dashboard pinning.
    pub fn build_board_tile(&self) -> AppTile {
        let tasks = self.scheduled_tasks(true);
        let active_count = tasks.iter().filter(|t| t.enabled).count();
        let paused_count = tasks.len().saturating_sub(active_count);
        let subtitle = if paused_count > 0 {
            format!("{} active · {} paused", active_count, paused_count)
        } else {
            format!("{} active scheduled task(s)", active_count)
        };

        let items = tasks.iter().map(|t| t.to_tile_item()).collect();
        let actions = vec![
            TileAction::new(
                "create_task",
                "New Task",
                "tasks.create",
                mcp_map(&[("trigger", McpValue::Text("every:60s".to_string()))]),
            ),
            TileAction::new("edit_task", "Edit Task", "tasks.edit", BTreeMap::new()),
            TileAction::new("refresh_tasks", "Refresh", "tasks.list", BTreeMap::new()),
        ];

        AppTile {
            tile_id: "tasks.board".to_string(),
            title: "Tasks".to_string(),
            subtitle,
            refresh_resource_uri: Some("mcp://tasks/schedules".to_string()),
            items,
            actions,
        }
    }

    fn enforce_scheduler_access(&self) -> Result<(), McpError> {
        self.guard
            .enforce_capability(&AppCapability::ScriptSchedulerAccess)
            .map_err(|e| e.to_mcp_error())?;
        self.guard
            .enforce_capability(&AppCapability::ScopedStorage(
                "/etc/schedules.conf".to_string(),
            ))
            .map_err(|e| e.to_mcp_error())?;
        Ok(())
    }
}

impl McpApp for TasksApp {
    fn manifest(&self) -> &AppManifest {
        &self.manifest
    }

    fn tools(&self) -> Vec<McpTool> {
        vec![
            McpTool::new(
                "tasks.list",
                "List all scheduled tasks in ScriptScheduler (/etc/schedules.conf and /scripts/*.js)",
                vec![McpParamSpec::new(
                    "include_disabled",
                    "boolean",
                    false,
                    "Whether to include paused/disabled scheduled tasks (default true)",
                )],
            ),
            McpTool::new(
                "tasks.create",
                "Create a new scheduled task in ScriptScheduler",
                vec![
                    McpParamSpec::new("title", "string", false, "Human-readable task title"),
                    McpParamSpec::new(
                        "script_path",
                        "string",
                        true,
                        "Path to the JavaScript file under /scripts/*.js",
                    ),
                    McpParamSpec::new(
                        "trigger",
                        "string",
                        true,
                        "Schedule trigger (e.g. 'every:60s', 'cron:*/5 * * * *', 'once:10s', 'boot', 'fs:create:/inbox/*')",
                    ),
                    McpParamSpec::new("id", "string", false, "Optional custom schedule ID"),
                    McpParamSpec::new(
                        "source",
                        "string",
                        false,
                        "Optional JavaScript ES module source code to register for script_path",
                    ),
                    McpParamSpec::new(
                        "enabled",
                        "boolean",
                        false,
                        "Whether the task is immediately active in ScriptScheduler (default true)",
                    ),
                ],
            ),
            McpTool::new(
                "tasks.edit",
                "Edit an existing scheduled task's title, trigger, script_path, JS source, or enabled state",
                vec![
                    McpParamSpec::new("id", "string", true, "Scheduled task ID to edit"),
                    McpParamSpec::new("title", "string", false, "Updated human-readable title"),
                    McpParamSpec::new("trigger", "string", false, "Updated schedule trigger"),
                    McpParamSpec::new("script_path", "string", false, "Updated /scripts/*.js path"),
                    McpParamSpec::new("source", "string", false, "Updated JavaScript source code"),
                    McpParamSpec::new(
                        "enabled",
                        "boolean",
                        false,
                        "Pause (false) or resume (true) the scheduled task",
                    ),
                ],
            ),
            McpTool::new(
                "tasks.delete",
                "Delete an existing scheduled task from ScriptScheduler by ID",
                vec![McpParamSpec::new(
                    "id",
                    "string",
                    true,
                    "Scheduled task ID to delete",
                )],
            ),
        ]
    }

    fn resources(&self) -> Vec<McpResource> {
        vec![
            McpResource::new(
                "mcp://tasks/schedules",
                "Scheduled Tasks",
                "application/json",
                "Live snapshot of all scheduled tasks in ScriptScheduler and /etc/schedules.conf",
            ),
            McpResource::new(
                "mcp://tasks/tiles",
                "Tasks Declarative Tiles",
                "application/json",
                "Renderer-agnostic Tile descriptors for chat stream embedding and Dashboard pinning",
            ),
            McpResource::new(
                "mcp://tasks/skill",
                "Tasks App SKILL.md",
                "text/markdown",
                "Task instructions and MCP/Tile usage guide (/apps/tasks/SKILL.md)",
            ),
        ]
    }

    fn prompts(&self) -> Vec<McpPrompt> {
        vec![McpPrompt::new(
            "tasks.manage",
            "Guide Welder to list, create, or edit scheduled tasks in ScriptScheduler",
            vec!["goal".to_string()],
        )]
    }

    fn skills(&self) -> Vec<AppSkill> {
        vec![AppSkill::new(
            TASKS_APP_ID,
            "Tasks",
            TASKS_SKILL_PATH,
            "List, create, edit, and delete scheduled tasks backed by metalOS ScriptScheduler, and present interactive Tasks Tiles in the chat stream or on the Dashboard.",
            TASKS_SKILL_MD,
        )]
    }

    fn tiles(&self) -> Vec<AppTile> {
        vec![self.build_board_tile()]
    }

    fn call_tool(
        &mut self,
        tool_name: &str,
        args: &BTreeMap<String, McpValue>,
    ) -> Result<McpValue, McpError> {
        self.enforce_scheduler_access()?;
        let short = tool_name.strip_prefix("tasks.").unwrap_or(tool_name);

        match short {
            "list" => {
                let include_disabled = args
                    .get("include_disabled")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(true);
                let tasks = self.scheduled_tasks(include_disabled);
                let items: Vec<McpValue> = tasks.iter().map(|t| t.to_mcp_value()).collect();
                let schedules_conf = self.scheduler.borrow().serialize_schedules_conf();
                let tile = self.build_board_tile();

                Ok(McpValue::Map(mcp_map(&[
                    ("status", McpValue::Text("ok".to_string())),
                    ("title", McpValue::Text("Scheduled Tasks".to_string())),
                    (
                        "summary",
                        McpValue::Text(format!("{} scheduled task(s)", items.len())),
                    ),
                    ("count", McpValue::Int(items.len() as i64)),
                    ("tasks", McpValue::Array(items)),
                    ("schedules_conf", McpValue::Text(schedules_conf)),
                    ("tile", tile.to_mcp_value()),
                ])))
            }
            "create" => {
                let raw_path = args
                    .get("script_path")
                    .or_else(|| args.get("path"))
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| {
                        McpError::invalid_params("missing required string 'script_path'")
                    })?;
                let script_path = Self::normalize_script_path(raw_path);
                self.guard
                    .enforce_capability(&AppCapability::ScopedStorage(script_path.clone()))
                    .map_err(|e| e.to_mcp_error())?;

                let trigger_str = args
                    .get("trigger")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| McpError::invalid_params("missing required string 'trigger'"))?;
                let parsed_trigger = ScheduleTrigger::deserialize(trigger_str).map_err(|e| {
                    McpError::invalid_params(format!("invalid trigger '{}': {}", trigger_str, e))
                })?;

                let source_opt = args
                    .get("source")
                    .or_else(|| args.get("code"))
                    .and_then(|v| v.as_str())
                    .map(String::from);

                if let Some(ref src) = source_opt {
                    parse_program(src).map_err(|e| {
                        McpError::invalid_params(format!(
                            "JavaScript syntax error in '{}': {}",
                            script_path, e
                        ))
                    })?;
                }

                let id = match args.get("id").and_then(|v| v.as_str()) {
                    Some(custom_id) if !custom_id.trim().is_empty() => {
                        custom_id.trim().to_string()
                    }
                    _ => {
                        let seq = self.next_seq;
                        self.next_seq = self.next_seq.saturating_add(1);
                        let stem = script_path
                            .rsplit('/')
                            .next()
                            .unwrap_or("job")
                            .trim_end_matches(".js");
                        format!("sched-{}-{}", seq, stem)
                    }
                };

                let title = args
                    .get("title")
                    .and_then(|v| v.as_str())
                    .filter(|s| !s.trim().is_empty())
                    .map(|s| s.trim().to_string())
                    .unwrap_or_else(|| Self::default_title_for(&id, &script_path));

                let enabled = args
                    .get("enabled")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(true);

                {
                    let mut sched = self.scheduler.borrow_mut();
                    if let Some(ref src) = source_opt {
                        sched.register_script_source(&script_path, src);
                    }
                    if enabled {
                        sched.schedule_script(&id, &script_path, parsed_trigger.clone());
                    } else {
                        sched.unregister_script(&id);
                    }
                }

                self.metadata.insert(
                    id.clone(),
                    TaskMetadata {
                        title: title.clone(),
                        enabled,
                        paused_entry: if enabled {
                            None
                        } else {
                            Some((script_path.clone(), parsed_trigger.clone()))
                        },
                    },
                );

                let stored_source = self
                    .scheduler
                    .borrow()
                    .store()
                    .get_script(&script_path)
                    .map(String::from);
                let task = ScheduledTask {
                    id: id.clone(),
                    title: title.clone(),
                    script_path: script_path.clone(),
                    trigger: parsed_trigger.serialize(),
                    enabled,
                    status: if enabled {
                        "active".to_string()
                    } else {
                        "paused".to_string()
                    },
                    source: stored_source,
                };
                let schedules_conf = self.scheduler.borrow().serialize_schedules_conf();
                let tile = self.build_board_tile();

                Ok(McpValue::Map(mcp_map(&[
                    ("status", McpValue::Text("created".to_string())),
                    ("id", McpValue::Text(id)),
                    ("title", McpValue::Text(title)),
                    (
                        "summary",
                        McpValue::Text(format!(
                            "Scheduled {} ({})",
                            script_path,
                            parsed_trigger.serialize()
                        )),
                    ),
                    ("task", task.to_mcp_value()),
                    ("schedules_conf", McpValue::Text(schedules_conf)),
                    ("tile", tile.to_mcp_value()),
                ])))
            }
            "edit" => {
                let id = args
                    .get("id")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| McpError::invalid_params("missing required string 'id'"))?
                    .trim()
                    .to_string();

                // Locate current task in active ScriptScheduler or paused metadata
                let existing_active = self
                    .scheduler
                    .borrow()
                    .schedules()
                    .iter()
                    .find(|s| s.id == id)
                    .cloned();
                let existing_meta = self.metadata.get(&id).cloned();

                let (mut script_path, mut parsed_trigger, mut enabled) =
                    if let Some(active) = existing_active {
                        (active.script_path, active.trigger, true)
                    } else if let Some(ref meta) = existing_meta {
                        if let Some((ref p, ref t)) = meta.paused_entry {
                            (p.clone(), t.clone(), meta.enabled)
                        } else {
                            return Err(McpError::invalid_params(format!(
                                "scheduled task '{}' not found",
                                id
                            )));
                        }
                    } else {
                        return Err(McpError::invalid_params(format!(
                            "scheduled task '{}' not found",
                            id
                        )));
                    };

                if let Some(new_path) = args
                    .get("script_path")
                    .or_else(|| args.get("path"))
                    .and_then(|v| v.as_str())
                {
                    let norm = Self::normalize_script_path(new_path);
                    self.guard
                        .enforce_capability(&AppCapability::ScopedStorage(norm.clone()))
                        .map_err(|e| e.to_mcp_error())?;
                    script_path = norm;
                }

                if let Some(new_trigger) = args.get("trigger").and_then(|v| v.as_str()) {
                    parsed_trigger = ScheduleTrigger::deserialize(new_trigger).map_err(|e| {
                        McpError::invalid_params(format!(
                            "invalid trigger '{}': {}",
                            new_trigger, e
                        ))
                    })?;
                }

                if let Some(new_enabled) = args.get("enabled").and_then(|v| v.as_bool()) {
                    enabled = new_enabled;
                }

                let source_opt = args
                    .get("source")
                    .or_else(|| args.get("code"))
                    .and_then(|v| v.as_str())
                    .map(String::from);

                if let Some(ref src) = source_opt {
                    parse_program(src).map_err(|e| {
                        McpError::invalid_params(format!(
                            "JavaScript syntax error in '{}': {}",
                            script_path, e
                        ))
                    })?;
                }

                let title = args
                    .get("title")
                    .and_then(|v| v.as_str())
                    .filter(|s| !s.trim().is_empty())
                    .map(|s| s.trim().to_string())
                    .or_else(|| existing_meta.map(|m| m.title))
                    .unwrap_or_else(|| Self::default_title_for(&id, &script_path));

                {
                    let mut sched = self.scheduler.borrow_mut();
                    if let Some(ref src) = source_opt {
                        sched.register_script_source(&script_path, src);
                    }
                    if enabled {
                        sched.schedule_script(&id, &script_path, parsed_trigger.clone());
                    } else {
                        sched.unregister_script(&id);
                    }
                }

                self.metadata.insert(
                    id.clone(),
                    TaskMetadata {
                        title: title.clone(),
                        enabled,
                        paused_entry: if enabled {
                            None
                        } else {
                            Some((script_path.clone(), parsed_trigger.clone()))
                        },
                    },
                );

                let stored_source = self
                    .scheduler
                    .borrow()
                    .store()
                    .get_script(&script_path)
                    .map(String::from);
                let task = ScheduledTask {
                    id: id.clone(),
                    title: title.clone(),
                    script_path: script_path.clone(),
                    trigger: parsed_trigger.serialize(),
                    enabled,
                    status: if enabled {
                        "active".to_string()
                    } else {
                        "paused".to_string()
                    },
                    source: stored_source,
                };
                let schedules_conf = self.scheduler.borrow().serialize_schedules_conf();
                let tile = self.build_board_tile();

                Ok(McpValue::Map(mcp_map(&[
                    ("status", McpValue::Text("edited".to_string())),
                    ("id", McpValue::Text(id)),
                    ("title", McpValue::Text(title)),
                    (
                        "summary",
                        McpValue::Text(format!(
                            "Updated {} ({})",
                            script_path,
                            parsed_trigger.serialize()
                        )),
                    ),
                    ("task", task.to_mcp_value()),
                    ("schedules_conf", McpValue::Text(schedules_conf)),
                    ("tile", tile.to_mcp_value()),
                ])))
            }
            "delete" => {
                let id = args
                    .get("id")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| McpError::invalid_params("missing required string 'id'"))?
                    .trim()
                    .to_string();

                let removed_active = self.scheduler.borrow_mut().unregister_script(&id);
                let removed_meta = self.metadata.remove(&id).is_some();
                let deleted = removed_active || removed_meta;
                let schedules_conf = self.scheduler.borrow().serialize_schedules_conf();
                let tile = self.build_board_tile();

                Ok(McpValue::Map(mcp_map(&[
                    ("status", McpValue::Text("deleted".to_string())),
                    ("deleted", McpValue::Bool(deleted)),
                    ("id", McpValue::Text(id)),
                    ("schedules_conf", McpValue::Text(schedules_conf)),
                    ("tile", tile.to_mcp_value()),
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
            "mcp://tasks/schedules" => {
                self.enforce_scheduler_access()?;
                let tasks = self.scheduled_tasks(true);
                let active = tasks.iter().filter(|t| t.enabled).count() as i64;
                let paused = (tasks.len() as i64) - active;
                let items: Vec<McpValue> = tasks.iter().map(|t| t.to_mcp_value()).collect();
                let schedules_conf = self.scheduler.borrow().serialize_schedules_conf();

                let payload = McpValue::Map(mcp_map(&[
                    ("total", McpValue::Int(tasks.len() as i64)),
                    ("active", McpValue::Int(active)),
                    ("paused", McpValue::Int(paused)),
                    ("tasks", McpValue::Array(items)),
                    ("schedules_conf", McpValue::Text(schedules_conf)),
                ]));
                Ok(McpResourceContent::new(
                    uri,
                    "application/json",
                    payload.to_json(),
                ))
            }
            "mcp://tasks/tiles" => {
                let tile = self.build_board_tile();
                let payload = McpValue::Map(mcp_map(&[(
                    "tiles",
                    McpValue::Array(vec![tile.to_mcp_value()]),
                )]));
                Ok(McpResourceContent::new(
                    uri,
                    "application/json",
                    payload.to_json(),
                ))
            }
            "mcp://tasks/skill" => Ok(McpResourceContent::new(
                uri,
                "text/markdown",
                TASKS_SKILL_MD.to_string(),
            )),
            _ => Err(McpError::resource_not_found(format!(
                "TasksApp has no resource at '{}'",
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
            "manage" => {
                let goal = args
                    .get("goal")
                    .map(|s| s.as_str())
                    .unwrap_or("inspect or update scheduled tasks");
                let count = self.scheduled_tasks(true).len();
                Ok(format!(
                    "Use the Tasks app (tasks.list, tasks.create, tasks.edit, tasks.delete) to {} across {} scheduled task(s), and embed the 'tasks.board' Tile in the conversation stream.",
                    goal, count
                ))
            }
            _ => Err(McpError::method_not_found(format!(
                "unknown prompt '{}' in TasksApp",
                name
            ))),
        }
    }
}

/// Clock reports host uptime until a real wall-clock source is available.
#[derive(Debug, Clone)]
pub struct ClockApp {
    manifest: AppManifest,
    uptime_ms: u64,
}
impl Default for ClockApp {
    fn default() -> Self { Self::new() }
}
impl ClockApp {
    pub fn new() -> Self {
        Self { manifest: AppManifest::parse(include_str!("../../../apps/clock/clock.mcp"))
            .expect("valid built-in Clock manifest"), uptime_ms: 0 }
    }
    fn time_value(&self) -> McpValue {
        McpValue::Map(mcp_map(&[
            ("uptime_ms", McpValue::Int(self.uptime_ms.min(i64::MAX as u64) as i64)),
            ("wall_clock_available", McpValue::Bool(false)),
        ]))
    }
}
impl McpApp for ClockApp {
    fn manifest(&self) -> &AppManifest { &self.manifest }
    fn update_uptime(&mut self, uptime_ms: u64) { self.uptime_ms = uptime_ms; }
    fn tools(&self) -> Vec<McpTool> {
        vec![McpTool::new("clock.now", "Read system uptime; wall clock is unavailable", vec![])]
    }
    fn resources(&self) -> Vec<McpResource> {
        vec![McpResource::new("mcp://clock/now", "Clock", "application/json", "Current system uptime")]
    }
    fn prompts(&self) -> Vec<McpPrompt> { vec![] }
    fn skills(&self) -> Vec<AppSkill> {
        vec![AppSkill::new("clock", "clock", "/apps/clock/SKILL.md", "Show system uptime",
            include_str!("../../../apps/clock/SKILL.md"))]
    }
    fn tiles(&self) -> Vec<AppTile> {
        let seconds = self.uptime_ms / 1000;
        vec![AppTile {
            tile_id: "clock.now".into(), title: "Clock".into(),
            subtitle: "System uptime · wall clock unavailable".into(),
            refresh_resource_uri: Some("mcp://clock/now".into()),
            items: vec![TileItem::new("uptime", format!("{:02}:{:02}:{:02}", seconds / 3600,
                seconds / 60 % 60, seconds % 60), "Since boot", "Uptime", "live")],
            actions: vec![TileAction::new("refresh", "Refresh Clock", "clock.now", BTreeMap::new())],
        }]
    }
    fn call_tool(&mut self, tool_name: &str, _args: &BTreeMap<String, McpValue>) -> Result<McpValue, McpError> {
        if tool_name == "clock.now" { Ok(self.time_value()) }
        else { Err(McpError::resource_not_found("unknown Clock tool")) }
    }
    fn read_resource(&self, uri: &str) -> Result<McpResourceContent, McpError> {
        if uri == "mcp://clock/now" { Ok(McpResourceContent::new(uri, "application/json", self.time_value().to_json())) }
        else { Err(McpError::resource_not_found("unknown Clock resource")) }
    }
    fn get_prompt(&self, _name: &str, _args: &BTreeMap<String, String>) -> Result<String, McpError> {
        Err(McpError::resource_not_found("Clock has no prompts"))
    }
}
