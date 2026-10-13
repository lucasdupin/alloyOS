use alloc::collections::{BTreeMap, BTreeSet};
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

use crate::host::{CapabilityScopedHost, AlloyOsHost, OsEvent, ScriptCapabilities};
use crate::runtime::{JsIsolate, DEFAULT_MAX_STEPS};
use crate::value::{JsError, JsValue};

/// Standard configuration path in CobaltFS/VFS for persisted schedules.
pub const SCHEDULES_CONF_PATH: &str = "/etc/schedules.conf";

/// Calendar timestamp passed to `ScriptScheduler::tick_time` for 5-field cron evaluation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CalendarTime {
    pub year: u32,
    pub month: u8,       // 1..=12
    pub day: u8,         // 1..=31
    pub hour: u8,        // 0..=23
    pub minute: u8,      // 0..=59
    pub day_of_week: u8, // 0..=6 (0 = Sunday, or 7 = Sunday)
}

impl CalendarTime {
    pub const fn new(
        year: u32,
        month: u8,
        day: u8,
        hour: u8,
        minute: u8,
        day_of_week: u8,
    ) -> Self {
        Self {
            year,
            month,
            day,
            hour,
            minute,
            day_of_week: if day_of_week == 7 { 0 } else { day_of_week },
        }
    }
}

/// Error returned when parsing an invalid 5-field cron expression.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CronParseError(pub String);

/// A single parsed cron field containing its set of matching values.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CronField {
    allowed: BTreeSet<u8>,
}

impl CronField {
    fn parse(part: &str, min: u8, max: u8, is_dow: bool) -> Result<Self, CronParseError> {
        let mut allowed = BTreeSet::new();
        for item in part.split(',') {
            let item = item.trim();
            if item.is_empty() {
                return Err(CronParseError("Empty cron list item".to_string()));
            }

            let (range_part, step) = if let Some((r, s)) = item.split_once('/') {
                let step_val = s.parse::<u8>().map_err(|_| {
                    CronParseError(format!("Invalid step value '{}' in '{}'", s, item))
                })?;
                if step_val == 0 {
                    return Err(CronParseError("Cron step cannot be 0".to_string()));
                }
                (r, Some(step_val))
            } else {
                (item, None)
            };

            let (start, end) = if range_part == "*" {
                (min, max)
            } else if let Some((a, b)) = range_part.split_once('-') {
                let mut start_val = a.parse::<u8>().map_err(|_| {
                    CronParseError(format!("Invalid range start '{}' in '{}'", a, item))
                })?;
                let mut end_val = b.parse::<u8>().map_err(|_| {
                    CronParseError(format!("Invalid range end '{}' in '{}'", b, item))
                })?;
                if is_dow && start_val == 7 {
                    start_val = 0;
                }
                if is_dow && end_val == 7 && start_val == 0 {
                    end_val = 6;
                }
                if start_val < min || end_val > max || start_val > end_val {
                    return Err(CronParseError(format!(
                        "Cron range {}-{} out of bounds ({}..={})",
                        start_val, end_val, min, max
                    )));
                }
                (start_val, end_val)
            } else {
                if step.is_some() {
                    return Err(CronParseError(format!(
                        "Step '/' requires '*' or range in '{}'",
                        item
                    )));
                }
                let mut exact = range_part.parse::<u8>().map_err(|_| {
                    CronParseError(format!("Invalid number '{}' in cron field", range_part))
                })?;
                if is_dow && exact == 7 {
                    exact = 0;
                }
                if exact < min || exact > max {
                    return Err(CronParseError(format!(
                        "Cron value {} out of bounds ({}..={})",
                        exact, min, max
                    )));
                }
                (exact, exact)
            };

            let step_by = step.unwrap_or(1);
            let mut v = start;
            loop {
                let norm = if is_dow && v == 7 { 0 } else { v };
                allowed.insert(norm);
                match v.checked_add(step_by) {
                    Some(next) if next <= end => v = next,
                    _ => break,
                }
            }
        }

        if allowed.is_empty() {
            return Err(CronParseError("Cron field matched no values".to_string()));
        }
        Ok(Self { allowed })
    }

    pub fn contains(&self, val: u8) -> bool {
        self.allowed.contains(&val)
    }
}

