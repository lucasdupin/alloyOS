use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec;
use alloc::vec::Vec;

use alloyos_forge::provider::{extract_json_string_field, json_escape, AiError};
use alloyos_mcp::AppTile;
use alloyos_proto::DirtyRect;

/// Exclusive view archetypes synthesized by the alloyOS AI agent for Ferrofluid (`ferrofluidd`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ViewKind {
    Console,
    MetricDashboard,
    Table,
    ScriptScheduleList,
    McpAppView,
}

impl ViewKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            ViewKind::Console => "console",
            ViewKind::MetricDashboard => "metric_dashboard",
            ViewKind::Table => "table",
            ViewKind::ScriptScheduleList => "script_schedule_list",
            ViewKind::McpAppView => "mcp_app_view",
        }
    }

    pub fn from_str(raw: &str) -> Option<Self> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "console" => Some(ViewKind::Console),
            "metric_dashboard" | "metrics" | "dashboard" => Some(ViewKind::MetricDashboard),
            "table" => Some(ViewKind::Table),
            "script_schedule_list" | "schedules" | "schedule_list" => {
                Some(ViewKind::ScriptScheduleList)
            }
            "mcp_app_view" | "mcp" | "app_view" => Some(ViewKind::McpAppView),
            _ => None,
        }
    }
}

/// Caller identity checked by the UI synthesizer to enforce Phase 7 Zero App-Owned UI rules.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UiCallerOrigin {
    /// The privileged alloyOS AI runtime (Welder).
    AiAgent,
    /// A third-party MCP application (strictly forbidden from creating UI directly).
    ThirdPartyMcpApp(String),
}

/// A single key metric card rendered inside a `MetricDashboard` or `McpAppView`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UiMetric {
    pub label: String,
    pub value: String,
    pub unit: String,
}

impl UiMetric {
    pub fn new(
        label: impl Into<String>,
        value: impl Into<String>,
        unit: impl Into<String>,
    ) -> Self {
        Self {
            label: label.into(),
            value: value.into(),
            unit: unit.into(),
        }
    }
}

/// Structured tabular layout rendered inside a `Table` or `McpAppView`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UiTable {
    pub columns: Vec<String>,
    pub rows: Vec<Vec<String>>,
}

impl UiTable {
    pub fn new(columns: Vec<String>, rows: Vec<Vec<String>>) -> Self {
        Self { columns, rows }
    }
}

/// Entry representing a model-authored JavaScript automation in `ScriptScheduleList`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UiScheduleEntry {
    pub script_path: String,
    pub trigger: String,
    pub status: String,
}

impl UiScheduleEntry {
    pub fn new(
        script_path: impl Into<String>,
        trigger: impl Into<String>,
        status: impl Into<String>,
    ) -> Self {
        Self {
            script_path: script_path.into(),
            trigger: trigger.into(),
            status: status.into(),
        }
    }
}

/// Agent-synthesized UI surface descriptor delivered to Ferrofluid (`ferrofluidd`).
///
/// In alloyOS, developer applications are headless MCP servers and never own windows or widgets;
/// only the AI agent creates and updates `SynthesizedUiView` instances (including declarative
/// `AppTile` descriptors that can be embedded in the chat stream or pinned to the Dashboard).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SynthesizedUiView {
    pub view_id: u64,
    pub kind: ViewKind,
    pub title: String,
    pub source_app: Option<String>,
    pub summary: String,
    pub metrics: Vec<UiMetric>,
    pub table: Option<UiTable>,
    pub schedules: Vec<UiScheduleEntry>,
    pub tile: Option<AppTile>,
    pub damage_rect: DirtyRect,
    pub raw_payload_json: String,
}

impl SynthesizedUiView {
    /// Serializes this agent-synthesized view into a Ferrofluid IPC frame description.
    pub fn to_ferrofluid_descriptor(&self) -> String {
        format!(
            "{{\"view_id\":{},\"kind\":\"{}\",\"title\":\"{}\",\"source_app\":{},\"summary\":\"{}\",\"metrics_count\":{},\"has_table\":{},\"schedules_count\":{},\"has_tile\":{}}}",
            self.view_id,
            self.kind.as_str(),
            json_escape(&self.title),
            match &self.source_app {
                Some(app) => format!("\"{}\"", json_escape(app)),
                None => "null".to_string(),
            },
            json_escape(&self.summary),
            self.metrics.len(),
            self.table.is_some(),
            self.schedules.len(),
            self.tile.is_some()
        )
    }
}

/// Exclusive generative UI synthesizer owned by the AI agent runtime.
#[derive(Debug, Clone)]
pub struct AgentUiSynthesizer {
    next_view_id: u64,
    active_views: Vec<SynthesizedUiView>,
}

impl Default for AgentUiSynthesizer {
    fn default() -> Self {
        Self::new()
    }
}

