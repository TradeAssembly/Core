//! Session-relative occurrences are distinct from permitted execution windows.
use super::*;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
pub const SESSION_CALENDAR_SCHEMA: &str = "schema://tradeassembly/session-calendar@1";
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub enum SessionTriggerVersion {
    #[serde(rename = "tradeassembly.session_trigger.v1")]
    V1,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SessionAnchor {
    Open,
    Close,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "policy", rename_all = "snake_case", deny_unknown_fields)]
pub enum MissedSessionPolicy {
    Skip,
    CatchUp { max_lateness_ms: u64 },
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SessionCalendarBinding {
    pub capability_requirement_ref: String,
    pub calendar_ref: String,
    pub timezone: String,
    pub session_scope: String,
    pub max_age_ms: u64,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SessionSelectionTarget {
    pub signal_requirement_ref: String,
    pub selector_ref: String,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SessionRetryPolicy {
    pub max_attempts: u32,
    pub backoff_ms: u64,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SessionExecutionWindow {
    pub calendar: SessionCalendarBinding,
    pub open_offset_ms: u64,
    pub close_offset_ms: u64,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SessionTrigger {
    pub version: SessionTriggerVersion,
    pub trigger_id: String,
    pub calendar: SessionCalendarBinding,
    pub anchor: SessionAnchor,
    pub offset_ms: i64,
    pub missed_run: MissedSessionPolicy,
    pub retry: SessionRetryPolicy,
    pub execution_window: SessionExecutionWindow,
    pub selections: Vec<SessionSelectionTarget>,
}
fn label(s: &str) -> bool {
    !s.is_empty() && s.len() <= 128 && s.trim() == s && !s.chars().any(char::is_control)
}
impl SessionTrigger {
    pub fn validate(&self) -> Result<(), &'static str> {
        if !label(&self.trigger_id)
            || self.offset_ms.unsigned_abs() > 86_400_000
            || !(1..=16).contains(&self.retry.max_attempts)
            || !(1..=3_600_000).contains(&self.retry.backoff_ms)
            || self.selections.is_empty()
            || self.selections.len() > 32
            || self.execution_window.open_offset_ms > 43_200_000
            || self.execution_window.close_offset_ms > 43_200_000
        {
            return Err("session_trigger_bounds_invalid");
        }
        if matches!(self.missed_run,MissedSessionPolicy::CatchUp{max_lateness_ms} if max_lateness_ms==0||max_lateness_ms>86_400_000)
        {
            return Err("session_trigger_missed_policy_invalid");
        }
        for c in [&self.calendar, &self.execution_window.calendar] {
            if [
                &c.capability_requirement_ref,
                &c.calendar_ref,
                &c.timezone,
                &c.session_scope,
            ]
            .iter()
            .any(|s| !label(s))
                || c.max_age_ms == 0
                || c.max_age_ms > 31 * 86_400_000
            {
                return Err("session_trigger_calendar_invalid");
            }
        }
        let mut targets = BTreeSet::new();
        for s in &self.selections {
            if !label(&s.signal_requirement_ref)
                || !label(&s.selector_ref)
                || !targets.insert((&s.signal_requirement_ref, &s.selector_ref))
            {
                return Err("session_trigger_target_invalid");
            }
        }
        Ok(())
    }
}
pub(super) fn validate_bindings(spec: &StrategySpec, diagnostics: &mut Vec<Diagnostic>) {
    let triggers = &spec.stages.market_clock.policy.session_triggers;
    let mut ids = BTreeSet::new();
    for (i, t) in triggers.iter().enumerate() {
        let valid = triggers.len() <= 16
            && t.validate().is_ok()
            && ids.insert(&t.trigger_id)
            && spec.stages.market_clock.enabled
            && t.selections.iter().all(|target| {
                spec.signal_market_requirements
                    .iter()
                    .filter(|m| m.requirement_id == target.signal_requirement_ref)
                    .flat_map(|m| &m.selectors)
                    .filter(|s| s.selector_id == target.selector_ref)
                    .count()
                    == 1
            })
            && [&t.calendar, &t.execution_window.calendar].iter().all(|c| {
                spec.capability_requirements.required.iter().any(|r| {
                    r.requirement_id == c.capability_requirement_ref
                        && r.fallback_policy == FallbackPolicy::None
                        && r.stage_refs.contains(&StageName::MarketClock)
                        && r.constraints
                            .schema_refs
                            .iter()
                            .any(|s| s == SESSION_CALENDAR_SCHEMA)
                })
            });
        if !valid {
            diagnostics.push(Diagnostic::error(3,"SPEC_SESSION_TRIGGER_INVALID",format!("/stages/market_clock/policy/session_triggers/{i}"),"session trigger requires bounded explicit policies, existing selector targets, and mandatory exact calendar capabilities"));
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    fn trigger() -> serde_json::Value {
        json!({"version":"tradeassembly.session_trigger.v1","trigger_id":"preopen","calendar":{"capability_requirement_ref":"calendar","calendar_ref":"XNYS","timezone":"America/New_York","session_scope":"regular_equity","max_age_ms":86400000},"anchor":"open","offset_ms":-300000,"missed_run":{"policy":"catch_up","max_lateness_ms":60000},"retry":{"max_attempts":3,"backoff_ms":1000},"execution_window":{"calendar":{"capability_requirement_ref":"calendar","calendar_ref":"XNYS","timezone":"America/New_York","session_scope":"regular_equity","max_age_ms":86400000},"open_offset_ms":0,"close_offset_ms":0},"selections":[{"signal_requirement_ref":"signal_primary","selector_ref":"primary"}]})
    }
    #[test]
    fn explicit_closed_version_and_bounded_policies() {
        let raw = trigger();
        let t: SessionTrigger = serde_json::from_value(raw.clone()).unwrap();
        assert!(t.validate().is_ok());
        for key in ["execution_window", "missed_run", "retry"] {
            let mut v = raw.clone();
            v.as_object_mut().unwrap().remove(key);
            assert!(serde_json::from_value::<SessionTrigger>(v).is_err());
        }
        let mut t = t;
        t.retry.max_attempts = 0;
        assert!(t.validate().is_err());
    }
    #[test]
    fn static_wire_stays_unchanged_and_targets_are_bound() {
        let mut v = crate::spec::strategy_template_spec_payload("blank");
        assert!(v["stages"]["market_clock"]["policy"]
            .get("session_triggers")
            .is_none());
        let selector = v["signal_market_requirements"][0]["selectors"][0]["selector_id"].clone();
        let mut t = trigger();
        t["selections"][0]["selector_ref"] = selector;
        v["stages"]["market_clock"]["policy"]["session_triggers"] = json!([t]);
        assert!(!crate::spec::validate(&v).valid);
        let mut cap = v["capability_requirements"]["required"][0].clone();
        cap["requirement_id"] = json!("calendar");
        cap["capability"] = json!("test.calendar.read@1");
        cap["substep_refs"] = json!([]);
        cap["stage_refs"] = json!(["market_clock"]);
        cap["constraints"]["schema_refs"] = json!([SESSION_CALENDAR_SCHEMA]);
        v["capability_requirements"]["required"]
            .as_array_mut()
            .unwrap()
            .push(cap);
        let report = crate::spec::validate(&v);
        assert!(report.valid, "{:?}", report.diagnostics);
        assert!(crate::strategy_kernel::CompiledStrategy::compile_json(&v).is_err());
    }
}