/// A parsed 5-field cron expression (`minute hour day_of_month month day_of_week`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CronExpr {
    raw: String,
    pub minute: CronField,
    pub hour: CronField,
    pub day_of_month: CronField,
    pub month: CronField,
    pub day_of_week: CronField,
}

impl CronExpr {
    /// Parses a 5-field cron string (`minute hour day_of_month month day_of_week`).
    pub fn parse(expr: &str) -> Result<Self, CronParseError> {
        let fields: Vec<&str> = expr.split_whitespace().collect();
        if fields.len() != 5 {
            return Err(CronParseError(format!(
                "Expected 5 cron fields, found {} in '{}'",
                fields.len(),
                expr
            )));
        }

        let minute = CronField::parse(fields[0], 0, 59, false)?;
        let hour = CronField::parse(fields[1], 0, 23, false)?;
        let day_of_month = CronField::parse(fields[2], 1, 31, false)?;
        let month = CronField::parse(fields[3], 1, 12, false)?;
        let day_of_week = CronField::parse(fields[4], 0, 7, true)?;

        Ok(Self {
            raw: fields.join(" "),
            minute,
            hour,
            day_of_month,
            month,
            day_of_week,
        })
    }

    /// Returns the normalized 5-field cron expression string.
    pub fn as_str(&self) -> &str {
        &self.raw
    }

    /// Checks whether the given calendar components match this 5-field cron expression.
    pub fn matches(
        &self,
        _year: u32,
        month: u8,
        day: u8,
        hour: u8,
        minute: u8,
        day_of_week: u8,
    ) -> bool {
        let dow = if day_of_week == 7 { 0 } else { day_of_week };
        self.minute.contains(minute)
            && self.hour.contains(hour)
            && self.day_of_month.contains(day)
            && self.month.contains(month)
            && self.day_of_week.contains(dow)
    }

    /// Convenience helper matching a `CalendarTime`.
    pub fn matches_calendar(&self, cal: &CalendarTime) -> bool {
        self.matches(
            cal.year,
            cal.month,
            cal.day,
            cal.hour,
            cal.minute,
            cal.day_of_week,
        )
    }
}

/// Pattern for subscribing scripts to OS events (`OsEvent`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EventPattern {
    /// Matches `OsEvent::FsCreated { path }` using exact, prefix (`/inbox/*`), or suffix (`*.json`) globbing.
    FsCreated(String),
    /// Matches `OsEvent::FsModified { path }` using glob pattern.
    FsModified(String),
    /// Matches `OsEvent::FsDeleted { path }` using glob pattern.
    FsDeleted(String),
    /// Matches `OsEvent::NetInterfaceUp`.
    NetInterfaceUp,
    /// Matches `OsEvent::NetPacketReceived` on a specific port (or any port if `None`).
    NetPacketReceived { port: Option<u16> },
    /// Matches `OsEvent::SystemBoot`.
    SystemBoot,
    /// Matches `OsEvent::LowMemory` (optionally only when `free_bytes <= threshold_bytes`).
    LowMemory { threshold_bytes: Option<u64> },
    /// Matches `OsEvent::UserIdle`.
    UserIdle,
}

impl EventPattern {
    /// Returns `true` if `event` matches this `EventPattern`.
    pub fn matches(&self, event: &OsEvent) -> bool {
        match (self, event) {
            (EventPattern::FsCreated(pat), OsEvent::FsCreated { path }) => fs_path_match(pat, path),
            (EventPattern::FsModified(pat), OsEvent::FsModified { path }) => {
                fs_path_match(pat, path)
            }
            (EventPattern::FsDeleted(pat), OsEvent::FsDeleted { path }) => fs_path_match(pat, path),
            (EventPattern::NetInterfaceUp, OsEvent::NetInterfaceUp) => true,
            (
                EventPattern::NetPacketReceived { port: pat_port },
                OsEvent::NetPacketReceived { port, .. },
            ) => pat_port.map(|p| p == *port).unwrap_or(true),
            (EventPattern::SystemBoot, OsEvent::SystemBoot) => true,
            (
                EventPattern::LowMemory { threshold_bytes },
                OsEvent::LowMemory { free_bytes },
            ) => threshold_bytes
                .map(|max_free| *free_bytes <= max_free)
                .unwrap_or(true),
            (EventPattern::UserIdle, OsEvent::UserIdle) => true,
            _ => false,
        }
    }

