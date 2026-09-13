// Copyright (c) 2026 OptionLab LLC. All rights reserved.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};

pub const LIFECYCLE_CALENDAR_SCHEMA_VERSION: &str = "tradeassembly.lifecycle_calendar.event.v1";
pub const LIFECYCLE_CALENDAR_NOTICE: &str = "Descriptive lifecycle calendar evidence only. TradeAssembly did not recommend exercise, assignment avoidance, rolling, closing, sizing, or activation decisions.";

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LifecycleCalendarEventType {
    Expiration,
    LastTradeDate,
    ExerciseWindow,
    AssignmentWindow,
    ExDividendDate,
    EarningsDate,
    CorporateAction,
    MarginEvent,
    UserReminder,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LifecycleCalendarSourceKind {
    ProviderAdapter,
    BrokerAdapter,
    ExchangeCalendar,
    LocalDerived,
    UserReminder,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LifecycleCalendarWarningSeverity {
    Warning,
    Error,
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LifecycleCalendarWarningCode {
    MissingSourceData,
    StaleSourceData,
    AmbiguousIdentity,
    UnsupportedEventType,
    AccountSpecificUnknown,
    InvalidEventWindow,
    HostedIntegrationUnsupported,
    RawSecretRejected,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LifecycleCalendarSourceProvenance {
    pub source: String,
    pub source_ref: String,
    pub collected_at_unix_ms: u64,
    pub raw_secrets_exposed: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LifecycleCalendarSourceContract {
    pub source_id: String,
    pub source_kind: LifecycleCalendarSourceKind,
    pub adapter_ref: String,
    pub provider_ref: Option<String>,
    pub local_first: bool,
    pub hosted_integration: bool,
    pub account_specific: bool,
    #[serde(default)]
    pub supported_event_types: Vec<LifecycleCalendarEventType>,
    pub provenance: LifecycleCalendarSourceProvenance,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LifecycleCalendarAffectedRef {
    pub instrument_id: String,
    pub symbol: String,
    pub asset_class: String,
    pub position_id: Option<String>,
    pub strategy_id: Option<String>,
    pub account_scope_ref: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LifecycleCalendarWindow {
    pub starts_at_unix_ms: u64,
    pub ends_at_unix_ms: u64,
    pub timezone: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LifecycleCalendarReplayRef {
    pub replay_ref: String,
    pub deterministic: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LifecycleCalendarWarning {
    pub code: LifecycleCalendarWarningCode,
    pub severity: LifecycleCalendarWarningSeverity,
    pub message: String,
    pub replay_ref: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LifecycleCalendarEventContract {
    pub schema_version: String,
    pub event_id: String,
    pub event_type: LifecycleCalendarEventType,
    #[serde(default)]
    pub affected: Vec<LifecycleCalendarAffectedRef>,
    pub window: LifecycleCalendarWindow,
    pub as_of_unix_ms: u64,
    pub stale_after_unix_ms: Option<u64>,
    pub confidence: f64,
    pub source: LifecycleCalendarSourceContract,
    pub unsupported_reason: Option<String>,
    #[serde(default)]
    pub warnings: Vec<LifecycleCalendarWarning>,
    #[serde(default)]
    pub deep_links: BTreeMap<String, String>,
    #[serde(default)]
    pub replay_refs: Vec<LifecycleCalendarReplayRef>,
    #[serde(default)]
    pub metadata: BTreeMap<String, Value>,
    pub no_advice_notice: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LifecycleCalendarValidationReport {
    pub ok: bool,
    #[serde(default)]
    pub warnings: Vec<LifecycleCalendarWarning>,
}

impl LifecycleCalendarEventContract {
    pub fn validate(&self) -> LifecycleCalendarValidationReport {
        validate_lifecycle_calendar_event(self)
    }
}

pub fn validate_lifecycle_calendar_event(
    event: &LifecycleCalendarEventContract,
) -> LifecycleCalendarValidationReport {
    let mut warnings = event.warnings.clone();
    validate_schema_and_type(event, &mut warnings);
    validate_source(event, &mut warnings);
    validate_affected_refs(event, &mut warnings);
    validate_window(event, &mut warnings);
    validate_staleness_and_confidence(event, &mut warnings);
    validate_secret_surfaces(event, &mut warnings);
    let ok = warnings
        .iter()
        .all(|warning| warning.severity != LifecycleCalendarWarningSeverity::Error);
    LifecycleCalendarValidationReport { ok, warnings }
}

pub fn lifecycle_calendar_event_hash(event: &LifecycleCalendarEventContract) -> String {
    let bytes = serde_json::to_vec(event).expect("lifecycle calendar event serializes");
    format!("sha256:{:x}", Sha256::digest(bytes))
}

pub fn required_lifecycle_calendar_event_types() -> BTreeSet<LifecycleCalendarEventType> {
    BTreeSet::from([
        LifecycleCalendarEventType::Expiration,
        LifecycleCalendarEventType::LastTradeDate,
        LifecycleCalendarEventType::ExerciseWindow,
        LifecycleCalendarEventType::AssignmentWindow,
        LifecycleCalendarEventType::ExDividendDate,
        LifecycleCalendarEventType::EarningsDate,
        LifecycleCalendarEventType::CorporateAction,
        LifecycleCalendarEventType::MarginEvent,
        LifecycleCalendarEventType::UserReminder,
    ])
}

fn validate_schema_and_type(
    event: &LifecycleCalendarEventContract,
    warnings: &mut Vec<LifecycleCalendarWarning>,
) {
    if event.schema_version != LIFECYCLE_CALENDAR_SCHEMA_VERSION {
        warnings.push(warning(
            event,
            LifecycleCalendarWarningCode::UnsupportedEventType,
            LifecycleCalendarWarningSeverity::Error,
            "Lifecycle calendar event schema version is unsupported.",
        ));
    }
    if !event
        .source
        .supported_event_types
        .contains(&event.event_type)
    {
        warnings.push(warning(
            event,
            LifecycleCalendarWarningCode::UnsupportedEventType,
            LifecycleCalendarWarningSeverity::Error,
            "Lifecycle calendar source does not support this event type.",
        ));
    }
    if event
        .unsupported_reason
        .as_ref()
        .is_some_and(|value| !value.trim().is_empty())
    {
        warnings.push(warning(
            event,
            LifecycleCalendarWarningCode::UnsupportedEventType,
            LifecycleCalendarWarningSeverity::Error,
            "Lifecycle calendar event is explicitly marked unsupported.",
        ));
    }
}

fn validate_source(
    event: &LifecycleCalendarEventContract,
    warnings: &mut Vec<LifecycleCalendarWarning>,
) {
    if event.source.source_id.trim().is_empty()
        || event.source.adapter_ref.trim().is_empty()
        || event.source.provenance.source_ref.trim().is_empty()
    {
        warnings.push(warning(
            event,
            LifecycleCalendarWarningCode::MissingSourceData,
            LifecycleCalendarWarningSeverity::Error,
            "Lifecycle calendar event source is missing adapter or provenance data.",
        ));
    }
    if event.source.hosted_integration && !event.source.local_first {
        warnings.push(warning(
            event,
            LifecycleCalendarWarningCode::HostedIntegrationUnsupported,
            LifecycleCalendarWarningSeverity::Error,
            "Hosted calendar integrations are outside public-core v0 unless exposed through a public-safe local adapter.",
        ));
    }
}

fn validate_affected_refs(
    event: &LifecycleCalendarEventContract,
    warnings: &mut Vec<LifecycleCalendarWarning>,
) {
    if event.affected.is_empty()
        || event.affected.iter().any(|affected| {
            affected.instrument_id.trim().is_empty()
                || affected.instrument_id.starts_with("ambiguous:")
                || affected.instrument_id.starts_with("unsupported:")
        })
    {
        warnings.push(warning(
            event,
            LifecycleCalendarWarningCode::AmbiguousIdentity,
            LifecycleCalendarWarningSeverity::Error,
            "Lifecycle calendar event does not identify supported canonical instruments or positions.",
        ));
    }
    if matches!(
        event.event_type,
        LifecycleCalendarEventType::AssignmentWindow
            | LifecycleCalendarEventType::ExerciseWindow
            | LifecycleCalendarEventType::MarginEvent
    ) && event
        .affected
        .iter()
        .all(|affected| affected.account_scope_ref.is_none())
    {
        warnings.push(warning(
            event,
            LifecycleCalendarWarningCode::AccountSpecificUnknown,
            LifecycleCalendarWarningSeverity::Error,
            "Account-specific lifecycle state is unknown for this calendar event.",
        ));
    }
}

fn validate_window(
    event: &LifecycleCalendarEventContract,
    warnings: &mut Vec<LifecycleCalendarWarning>,
) {
    if event.window.ends_at_unix_ms < event.window.starts_at_unix_ms
        || event.window.timezone.trim().is_empty()
    {
        warnings.push(warning(
            event,
            LifecycleCalendarWarningCode::InvalidEventWindow,
            LifecycleCalendarWarningSeverity::Error,
            "Lifecycle calendar event window is invalid.",
        ));
    }
}

fn validate_staleness_and_confidence(
    event: &LifecycleCalendarEventContract,
    warnings: &mut Vec<LifecycleCalendarWarning>,
) {
    if event
        .stale_after_unix_ms
        .is_some_and(|stale_after| event.as_of_unix_ms > stale_after)
    {
        warnings.push(warning(
            event,
            LifecycleCalendarWarningCode::StaleSourceData,
            LifecycleCalendarWarningSeverity::Error,
            "Lifecycle calendar event source data is stale.",
        ));
    }
    if !event.confidence.is_finite() || !(0.0..=1.0).contains(&event.confidence) {
        warnings.push(warning(
            event,
            LifecycleCalendarWarningCode::MissingSourceData,
            LifecycleCalendarWarningSeverity::Error,
            "Lifecycle calendar confidence must be a finite value from 0.0 to 1.0.",
        ));
    }
}

fn validate_secret_surfaces(
    event: &LifecycleCalendarEventContract,
    warnings: &mut Vec<LifecycleCalendarWarning>,
) {
    if event.source.provenance.raw_secrets_exposed
        || contains_secret_key(&event.metadata)
        || event
            .source
            .adapter_ref
            .to_ascii_lowercase()
            .contains("secret")
    {
        warnings.push(warning(
            event,
            LifecycleCalendarWarningCode::RawSecretRejected,
            LifecycleCalendarWarningSeverity::Error,
            "Lifecycle calendar contract contains raw secret-shaped metadata.",
        ));
    }
}

fn warning(
    event: &LifecycleCalendarEventContract,
    code: LifecycleCalendarWarningCode,
    severity: LifecycleCalendarWarningSeverity,
    message: &str,
) -> LifecycleCalendarWarning {
    LifecycleCalendarWarning {
        code,
        severity,
        message: message.to_string(),
        replay_ref: event
            .replay_refs
            .first()
            .map(|replay| replay.replay_ref.clone())
            .unwrap_or_else(|| {
                format!(
                    "tradeassembly://lifecycle-calendar/{}/replay",
                    event.event_id
                )
            }),
    }
}

fn contains_secret_key(metadata: &BTreeMap<String, Value>) -> bool {
    metadata
        .iter()
        .any(|(key, value)| secret_key(key) || value_contains_secret_key(value))
}

fn value_contains_secret_key(value: &Value) -> bool {
    match value {
        Value::Object(map) => map
            .iter()
            .any(|(key, value)| secret_key(key) || value_contains_secret_key(value)),
        Value::Array(values) => values.iter().any(value_contains_secret_key),
        _ => false,
    }
}

fn secret_key(key: &str) -> bool {
    let normalized = key.to_ascii_lowercase();
    normalized.contains("api_key")
        || normalized.contains("api_secret")
        || normalized.contains("secret")
        || normalized.contains("password")
        || normalized.contains("token")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn lifecycle_calendar_event_serializes_deterministically() {
        let event = sample_event(LifecycleCalendarEventType::Expiration);

        let serialized = serde_json::to_string(&event).expect("serialize lifecycle event");
        let hash = lifecycle_calendar_event_hash(&event);

        assert!(
            serialized.contains("\"schemaVersion\":\"tradeassembly.lifecycle_calendar.event.v1\"")
        );
        assert!(serialized.contains("\"eventType\":\"expiration\""));
        assert!(serialized.contains("\"instrumentId\":\"occ:SPY260717C00550000\""));
        assert!(hash.starts_with("sha256:"));
        assert_eq!(hash, lifecycle_calendar_event_hash(&event));
        assert!(event.validate().ok);
        assert!(!serialized.contains("api_secret"));
    }

    #[test]
    fn required_event_types_cover_options_lifecycle_and_user_reminders() {
        let required = required_lifecycle_calendar_event_types();

        for event_type in [
            LifecycleCalendarEventType::Expiration,
            LifecycleCalendarEventType::LastTradeDate,
            LifecycleCalendarEventType::ExerciseWindow,
            LifecycleCalendarEventType::AssignmentWindow,
            LifecycleCalendarEventType::ExDividendDate,
            LifecycleCalendarEventType::EarningsDate,
            LifecycleCalendarEventType::CorporateAction,
            LifecycleCalendarEventType::MarginEvent,
            LifecycleCalendarEventType::UserReminder,
        ] {
            assert!(required.contains(&event_type), "{event_type:?}");
        }
    }

    #[test]
    fn warning_cases_fail_closed_without_raw_secret_echo() {
        let mut event = sample_event(LifecycleCalendarEventType::AssignmentWindow);
        event.source.supported_event_types = vec![LifecycleCalendarEventType::Expiration];
        event.source.provenance.source_ref = "".to_string();
        event.source.provenance.raw_secrets_exposed = true;
        event.affected[0].instrument_id = "ambiguous:SPY-option".to_string();
        event.affected[0].account_scope_ref = None;
        event.window.ends_at_unix_ms = event.window.starts_at_unix_ms - 1;
        event.as_of_unix_ms = 2_000;
        event.stale_after_unix_ms = Some(1_999);
        event.confidence = 1.2;
        event
            .metadata
            .insert("api_secret".to_string(), json!("redacted"));

        let report = event.validate();
        let codes = report
            .warnings
            .iter()
            .map(|warning| warning.code.clone())
            .collect::<BTreeSet<_>>();
        let serialized = serde_json::to_string(&report).expect("serialize warnings");

        assert!(!report.ok);
        assert!(codes.contains(&LifecycleCalendarWarningCode::UnsupportedEventType));
        assert!(codes.contains(&LifecycleCalendarWarningCode::MissingSourceData));
        assert!(codes.contains(&LifecycleCalendarWarningCode::AmbiguousIdentity));
        assert!(codes.contains(&LifecycleCalendarWarningCode::AccountSpecificUnknown));
        assert!(codes.contains(&LifecycleCalendarWarningCode::InvalidEventWindow));
        assert!(codes.contains(&LifecycleCalendarWarningCode::StaleSourceData));
        assert!(codes.contains(&LifecycleCalendarWarningCode::RawSecretRejected));
        assert!(!serialized.contains("redacted"));
    }

    #[test]
    fn hosted_nonlocal_sources_fail_closed_but_user_reminders_are_local() {
        let mut hosted = sample_event(LifecycleCalendarEventType::EarningsDate);
        hosted.source.hosted_integration = true;
        hosted.source.local_first = false;
        let hosted_report = hosted.validate();

        let mut reminder = sample_event(LifecycleCalendarEventType::UserReminder);
        reminder.source.source_kind = LifecycleCalendarSourceKind::UserReminder;
        reminder.source.adapter_ref = "local-user-reminder-adapter".to_string();
        reminder.source.supported_event_types = vec![LifecycleCalendarEventType::UserReminder];
        reminder.affected[0].account_scope_ref = None;
        reminder.confidence = 1.0;
        let reminder_report = reminder.validate();

        assert!(!hosted_report.ok);
        assert!(hosted_report.warnings.iter().any(|warning| {
            warning.code == LifecycleCalendarWarningCode::HostedIntegrationUnsupported
        }));
        assert!(reminder_report.ok, "{reminder_report:#?}");
    }

    #[test]
    fn calendar_notice_contains_no_action_advice_copy() {
        let event = sample_event(LifecycleCalendarEventType::ExerciseWindow);
        let serialized = serde_json::to_string(&event).expect("serialize event");

        assert!(serialized.contains("did not recommend exercise"));
        assert!(!serialized.to_lowercase().contains("should exercise"));
        assert!(!serialized.to_lowercase().contains("should roll"));
        assert!(!serialized.to_lowercase().contains("should close"));
    }

    fn sample_event(event_type: LifecycleCalendarEventType) -> LifecycleCalendarEventContract {
        LifecycleCalendarEventContract {
            schema_version: LIFECYCLE_CALENDAR_SCHEMA_VERSION.to_string(),
            event_id: "lc_evt_001".to_string(),
            event_type: event_type.clone(),
            affected: vec![LifecycleCalendarAffectedRef {
                instrument_id: "occ:SPY260717C00550000".to_string(),
                symbol: "SPY".to_string(),
                asset_class: "option".to_string(),
                position_id: Some("position-spy-call".to_string()),
                strategy_id: Some("strat_local_btc_demo".to_string()),
                account_scope_ref: Some("acct_scope_redacted".to_string()),
            }],
            window: LifecycleCalendarWindow {
                starts_at_unix_ms: 1_785_715_200_000,
                ends_at_unix_ms: 1_785_801_600_000,
                timezone: "America/New_York".to_string(),
            },
            as_of_unix_ms: 1_785_000_000_000,
            stale_after_unix_ms: Some(1_785_100_000_000),
            confidence: 0.93,
            source: LifecycleCalendarSourceContract {
                source_id: "alpaca-options-calendar".to_string(),
                source_kind: LifecycleCalendarSourceKind::ProviderAdapter,
                adapter_ref: "adapter://local/alpaca-paper/calendar".to_string(),
                provider_ref: Some("alpaca-paper".to_string()),
                local_first: true,
                hosted_integration: false,
                account_specific: true,
                supported_event_types: required_lifecycle_calendar_event_types()
                    .into_iter()
                    .collect(),
                provenance: LifecycleCalendarSourceProvenance {
                    source: "alpaca-paper-fixture".to_string(),
                    source_ref: "calendar://alpaca-paper/SPY/2026-07-17".to_string(),
                    collected_at_unix_ms: 1_785_000_000_000,
                    raw_secrets_exposed: false,
                },
            },
            unsupported_reason: None,
            warnings: vec![],
            deep_links: BTreeMap::from([(
                "studio".to_string(),
                "/app/strategies/strat_local_btc_demo/run?calendarEvent=lc_evt_001".to_string(),
            )]),
            replay_refs: vec![LifecycleCalendarReplayRef {
                replay_ref: "tradeassembly://lifecycle-calendar/lc_evt_001/replay".to_string(),
                deterministic: true,
            }],
            metadata: BTreeMap::new(),
            no_advice_notice: LIFECYCLE_CALENDAR_NOTICE.to_string(),
        }
    }
}
