//! Explicit primary-session coverage. UTC anchors are provider facts, never
//! inferred by the host from weekdays or an omitted holiday.
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

pub const SESSION_CALENDAR_SCHEMA: &str = "schema://tradeassembly/session-calendar@1";
pub const MAX_SESSION_DAYS: usize = 370;
pub const MAX_CALENDAR_BYTES: usize = 262_144;
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum SessionCalendarVersion {
    #[serde(rename = "tradeassembly.session_calendar.v1")]
    V1,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "status",
    rename_all = "snake_case",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum SessionState {
    Closed {
        reason: String,
    },
    Open {
        open_at_ms: i64,
        close_at_ms: i64,
        early_close: bool,
    },
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SessionDay {
    pub session_date: String,
    pub session: SessionState,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SessionCalendar {
    pub schema_version: SessionCalendarVersion,
    pub calendar_ref: String,
    pub time_zone: String,
    /// E.g. regular stock session versus a particular option product session.
    pub session_scope: String,
    pub revision: String,
    pub range_start: String,
    pub range_end: String,
    /// Actual provider knowledge time. Host observation remains separate.
    pub known_at_ms: i64,
    /// Exactly one explicit primary-session outcome for every covered date.
    pub days: Vec<SessionDay>,
}
fn label(s: &str) -> bool {
    !s.is_empty() && s.len() <= 256 && s.trim() == s && !s.chars().any(char::is_control)
}
/// Stable civil-day ordinal, including Gregorian leap-year validation.
pub fn date_ordinal(s: &str) -> Result<i64, &'static str> {
    let b = s.as_bytes();
    if b.len() != 10
        || b[4] != b'-'
        || b[7] != b'-'
        || b.iter()
            .enumerate()
            .any(|(i, c)| i != 4 && i != 7 && !c.is_ascii_digit())
    {
        return Err("session_calendar_date_invalid");
    }
    let y: i64 = s[..4]
        .parse()
        .map_err(|_| "session_calendar_date_invalid")?;
    let m: usize = s[5..7]
        .parse()
        .map_err(|_| "session_calendar_date_invalid")?;
    let d: i64 = s[8..]
        .parse()
        .map_err(|_| "session_calendar_date_invalid")?;
    let leap = y % 4 == 0 && (y % 100 != 0 || y % 400 == 0);
    let months = [
        31,
        if leap { 29 } else { 28 },
        31,
        30,
        31,
        30,
        31,
        31,
        30,
        31,
        30,
        31,
    ];
    if !(1..=9999).contains(&y) || !(1..=12).contains(&m) || d < 1 || d > months[m - 1] {
        return Err("session_calendar_date_invalid");
    }
    let prior = y - 1;
    Ok(prior * 365 + prior / 4 - prior / 100
        + prior / 400
        + months[..m - 1].iter().sum::<i64>()
        + d)
}
impl SessionCalendar {
    pub fn decode(mut value: serde_json::Value) -> Result<Self, &'static str> {
        if serde_json::to_vec(&value)
            .map_err(|_| "session_calendar_encoding_invalid")?
            .len()
            > MAX_CALENDAR_BYTES
        {
            return Err("session_calendar_too_large");
        }
        crate::canonical::normalize_numbers(&mut value)
            .map_err(|_| "session_calendar_number_invalid")?;
        let document: Self =
            serde_json::from_value(value).map_err(|_| "session_calendar_document_invalid")?;
        document.validate()?;
        Ok(document)
    }

    pub fn validate(&self) -> Result<(), &'static str> {
        if serde_json::to_vec(self)
            .map_err(|_| "session_calendar_encoding_invalid")?
            .len()
            > MAX_CALENDAR_BYTES
        {
            return Err("session_calendar_too_large");
        }
        if [
            &self.calendar_ref,
            &self.time_zone,
            &self.session_scope,
            &self.revision,
        ]
        .iter()
        .any(|s| !label(s))
            || self.known_at_ms <= 0
            || self.known_at_ms > crate::canonical::MAX_EXACT_INTEGER as i64
        {
            return Err("session_calendar_identity_invalid");
        }
        let start = date_ordinal(&self.range_start)?;
        let end = date_ordinal(&self.range_end)?;
        if end < start
            || end - start >= MAX_SESSION_DAYS as i64
            || self.days.len() as i64 != end - start + 1
        {
            return Err("session_calendar_coverage_incomplete");
        }
        let mut anchors = BTreeSet::new();
        let mut last_close = None;
        for (i, day) in self.days.iter().enumerate() {
            if date_ordinal(&day.session_date)? != start + i as i64 {
                return Err("session_calendar_coverage_incomplete");
            }
            match &day.session {
                SessionState::Closed { reason } if !label(reason) => {
                    return Err("session_calendar_closed_reason_invalid")
                }
                SessionState::Open {
                    open_at_ms,
                    close_at_ms,
                    ..
                } => {
                    if *open_at_ms <= 0
                        || close_at_ms <= open_at_ms
                        || close_at_ms - open_at_ms > 7 * 86_400_000
                        || *close_at_ms > crate::canonical::MAX_EXACT_INTEGER as i64
                        || !anchors.insert(*open_at_ms)
                        || last_close.is_some_and(|close| close > *open_at_ms)
                    {
                        return Err("session_calendar_window_invalid");
                    }
                    last_close = Some(*close_at_ms);
                }
                _ => {}
            }
        }
        Ok(())
    }
    pub fn day(&self, date: &str) -> Result<&SessionDay, &'static str> {
        self.validate()?;
        let index = date_ordinal(date)? - date_ordinal(&self.range_start)?;
        usize::try_from(index)
            .ok()
            .and_then(|i| self.days.get(i))
            .ok_or("session_calendar_date_uncovered")
    }
}
/// Exact schema bytes are separately hashed by package admission; semantic
/// coverage and ordered-session validation still run in `validate`.
pub fn schema() -> serde_json::Value {
    use serde_json::json;
    let label = json!({"type":"string","minLength":1,"maxLength":256});
    let date = json!({"type":"string","pattern":"^[0-9]{4}-[0-9]{2}-[0-9]{2}$"});
    let time = json!({"type":"integer","minimum":1,"maximum":crate::canonical::MAX_EXACT_INTEGER});
    json!({"type":"object","additionalProperties":false,"required":["schemaVersion","calendarRef","timeZone","sessionScope","revision","rangeStart","rangeEnd","knownAtMs","days"],"properties":{
      "schemaVersion":{"const":"tradeassembly.session_calendar.v1"},"calendarRef":label,"timeZone":label,"sessionScope":label,"revision":label,"rangeStart":date,"rangeEnd":date,"knownAtMs":time,
      "days":{"type":"array","minItems":1,"maxItems":MAX_SESSION_DAYS,"items":{"type":"object","additionalProperties":false,"required":["sessionDate","session"],"properties":{"sessionDate":date,"session":{"oneOf":[{"type":"object","additionalProperties":false,"required":["status","reason"],"properties":{"status":{"const":"closed"},"reason":label}},{"type":"object","additionalProperties":false,"required":["status","openAtMs","closeAtMs","earlyClose"],"properties":{"status":{"const":"open"},"openAtMs":time,"closeAtMs":time,"earlyClose":{"type":"boolean"}}}]}}}}
    }})
}