    pub fn serialize(&self) -> String {
        match self {
            EventPattern::FsCreated(p) => format!("FsCreated:{}", p),
            EventPattern::FsModified(p) => format!("FsModified:{}", p),
            EventPattern::FsDeleted(p) => format!("FsDeleted:{}", p),
            EventPattern::NetInterfaceUp => "NetInterfaceUp".to_string(),
            EventPattern::NetPacketReceived { port: Some(p) } => {
                format!("NetPacketReceived:{}", p)
            }
            EventPattern::NetPacketReceived { port: None } => "NetPacketReceived:*".to_string(),
            EventPattern::SystemBoot => "SystemBoot".to_string(),
            EventPattern::LowMemory {
                threshold_bytes: Some(b),
            } => format!("LowMemory:{}", b),
            EventPattern::LowMemory {
                threshold_bytes: None,
            } => "LowMemory:*".to_string(),
            EventPattern::UserIdle => "UserIdle".to_string(),
        }
    }

    pub fn deserialize(s: &str) -> Result<Self, String> {
        let s = s.trim();
        match s {
            "NetInterfaceUp" | "NetworkOnline" | "net:online" => {
                return Ok(EventPattern::NetInterfaceUp)
            }
            "SystemBoot" | "boot" => return Ok(EventPattern::SystemBoot),
            "UserIdle" | "idle" => return Ok(EventPattern::UserIdle),
            "NetPacketReceived" | "NetworkRequest" | "net:request" => {
                return Ok(EventPattern::NetPacketReceived { port: None })
            }
            "LowMemory" => {
                return Ok(EventPattern::LowMemory {
                    threshold_bytes: None,
                })
            }
            _ => {}
        }
        if let Some(rest) = s
            .strip_prefix("fs:create:")
            .or_else(|| s.strip_prefix("fs:created:"))
        {
            return Ok(EventPattern::FsCreated(rest.to_string()));
        }
        if let Some(rest) = s
            .strip_prefix("fs:modify:")
            .or_else(|| s.strip_prefix("fs:modified:"))
        {
            return Ok(EventPattern::FsModified(rest.to_string()));
        }
        if let Some(rest) = s
            .strip_prefix("fs:delete:")
            .or_else(|| s.strip_prefix("fs:deleted:"))
        {
            return Ok(EventPattern::FsDeleted(rest.to_string()));
        }
        if let Some(rest) = s.strip_prefix("net:request:") {
            if rest == "*" {
                return Ok(EventPattern::NetPacketReceived { port: None });
            }
            let p = rest
                .parse::<u16>()
                .map_err(|_| format!("Invalid port in EventPattern '{}'", s))?;
            return Ok(EventPattern::NetPacketReceived { port: Some(p) });
        }
        if let Some((kind, arg)) = s.split_once(':') {
            match kind {
                "FsCreated" | "FileCreated" => Ok(EventPattern::FsCreated(arg.to_string())),
                "FsModified" | "FileModified" => Ok(EventPattern::FsModified(arg.to_string())),
                "FsDeleted" | "FileDeleted" => Ok(EventPattern::FsDeleted(arg.to_string())),
                "NetPacketReceived" | "NetworkRequest" => {
                    if arg == "*" {
                        Ok(EventPattern::NetPacketReceived { port: None })
                    } else {
                        let p = arg
                            .parse::<u16>()
                            .map_err(|_| format!("Invalid port in EventPattern '{}'", s))?;
                        Ok(EventPattern::NetPacketReceived { port: Some(p) })
                    }
                }
                "LowMemory" => {
                    if arg == "*" {
                        Ok(EventPattern::LowMemory {
                            threshold_bytes: None,
                        })
                    } else {
                        let b = arg.parse::<u64>().map_err(|_| {
                            format!("Invalid byte threshold in EventPattern '{}'", s)
                        })?;
                        Ok(EventPattern::LowMemory {
                            threshold_bytes: Some(b),
                        })
                    }
                }
                _ => Err(format!("Unknown EventPattern kind '{}'", kind)),
            }
        } else {
            Err(format!("Invalid EventPattern string '{}'", s))
        }
    }
}

