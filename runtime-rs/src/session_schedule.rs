//! Calendar-derived occurrence planning. An occurrence is not an execution grant.
use crate::data_pipeline::canonical_sha256;
use serde::{Deserialize, Serialize};
use tradeassembly_plugin_sdk::session_calendar::{SessionCalendar, SessionState};
use tradeassembly_strategy::spec::{MissedSessionPolicy, SessionAnchor, SessionTrigger};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionOccurrenceStatus {
    Scheduled,
    Closed,
    SkippedMissed,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SessionOccurrence {
    pub schema_version: String,
    /// Logical identity excludes mutable calendar revision to prevent duplicate entries.
    pub occurrence_id: String,
    pub strategy_id: String,
    pub strategy_version_id: String,
    pub strategy_spec_hash: String,
    pub trigger_id: String,
    pub session_date: String,
    pub trigger_policy_sha256: String,
    pub calendar_receipt_sha256: String,
    pub execution_calendar_receipt_sha256: String,
    pub calendar_revision: String,
    pub execution_calendar_revision: String,
    pub due_at_ms: Option<i64>,
    pub latest_start_at_ms: Option<i64>,
    pub execution_open_at_ms: Option<i64>,
    pub execution_close_at_ms: Option<i64>,
    pub calendar_known_at_ms: i64,
    pub status: SessionOccurrenceStatus,
}
fn digest(s: &str) -> bool {
    s.len() == 64
        && s.bytes()
            .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
}
fn timestamp_add(t: i64, offset: i64) -> Result<i64, String> {
    t.checked_add(offset)
        .filter(|v| *v > 0 && *v <= tradeassembly_plugin_sdk::canonical::MAX_EXACT_INTEGER as i64)
        .ok_or_else(|| "session_schedule_time_invalid".into())
}
/// Inputs must be constructed from current host-admitted collection receipts.
/// This planner never accepts caller-authored provider facts at a public boundary.
#[allow(clippy::too_many_arguments)]
pub fn plan_occurrence(
    strategy_id: &str,
    strategy_version_id: &str,
    strategy_spec_hash: &str,
    trigger: &SessionTrigger,
    session_date: &str,
    calendar: &SessionCalendar,
    calendar_receipt_sha256: &str,
    execution_calendar: &SessionCalendar,
    execution_calendar_receipt_sha256: &str,
    now_ms: i64,
) -> Result<SessionOccurrence, String> {
    trigger.validate()?;
    if now_ms <= 0
        || !digest(calendar_receipt_sha256)
        || !digest(execution_calendar_receipt_sha256)
        || [strategy_id, strategy_version_id, strategy_spec_hash]
            .iter()
            .any(|s| s.is_empty() || s.len() > 256)
    {
        return Err("session_schedule_identity_invalid".into());
    }
    for (document, binding) in [
        (calendar, &trigger.calendar),
        (execution_calendar, &trigger.execution_window.calendar),
    ] {
        document.validate()?;
        if document.calendar_ref != binding.calendar_ref
            || document.time_zone != binding.timezone
            || document.session_scope != binding.session_scope
        {
            return Err("session_schedule_calendar_mismatch".into());
        }
        if document.known_at_ms > now_ms {
            return Err("session_schedule_calendar_future".into());
        }
    }
    let day = calendar.day(session_date)?;
    let execution_day = execution_calendar.day(session_date)?;
    let identity = serde_json::json!({"strategyId":strategy_id,"strategyVersionId":strategy_version_id,"strategySpecHash":strategy_spec_hash,"triggerId":trigger.trigger_id,"sessionDate":session_date});
    let mut occurrence = SessionOccurrence {
        schema_version: "tradeassembly.session_occurrence.v1".into(),
        occurrence_id: format!(
            "occurrence_{}",
            canonical_sha256(&identity).map_err(|_| "session_schedule_encoding_invalid")?
        ),
        strategy_id: strategy_id.into(),
        strategy_version_id: strategy_version_id.into(),
        strategy_spec_hash: strategy_spec_hash.into(),
        trigger_id: trigger.trigger_id.clone(),
        session_date: session_date.into(),
        trigger_policy_sha256: canonical_sha256(trigger)
            .map_err(|_| "session_schedule_encoding_invalid")?,
        calendar_receipt_sha256: calendar_receipt_sha256.into(),
        execution_calendar_receipt_sha256: execution_calendar_receipt_sha256.into(),
        calendar_revision: calendar.revision.clone(),
        execution_calendar_revision: execution_calendar.revision.clone(),
        due_at_ms: None,
        latest_start_at_ms: None,
        execution_open_at_ms: None,
        execution_close_at_ms: None,
        calendar_known_at_ms: calendar.known_at_ms.max(execution_calendar.known_at_ms),
        status: SessionOccurrenceStatus::Closed,
    };
    let (
        SessionState::Open {
            open_at_ms,
            close_at_ms,
            ..
        },
        SessionState::Open {
            open_at_ms: execution_open,
            close_at_ms: execution_close,
            ..
        },
    ) = (&day.session, &execution_day.session)
    else {
        return Ok(occurrence);
    };
    let anchor = match trigger.anchor {
        SessionAnchor::Open => *open_at_ms,
        SessionAnchor::Close => *close_at_ms,
    };
    let due = timestamp_add(anchor, trigger.offset_ms)?;
    let execution_open = timestamp_add(
        *execution_open,
        trigger.execution_window.open_offset_ms as i64,
    )?;
    let execution_close = timestamp_add(
        *execution_close,
        -(trigger.execution_window.close_offset_ms as i64),
    )?;
    if execution_open >= execution_close {
        return Err("session_schedule_execution_window_empty".into());
    }
    let latest = match trigger.missed_run {
        MissedSessionPolicy::Skip => due,
        MissedSessionPolicy::CatchUp { max_lateness_ms } => {
            timestamp_add(due, max_lateness_ms as i64)?
        }
    };
    // Selection after the permissible execution session is never a useful entry occurrence.
    let latest = latest.min(execution_close - 1);
    occurrence.due_at_ms = Some(due);
    occurrence.latest_start_at_ms = Some(latest);
    occurrence.execution_open_at_ms = Some(execution_open);
    occurrence.execution_close_at_ms = Some(execution_close);
    occurrence.status = if now_ms > latest || due >= execution_close {
        SessionOccurrenceStatus::SkippedMissed
    } else {
        SessionOccurrenceStatus::Scheduled
    };
    Ok(occurrence)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    fn trigger() -> SessionTrigger {
        serde_json::from_value(json!({"version":"tradeassembly.session_trigger.v1","trigger_id":"preopen","calendar":{"capability_requirement_ref":"cal","calendar_ref":"XNYS","timezone":"America/New_York","session_scope":"regular_equity","max_age_ms":86400000},"anchor":"open","offset_ms":-300000,"missed_run":{"policy":"catch_up","max_lateness_ms":60000},"retry":{"max_attempts":3,"backoff_ms":1000},"execution_window":{"calendar":{"capability_requirement_ref":"cal","calendar_ref":"XNYS","timezone":"America/New_York","session_scope":"regular_equity","max_age_ms":86400000},"open_offset_ms":0,"close_offset_ms":0},"selections":[{"signal_requirement_ref":"primary","selector_ref":"static"}]})).unwrap()
    }
    fn ms(s: &str) -> i64 {
        chrono::DateTime::parse_from_rfc3339(s)
            .unwrap()
            .timestamp_millis()
    }
    fn document() -> SessionCalendar {
        serde_json::from_value(json!({"schemaVersion":"tradeassembly.session_calendar.v1","calendarRef":"XNYS","timeZone":"America/New_York","sessionScope":"regular_equity","revision":"fixture.v1","rangeStart":"2026-03-06","rangeEnd":"2026-03-09","knownAtMs":1,"days":[{"sessionDate":"2026-03-06","session":{"status":"open","openAtMs":ms("2026-03-06T14:30:00Z"),"closeAtMs":ms("2026-03-06T21:00:00Z"),"earlyClose":false}},{"sessionDate":"2026-03-07","session":{"status":"closed","reason":"fixture_weekend"}},{"sessionDate":"2026-03-08","session":{"status":"closed","reason":"fixture_weekend"}},{"sessionDate":"2026-03-09","session":{"status":"open","openAtMs":ms("2026-03-09T13:30:00Z"),"closeAtMs":ms("2026-03-09T20:00:00Z"),"earlyClose":false}}]})).unwrap()
    }
    fn plan(
        t: &SessionTrigger,
        c: &SessionCalendar,
        date: &str,
        now: i64,
    ) -> Result<SessionOccurrence, String> {
        plan_occurrence(
            "strategy",
            "version",
            "spec",
            t,
            date,
            c,
            &"a".repeat(64),
            c,
            &"b".repeat(64),
            now,
        )
    }
    #[test]
    fn provider_dst_anchors_closed_days_and_no_inferred_coverage() {
        let c = document();
        let t = trigger();
        let a = plan(&t, &c, "2026-03-06", 1).unwrap();
        let b = plan(&t, &c, "2026-03-09", 1).unwrap();
        assert_eq!(a.due_at_ms, Some(ms("2026-03-06T14:25:00Z")));
        assert_eq!(b.due_at_ms, Some(ms("2026-03-09T13:25:00Z")));
        assert_eq!(
            plan(&t, &c, "2026-03-08", 1).unwrap().status,
            SessionOccurrenceStatus::Closed
        );
        assert!(plan(&t, &c, "2026-03-10", 1).is_err());
    }
    #[test]
    fn late_boundary_and_revision_identity_do_not_duplicate_entries() {
        let mut c = document();
        let t = trigger();
        let due = ms("2026-03-06T14:25:00Z");
        let a = plan(&t, &c, "2026-03-06", due + 60000).unwrap();
        assert_eq!(a.status, SessionOccurrenceStatus::Scheduled);
        assert_eq!(
            plan(&t, &c, "2026-03-06", due + 60001).unwrap().status,
            SessionOccurrenceStatus::SkippedMissed
        );
        c.revision = "fixture.v2".into();
        let b = plan(&t, &c, "2026-03-06", 1).unwrap();
        assert_eq!(a.occurrence_id, b.occurrence_id);
        assert_ne!(a.calendar_revision, b.calendar_revision);
    }
    #[test]
    fn early_close_and_separate_execution_session_are_explicit() {
        let t = trigger();
        let mut c = document();
        c.days[0].session = SessionState::Open {
            open_at_ms: ms("2026-03-06T14:30:00Z"),
            close_at_ms: ms("2026-03-06T18:00:00Z"),
            early_close: true,
        };
        let p = plan(&t, &c, "2026-03-06", 1).unwrap();
        assert_eq!(p.execution_close_at_ms, Some(ms("2026-03-06T18:00:00Z")));
        let mut execution = c.clone();
        execution.session_scope = "regular_options".into();
        assert_eq!(
            plan_occurrence(
                "s",
                "v",
                "h",
                &t,
                "2026-03-06",
                &c,
                &"a".repeat(64),
                &execution,
                &"b".repeat(64),
                1
            )
            .unwrap_err(),
            "session_schedule_calendar_mismatch"
        );
        let mut t = t;
        t.execution_window.calendar.session_scope = "regular_options".into();
        assert!(plan_occurrence(
            "s",
            "v",
            "h",
            &t,
            "2026-03-06",
            &c,
            &"a".repeat(64),
            &execution,
            &"b".repeat(64),
            1
        )
        .is_ok());
    }
}
