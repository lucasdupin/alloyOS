//! Pure, bounded IANA time-zone conversion for no_std JavaScript apps.
//! Embedded transition data covers UTC instants in 1970..=2100. Unsupported zones,
//! invalid preferences, and dates outside that range produce an explicit error.
use alloc::{format, string::{String}};
use crate::value::{JsValue, json_parse, json_stringify};
use alloc::collections::BTreeMap;

mod data { include!("../data/clock_zones.rs"); }

pub const SUPPORTED_TIMEZONES: &[&str] = &[
    "UTC", "America/Los_Angeles", "America/Denver", "America/Chicago",
    "America/New_York", "America/Sao_Paulo", "Europe/London", "Europe/Paris",
    "Asia/Shanghai", "Asia/Tokyo", "Asia/Kolkata", "Australia/Sydney",
];

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ClockSettings {
    pub timezone: String,
    pub clock_format: String,
}
impl Default for ClockSettings {
    fn default() -> Self { Self { timezone: "UTC".into(), clock_format: "24h".into() } }
}
pub fn timezone_supported(zone: &str) -> bool { SUPPORTED_TIMEZONES.contains(&zone) }
impl ClockSettings {
    pub fn parse(text: &str) -> Result<Self, String> {
        let value = json_parse(text).map_err(|_| String::from("Invalid clock preferences"))?;
        let settings = Self {
            timezone: match value.get_prop("timezone") { Some(JsValue::String(v)) => v.clone(), _ => return Err("Missing timezone".into()) },
            clock_format: match value.get_prop("clock_format") { Some(JsValue::String(v)) => v.clone(), _ => return Err("Missing clock_format".into()) },
        };
        settings.validate()?;
        Ok(settings)
    }
    pub fn validate(&self) -> Result<(), String> {
        if !timezone_supported(&self.timezone) { return Err("Unsupported time zone".into()); }
        if self.clock_format != "12h" && self.clock_format != "24h" { return Err("Clock format must be 12h or 24h".into()); }
        Ok(())
    }
}

#[derive(Debug)]
pub struct ClockDisplay {
    pub timezone: String,
    pub clock_format: String,
    pub local_time: String,
    pub local_date: String,
    pub utc_offset_seconds: Option<i32>,
    pub error: Option<String>,
}
impl ClockDisplay {
    pub fn from_settings(settings: Result<ClockSettings, String>, unix_ms: Option<u64>) -> Self {
        let mut out = Self { timezone: String::new(), clock_format: String::new(), local_time: "Unavailable".into(), local_date: String::new(), utc_offset_seconds: None, error: None };
        let settings = match settings { Ok(s) => s, Err(e) => { out.error = Some(e); return out; } };
        out.timezone = settings.timezone.clone();
        out.clock_format = settings.clock_format.clone();
        if let Err(e) = settings.validate() { out.error = Some(e); return out; }
        let Some(ms) = unix_ms else { out.error = Some("Wall clock unavailable".into()); return out; };
        let seconds = ms / 1000;
        if seconds >= 4_133_980_800 { out.error = Some("Time-zone data supports years 1970 through 2100".into()); return out; }
        let transitions = data::ZONES.iter().find(|(name, _)| *name == settings.timezone).unwrap().1;
        let offset = transitions.iter().rev().find(|(at, _)| *at <= seconds as i64).unwrap().1;
        let local_seconds = seconds as i64 + i64::from(offset);
        let day = local_seconds.div_euclid(86400);
        let time = local_seconds.rem_euclid(86400);
        let hour = time / 3600;
        let minute = time % 3600 / 60;
        let second = time % 60;
        out.local_time = if settings.clock_format == "12h" {
            format!("{}:{:02}:{:02} {}", if hour % 12 == 0 { 12 } else { hour % 12 }, minute, second, if hour < 12 { "AM" } else { "PM" })
        } else { format!("{:02}:{:02}:{:02}", hour, minute, second) };
        let (year, month, day) = civil_from_days(day);
        out.local_date = format!("{:04}-{:02}-{:02}", year, month, day);
        out.utc_offset_seconds = Some(offset);
        out
    }
    pub fn to_value(&self) -> JsValue {
        let mut map = BTreeMap::new();
        map.insert("timezone".into(), JsValue::String(self.timezone.clone()));
        map.insert("clock_format".into(), JsValue::String(self.clock_format.clone()));
        map.insert("local_time".into(), JsValue::String(self.local_time.clone()));
        map.insert("local_date".into(), JsValue::String(self.local_date.clone()));
        map.insert("utc_offset_seconds".into(), self.utc_offset_seconds.map(|v|JsValue::Number(v as i64)).unwrap_or(JsValue::Null));
        map.insert("error".into(), self.error.as_ref().map(|v|JsValue::String(v.clone())).unwrap_or(JsValue::Null));
        JsValue::Object(map)
    }
    pub fn to_json(&self) -> String { json_stringify(&self.to_value()) }
}