fn fs_path_match(pattern: &str, text: &str) -> bool {
    if glob_match(pattern, text) {
        return true;
    }
    if !pattern.contains('*') {
        let dir_prefix = pattern.trim_end_matches('/');
        if !dir_prefix.is_empty() {
            let mut with_slash = String::from(dir_prefix);
            with_slash.push('/');
            if text.starts_with(&with_slash) {
                return true;
            }
        }
    }
    false
}

/// Glob matching supporting `*`, prefix `/inbox/*`, suffix `*.json`, middle `/a/*/b`, and exact paths.
pub fn glob_match(pattern: &str, text: &str) -> bool {
    if pattern == "*" {
        return true;
    }
    if !pattern.contains('*') {
        return pattern == text;
    }
    let parts: Vec<&str> = pattern.split('*').collect();
    if parts.len() == 2 {
        let prefix = parts[0];
        let suffix = parts[1];
        return text.starts_with(prefix)
            && text.ends_with(suffix)
            && text.len() >= prefix.len() + suffix.len();
    }

    // General multi-wildcard matching
    let mut pos = 0usize;
    for (i, part) in parts.iter().enumerate() {
        if part.is_empty() {
            continue;
        }
        if i == 0 {
            if !text.starts_with(part) {
                return false;
            }
            pos = part.len();
        } else if i == parts.len() - 1 {
            if !text[pos..].ends_with(part) {
                return false;
            }
            pos = text.len();
        } else if let Some(found) = text[pos..].find(part) {
            pos += found + part.len();
        } else {
            return false;
        }
    }
    true
}

fn parse_duration_ms(raw: &str) -> Result<u64, String> {
    let trimmed = raw.trim();
    if let Some(num) = trimmed.strip_suffix("ms") {
        return num
            .trim()
            .parse::<u64>()
            .map_err(|_| format!("Invalid ms duration '{}'", raw));
    }
    if let Some(num) = trimmed.strip_suffix('s') {
        let secs = num
            .trim()
            .parse::<u64>()
            .map_err(|_| format!("Invalid seconds duration '{}'", raw))?;
        return Ok(secs.saturating_mul(1_000));
    }
    if let Some(num) = trimmed.strip_suffix('m') {
        let mins = num
            .trim()
            .parse::<u64>()
            .map_err(|_| format!("Invalid minutes duration '{}'", raw))?;
        return Ok(mins.saturating_mul(60_000));
    }
    if let Some(num) = trimmed.strip_suffix('h') {
        let hours = num
            .trim()
            .parse::<u64>()
            .map_err(|_| format!("Invalid hours duration '{}'", raw))?;
        return Ok(hours.saturating_mul(3_600_000));
    }
    trimmed
        .parse::<u64>()
        .map_err(|_| format!("Invalid duration_ms '{}'", raw))
}

/// Time or event trigger for a scheduled JavaScript automation in `scriptd`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ScheduleTrigger {
    /// One-shot timer firing once when `now_ms >= target_ms`.
    OnceAtMs(u64),
    /// Recurring interval timer firing every `every_ms` milliseconds.
    IntervalMs { every_ms: u64 },
    /// Recurring 5-field cron schedule.
    Cron(CronExpr),
    /// Reactive OS event trigger.
    OnEvent(EventPattern),
}

impl ScheduleTrigger {
    /// Serializes this trigger into a single-column specification for `/etc/schedules.conf`.
    pub fn serialize(&self) -> String {
        match self {
            ScheduleTrigger::OnceAtMs(ms) => format!("once:{}", ms),
            ScheduleTrigger::IntervalMs { every_ms } => format!("interval:{}", every_ms),
            ScheduleTrigger::Cron(expr) => format!("cron:{}", expr.as_str()),
            ScheduleTrigger::OnEvent(pat) => format!("event:{}", pat.serialize()),
        }
    }