impl AgentUiSynthesizer {
    pub fn new() -> Self {
        let mut synth = Self {
            next_view_id: 1,
            active_views: Vec::new(),
        };
        let boot_console = SynthesizedUiView {
            view_id: synth.alloc_id(),
            kind: ViewKind::Console,
            title: "Ferrofluid AI Console".to_string(),
            source_app: None,
            summary: "Single-window liquid-mercury AI conversational console".to_string(),
            metrics: Vec::new(),
            table: None,
            schedules: Vec::new(),
            tile: None,
            damage_rect: DirtyRect::new(0, 0, 1280, 800),
            raw_payload_json: "{\"mode\":\"single_window_console\"}".to_string(),
        };
        synth.active_views.push(boot_console);
        synth
    }

    fn alloc_id(&mut self) -> u64 {
        let id = self.next_view_id;
        self.next_view_id = self.next_view_id.saturating_add(1);
        id
    }

    /// Returns all currently active views synthesized by the AI agent.
    pub fn views(&self) -> &[SynthesizedUiView] {
        &self.active_views
    }

    /// Enforces that only the AI agent can create or mutate UI views.
    pub fn verify_caller_origin(origin: &UiCallerOrigin) -> Result<(), AiError> {
        match origin {
            UiCallerOrigin::AiAgent => Ok(()),
            UiCallerOrigin::ThirdPartyMcpApp(app_name) => Err(AiError::PolicyViolation(format!(
                "MCP app '{}' attempted direct UI surface creation; in alloyOS only the AI agent can create UI",
                app_name
            ))),
        }
    }

    /// Synthesizes a `MetricDashboard` view from OS telemetry or system status.
    pub fn synthesize_metric_dashboard(
        &mut self,
        title: impl Into<String>,
        summary: impl Into<String>,
        metrics: Vec<UiMetric>,
    ) -> SynthesizedUiView {
        let view = SynthesizedUiView {
            view_id: self.alloc_id(),
            kind: ViewKind::MetricDashboard,
            title: title.into(),
            source_app: None,
            summary: summary.into(),
            metrics,
            table: None,
            schedules: Vec::new(),
            tile: None,
            damage_rect: DirtyRect::new(40, 40, 1200, 320),
            raw_payload_json: "{}".to_string(),
        };
        self.active_views.push(view.clone());
        view
    }

    /// Synthesizes a structured `Table` view (e.g. from filesystem listings or dataset rows).
    pub fn synthesize_table(
        &mut self,
        title: impl Into<String>,
        summary: impl Into<String>,
        columns: Vec<String>,
        rows: Vec<Vec<String>>,
    ) -> SynthesizedUiView {
        let view = SynthesizedUiView {
            view_id: self.alloc_id(),
            kind: ViewKind::Table,
            title: title.into(),
            source_app: None,
            summary: summary.into(),
            metrics: Vec::new(),
            table: Some(UiTable::new(columns, rows)),
            schedules: Vec::new(),
            tile: None,
            damage_rect: DirtyRect::new(40, 80, 1200, 600),
            raw_payload_json: "{}".to_string(),
        };
        self.active_views.push(view.clone());
        view
    }

    /// Synthesizes a `ScriptScheduleList` view displaying active JS time/event schedules.
    pub fn synthesize_schedule_list(
        &mut self,
        title: impl Into<String>,
        schedules: Vec<UiScheduleEntry>,
    ) -> SynthesizedUiView {
        self.synthesize_schedule_list_with_tile(title, schedules, None)
    }

    /// Synthesizes a `ScriptScheduleList` view along with an optional declarative `AppTile`
    /// (such as the `tasks.board` tile from the `Tasks` app).
    pub fn synthesize_schedule_list_with_tile(
        &mut self,
        title: impl Into<String>,
        schedules: Vec<UiScheduleEntry>,
        tile: Option<AppTile>,
    ) -> SynthesizedUiView {
        let count = schedules.len();
        let source_app = tile.as_ref().and_then(|t| t.folder().map(String::from));
        let raw_payload_json = tile
            .as_ref()
            .map(|t| t.to_mcp_value().to_json())
            .unwrap_or_else(|| "{}".to_string());
        let view = SynthesizedUiView {
            view_id: self.alloc_id(),
            kind: ViewKind::ScriptScheduleList,
            title: title.into(),
            source_app,
            summary: format!("{} scheduled local JavaScript automation(s)", count),
            metrics: Vec::new(),
            table: None,
            schedules,
            tile,
            damage_rect: DirtyRect::new(40, 120, 1200, 480),
            raw_payload_json,
        };
        self.active_views.push(view.clone());
        view
    }

