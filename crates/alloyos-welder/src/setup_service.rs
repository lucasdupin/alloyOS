//! Versioned setup-worker contract. The protected worker proposes changes; the
//! host independently validates and commits only the fixed, step-scoped effects.
use crate::setup::{SetupStage, SetupState};
use alloc::{format, string::String, vec, vec::Vec};
use alloyos_mcp::display::{SetupChoice, SetupRow, SetupView};
use serde::{Deserialize, Serialize};

pub const VERSION: u32 = 1;
pub const MAX_BYTES: usize = 128 * 1024;
pub const MAX_CHUNK: usize = 4096;
const HEADER: usize = 24;
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Request {
    pub version: u32,
    pub id: u64,
    pub state: SetupState,
    pub revision: u64,
    pub operation: Operation,
    pub clock_report: String,
    pub persistent: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "op", deny_unknown_fields)]
pub enum Operation {
    Render,
    Answer {
        value: String,
    },
    /// Supervisor-only containment gate, never exposed as a model/app tool.
    FaultProbe,
    HangProbe,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "effect", deny_unknown_fields)]
pub enum Effect {
    PreferredName { value: String },
    Clock { timezone: String, format: String },
    Learning { paused: bool },
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Plan {
    pub next: SetupState,
    pub effects: Vec<Effect>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Response {
    pub version: u32,
    pub id: u64,
    pub view: Option<SetupView>,
    pub plan: Option<Plan>,
    pub error: Option<String>,
}
pub trait Transport {
    fn exchange(&mut self, request: &Request) -> Result<Response, String>;
}
fn text(value: &str, max: usize) -> bool {
    value.len() <= max && !value.chars().any(char::is_control)
}
fn active(stage: SetupStage) -> bool {
    !matches!(stage, SetupStage::Apps | SetupStage::AppSetup)
}
impl Request {
    pub fn validate(&self) -> Result<(), String> {
        let s = &self.state;
        if self.version != VERSION
            || self.id == 0
            || !active(s.stage)
            || s.version != 1
            || !text(&s.user_name, 80)
            || !text(&s.agent_name, 80)
            || s.agent_name.is_empty()
            || !text(&s.draft_timezone, 80)
            || !matches!(s.draft_clock_format.as_str(), "12h" | "24h")
            || s.selected_apps.len() > 64
            || s.app_status.len() > 64
            || s.search_results.len() > 8
            || s.catalog_message.len() > 1024
            || self.clock_report.len() > 2048
            || serde_json::to_vec(self)
                .map_err(|_| "Setup request encoding failed")?
                .len()
                > MAX_BYTES
        {
            return Err("Invalid setup service request".into());
        }
        if let Operation::Answer { value } = &self.operation {
            if value.len() > 1024 {
                return Err("Invalid setup answer".into());
            }
        }
        Ok(())
    }
    pub fn normalized_answer(&self) -> Result<String, String> {
        let Operation::Answer { value } = &self.operation else {
            return Err("Not a setup answer".into());
        };
        let trimmed = value.trim();
        let lower = trimmed.to_lowercase();
        // This is broker policy, never a mapping supplied by the worker's view.
        Ok(match (self.state.stage, lower.as_str()) {
            (SetupStage::AgentName, "keep alloy") => "Alloy".into(),
            (SetupStage::AgentName, "keep" | "keep it" | "yes") => self.state.agent_name.clone(),
            (SetupStage::ClockFormat, "12" | "12h" | "12-hour" | "12 hour") => "12h".into(),
            (SetupStage::ClockFormat, "24" | "24h" | "24-hour" | "24 hour") => "24h".into(),
            (SetupStage::Memory, "yes" | "yes please" | "learn" | "learn my preferences") => {
                "learn".into()
            }
            (SetupStage::Memory, "no" | "no thanks" | "pause" | "keep learning paused") => {
                "pause".into()
            }
            (
                SetupStage::Review,
                "yes" | "done" | "looks good" | "finish" | "start using alloyos",
            ) => "finish".into(),
            (_, "skip clock" | "set up clock later") => "skip_clock".into(),
            (SetupStage::TimeRegion, _) => alloyos_mcp::clock::SUPPORTED_TIMEZONES
                .iter()
                .find(|zone| zone.to_lowercase() == lower)
                .map(|zone| (*zone).into())
                .unwrap_or_else(|| trimmed.into()),
            _ => trimmed.into(),
        })
    }
}
fn back(stage: SetupStage) -> SetupStage {
    match stage {
        SetupStage::AgentName => SetupStage::Welcome,
        SetupStage::TimeRegion => SetupStage::AgentName,
        SetupStage::ClockFormat => SetupStage::TimeRegion,
        SetupStage::Memory => SetupStage::ClockFormat,
        SetupStage::Review => SetupStage::Memory,
        _ => SetupStage::Welcome,
    }
}
fn control(state: &SetupState, value: &str) -> Option<SetupState> {
    let mut next = state.clone();
    if matches!(value, "later" | "/setup later") {
        next.deferred = true;
    } else if value == "back" {
        next.stage = back(state.stage);
    } else if value == "skip_clock"
        && matches!(
            state.stage,
            SetupStage::TimeRegion | SetupStage::ClockFormat
        )
    {
        next.stage = SetupStage::Memory;
    } else {
        return None;
    }
    Some(next)
}
/// Broker policy: no file paths, generic tool dispatch, account configuration or
/// install authority crosses this boundary. Unchanged fields must stay exact.
pub fn validate_plan(request: &Request, plan: &Plan) -> Result<(), String> {
    request.validate()?;
    if request.state.deferred || request.state.stage == SetupStage::Complete {
        return Err("Open setup before answering".into());
    }
    let value = request.normalized_answer()?;
    if let Some(next) = control(&request.state, &value) {
        return if next == plan.next && plan.effects.is_empty() {
            Ok(())
        } else {
            Err("Invalid setup control effect".into())
        };
    }
    let mut expected = request.state.clone();
    let effects = match request.state.stage {
        SetupStage::Welcome if !value.is_empty() && text(&value, 80) => {
            expected.user_name = value.clone();
            expected.stage = SetupStage::AgentName;
            vec![Effect::PreferredName { value }]
        }
        SetupStage::AgentName if !value.is_empty() && text(&value, 80) => {
            expected.agent_name = value;
            expected.stage = SetupStage::TimeRegion;
            vec![]
        }
        SetupStage::TimeRegion if !value.is_empty() && text(&value, 80) => {
            expected.draft_timezone = value;
            expected.stage = SetupStage::ClockFormat;
            vec![Effect::Clock {
                timezone: expected.draft_timezone.clone(),
                format: expected.draft_clock_format.clone(),
            }]
        }
        SetupStage::ClockFormat if matches!(value.as_str(), "12h" | "24h") => {
            expected.draft_clock_format = value;
            expected.stage = SetupStage::Memory;
            vec![Effect::Clock {
                timezone: expected.draft_timezone.clone(),
                format: expected.draft_clock_format.clone(),
            }]
        }
        SetupStage::Memory if matches!(value.as_str(), "learn" | "pause") => {
            expected.learning_paused = value == "pause";
            expected.stage = SetupStage::Review;
            vec![Effect::Learning {
                paused: expected.learning_paused,
            }]
        }
        SetupStage::Review if value == "finish" => {
            expected.stage = SetupStage::Complete;
            vec![]
        }
        _ => return Err("Invalid answer for the current setup step".into()),
    };
    if expected != plan.next || effects != plan.effects {
        return Err("Setup worker exceeded the current step authority".into());
    }
    Ok(())
}
fn plan(request: &Request) -> Result<Plan, String> {
    let value = request.normalized_answer()?;
    let mut next = request.state.clone();
    let mut effects = Vec::new();
    if let Some(control) = control(&next, &value) {
        next = control;
    } else {
        match next.stage {
            SetupStage::Welcome => {
                next.user_name = value.clone();
                next.stage = SetupStage::AgentName;
                effects.push(Effect::PreferredName { value });
            }
            SetupStage::AgentName => {
                next.agent_name = value;
                next.stage = SetupStage::TimeRegion;
            }
            SetupStage::TimeRegion => {
                next.draft_timezone = value;
                next.stage = SetupStage::ClockFormat;
                effects.push(Effect::Clock {
                    timezone: next.draft_timezone.clone(),
                    format: next.draft_clock_format.clone(),
                });
            }
            SetupStage::ClockFormat => {
                next.draft_clock_format = value;
                next.stage = SetupStage::Memory;
                effects.push(Effect::Clock {
                    timezone: next.draft_timezone.clone(),
                    format: next.draft_clock_format.clone(),
                });
            }
            SetupStage::Memory => {
                next.learning_paused = value == "pause";
                next.stage = SetupStage::Review;
                effects.push(Effect::Learning {
                    paused: next.learning_paused,
                });
            }
            SetupStage::Review => next.stage = SetupStage::Complete,
            _ => return Err("Setup is already complete".into()),
        }
    }
    let plan = Plan { next, effects };
    validate_plan(request, &plan)?;
    Ok(plan)
}
pub const LEARNING_PROMPT: &str = "May I learn your preferences from our conversations? You can review or change what I remember, or pause learning, anytime. Say ‘yes’ to allow it or ‘no’ to keep learning paused.";
fn choice(value: &str, label: &str) -> SetupChoice {
    SetupChoice {
        value: value.into(),
        label: label.into(),
    }
}
fn canonical_choices(state: &SetupState) -> Vec<SetupChoice> {
    if state.deferred {
        return Vec::new();
    }
    match state.stage {
        SetupStage::AgentName => vec![choice("Alloy", "Keep Alloy")],
        SetupStage::TimeRegion => {
            let mut choices = vec![choice("skip_clock", "Set up Clock later")];
            choices.extend(
                alloyos_mcp::clock::SUPPORTED_TIMEZONES
                    .iter()
                    .map(|zone| choice(zone, zone)),
            );
            choices
        }
        SetupStage::ClockFormat => vec![
            choice("skip_clock", "Set up Clock later"),
            choice("12h", "12-hour"),
            choice("24h", "24-hour"),
        ],
        SetupStage::Memory => vec![
            choice("learn", "Learn my preferences"),
            choice("pause", "Keep learning paused"),
        ],
        SetupStage::Review => vec![choice("finish", "Start using alloyOS")],
        _ => Vec::new(),
    }
}
fn progress(stage: SetupStage) -> String {
    format!(
        "Step {} of 6",
        match stage {
            SetupStage::Welcome => 1,
            SetupStage::AgentName => 2,
            SetupStage::TimeRegion => 3,
            SetupStage::ClockFormat => 4,
            SetupStage::Memory => 5,
            _ => 6,
        }
    )
}
fn validate_view(request: &Request, view: &SetupView) -> Result<(), String> {
    let s = &request.state;
    if view.revision != request.revision.saturating_add(1)
        || view.choices != canonical_choices(s)
        || view.progress != progress(s.stage)
        || view.can_back
            != (!s.deferred && !matches!(s.stage, SetupStage::Welcome | SetupStage::Complete))
        || view.can_defer != (!s.deferred && s.stage != SetupStage::Complete)
        || !text(&view.title, 200)
        || view.prompt.len() > 4096
        || !text(&view.field_label, 200)
        || !text(&view.field_value, 80)
        || !view.field_prefix.is_empty()
        || !text(&view.field_placeholder, 200)
        || view.error.len() > 1024
        || view.rows.len() > 4
        || view
            .rows
            .iter()
            .any(|row| !text(&row.title, 200) || row.detail.len() > 2048)
        || (!s.deferred && s.stage == SetupStage::Memory && view.prompt != LEARNING_PROMPT)
        || (s.deferred && !request.persistent && !view.prompt.contains("this session only"))
    {
        return Err("Invalid setup view or consent controls".into());
    }
    Ok(())
}
fn render(request: &Request) -> SetupView {
    let s = &request.state;
    let (title,prompt,step) = match s.stage {
        SetupStage::Welcome => ("Welcome to alloyOS", "Hi! Let’s get to know each other and set up a few things. We can pause anytime. What should I call you?".into(),1),
        SetupStage::AgentName => ("Name your assistant",format!("Nice to meet you, {}! What would you like to call me? You can choose a name, or use the default, Alloy.",s.user_name),2),
        SetupStage::TimeRegion => ("Set your local time",format!("{} it is. Let's get your clock right. What city and country are you in? I’ll use that to set your local time.",s.agent_name),3),
        SetupStage::ClockFormat => ("Set your clock format","Got it. Do you prefer a 12-hour clock, like 3:00 PM, or a 24-hour clock, like 15:00?".into(),4),
        SetupStage::Memory => ("Memory and permissions",LEARNING_PROMPT.into(),5),
        SetupStage::Review => ("Ready to start",format!("We're almost ready, {}. I'll go by {}. Your clock reports {}. Learning is {}. Does that look right? Say ‘yes’ to finish, or ‘back’ to change something.",s.user_name,s.agent_name,request.clock_report,if s.learning_paused {"paused"}else{"enabled"}),6),
        _ => ("Setup complete",format!("You're all set, {}! What would you like to do first?",s.user_name),6),
    };
    let mut view = SetupView {
        title: title.into(),
        prompt,
        progress: format!("Step {step} of 6"),
        revision: request.revision.saturating_add(1),
        ..SetupView::default()
    };
    if s.deferred {
        view.title = "Finish setup later".into();
        view.prompt=if request.persistent {"Of course. I've saved where we left off. Just say ‘open setup’ when you're ready to continue."} else {"Of course. I’ll keep where we left off for this session only; storage is volatile. Just say ‘open setup’ when you’re ready to continue."}.into();
        return view;
    }
    view.can_back = !matches!(s.stage, SetupStage::Welcome | SetupStage::Complete);
    view.can_defer = s.stage != SetupStage::Complete;
    match s.stage {
        SetupStage::Welcome => {
            view.field_label = "Your name".into();
            view.field_value = s.user_name.clone();
            view.field_placeholder = "Preferred name".into();
        }
        SetupStage::AgentName => {
            view.field_label = "Assistant name".into();
            view.field_value = s.agent_name.clone();
            view.field_placeholder = "Alloy".into();
            view.choices.push(choice("Alloy", "Keep Alloy"));
        }
        SetupStage::TimeRegion => {
            view.field_label = "City and country".into();
            view.field_placeholder = "For example, Los Angeles, United States".into();
            view.rows.push(SetupRow::new(
                "timezone",
                &s.draft_timezone,
                "Time zone draft (Clock owns the saved setting)",
                "",
                "",
            ));
            view.choices
                .push(choice("skip_clock", "Set up Clock later"));
            for zone in alloyos_mcp::clock::SUPPORTED_TIMEZONES {
                view.choices.push(choice(zone, zone));
            }
        }
        SetupStage::ClockFormat => {
            view.choices
                .push(choice("skip_clock", "Set up Clock later"));
            view.choices.push(choice("12h", "12-hour"));
            view.choices.push(choice("24h", "24-hour"));
        }
        SetupStage::Memory => {
            view.choices.push(choice("learn", "Learn my preferences"));
            view.choices.push(choice("pause", "Keep learning paused"));
        }
        SetupStage::Review => {
            for (label, value) in [
                ("Your name", s.user_name.clone()),
                ("Assistant", s.agent_name.clone()),
                ("Clock", request.clock_report.clone()),
                (
                    "Learning",
                    if s.learning_paused {
                        "Paused".into()
                    } else {
                        "Enabled".into()
                    },
                ),
            ] {
                view.rows.push(SetupRow::new(label, label, value, "", ""));
            }
            view.choices.push(choice("finish", "Start using alloyOS"));
        }
        _ => {}
    }
    view.choices = canonical_choices(s);
    view
}
pub fn execute(request: &Request) -> Response {
    let mut response = Response {
        version: VERSION,
        id: request.id,
        view: None,
        plan: None,
        error: None,
    };
    let result = (|| {
        request.validate()?;
        match request.operation {
            Operation::Render => response.view = Some(render(request)),
            Operation::Answer { .. } => response.plan = Some(plan(request)?),
            Operation::FaultProbe | Operation::HangProbe => {
                return Err("Fault probe requires protected execution".into())
            }
        };
        Ok::<(), String>(())
    })();
    if let Err(error) = result {
        response.error = Some(error);
    }
    response
}
impl Response {
    pub fn validate_for(&self, request: &Request) -> Result<(), String> {
        if self.version != VERSION
            || self.id != request.id
            || serde_json::to_vec(self)
                .map_err(|_| "Invalid setup response")?
                .len()
                > MAX_BYTES
        {
            return Err("Invalid setup worker response".into());
        }
        if let Some(error) = &self.error {
            if error.len() > 1024 || self.view.is_some() || self.plan.is_some() {
                return Err("Invalid setup worker error".into());
            }
            return Err(error.clone());
        }
        match &request.operation {
            Operation::Render => {
                let view = self
                    .view
                    .as_ref()
                    .filter(|_| self.plan.is_none())
                    .ok_or("Missing setup view")?;
                validate_view(request, view)
            }
            Operation::Answer { .. } => {
                if self.view.is_some() {
                    return Err("Unexpected setup view".into());
                }
                validate_plan(request, self.plan.as_ref().ok_or("Missing setup plan")?)
            }
            Operation::FaultProbe | Operation::HangProbe => {
                Err("Fault probe returned unexpectedly".into())
            }
        }
    }
}
/// Strict contiguous framing over the existing 4 KiB process IPC payload limit.
pub fn chunk(id: u64, bytes: &[u8], offset: usize) -> Result<Vec<u8>, String> {
    if id == 0 || bytes.is_empty() || bytes.len() > MAX_BYTES || offset >= bytes.len() {
        return Err("Invalid Welder message".into());
    }
    let end = bytes.len().min(offset + MAX_CHUNK - HEADER);
    let mut out = Vec::with_capacity(HEADER + end - offset);
    out.extend_from_slice(b"WLD1");
    out.extend_from_slice(&VERSION.to_le_bytes());
    out.extend_from_slice(&id.to_le_bytes());
    out.extend_from_slice(&(bytes.len() as u32).to_le_bytes());
    out.extend_from_slice(&(offset as u32).to_le_bytes());
    out.extend_from_slice(&bytes[offset..end]);
    Ok(out)
}
pub const fn chunk_payload_bytes(frame_bytes: usize) -> usize {
    frame_bytes - HEADER
}
#[derive(Default)]
pub struct Assembly {
    id: Option<u64>,
    total: usize,
    bytes: Vec<u8>,
}
impl Assembly {
    pub fn push(
        &mut self,
        frame: &[u8],
        expected: Option<u64>,
    ) -> Result<Option<(u64, Vec<u8>)>, String> {
        if frame.len() <= HEADER
            || frame.len() > MAX_CHUNK
            || &frame[..4] != b"WLD1"
            || u32::from_le_bytes(frame[4..8].try_into().unwrap()) != VERSION
        {
            return Err("Invalid Welder frame".into());
        }
        let id = u64::from_le_bytes(frame[8..16].try_into().unwrap());
        let total = u32::from_le_bytes(frame[16..20].try_into().unwrap()) as usize;
        let offset = u32::from_le_bytes(frame[20..24].try_into().unwrap()) as usize;
        if id == 0
            || expected.is_some_and(|expected| expected != id)
            || total == 0
            || total > MAX_BYTES
            || offset != self.bytes.len()
            || self.id.is_some_and(|old| old != id)
            || self.id.is_some() && self.total != total
            || frame.len() - HEADER > total.saturating_sub(offset)
        {
            return Err("Welder framing mismatch".into());
        }
        self.id = Some(id);
        self.total = total;
        self.bytes
            .try_reserve(frame.len() - HEADER)
            .map_err(|_| "Welder buffer allocation failed")?;
        self.bytes.extend_from_slice(&frame[HEADER..]);
        if self.bytes.len() == total {
            self.id = None;
            self.total = 0;
            Ok(Some((id, core::mem::take(&mut self.bytes))))
        } else {
            Ok(None)
        }
    }
}

#[derive(Default)]
pub(crate) struct Port {
    next: u64,
    transport: Option<alloc::boxed::Box<dyn Transport>>,
}
impl crate::NativeToolRegistry {
    pub fn set_setup_service(&mut self, transport: alloc::boxed::Box<dyn Transport>) {
        self.setup_service.borrow_mut().transport = Some(transport);
    }
    pub(crate) fn setup_exchange(&mut self, mut request: Request) -> Result<Response, String> {
        let mut port = self.setup_service.borrow_mut();
        port.next = port
            .next
            .checked_add(1)
            .ok_or("Setup request limit reached")?;
        request.id = port.next;
        request.validate()?;
        let response = if let Some(transport) = port.transport.as_mut() {
            transport.exchange(&request)?
        } else {
            #[cfg(not(target_os = "none"))]
            {
                execute(&request)
            }
            #[cfg(target_os = "none")]
            {
                return Err("Protected Welder setup service is unavailable".into());
            }
        };
        response.validate_for(&request)?;
        Ok(response)
    }
}