    /// Parses a trigger specification from `/etc/schedules.conf` or `script_schedule` tool calls.
    pub fn deserialize(spec: &str) -> Result<Self, String> {
        let trimmed = spec.trim();
        // Direct event shorthands without ':'
        if matches!(
            trimmed,
            "SystemBoot" | "boot" | "NetInterfaceUp" | "NetworkOnline" | "UserIdle"
        ) {
            return Ok(ScheduleTrigger::OnEvent(EventPattern::deserialize(trimmed)?));
        }
        // Shorthand prefixes like `fs:modify:/etc`, `fs:create:/inbox/*`, `net:online`, `net:request:8080`
        if trimmed.starts_with("fs:") || trimmed.starts_with("net:") {
            return Ok(ScheduleTrigger::OnEvent(EventPattern::deserialize(trimmed)?));
        }

        let (kind, rest) = trimmed
            .split_once(':')
            .ok_or_else(|| format!("Invalid trigger spec '{}'", spec))?;
        match kind {
            "once" => {
                let ms = parse_duration_ms(rest)?;
                Ok(ScheduleTrigger::OnceAtMs(ms))
            }
            "interval" | "every" => {
                let every_ms = parse_duration_ms(rest)?;
                Ok(ScheduleTrigger::IntervalMs { every_ms })
            }
            "cron" => {
                let expr = CronExpr::parse(rest).map_err(|e| e.0)?;
                Ok(ScheduleTrigger::Cron(expr))
            }
            "event" => {
                let pat = EventPattern::deserialize(rest)?;
                Ok(ScheduleTrigger::OnEvent(pat))
            }
            "FsCreated" | "FileCreated" | "FsModified" | "FileModified" | "FsDeleted"
            | "FileDeleted" | "NetPacketReceived" | "NetworkRequest" | "LowMemory" => {
                let pat = EventPattern::deserialize(trimmed)?;
                Ok(ScheduleTrigger::OnEvent(pat))
            }
            other => Err(format!("Unknown trigger type '{}'", other)),
        }
    }
}

/// Store for model-authored `.js` scripts keyed by virtual path (e.g. `/scripts/backup.js`).
#[derive(Debug, Clone, Default)]
pub struct ScriptStore {
    scripts: BTreeMap<String, String>,
    capabilities: BTreeMap<String, ScriptCapabilities>,
}

impl ScriptStore {
    /// Creates an empty `ScriptStore`.
    pub fn new() -> Self {
        Self {
            scripts: BTreeMap::new(),
            capabilities: BTreeMap::new(),
        }
    }

    /// Inserts or updates a `.js` script at `path`.
    pub fn put_script(&mut self, path: &str, source: &str) {
        self.scripts.insert(path.to_string(), source.to_string());
    }

    /// Sets capability constraints for a script at `path`.
    pub fn set_capabilities(&mut self, path: &str, caps: ScriptCapabilities) {
        self.capabilities.insert(path.to_string(), caps);
    }

    /// Gets custom capability constraints for `path`, if configured.
    pub fn get_capabilities(&self, path: &str) -> Option<&ScriptCapabilities> {
        self.capabilities.get(path)
    }

    /// Retrieves the `.js` source for `path`.
    pub fn get_script(&self, path: &str) -> Option<&str> {
        self.scripts.get(path).map(|s| s.as_str())
    }

    /// Removes a script from the store.
    pub fn remove_script(&mut self, path: &str) -> bool {
        self.capabilities.remove(path);
        self.scripts.remove(path).is_some()
    }

    /// Lists all script paths stored in `ScriptStore`.
    pub fn list_scripts(&self) -> Vec<String> {
        self.scripts.keys().cloned().collect()
    }
}

/// Report produced each time `ScriptScheduler` executes a scheduled script.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScriptExecutionReport {
    pub schedule_id: String,
    pub script_path: String,
    pub result: Result<JsValue, JsError>,
    pub steps_executed: usize,
}

/// A registered entry in `ScriptScheduler`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScheduledScript {
    pub id: String,
    pub script_path: String,
    pub trigger: ScheduleTrigger,
    pub once_fired: bool,
    pub next_interval_ms: Option<u64>,
    pub last_cron_stamp: Option<(u32, u8, u8, u8, u8)>,
}

/// `scriptd` Event & Time Scheduler managing model-authored JavaScript automations.
#[derive(Debug, Clone)]
pub struct ScriptScheduler {
    store: ScriptStore,
    schedules: Vec<ScheduledScript>,
    max_steps_per_script: usize,
}

impl Default for ScriptScheduler {
    fn default() -> Self {
        Self::new()
    }
}

impl ScriptScheduler {
    /// Creates a new `ScriptScheduler` (`scriptd`) with an empty `ScriptStore`.
    pub fn new() -> Self {
        Self {
            store: ScriptStore::new(),
            schedules: Vec::new(),
            max_steps_per_script: DEFAULT_MAX_STEPS,
        }
    }

