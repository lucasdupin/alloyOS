//! First-run system setup. Deterministic and resumable; never changes AI providers.
//! Apps may opt into `setup_status` and `setup_start` tools, accepting no arguments
//! and returning {"status":"ready|needs_setup|waiting|error", "message":"..."}.
//! Authorization and credentials belong to the app. Missing hooks never imply ready.
use crate::{AgentRuntime, KnowledgeBasis, KnowledgeCategory, KnowledgeEntry};
use alloc::{
    collections::BTreeMap,
    format,
    string::{String, ToString},
    vec::Vec,
};
use alloyos_forge::provider::AiError;
use alloyos_mcp::display::{SetupView, SetupChoice, SetupRow, SetupAnswer};
use serde::{Deserialize, Serialize};

pub const SETUP_PATH: &str = "/etc/setup.json";
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum SetupStage {
    Welcome,
    AgentName,
    TimeRegion,
    ClockFormat,
    Apps,
    AppSetup,
    Memory,
    Review,
    Complete,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct SetupState {
    pub version: u32,
    pub stage: SetupStage,
    pub deferred: bool,
    pub user_name: String,
    pub agent_name: String,
    /// Uncommitted form draft; Clock MCP app owns the authoritative preference.
    #[serde(alias = "timezone")]
    pub draft_timezone: String,
    #[serde(alias = "clock_format")]
    pub draft_clock_format: String,
    pub selected_apps: Vec<String>,
    pub app_status: BTreeMap<String, String>,
    pub learning_paused: bool,
    pub search_results: BTreeMap<String, String>,
    pub catalog_message: String,
}
impl Default for SetupState {
    fn default() -> Self {
        Self {
            version: 1,
            stage: SetupStage::Welcome,
            deferred: false,
            user_name: String::new(),
            agent_name: crate::personality::DEFAULT_AGENT_NAME.into(),
            draft_timezone: "UTC".into(),
            draft_clock_format: "24h".into(),
            selected_apps: Vec::new(),
            app_status: BTreeMap::new(),
            learning_paused: false,
            search_results: BTreeMap::new(),
            catalog_message: String::new(),
        }
    }
}
fn invalid(message: &str) -> AiError {
    AiError::InvalidToolArguments(message.into())
}
fn action(value: &str,label: &str)->SetupChoice { SetupChoice {value:value.into(),label:label.into()} }
impl AgentRuntime {
    pub fn setup_state(&self) -> &SetupState {
        &self.setup
    }
    pub fn agent_name(&self) -> &str {
        &self.setup.agent_name
    }
    pub fn setup_active(&self) -> bool {
        self.setup_visible && !self.setup.deferred && self.setup.stage != SetupStage::Complete
    }
    pub(crate) fn restore_setup(&mut self) {
        if let Some(raw) = self.tool_registry.vfs_read(SETUP_PATH) {
            if raw.len() > 65536 {
                self.setup_load_error = true;
                return;
            }
            // Keep malformed state untouched and require an explicit recovery instead of overwriting it.
            match serde_json::from_str::<SetupState>(raw) {
                Ok(state)
                    if state.version == 1
                        && state.user_name.len() <= 80
                        && state.agent_name.len() <= 80
                        && !state.agent_name.is_empty()
                        && state.selected_apps.len() <= 64
                        && state.app_status.len() <= 64
                        && state.search_results.len() <= 8
                        && state.catalog_message.len() <= 1024
                        && !state.user_name.chars().any(char::is_control)
                        && !state.agent_name.chars().any(char::is_control)
                        && state.draft_timezone.len() <= 80
                        && !state.draft_timezone.chars().any(char::is_control)
                        && matches!(state.draft_clock_format.as_str(), "12h" | "24h") =>
                {
                    self.setup = state;
                    self.setup_load_error = false;
                }
                _ => self.setup_load_error = true,
            }
        } else {
            self.setup.learning_paused = self.is_learning_paused();
        }
    }
    fn save_setup(&mut self, next: SetupState) -> Result<(), AiError> {
        let text = serde_json::to_string(&next).map_err(|_| invalid("Could not encode setup"))?;
        self.tool_registry
            .persist_system_config(SETUP_PATH, &text)
            .map_err(AiError::ConfigError)?;
        self.setup = next;
        Ok(())
    }
    /// Called by graphical startup. Deferred setup stays deferred across boots.
    pub fn begin_setup(&mut self) -> Result<Option<SetupView>, AiError> {
        if self.setup_load_error {
            return Err(AiError::ConfigError(
                "Saved setup is invalid; repair /etc/setup.json before continuing".into(),
            ));
        }
        self.setup_visible = true;
        if self.setup_active() {
            self.render_setup().map(Some)
        } else {
            Ok(None)
        }
    }
    /// One chat message at startup: resume setup, or greet the returning user.
    pub fn startup_greeting(&mut self) -> Result<String, AiError> {
        if let Some(view) = self.begin_setup()? {
            return Ok(view.chat_text());
        }
        let name = self.setup.user_name.trim();
        Ok(if name.is_empty() {
            "Hi! What would you like to do today?".into()
        } else {
            format!("Hi, {name}! What would you like to do today?")
        })
    }

    fn setup_apps(&self) -> Vec<(String, String)> {
        self.tool_registry
            .mcp_registry()
            .manifests()
            .into_iter()
            .map(|m| (m.app_id.clone(), m.name.clone()))
            .collect()
    }
    pub(crate) fn setup_console(
        &mut self,
        input: &str,
    ) -> Option<Result<crate::ConsoleTurnOutput, AiError>> {
        let reopen = matches!(
            input,
            "/setup" | "open setup" | "finish setting up my system"
        );
        if !reopen && !self.setup_active() {
            return None;
        }
        let result = if reopen {
            if self.setup_load_error {
                Err(AiError::ConfigError(
                    "Saved setup is invalid; repair /etc/setup.json before continuing".into(),
                ))
            } else {
                let mut next = self.setup.clone();
                next.deferred = false;
                if next.stage == SetupStage::Complete {
                    next.stage = SetupStage::Review;
                }
                self.setup_visible = true;
                self.save_setup(next).and_then(|_| self.render_setup())
            }
        } else {
            self.answer_setup(input)
        };
        Some(result.map(|output| {
            crate::ConsoleTurnOutput {
                active_provider: self.active_provider_name(),
                active_model: self.active_model_id().into(),
                response_text: output.chat_text(),
                tool_outputs: Vec::new(),
                config_persisted: true,
            }
        }))
    }
    pub fn render_setup(&mut self) -> Result<SetupView, AiError> {
        let clock_report = if self.setup.stage == SetupStage::Review {
            match self.clock_settings("clock.settings", serde_json::json!({})) {
                Ok((zone, format)) => format!("{zone} · {format} (reported by Clock)"),
                Err(error) => format!("Clock settings unavailable: {error}"),
            }
        } else {
            String::new()
        };
        let s = &self.setup;
        let (title, prompt) = if s.deferred {
            (
                "Finish setup later",
                "Your progress is saved. Type /setup whenever you are ready.",
            )
        } else {
            match s.stage {
            SetupStage::Welcome => ("Welcome to alloyOS", "What should I call you? Type your preferred name below."),
            SetupStage::AgentName => ("Name your assistant", "What would you like to call your assistant? Type a name, or keep Alloy."),
            SetupStage::TimeRegion => ("Set your time zone", "Type a supported time zone or choose one below. This sets your Clock's local time."),
            SetupStage::ClockFormat => ("Set your clock format", "How should Clock show the time? Clock saves this preference in its own storage."),
            SetupStage::Apps => ("Choose your apps", "Select installed apps for your dashboard. To find more apps, type search followed by a service, such as search calendar. Installation availability depends on registry connectivity."),
            SetupStage::AppSetup => ("Set up your apps", "Apps own their setup and authorization. Check status or launch setup where supported; skip any app and continue later."),
            SetupStage::Memory => ("Memory and permissions", "May I learn preferences from our conversations? You can inspect, correct, forget, pause or resume learning later. App capabilities remain enforced; setup does not grant new action permissions."),
            SetupStage::Review => ("Ready to start", "Review your settings. Go back to make changes, or start using alloyOS."),
            SetupStage::Complete => ("Setup complete", "Your settings are saved. What would you like to do?"),
        }
        };
        let (field_label,field_value,field_prefix,field_placeholder) = match s.stage {
            SetupStage::Welcome => ("Your name",s.user_name.clone(),"","Preferred name"),
            SetupStage::AgentName => ("Assistant name",s.agent_name.clone(),"","Alloy"),
            SetupStage::TimeRegion => ("Time zone",s.draft_timezone.clone(),"","Search or enter a time zone"),
            SetupStage::Apps => ("Find more apps",String::new(),"search ","Email, calendar, documents…"),
            _ => ("",String::new(),"",""),
        };
        let mut view = SetupView {title:title.into(),prompt:prompt.into(),
            progress: format!("Step {} of 8",match s.stage {SetupStage::Welcome|SetupStage::AgentName=>1,SetupStage::TimeRegion=>2,SetupStage::ClockFormat=>3,SetupStage::Apps=>4,SetupStage::AppSetup=>5,SetupStage::Memory=>6,SetupStage::Review=>7,SetupStage::Complete=>8}),
            field_label:field_label.into(),field_value,field_prefix:field_prefix.into(),field_placeholder:field_placeholder.into(),
            ..SetupView::default()};
        if !s.deferred {
            match s.stage {
                SetupStage::Welcome => {}
                SetupStage::AgentName => view.choices.push(action("Alloy", "Keep Alloy")),
                SetupStage::TimeRegion => {
                    view.rows.push(SetupRow::new(
                        "timezone",
                        &s.draft_timezone,
                        "Time zone draft (Clock owns the saved setting)",
                        "",
                        "",
                    ));
                    view.choices
                        .push(action("skip_clock", "Set up Clock later"));
                    for zone in alloyos_mcp::clock::SUPPORTED_TIMEZONES {
                        view.choices.push(action(zone, zone));
                    }
                }
                SetupStage::ClockFormat => {
                    view.choices
                        .push(action("skip_clock", "Set up Clock later"));
                    view.choices.push(action("12h", "12-hour"));
                    view.choices.push(action("24h", "24-hour"));
                }
                SetupStage::Apps => {
                    if !s.catalog_message.is_empty() {
                        view.rows.push(SetupRow::new(
                            "catalog",
                            "App catalog",
                            &s.catalog_message,
                            "",
                            "",
                        ));
                    }
                    for (server, name) in &s.search_results {
                        view.choices.push(action(
                            &format!("install:{server}"),
                            &format!("Install {name}"),
                        ));
                    }
                    for (id, name) in self.setup_apps() {
                        view.choices.push(action(
                            &format!("app:{id}"),
                            &format!(
                                "{} {name}",
                                if s.selected_apps.contains(&id) {
                                    "✓"
                                } else {
                                    "+"
                                }
                            ),
                        ));
                    }
                    view.choices.push(action("next", "Continue"));
                }
                SetupStage::AppSetup => {
                    let tools = self.tool_registry.mcp_registry().list_tools();
                    for id in &s.selected_apps {
                        view.rows.push(SetupRow::new(
                            id,
                            id,
                            s.app_status
                                .get(id)
                                .map(|s| s.as_str())
                                .unwrap_or("Installed; setup status not checked"),
                            "",
                            "",
                        ));
                        if tools.iter().any(|t| t.name == format!("{id}.setup_status")) {
                            view.choices
                                .push(action(&format!("check:{id}"), &format!("Check {id}")));
                        }
                        if tools.iter().any(|t| t.name == format!("{id}.setup_start")) {
                            view.choices
                                .push(action(&format!("start:{id}"), &format!("Set up {id}")));
                        }
                    }
                    view.choices
                        .push(action("next", "Continue / skip remaining"));
                }
                SetupStage::Memory => {
                    view.choices.push(action("learn", "Learn my preferences"));
                    view.choices.push(action("pause", "Keep learning paused"));
                }
                SetupStage::Review => {
                    for (id, value) in [
                        ("Your name", s.user_name.clone()),
                        ("Assistant", s.agent_name.clone()),
                        ("Clock", clock_report.clone()),
                        ("Apps", s.selected_apps.join(", ")),
                        (
                            "Learning",
                            if s.learning_paused {
                                "Paused".into()
                            } else {
                                "Enabled".into()
                            },
                        ),
                    ] {
                        view.rows.push(SetupRow::new(id, id, value, "", ""));
                    }
                    for (id, status) in &s.app_status {
                        view.rows.push(SetupRow::new(id, id, status, "", ""));
                    }
                    view.choices.push(action("finish", "Start using alloyOS"));
                }
                SetupStage::Complete => {}
            }
            view.can_back = !matches!(s.stage,SetupStage::Welcome|SetupStage::Complete);
            view.can_defer = s.stage != SetupStage::Complete;
        }
        // Chat gets one conversational turn. Structured fields remain available
        // to clients, but are never dumped as a form or command menu.
        view.prompt = if s.deferred {
            "Of course. I've saved where we left off. Just say ‘open setup’ when you're ready to continue.".into()
        } else {
            match s.stage {
                SetupStage::Welcome => format!("Hi! I'm {}, your personal assistant. Let's get to know each other and set up a few things. We can pause anytime—just say ‘later’. What should I call you?", s.agent_name),
                SetupStage::AgentName => format!("Nice to meet you, {}! I'm your assistant, {}. Would you like to keep that name, or call me something else?", s.user_name, s.agent_name),
                SetupStage::TimeRegion => format!("{} it is. Let's get your clock right. What time zone do you use? For example, America/Los_Angeles or Europe/London. You can also say ‘skip clock’ for now.", s.agent_name),
                SetupStage::ClockFormat => "Got it. Do you prefer a 12-hour clock, like 3:00 PM, or a 24-hour clock, like 15:00?".into(),
                SetupStage::Apps => {
                    let apps = self.setup_apps().into_iter().map(|(_, name)|name).collect::<Vec<_>>().join(", ");
                    let selected = if s.selected_apps.is_empty() { "You haven't picked any yet.".into() } else { format!("You've picked {}. Say an app's name again to remove it.", s.selected_apps.join(", ")) };
                    let results = s.search_results.iter().map(|(id,name)|format!("{name} (say ‘install:{id}’ to install it)")).collect::<Vec<_>>().join(", ");
                    format!("Let's choose what to keep on your dashboard. {} {} {} {} Which app would you like to add? Say its name, ‘search calendar’ to look for something else, or ‘continue’ when you're ready.", if apps.is_empty() { "There aren't any installed apps yet.".into() } else { format!("Available apps are {apps}.") }, selected, s.catalog_message, results)
                }
                SetupStage::AppSetup => {
                    let actions = view.choices.iter().filter(|choice|choice.value != "next").map(|choice|format!("‘{}’",choice.label)).collect::<Vec<_>>().join(" or ");
                    let status = view.rows.iter().map(|row|format!("{}: {}.",row.title,row.detail)).collect::<Vec<_>>().join(" ");
                    if actions.is_empty() { format!("{} We can come back to connecting apps later. Shall we continue?",status) }
                    else { format!("{} Some apps may need you to connect an account. Would you like to {}? You can also say ‘continue’ and do this later.",status,actions) }
                }
                SetupStage::Memory => "May I learn your preferences from our conversations? You can review or change what I remember, or pause learning, anytime. Say ‘yes’ to allow it or ‘no’ to keep learning paused.".into(),
                SetupStage::Review => format!("We're almost ready, {}. I'll go by {}. Your clock reports {}. {} Learning is {}. Does that look right? Say ‘yes’ to finish, or ‘back’ to change something.", s.user_name,s.agent_name,clock_report,if s.selected_apps.is_empty() { "You haven't selected any dashboard apps.".into() } else { format!("Your dashboard will include {}.",s.selected_apps.join(", ")) },if s.learning_paused { "paused" } else { "enabled" }),
                SetupStage::Complete => format!("You're all set, {}! What would you like to do first?",s.user_name),
            }
        };
        self.setup_revision = self.setup_revision.saturating_add(1);
        view.revision = self.setup_revision;
        self.setup_view = if self.setup_active() {Some(view.clone())} else {None};
        Ok(view)
    }
    pub fn setup_view(&self)->Option<&SetupView> { self.setup_view.as_ref() }
    pub fn submit_setup_answer(&mut self, answer: &SetupAnswer)->Result<SetupView,AiError> {
        if self.setup_view.as_ref().map(|view|view.revision) != Some(answer.revision) {
            return Err(invalid("This setup question has changed; use the current controls"));
        }
        match self.answer_setup(&answer.value) {
            Ok(view)=>Ok(view),
            Err(error)=>{if let Some(view)=self.setup_view.as_mut(){view.error=error.to_string();} Err(error)}
        }
    }
    fn clock_settings(
        &mut self,
        tool: &str,
        args: serde_json::Value,
    ) -> Result<(String, String), AiError> {
        let call = alloyos_forge::provider::ToolCall::new(
            "setup_clock",
            "mcp_call_tool",
            serde_json::json!({"app":"clock","tool":tool,"args":args}).to_string(),
        );
        let mut synthesis = self.ui_synthesizer.clone();
        let output = self.tool_registry.execute_tool(&call, &mut synthesis)?;
        let body: serde_json::Value = serde_json::from_str(&output.output_json)
            .map_err(|_| invalid("Clock returned invalid settings"))?;
        if let Some(message) = body["error"].as_str() {
            return Err(invalid(message));
        }
        if body["ok"].as_bool() == Some(false) {
            return Err(invalid("Clock could not save its settings"));
        }
        let timezone = body["timezone"]
            .as_str()
            .ok_or_else(|| invalid("Clock did not report its time zone"))?;
        let format = body["clock_format"]
            .as_str()
            .ok_or_else(|| invalid("Clock did not report its time format"))?;
        Ok((timezone.into(), format.into()))
    }
    fn apply_clock_settings(&mut self, next: &SetupState) -> Result<(), AiError> {
        self.clock_settings("clock.configure",serde_json::json!({"timezone":next.draft_timezone,"clock_format":next.draft_clock_format})).map(|_|())
    }
    pub(crate) fn answer_setup(&mut self, input: &str) -> Result<SetupView, AiError> {
        if !self.setup_active() {
            return Err(invalid("Open setup before answering"));
        }
        let trimmed = input.trim();
        let lower = trimmed.to_lowercase();
        let matched_choice = self.setup_view.as_ref().and_then(|view| view.choices.iter()
            .find(|choice|choice.label.to_lowercase() == lower).map(|choice|choice.value.clone()));
        let normalized = matched_choice.unwrap_or_else(|| match (self.setup.stage, lower.as_str()) {
            (SetupStage::AgentName, "keep" | "keep it" | "yes") => self.setup.agent_name.clone(),
            (SetupStage::ClockFormat, "12" | "12-hour" | "12 hour") => "12h".into(),
            (SetupStage::ClockFormat, "24" | "24-hour" | "24 hour") => "24h".into(),
            (SetupStage::Memory, "yes" | "yes please") => "learn".into(),
            (SetupStage::Memory, "no" | "no thanks") => "pause".into(),
            (SetupStage::Review, "yes" | "done" | "looks good") => "finish".into(),
            (SetupStage::Apps | SetupStage::AppSetup, "continue" | "done" | "yes") => "next".into(),
            (_, "skip clock") => "skip_clock".into(),
            (SetupStage::Apps, _) => self.setup_apps().iter()
                .find(|(id,name)|id.to_lowercase() == lower || name.to_lowercase() == lower)
                .map(|(id,_)|format!("app:{id}")).unwrap_or_else(||trimmed.into()),
            _ => trimmed.into(),
        });
        let value = normalized.as_str();
        let mut next = self.setup.clone();
        if value == "later" || value == "/setup later" {
            next.deferred = true;
        } else if value == "skip_clock"
            && matches!(next.stage, SetupStage::TimeRegion | SetupStage::ClockFormat)
        {
            next.stage = SetupStage::Apps;
        } else if value == "back" {
            next.stage = match next.stage {
                SetupStage::AgentName => SetupStage::Welcome,
                SetupStage::TimeRegion => SetupStage::AgentName,
                SetupStage::ClockFormat => SetupStage::TimeRegion,
                SetupStage::Apps => SetupStage::ClockFormat,
                SetupStage::AppSetup => SetupStage::Apps,
                SetupStage::Memory => SetupStage::AppSetup,
                SetupStage::Review => SetupStage::Memory,
                _ => SetupStage::Welcome,
            };
        } else {
            match next.stage {
                SetupStage::Welcome | SetupStage::AgentName => {
                    if value.is_empty() || value.len() > 80 || value.chars().any(char::is_control) {
                        return Err(invalid("Enter a name of 1–80 characters"));
                    }
                    if next.stage == SetupStage::Welcome {
                        let entry = KnowledgeEntry::new(
                            KnowledgeCategory::Profile,
                            "Preferred name",
                            value,
                            KnowledgeBasis::Explicit,
                            "System setup",
                            "setup",
                            "global",
                        );
                        self.tool_registry.knowledge_upsert(entry, None)?;
                        next.user_name = value.into();
                        next.stage = SetupStage::AgentName;
                    } else {
                        next.agent_name = value.into();
                        next.stage = SetupStage::TimeRegion;
                    }
                }
                SetupStage::TimeRegion => {
                    if value.is_empty() || value.len() > 80 || value.chars().any(char::is_control) {
                        return Err(invalid(
                            "Choose one of the supported time zones shown in setup",
                        ));
                    }
                    next.draft_timezone = value.into();
                    self.apply_clock_settings(&next)?;
                    next.stage = SetupStage::ClockFormat;
                }
                SetupStage::ClockFormat => {
                    if value != "12h" && value != "24h" {
                        return Err(invalid("Choose 12h or 24h"));
                    }
                    next.draft_clock_format = value.into();
                    self.apply_clock_settings(&next)?;
                    next.stage = SetupStage::Apps;
                }
                SetupStage::Apps => {
                    if value == "next" {
                        next.stage = SetupStage::AppSetup;
                    } else if let Some(query) = value.strip_prefix("search ") {
                        if query.trim().is_empty() || query.len() > 200 {
                            return Err(invalid("Enter a service search of 1–200 characters"));
                        }
                        let call = alloyos_forge::provider::ToolCall::new(
                            "setup_search",
                            "smithery_search",
                            serde_json::json!({"query":query.trim(),"page_size":8}).to_string(),
                        );
                        next.search_results.clear();
                        match self.tool_registry.execute_tool(&call,&mut self.ui_synthesizer) {
                            Ok(output) => {
                                let body: serde_json::Value = serde_json::from_str(&output.output_json).map_err(|_|invalid("Invalid app catalog response"))?;
                                if let Some(servers) = body["servers"].as_array() {
                                    for server in servers.iter().take(8) {
                                        if let Some(id) = server["qualifiedName"].as_str().filter(|id|id.len()<=200 && !id.chars().any(char::is_control)) {
                                            let name = server["displayName"].as_str().unwrap_or(id).chars().take(100).collect::<String>();
                                            next.search_results.insert(id.into(),name);
                                        }
                                    }
                                }
                                next.catalog_message = format!("{} app(s) found. Install only the services you want to use.",next.search_results.len());
                            }
                            Err(error) => next.catalog_message = format!("Catalog search unavailable: {}. Continue with installed apps or retry later.",error).chars().take(1024).collect(),
                        }
                    } else if let Some(server) = value.strip_prefix("install:") {
                        if !next.search_results.contains_key(server) {
                            return Err(invalid("Search for an app before installing it"));
                        }
                        let call = alloyos_forge::provider::ToolCall::new(
                            "setup_install",
                            "smithery_install",
                            serde_json::json!({"server":server}).to_string(),
                        );
                        match self
                            .tool_registry
                            .execute_tool(&call, &mut self.ui_synthesizer)
                        {
                            Ok(output) => {
                                let body: serde_json::Value =
                                    serde_json::from_str(&output.output_json).map_err(|_| {
                                        invalid("Invalid app installation response")
                                    })?;
                                let id = body["app_id"].as_str().ok_or_else(|| {
                                    invalid("Installation did not report an app ID")
                                })?;
                                if !self.setup_apps().iter().any(|(app, _)| app == id) {
                                    return Err(invalid("Installed app was not registered"));
                                }
                                if !next.selected_apps.iter().any(|app| app == id)
                                    && next.selected_apps.len() < 64
                                {
                                    next.selected_apps.push(id.into());
                                }
                                next.search_results.remove(server);
                                next.catalog_message = format!("Installed {id}. Its authorization and setup are handled by the app.");
                            }
                            Err(error) => {
                                next.catalog_message = format!("Installation failed: {error}")
                                    .chars()
                                    .take(1024)
                                    .collect()
                            }
                        }
                    } else if let Some(id) = value.strip_prefix("app:") {
                        if !self.setup_apps().iter().any(|(app, _)| app == id) {
                            return Err(invalid("App is not installed"));
                        }
                        if next.selected_apps.iter().any(|app| app == id) {
                            next.selected_apps.retain(|app| app != id);
                            next.app_status.remove(id);
                        } else if next.selected_apps.len() < 64 {
                            next.selected_apps.push(id.into());
                        }
                    } else {
                        return Err(invalid("Choose an app or Continue"));
                    }
                }
                SetupStage::AppSetup => {
                    if value == "next" {
                        next.stage = SetupStage::Memory;
                    } else {
                        let (verb, id) = value
                            .split_once(':')
                            .ok_or_else(|| invalid("Choose an app setup action or Continue"))?;
                        if !next.selected_apps.iter().any(|app| app == id)
                            || !matches!(verb, "check" | "start")
                        {
                            return Err(invalid("Invalid app setup action"));
                        }
                        let tool = format!(
                            "{id}.{}",
                            if verb == "check" {
                                "setup_status"
                            } else {
                                "setup_start"
                            }
                        );
                        if !self
                            .tool_registry
                            .mcp_registry()
                            .list_tools()
                            .iter()
                            .any(|t| t.name == tool)
                        {
                            return Err(invalid("App does not expose this setup hook"));
                        }
                        let call = alloyos_forge::provider::ToolCall::new(
                            "setup_app",
                            "mcp_call_tool",
                            serde_json::json!({"app":id,"tool":tool,"args":{}}).to_string(),
                        );
                        let result = self
                            .tool_registry
                            .execute_tool(&call, &mut self.ui_synthesizer);
                        let status = match result {
                            Ok(output) => setup_status_message(&output.output_json),
                            Err(error) => format!("Setup failed: {error}"),
                        };
                        next.app_status.insert(id.into(), status);
                    }
                }
                SetupStage::Memory => {
                    next.learning_paused = match value {
                        "learn" => false,
                        "pause" => true,
                        _ => return Err(invalid("Choose learning enabled or paused")),
                    };
                    let previous = self.is_learning_paused();
                    self.tool_registry
                        .set_knowledge_learning_paused(next.learning_paused);
                    let conf = self.serialize_config();
                    if let Err(error) = self
                        .tool_registry
                        .persist_system_config(Self::CONFIG_PATH, &conf)
                    {
                        self.tool_registry.set_knowledge_learning_paused(previous);
                        return Err(AiError::ConfigError(error));
                    }
                    next.stage = SetupStage::Review;
                }
                SetupStage::Review => {
                    if value != "finish" {
                        return Err(invalid("Choose Start or Back"));
                    }
                    let tiles = self.tool_registry.mcp_registry().list_tiles();
                    let mut pins = self.pinned_tile_ids().to_vec();
                    for tile in tiles {
                        if tile
                            .folder()
                            .map(|id| next.selected_apps.iter().any(|app| app == id))
                            .unwrap_or(false)
                            && !pins.contains(&tile.tile_id)
                        {
                            pins.push(tile.tile_id);
                        }
                    }
                    self.set_pinned_tile_ids(&pins)?;
                    next.stage = SetupStage::Complete;
                }
                SetupStage::Complete => return Err(invalid("Setup is already complete")),
            }
        }
        self.save_setup(next)?;
        self.render_setup()
    }
}
fn setup_status_message(raw: &str) -> String {
    let Ok(mut value) = serde_json::from_str::<serde_json::Value>(raw) else {
        return "App returned an invalid setup response".into();
    };
    if let Some(result) = value.get("result") {
        value = result.clone();
    }
    if value.get("isError").and_then(|v| v.as_bool()) == Some(true) {
        return "App reported a setup error".into();
    }
    if let Some(structured) = value.get("structuredContent") {
        value = structured.clone();
    } else if let Some(text) = value
        .get("content")
        .and_then(|c| c.as_array())
        .and_then(|c| c.first())
        .and_then(|c| c.get("text"))
        .and_then(|t| t.as_str())
    {
        if let Ok(decoded) = serde_json::from_str(text) {
            value = decoded;
        }
    }
    let status = value
        .get("status")
        .and_then(|v| v.as_str())
        .unwrap_or("unknown");
    if !matches!(status, "ready" | "needs_setup" | "waiting" | "error") {
        return "Installed; app did not report a recognized setup status".into();
    }
    let message = value.get("message").and_then(|v| v.as_str()).unwrap_or("");
    format!(
        "{}: {}",
        status,
        message.chars().take(500).collect::<String>()
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{SharedMapVfsBackend, ToolVfsBackend};
    use alloc::rc::Rc;
    use core::cell::RefCell;
    fn runtime() -> (AgentRuntime, Rc<RefCell<SharedMapVfsBackend>>) {
        let backend = SharedMapVfsBackend::shared();
        for (name, content) in [
            ("clock.mcp", include_str!("../../../apps/clock/clock.mcp")),
            ("main.js", include_str!("../../../apps/clock/main.js")),
            ("tools.json", include_str!("../../../apps/clock/tools.json")),
            (
                "resources.json",
                include_str!("../../../apps/clock/resources.json"),
            ),
            ("tiles.json", include_str!("../../../apps/clock/tiles.json")),
            ("SKILL.md", include_str!("../../../apps/clock/SKILL.md")),
        ] {
            backend
                .borrow_mut()
                .write_file(&format!("/apps/clock/{name}"), content)
                .unwrap();
        }
        (AgentRuntime::with_vfs_backend(backend.clone()), backend)
    }
    #[test]
    fn greeting_and_setup_are_conversational_and_survive_restart() {
        let (mut agent, backend) = runtime();
        let greeting = agent.startup_greeting().unwrap();
        assert!(greeting.starts_with("Hi! I'm Alloy"));
        assert!(greeting.ends_with("What should I call you?"));
        assert!(!greeting.contains("Step "));
        assert!(!greeting.contains("reply:"));
        let introduction = agent.start_console_input("Wagner").unwrap().unwrap();
        assert!(introduction.response_text.contains("Nice to meet you, Wagner!"));
        agent.start_console_input("Nova").unwrap();
        agent.start_console_input("America/Los_Angeles").unwrap();
        agent.start_console_input("12-hour").unwrap();
        assert_eq!(agent.setup.stage, SetupStage::Apps);
        let app_name = agent.setup_apps().into_iter().find(|(id,_)|id == "clock").unwrap().1;
        agent.start_console_input(&app_name).unwrap();
        assert!(agent.setup.selected_apps.iter().any(|id|id == "clock"));
        agent.start_console_input("continue").unwrap();
        agent.start_console_input("continue").unwrap();
        agent.start_console_input("no").unwrap();
        let finished = agent.start_console_input("yes").unwrap().unwrap();
        assert!(finished.response_text.contains("You're all set, Wagner!"));
        assert!(agent.is_learning_paused());
        let mut restored = AgentRuntime::with_vfs_backend(backend);
        assert_eq!(restored.startup_greeting().unwrap(), "Hi, Wagner! What would you like to do today?");
        assert!(!restored.setup_active());
        assert_eq!(restored.agent_name(), "Nova");
    }

    #[test]
    fn deferred_setup_greets_without_restarting_questions() {
        let (mut agent, backend) = runtime();
        agent.startup_greeting().unwrap();
        agent.answer_setup("Wagner").unwrap();
        let deferred = agent.answer_setup("later").unwrap().chat_text();
        assert!(deferred.contains("saved where we left off"));
        let mut restored = AgentRuntime::with_vfs_backend(backend);
        assert!(restored.startup_greeting().unwrap().starts_with("Hi, Wagner!"));
        assert!(!restored.setup_active());
        let resumed = restored.start_console_input("open setup").unwrap().unwrap();
        assert!(resumed.response_text.contains("Nice to meet you, Wagner!"));
        assert!(!resumed.response_text.contains("reply:"));
    }

    #[test]
    fn first_run_is_resumable_and_never_changes_provider() {
        let (mut runtime, backend) = runtime();
        let before = runtime
            .tool_registry
            .vfs_read(AgentRuntime::CONFIG_PATH)
            .unwrap()
            .to_string();
        assert!(runtime.begin_setup().unwrap().is_some());
        runtime.answer_setup("Wagner").unwrap();
        runtime.answer_setup("Alloy").unwrap();
        runtime.answer_setup("America/Los_Angeles").unwrap();
        runtime.answer_setup("12h").unwrap();
        assert_eq!(
            runtime
                .tool_registry
                .vfs_read(AgentRuntime::CONFIG_PATH)
                .unwrap(),
            before
        );
        let mut restored = AgentRuntime::with_vfs_backend(backend);
        assert_eq!(restored.setup.stage, SetupStage::Apps);
        assert_eq!(restored.agent_name(), "Alloy");
        assert!(restored.begin_setup().unwrap().is_some());
        restored.answer_setup("next").unwrap();
        restored.answer_setup("next").unwrap();
        restored.answer_setup("pause").unwrap();
        restored.answer_setup("finish").unwrap();
        assert!(restored.is_learning_paused());
        assert!(!restored.setup_active());
        assert_eq!(restored.setup.stage, SetupStage::Complete);
    }
    #[test]
    fn review_reads_clock_owned_preferences_instead_of_setup_drafts() {
        let (mut runtime, backend) = runtime();
        runtime.begin_setup().unwrap();
        for answer in [
            "Wagner",
            "Alloy",
            "America/Los_Angeles",
            "12h",
            "next",
            "next",
            "learn",
        ] {
            runtime.answer_setup(answer).unwrap();
        }
        // An app-side change after the wizard's draft must be reflected in review.
        runtime
            .clock_settings(
                "clock.configure",
                serde_json::json!({"timezone":"Asia/Tokyo","clock_format":"24h"}),
            )
            .unwrap();
        let output = runtime.render_setup().unwrap();
        assert!(output.rows.iter().any(|row| row.title == "Clock" && row.detail.contains("Asia/Tokyo")));
        let mut restored = AgentRuntime::with_vfs_backend(backend);
        assert_eq!(
            restored
                .clock_settings("clock.settings", serde_json::json!({}))
                .unwrap(),
            ("Asia/Tokyo".into(), "24h".into())
        );
    }
    #[test]
    fn missing_clock_is_explicit_and_can_be_skipped() {
        let mut runtime = AgentRuntime::with_vfs_backend(SharedMapVfsBackend::shared());
        runtime.begin_setup().unwrap();
        runtime.answer_setup("Wagner").unwrap();
        runtime.answer_setup("Alloy").unwrap();
        assert!(runtime.answer_setup("UTC").is_err());
        assert_eq!(runtime.setup.stage, SetupStage::TimeRegion);
        runtime.answer_setup("skip_clock").unwrap();
        assert_eq!(runtime.setup.stage, SetupStage::Apps);
    }
    #[test]
    fn defer_survives_reboot_and_reopen_resumes() {
        let (mut runtime, backend) = runtime();
        runtime.begin_setup().unwrap();
        runtime.answer_setup("Wagner").unwrap();
        runtime.answer_setup("later").unwrap();
        let mut restored = AgentRuntime::with_vfs_backend(backend);
        assert!(restored.begin_setup().unwrap().is_none());
        let output = restored
            .start_console_input("finish setting up my system")
            .unwrap()
            .unwrap();
        assert!(output.response_text.contains("assistant"));
        assert_eq!(restored.setup.stage, SetupStage::AgentName);
        restored.answer_setup("Nova").unwrap();
        assert_eq!(restored.agent_name(), "Nova");
        restored.answer_setup("back").unwrap();
        assert_eq!(restored.setup.stage, SetupStage::AgentName);
    }
    #[test]
    fn invalid_answers_do_not_advance_or_change_clock() {
        let (mut runtime, _) = runtime();
        runtime.begin_setup().unwrap();
        assert!(runtime.answer_setup("\n").is_err());
        runtime.answer_setup("Wagner").unwrap();
        runtime.answer_setup("Alloy").unwrap();
        assert!(runtime.answer_setup("Mars/Olympus").is_err());
        assert_eq!(runtime.setup.stage, SetupStage::TimeRegion);
        assert!(runtime
            .tool_registry
            .vfs_read("/var/clock/preferences.json")
            .is_none());
    }
    #[test]
    fn stale_setup_answers_are_rejected_without_widget_actions() {
        let (mut runtime, _) = runtime(); runtime.begin_setup().unwrap();
        let old=runtime.setup_view().unwrap().revision;
        runtime.answer_setup("Wagner").unwrap();
        assert!(runtime.submit_setup_answer(&SetupAnswer {revision:old,value:"Alloy".into()}).is_err());
        assert!(runtime.synthesized_tiles().iter().all(|tile|tile.folder()!=Some("setup")));
    }
    #[test]
    fn catalog_search_installs_only_an_explicitly_selected_result() {
        let (mut runtime, _) = runtime();
        runtime.begin_setup().unwrap();
        for answer in ["Wagner", "Alloy", "UTC", "24h"] {
            runtime.answer_setup(answer).unwrap();
        }
        assert!(runtime.answer_setup("install:acme/service").is_err());
        runtime.tool_registry.register_http_response(
            "https://api.smithery.ai/servers?q=calendar&pageSize=8",
            r#"{"servers":[{"qualifiedName":"acme/service","displayName":"Calendar"}]}"#,
        );
        runtime.tool_registry.register_http_response("https://api.smithery.ai/servers/acme%2Fservice",
            r#"{"qualifiedName":"acme/service","displayName":"Calendar","remote":true,"deploymentUrl":"https://calendar.example/mcp","tools":[{"name":"events","inputSchema":{"type":"object","properties":{}}}]}"#);
        runtime.answer_setup("search calendar").unwrap();
        assert!(runtime.setup.search_results.contains_key("acme/service"));
        assert!(runtime.setup.selected_apps.is_empty());
        runtime.answer_setup("install:acme/service").unwrap();
        assert!(runtime.setup.selected_apps.contains(&"service".into()));
        assert!(runtime
            .tool_registry
            .vfs_read("/apps/service/service.mcp")
            .is_some());
        assert_eq!(runtime.setup.stage, SetupStage::Apps);
    }
    #[test]
    fn unavailable_catalog_preserves_progress_and_allows_continuing() {
        let (mut runtime, _) = runtime();
        runtime.begin_setup().unwrap();
        for answer in ["Wagner", "Alloy", "UTC", "24h"] {
            runtime.answer_setup(answer).unwrap();
        }
        runtime.answer_setup("search calendar").unwrap();
        assert!(runtime.setup.catalog_message.contains("unavailable"));
        assert_eq!(runtime.setup.stage, SetupStage::Apps);
        runtime.answer_setup("next").unwrap();
        assert_eq!(runtime.setup.stage, SetupStage::AppSetup);
    }
    #[test]
    fn failed_progress_write_is_reported_without_advancing() {
        #[derive(Debug)]
        struct RejectSetup(SharedMapVfsBackend);
        impl ToolVfsBackend for RejectSetup {
            fn read_file(&mut self, path: &str) -> Option<String> {
                self.0.read_file(path)
            }
            fn write_file(&mut self, path: &str, content: &str) -> Result<(), String> {
                if path == SETUP_PATH {
                    Err("disk full".into())
                } else {
                    self.0.write_file(path, content)
                }
            }
            fn delete_file(&mut self, path: &str) -> Result<bool, String> {
                self.0.delete_file(path)
            }
            fn list_dir(&mut self, path: &str) -> Vec<String> {
                self.0.list_dir(path)
            }
        }
        let backend = Rc::new(RefCell::new(RejectSetup(SharedMapVfsBackend::new())));
        let mut runtime = AgentRuntime::with_vfs_backend(backend);
        runtime.begin_setup().unwrap();
        assert!(runtime.answer_setup("later").is_err());
        assert!(!runtime.setup.deferred);
        assert_eq!(runtime.setup.stage, SetupStage::Welcome);
    }
    #[test]
    fn malformed_progress_is_not_overwritten() {
        let backend = SharedMapVfsBackend::shared();
        backend
            .borrow_mut()
            .write_file(SETUP_PATH, "broken")
            .unwrap();
        let mut runtime = AgentRuntime::with_vfs_backend(backend.clone());
        assert!(runtime.begin_setup().is_err());
        assert!(runtime.start_console_input("/setup").is_err());
        assert_eq!(
            backend.borrow_mut().read_file(SETUP_PATH).unwrap(),
            "broken"
        );
    }
    #[test]
    fn setup_status_never_claims_readiness_without_explicit_result() {
        assert!(setup_status_message(r#"{"success":true}"#).contains("did not report"));
        assert_eq!(
            setup_status_message(r#"{"status":"waiting","message":"Authorize in the app"}"#),
            "waiting: Authorize in the app"
        );
        assert_eq!(
            setup_status_message(r#"{"content":[{"text":"{\"status\":\"ready\"}"}]}"#),
            "ready: "
        );
    }
}