// Gregorian civil date from days since Unix epoch (400-year era decomposition).
fn civil_from_days(days: i64) -> (i64, i64, i64) {
    let z = days + 719468;
    let era = z.div_euclid(146097);
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let mut year = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = mp + if mp < 10 { 3 } else { -9 };
    if month <= 2 { year += 1; }
    (year, month, day)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn display(zone: &str, format: &str, epoch: u64) -> ClockDisplay {
        ClockDisplay::from_settings(Ok(ClockSettings { timezone: zone.into(), clock_format: format.into() }), Some(epoch * 1000))
    }
    #[test]
    fn local_rollover_and_clock_formats() {
        let d = display("America/Los_Angeles", "12h", 0);
        assert_eq!(d.local_date, "1969-12-31"); assert_eq!(d.local_time, "4:00:00 PM");
        let d = display("Asia/Kolkata", "24h", 0);
        assert_eq!(d.local_date, "1970-01-01"); assert_eq!(d.local_time, "05:30:00");
        assert_eq!(display("UTC", "12h", 0).local_time, "12:00:00 AM");
        assert_eq!(display("UTC", "12h", 43200).local_time, "12:00:00 PM");
        let d = display("Asia/Tokyo", "24h", 1704067199);
        assert_eq!(d.local_date, "2024-01-01"); assert_eq!(d.local_time, "08:59:59");
    }
    #[test]
    fn dst_boundaries_and_abolished_dst() {
        // US spring-forward 2024-03-10 10:00 UTC; fall-back 2024-11-03 09:00 UTC.
        assert_eq!(display("America/Los_Angeles", "24h", 1710064799).local_time, "01:59:59");
        assert_eq!(display("America/Los_Angeles", "24h", 1710064800).local_time, "03:00:00");
        assert_eq!(display("America/Los_Angeles", "24h", 1730624399).local_time, "01:59:59");
        assert_eq!(display("America/Los_Angeles", "24h", 1730624400).local_time, "01:00:00");
        assert_eq!(display("America/Sao_Paulo", "24h", 1546300800).utc_offset_seconds, Some(-7200));
        assert_eq!(display("America/Sao_Paulo", "24h", 1704067200).utc_offset_seconds, Some(-10800));
        // Sydney's southern-hemisphere DST applies in January, not July.
        assert_eq!(display("Australia/Sydney", "24h", 1704067200).utc_offset_seconds, Some(39600));
        assert_eq!(display("Australia/Sydney", "24h", 1719792000).utc_offset_seconds, Some(36000));
    }
    #[test]
    fn invalid_and_unavailable_states_are_explicit() {
        assert!(ClockSettings::parse(r#"{"timezone":"Mars","clock_format":"24h"}"#).is_err());
        assert!(ClockSettings::parse(r#"{"timezone":"UTC","clock_format":"25h"}"#).is_err());
        assert!(display("UTC", "24h", 4133980800).error.is_some());
        assert!(ClockDisplay::from_settings(Ok(ClockSettings::default()), None).error.is_some());
        assert!(ClockDisplay::from_settings(Err("bad settings".into()), Some(0)).error.is_some());
    }
}