    /// Synthesizes an `McpAppView` (or specialized dashboard/table) exclusively by the AI agent
    /// from the headless JSON output of an MCP application tool or resource, attaching an optional
    /// declarative `AppTile`.
    pub fn synthesize_from_mcp_output(
        &mut self,
        origin: &UiCallerOrigin,
        app_id: &str,
        tool_or_resource_name: &str,
        mcp_output_json: &str,
    ) -> Result<SynthesizedUiView, AiError> {
        self.synthesize_from_mcp_output_with_tile(
            origin,
            app_id,
            tool_or_resource_name,
            mcp_output_json,
            None,
        )
    }

    /// Synthesizes an `McpAppView` with an attached declarative `AppTile`.
    pub fn synthesize_from_mcp_output_with_tile(
        &mut self,
        origin: &UiCallerOrigin,
        app_id: &str,
        tool_or_resource_name: &str,
        mcp_output_json: &str,
        tile: Option<AppTile>,
    ) -> Result<SynthesizedUiView, AiError> {
        Self::verify_caller_origin(origin)?;

        let title = tile
            .as_ref()
            .map(|t| t.title.clone())
            .or_else(|| extract_json_string_field(mcp_output_json, "title"))
            .unwrap_or_else(|| format!("{} :: {}", app_id, tool_or_resource_name));

        let summary = extract_json_string_field(mcp_output_json, "summary")
            .or_else(|| extract_json_string_field(mcp_output_json, "status"))
            .or_else(|| tile.as_ref().map(|t| t.subtitle.clone()))
            .unwrap_or_else(|| format!("Agent-synthesized view for headless MCP app '{}'", app_id));

        let mut metrics = Vec::new();
        if let Some(cpu) = extract_json_string_field(mcp_output_json, "cpu_usage") {
            metrics.push(UiMetric::new("CPU", cpu, "%"));
        }
        if let Some(mem) = extract_json_string_field(mcp_output_json, "memory_mb") {
            metrics.push(UiMetric::new("Memory", mem, "MB"));
        }
        if let Some(metric_val) = extract_json_string_field(mcp_output_json, "value") {
            let label = extract_json_string_field(mcp_output_json, "label")
                .unwrap_or_else(|| tool_or_resource_name.to_string());
            let unit = extract_json_string_field(mcp_output_json, "unit").unwrap_or_default();
            metrics.push(UiMetric::new(label, metric_val, unit));
        }
        if let Some(t) = &tile {
            metrics.push(UiMetric::new(
                "Tile",
                &t.tile_id,
                t.folder().unwrap_or(app_id),
            ));
            metrics.push(UiMetric::new(
                "Items",
                format!("{}", t.items.len()),
                "entries",
            ));
        }
        if metrics.is_empty() {
            metrics.push(UiMetric::new("App Source", app_id, "MCP"));
            metrics.push(UiMetric::new("Capability", tool_or_resource_name, "tool"));
        }

        let table = if mcp_output_json.contains("\"rows\"")
            || mcp_output_json.contains("\"items\"")
            || mcp_output_json.contains("\"tasks\"")
        {
            Some(UiTable::new(
                vec!["Key".to_string(), "Value".to_string()],
                vec![
                    vec!["app".to_string(), app_id.to_string()],
                    vec!["capability".to_string(), tool_or_resource_name.to_string()],
                    vec!["summary".to_string(), summary.clone()],
                ],
            ))
        } else {
            None
        };

        let schedules = tile
            .as_ref()
            .map(|t| {
                t.items
                    .iter()
                    .map(|item| {
                        UiScheduleEntry::new(
                            item.subtitle.clone(),
                            item.badge.clone(),
                            item.status.clone(),
                        )
                    })
                    .collect()
            })
            .unwrap_or_default();


        let view = SynthesizedUiView {
            view_id: self.alloc_id(),
            kind: ViewKind::McpAppView,
            title,
            source_app: Some(app_id.to_string()),
            summary,
            metrics,
            table,
            schedules,
            tile,
            damage_rect: DirtyRect::new(60, 60, 1160, 680),
            raw_payload_json: mcp_output_json.to_string(),
        };
        self.active_views.push(view.clone());
        Ok(view)
    }

    /// Synthesizes a custom view from an explicit `ui_render_view` tool invocation by the AI agent.
    pub fn synthesize_custom_view(
        &mut self,
        kind: ViewKind,
        title: impl Into<String>,
        summary: impl Into<String>,
        source_app: Option<String>,
        raw_payload_json: impl Into<String>,
    ) -> SynthesizedUiView {
        let view = SynthesizedUiView {
            view_id: self.alloc_id(),
            kind,
            title: title.into(),
            source_app,
            summary: summary.into(),
            metrics: Vec::new(),
            table: None,
            schedules: Vec::new(),
            tile: None,
            damage_rect: DirtyRect::new(32, 32, 1216, 736),
            raw_payload_json: raw_payload_json.into(),
        };
        self.active_views.push(view.clone());
        view
    }
}