    /// Creates a `ScriptScheduler` with a custom fuel limit per script execution.
    pub fn with_max_steps(max_steps_per_script: usize) -> Self {
        Self {
            store: ScriptStore::new(),
            schedules: Vec::new(),
            max_steps_per_script,
        }
    }

    /// Returns a reference to the underlying `ScriptStore`.
    pub fn store(&self) -> &ScriptStore {
        &self.store
    }

    /// Returns a mutable reference to the underlying `ScriptStore`.
    pub fn store_mut(&mut self) -> &mut ScriptStore {
        &mut self.store
    }

    /// Registers a script source in the `ScriptStore` at `script_path`.
    pub fn register_script_source(&mut self, script_path: &str, source: &str) {
        self.store.put_script(script_path, source);
    }

    /// Assigns capability constraints for a script at `script_path`.
    pub fn set_script_capabilities(&mut self, script_path: &str, caps: ScriptCapabilities) {
        self.store.set_capabilities(script_path, caps);
    }

    /// Returns capability constraints configured for `script_path`.
    pub fn get_script_capabilities(&self, script_path: &str) -> Option<&ScriptCapabilities> {
        self.store.get_capabilities(script_path)
    }

    /// Schedules a script by `id`, `script_path`, and `trigger`.
    /// If an entry with `id` already exists, it is replaced.
    pub fn schedule_script(&mut self, id: &str, script_path: &str, trigger: ScheduleTrigger) {
        self.unregister_script(id);
        self.schedules.push(ScheduledScript {
            id: id.to_string(),
            script_path: script_path.to_string(),
            trigger,
            once_fired: false,
            next_interval_ms: None,
            last_cron_stamp: None,
        });
    }

    /// Unregisters a scheduled script by `id`. Returns `true` if found and removed.
    pub fn unregister_script(&mut self, id: &str) -> bool {
        let before = self.schedules.len();
        self.schedules.retain(|s| s.id != id);
        self.schedules.len() < before
    }

    /// Returns all currently registered schedules.
    pub fn schedules(&self) -> &[ScheduledScript] {
        &self.schedules
    }

    /// Evaluates time-based triggers (`OnceAtMs`, `IntervalMs`, `Cron`) at `now_ms` and optional
    /// `calendar` wall-clock time, executing any due scripts in fresh `JsIsolate` instances.
    pub fn tick_time(
        &mut self,
        now_ms: u64,
        calendar: Option<CalendarTime>,
        host: &mut dyn AlloyOsHost,
    ) -> Vec<ScriptExecutionReport> {
        let mut due_runs: Vec<(String, String, JsValue)> = Vec::new();

        for entry in &mut self.schedules {
            match &entry.trigger {
                ScheduleTrigger::OnceAtMs(target_ms) => {
                    if !entry.once_fired && now_ms >= *target_ms {
                        entry.once_fired = true;
                        let mut ev = BTreeMap::new();
                        ev.insert("type".to_string(), JsValue::String("TimerOnce".to_string()));
                        ev.insert("nowMs".to_string(), JsValue::Number(now_ms as i64));
                        ev.insert(
                            "targetMs".to_string(),
                            JsValue::Number(*target_ms as i64),
                        );
                        due_runs.push((
                            entry.id.clone(),
                            entry.script_path.clone(),
                            JsValue::Object(ev),
                        ));
                    }
                }
                ScheduleTrigger::IntervalMs { every_ms } => {
                    let step = (*every_ms).max(1);
                    let should_fire = match entry.next_interval_ms {
                        None => {
                            entry.next_interval_ms = Some(now_ms.saturating_add(step));
                            now_ms >= step
                        }
                        Some(next_due) if now_ms >= next_due => {
                            // Advance next_interval_ms past now_ms
                            let mut next = next_due.saturating_add(step);
                            if next <= now_ms {
                                next = now_ms.saturating_add(step);
                            }
                            entry.next_interval_ms = Some(next);
                            true
                        }
                        _ => false,
                    };

                    if should_fire {
                        let mut ev = BTreeMap::new();
                        ev.insert(
                            "type".to_string(),
                            JsValue::String("TimerInterval".to_string()),
                        );
                        ev.insert("nowMs".to_string(), JsValue::Number(now_ms as i64));
                        ev.insert("everyMs".to_string(), JsValue::Number(*every_ms as i64));
                        due_runs.push((
                            entry.id.clone(),
                            entry.script_path.clone(),
                            JsValue::Object(ev),
                        ));
                    }
                }
                ScheduleTrigger::Cron(cron_expr) => {
                    if let Some(cal) = calendar {
                        let stamp = (cal.year, cal.month, cal.day, cal.hour, cal.minute);
                        if entry.last_cron_stamp != Some(stamp)
                            && cron_expr.matches_calendar(&cal)
                        {
                            entry.last_cron_stamp = Some(stamp);
                            let mut ev = BTreeMap::new();
                            ev.insert(
                                "type".to_string(),
                                JsValue::String("TimerCron".to_string()),
                            );
                            ev.insert("nowMs".to_string(), JsValue::Number(now_ms as i64));
                            ev.insert(
                                "cron".to_string(),
                                JsValue::String(cron_expr.as_str().to_string()),
                            );
                            ev.insert("year".to_string(), JsValue::Number(cal.year as i64));
                            ev.insert("month".to_string(), JsValue::Number(cal.month as i64));
                            ev.insert("day".to_string(), JsValue::Number(cal.day as i64));
                            ev.insert("hour".to_string(), JsValue::Number(cal.hour as i64));
                            ev.insert("minute".to_string(), JsValue::Number(cal.minute as i64));
                            due_runs.push((
                                entry.id.clone(),
                                entry.script_path.clone(),
                                JsValue::Object(ev),
                            ));
                        }
                    }
                }
                ScheduleTrigger::OnEvent(_) => {}
            }
        }

        let mut reports = Vec::with_capacity(due_runs.len());
        for (schedule_id, script_path, event_obj) in due_runs {
            reports.push(self.execute_script(&schedule_id, &script_path, event_obj, host));
        }
        reports
    }