#[cfg(test)]
mod tests {
    use super::*;
    fn calendar() -> SessionCalendar {
        SessionCalendar {
            schema_version: SessionCalendarVersion::V1,
            calendar_ref: "XNYS".into(),
            time_zone: "America/New_York".into(),
            session_scope: "regular_equity".into(),
            revision: "fixture.v1".into(),
            range_start: "2024-02-28".into(),
            range_end: "2024-03-01".into(),
            known_at_ms: 1,
            days: vec![
                SessionDay {
                    session_date: "2024-02-28".into(),
                    session: SessionState::Closed {
                        reason: "explicit_fixture".into(),
                    },
                },
                SessionDay {
                    session_date: "2024-02-29".into(),
                    session: SessionState::Closed {
                        reason: "explicit_fixture".into(),
                    },
                },
                SessionDay {
                    session_date: "2024-03-01".into(),
                    session: SessionState::Open {
                        open_at_ms: 10,
                        close_at_ms: 20,
                        early_close: true,
                    },
                },
            ],
        }
    }
    #[test]
    fn integral_json_forms_load_without_lossy_identity() {
        let c = calendar();
        let mut value = serde_json::to_value(&c).unwrap();
        value["knownAtMs"] = serde_json::json!(1.0);
        assert_eq!(SessionCalendar::decode(value).unwrap(), c);
    }
    #[test]
    fn exact_date_coverage_and_leap_rules() {
        let c = calendar();
        assert!(c.validate().is_ok());
        assert!(date_ordinal("1900-02-29").is_err());
        assert!(date_ordinal("2000-02-29").is_ok());
        assert!(c.day("2024-03-02").is_err());
        let mut bad = c.clone();
        bad.days.remove(1);
        assert_eq!(bad.validate(), Err("session_calendar_coverage_incomplete"));
        let mut bad = c;
        bad.days.swap(0, 1);
        assert!(bad.validate().is_err());
    }
    #[test]
    fn closed_version_and_no_ambiguous_session_windows() {
        let c = calendar();
        let mut v = serde_json::to_value(&c).unwrap();
        v["schemaVersion"] = serde_json::json!("tradeassembly.session_calendar.v2");
        assert!(serde_json::from_value::<SessionCalendar>(v).is_err());
        let mut c = c;
        c.days[2].session = SessionState::Open {
            open_at_ms: 20,
            close_at_ms: 10,
            early_close: false,
        };
        assert!(c.validate().is_err());
    }
}