    /// Reactively dispatches an `OsEvent` to all subscribed `OnEvent` scripts whose
    /// `EventPattern` matches `event`.
    pub fn dispatch_event(
        &mut self,
        event: &OsEvent,
        host: &mut dyn AlloyOsHost,
    ) -> Vec<ScriptExecutionReport> {
        let matching: Vec<(String, String)> = self
            .schedules
            .iter()
            .filter_map(|entry| match &entry.trigger {
                ScheduleTrigger::OnEvent(pat) if pat.matches(event) => {
                    Some((entry.id.clone(), entry.script_path.clone()))
                }
                _ => None,
            })
            .collect();

        let event_js = event.to_js_object();
        let mut reports = Vec::with_capacity(matching.len());
        for (schedule_id, script_path) in matching {
            reports.push(self.execute_script(
                &schedule_id,
                &script_path,
                event_js.clone(),
                host,
            ));
        }
        reports
    }

    /// Evaluates a script directly by `script_path` (resolving from `ScriptStore` or `host.fs_read`),
    /// returning `(result_value, steps_executed)`.
    pub fn eval_script_path(
        &self,
        script_path: &str,
        host: &mut dyn AlloyOsHost,
    ) -> Result<(JsValue, usize), JsError> {
        let source = self
            .store
            .get_script(script_path)
            .map(|s| s.to_string())
            .or_else(|| host.fs_read(script_path))
            .ok_or_else(|| JsError::HostError(format!("Script not found: {}", script_path)))?;

        let mut isolate = JsIsolate::with_max_steps(self.max_steps_per_script);
        let result = if let Some(caps) = self.store.get_capabilities(script_path) {
            let mut scoped = CapabilityScopedHost::new(host, caps.clone());
            isolate.eval_with_host(&source, &mut scoped)
        } else {
            isolate.eval_with_host(&source, host)
        };
        let steps = isolate.steps_executed();
        result.map(|val| (val, steps))
    }

    fn execute_script(
        &self,
        schedule_id: &str,
        script_path: &str,
        event_obj: JsValue,
        host: &mut dyn AlloyOsHost,
    ) -> ScriptExecutionReport {
        // Resolve script source from ScriptStore first, falling back to CobaltFS/VFS host.fs_read
        let source_opt = self
            .store
            .get_script(script_path)
            .map(|s| s.to_string())
            .or_else(|| host.fs_read(script_path));

        let Some(source) = source_opt else {
            return ScriptExecutionReport {
                schedule_id: schedule_id.to_string(),
                script_path: script_path.to_string(),
                result: Err(JsError::HostError(format!(
                    "Script not found: {}",
                    script_path
                ))),
                steps_executed: 0,
            };
        };

        let mut isolate = JsIsolate::with_max_steps(self.max_steps_per_script);
        isolate.set_global("EVENT", event_obj.clone());
        isolate.set_global("__event", event_obj);
        let result = if let Some(caps) = self.store.get_capabilities(script_path) {
            let mut scoped = CapabilityScopedHost::new(host, caps.clone());
            isolate.eval_with_host(&source, &mut scoped)
        } else {
            isolate.eval_with_host(&source, host)
        };
        let steps_executed = isolate.steps_executed();

        ScriptExecutionReport {
            schedule_id: schedule_id.to_string(),
            script_path: script_path.to_string(),
            result,
            steps_executed,
        }
    }

    /// Serializes all registered schedules into the `/etc/schedules.conf` text format.
    pub fn serialize_schedules_conf(&self) -> String {
        let mut out = String::from("# alloyOS /etc/schedules.conf\n");
        for entry in &self.schedules {
            out.push_str(&entry.id);
            out.push('|');
            out.push_str(&entry.script_path);
            out.push('|');
            out.push_str(&entry.trigger.serialize());
            out.push('\n');
        }
        out
    }

    /// Deserializes and registers schedules from `/etc/schedules.conf` content.
    pub fn load_schedules_conf_str(&mut self, conf: &str) -> Result<usize, String> {
        let mut count = 0usize;
        for (line_idx, raw_line) in conf.lines().enumerate() {
            let line = raw_line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            if let Some((path_part, trig_part)) = line.split_once("->") {
                let script_path = path_part.trim();
                let trigger = ScheduleTrigger::deserialize(trig_part.trim())?;
                let id = format!("sched-{}", count + 1);
                self.schedule_script(&id, script_path, trigger);
                count += 1;
                continue;
            }
            let parts: Vec<&str> = line.splitn(3, '|').collect();
            if parts.len() != 3 {
                return Err(format!(
                    "Line {}: expected 'id|script_path|trigger', got '{}'",
                    line_idx + 1,
                    line
                ));
            }
            let id = parts[0].trim();
            let script_path = parts[1].trim();
            let trigger = ScheduleTrigger::deserialize(parts[2].trim())?;
            self.schedule_script(id, script_path, trigger);
            count += 1;
        }
        Ok(count)
    }

    /// Persists the current schedule table to `/etc/schedules.conf` on `host`.
    pub fn save_to_host(&self, host: &mut dyn AlloyOsHost) -> Result<(), String> {
        let content = self.serialize_schedules_conf();
        host.fs_write(SCHEDULES_CONF_PATH, &content)
    }

    /// Loads `/scripts/*.js` and the schedule table from `/etc/schedules.conf` on `host`.
    pub fn load_from_host(&mut self, host: &mut dyn AlloyOsHost) -> Result<usize, String> {
        // Discover and cache any `/scripts/*.js` files present on the host
        for entry in host.fs_list("/scripts") {
            let clean = entry.trim_matches('/');
            if clean.is_empty() {
                continue;
            }
            let full_path = if entry.starts_with("/scripts/") {
                entry
            } else {
                format!("/scripts/{}", clean)
            };
            if full_path.ends_with(".js") {
                if let Some(source) = host.fs_read(&full_path) {
                    self.store.put_script(&full_path, &source);
                }
            }
        }

        let Some(content) = host.fs_read(SCHEDULES_CONF_PATH) else {
            return Ok(0);
        };
        let loaded = self.load_schedules_conf_str(&content)?;
        // Also ensure any scheduled script paths outside `/scripts` are loaded into `ScriptStore` if readable
        let scheduled_paths: Vec<String> = self
            .schedules
            .iter()
            .map(|s| s.script_path.clone())
            .collect();
        for path in scheduled_paths {
            if self.store.get_script(&path).is_none() {
                if let Some(source) = host.fs_read(&path) {
                    self.store.put_script(&path, &source);
                }
            }
        }
        Ok(loaded)
    }
}
