// Copyright (c) 2026 OptionLab LLC. All rights reserved.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

pub const EXECUTION_EVENT_SCHEMA_VERSION: &str = "tradeassembly.execution_event.v1";
pub const FILL_QUALITY_SCHEMA_VERSION: &str = "tradeassembly.fill_quality.v1";
pub const NO_ADVICE_EXECUTION_NOTICE: &str = "Descriptive execution evidence only. TradeAssembly did not recommend a broker, venue, order type, entry, exit, size, or activation decision.";

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExecutionMode {
    ResearchReplay,
    Paper,
    Live,
    ManualImport,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExecutionEventKind {
    OrderIntent,
    SimulatedFill,
    PaperFill,
    LiveFill,
    PartialFill,
    Cancel,
    Reject,
    ManualImport,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OrderSide {
    Buy,
    Sell,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OrderType {
    Market,
    Limit,
    Stop,
    StopLimit,
    Other,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FillStatus {
    Planned,
    Submitted,
    PartiallyFilled,
    Filled,
    Canceled,
    Rejected,
    Imported,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WarningSeverity {
    Warning,
    Error,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FillQualityWarningCode {
    MissingDecisionQuote,
    MissingFillQuote,
    StaleDecisionQuote,
    StaleFillQuote,
    UnsupportedProviderField,
    AmbiguousInstrumentIdentity,
    IncompleteFillEvent,
    RawSecretRejected,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExecutionOrderIntent {
    pub order_intent_id: String,
    pub strategy_id: String,
    pub instrument_id: String,
    pub side: OrderSide,
    pub order_type: OrderType,
    pub quantity: f64,
    pub limit_price: Option<f64>,
    pub created_at_unix_ms: u64,
    pub provider_ref: Option<String>,
    pub user_activation_ref: Option<String>,
    pub account_scope: ExecutionAccountScope,
    pub venue_metadata: BTreeMap<String, Value>,
    pub source_provenance: SourceProvenance,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExecutionAccountScope {
    pub mode: ExecutionMode,
    pub account_scope_ref: String,
    pub broker_account_id: Option<String>,
    pub raw_account_identifier_exposed: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct QuoteSnapshot {
    pub quote_ref: String,
    pub instrument_id: String,
    pub bid: Option<f64>,
    pub ask: Option<f64>,
    pub last: Option<f64>,
    pub captured_at_unix_ms: u64,
    pub stale_after_unix_ms: Option<u64>,
    pub provider_ref: String,
    pub venue_metadata: BTreeMap<String, Value>,
    pub source_provenance: SourceProvenance,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExecutionFill {
    pub fill_id: String,
    pub status: FillStatus,
    pub filled_quantity: Option<f64>,
    pub fill_price: Option<f64>,
    pub filled_at_unix_ms: Option<u64>,
    pub fee_amount: Option<f64>,
    pub fee_currency: Option<String>,
    pub reject_reason: Option<String>,
    pub cancel_reason: Option<String>,
    pub provider_venue_metadata: BTreeMap<String, Value>,
    pub source_provenance: SourceProvenance,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SourceProvenance {
    pub source: String,
    pub source_ref: String,
    pub collected_at_unix_ms: u64,
    pub raw_secrets_exposed: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReplayRef {
    pub replay_ref: String,
    pub deterministic: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExecutionEventContract {
    pub schema_version: String,
    pub event_id: String,
    pub event_kind: ExecutionEventKind,
    pub mode: ExecutionMode,
    pub order_intent: ExecutionOrderIntent,
    pub fill: Option<ExecutionFill>,
    pub decision_quote: Option<QuoteSnapshot>,
    pub fill_quote: Option<QuoteSnapshot>,
    pub replay_ref: ReplayRef,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FillQualityWarning {
    pub code: FillQualityWarningCode,
    pub severity: WarningSeverity,
    pub message: String,
    pub replay_ref: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FillQualityBenchmark {
    pub benchmark_ref: String,
    pub benchmark_price: Option<f64>,
    pub benchmark_source: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FillQualityReport {
    pub schema_version: String,
    pub event_id: String,
    pub mode: ExecutionMode,
    pub status: FillStatus,
    pub benchmark_reference: FillQualityBenchmark,
    pub slippage_amount: Option<f64>,
    pub spread_capture: Option<f64>,
    pub time_to_fill_ms: Option<u64>,
    pub reject_reason: Option<String>,
    pub cancel_reason: Option<String>,
    pub stale_quote_warnings: Vec<FillQualityWarning>,
    pub missing_quote_warnings: Vec<FillQualityWarning>,
    pub warnings: Vec<FillQualityWarning>,
    pub replay_refs: Vec<ReplayRef>,
    pub no_advice_notice: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ContractValidationReport {
    pub ok: bool,
    pub warnings: Vec<FillQualityWarning>,
}

impl ExecutionEventContract {
    pub fn validate(&self) -> ContractValidationReport {
        let mut warnings = Vec::new();
        validate_required_contract_fields(self, &mut warnings);
        validate_quote_state(self, &mut warnings);
        validate_secret_surfaces(self, &mut warnings);
        let ok = warnings
            .iter()
            .all(|warning| warning.severity != WarningSeverity::Error);
        ContractValidationReport { ok, warnings }
    }
}

pub fn analyze_fill_quality(event: &ExecutionEventContract) -> FillQualityReport {
    let validation = event.validate();
    let fill = event.fill.as_ref();
    let status = fill
        .map(|fill| fill.status.clone())
        .unwrap_or(FillStatus::Planned);
    let benchmark_price = event.decision_quote.as_ref().and_then(quote_midpoint);
    let fill_price = fill.and_then(|fill| fill.fill_price);
    let slippage_amount = match (&event.order_intent.side, benchmark_price, fill_price) {
        (OrderSide::Buy, Some(benchmark), Some(price)) => Some(price - benchmark),
        (OrderSide::Sell, Some(benchmark), Some(price)) => Some(benchmark - price),
        _ => None,
    };
    let spread_capture = match (
        &event.order_intent.side,
        event.decision_quote.as_ref(),
        fill_price,
    ) {
        (OrderSide::Buy, Some(quote), Some(price)) => quote.ask.map(|ask| ask - price),
        (OrderSide::Sell, Some(quote), Some(price)) => quote.bid.map(|bid| price - bid),
        _ => None,
    };
    let time_to_fill_ms = fill
        .and_then(|fill| fill.filled_at_unix_ms)
        .and_then(|filled_at| filled_at.checked_sub(event.order_intent.created_at_unix_ms));
    let stale_quote_warnings = warnings_by_code(
        &validation.warnings,
        &[
            FillQualityWarningCode::StaleDecisionQuote,
            FillQualityWarningCode::StaleFillQuote,
        ],
    );
    let missing_quote_warnings = warnings_by_code(
        &validation.warnings,
        &[
            FillQualityWarningCode::MissingDecisionQuote,
            FillQualityWarningCode::MissingFillQuote,
        ],
    );

    FillQualityReport {
        schema_version: FILL_QUALITY_SCHEMA_VERSION.to_string(),
        event_id: event.event_id.clone(),
        mode: event.mode.clone(),
        status,
        benchmark_reference: FillQualityBenchmark {
            benchmark_ref: event
                .decision_quote
                .as_ref()
                .map(|quote| quote.quote_ref.clone())
                .unwrap_or_else(|| "missing_decision_quote".to_string()),
            benchmark_price,
            benchmark_source: "decision_quote_midpoint".to_string(),
        },
        slippage_amount,
        spread_capture,
        time_to_fill_ms,
        reject_reason: fill.and_then(|fill| fill.reject_reason.clone()),
        cancel_reason: fill.and_then(|fill| fill.cancel_reason.clone()),
        stale_quote_warnings,
        missing_quote_warnings,
        warnings: validation.warnings,
        replay_refs: vec![event.replay_ref.clone()],
        no_advice_notice: NO_ADVICE_EXECUTION_NOTICE.to_string(),
    }
}

fn validate_required_contract_fields(
    event: &ExecutionEventContract,
    warnings: &mut Vec<FillQualityWarning>,
) {
    if event.schema_version != EXECUTION_EVENT_SCHEMA_VERSION {
        warnings.push(warning(
            FillQualityWarningCode::UnsupportedProviderField,
            WarningSeverity::Error,
            "Execution event schema version is unsupported.",
            event,
        ));
    }
    if event.order_intent.instrument_id.trim().is_empty()
        || event.order_intent.instrument_id.starts_with("ambiguous:")
        || event.order_intent.instrument_id.starts_with("unsupported:")
    {
        warnings.push(warning(
            FillQualityWarningCode::AmbiguousInstrumentIdentity,
            WarningSeverity::Error,
            "Execution event does not identify one supported canonical instrument.",
            event,
        ));
    }
    if event
        .order_intent
        .account_scope
        .raw_account_identifier_exposed
        || event.order_intent.account_scope.broker_account_id.is_some()
    {
        warnings.push(warning(
            FillQualityWarningCode::RawSecretRejected,
            WarningSeverity::Error,
            "Execution account scope must use redacted account refs only.",
            event,
        ));
    }
    if matches!(
        event.event_kind,
        ExecutionEventKind::SimulatedFill
            | ExecutionEventKind::PaperFill
            | ExecutionEventKind::LiveFill
            | ExecutionEventKind::PartialFill
            | ExecutionEventKind::ManualImport
    ) {
        let incomplete = event.fill.as_ref().is_none_or(|fill| {
            fill.filled_quantity.is_none()
                || fill.fill_price.is_none()
                || fill.filled_at_unix_ms.is_none()
        });
        if incomplete {
            warnings.push(warning(
                FillQualityWarningCode::IncompleteFillEvent,
                WarningSeverity::Error,
                "Fill event is missing quantity, price, or timestamp.",
                event,
            ));
        }
    }
}

fn validate_quote_state(event: &ExecutionEventContract, warnings: &mut Vec<FillQualityWarning>) {
    if event.decision_quote.is_none() {
        warnings.push(warning(
            FillQualityWarningCode::MissingDecisionQuote,
            WarningSeverity::Error,
            "Decision-time quote is missing.",
            event,
        ));
    }
    if event.fill_quote.is_none()
        && matches!(
            event.event_kind,
            ExecutionEventKind::SimulatedFill
                | ExecutionEventKind::PaperFill
                | ExecutionEventKind::LiveFill
                | ExecutionEventKind::PartialFill
                | ExecutionEventKind::ManualImport
        )
    {
        warnings.push(warning(
            FillQualityWarningCode::MissingFillQuote,
            WarningSeverity::Error,
            "Fill-time quote is missing.",
            event,
        ));
    }
    if quote_is_stale(
        event.decision_quote.as_ref(),
        event.order_intent.created_at_unix_ms,
    ) {
        warnings.push(warning(
            FillQualityWarningCode::StaleDecisionQuote,
            WarningSeverity::Error,
            "Decision-time quote is stale for the order intent timestamp.",
            event,
        ));
    }
    if let (Some(fill), Some(fill_quote)) = (event.fill.as_ref(), event.fill_quote.as_ref()) {
        if fill
            .filled_at_unix_ms
            .is_some_and(|filled_at| quote_is_stale(Some(fill_quote), filled_at))
        {
            warnings.push(warning(
                FillQualityWarningCode::StaleFillQuote,
                WarningSeverity::Error,
                "Fill-time quote is stale for the fill timestamp.",
                event,
            ));
        }
    }
}

fn validate_secret_surfaces(
    event: &ExecutionEventContract,
    warnings: &mut Vec<FillQualityWarning>,
) {
    if event.order_intent.source_provenance.raw_secrets_exposed
        || event
            .fill
            .as_ref()
            .is_some_and(|fill| fill.source_provenance.raw_secrets_exposed)
        || event
            .decision_quote
            .as_ref()
            .is_some_and(|quote| quote.source_provenance.raw_secrets_exposed)
        || event
            .fill_quote
            .as_ref()
            .is_some_and(|quote| quote.source_provenance.raw_secrets_exposed)
        || contains_secret_key(&event.order_intent.venue_metadata)
        || event
            .fill
            .as_ref()
            .is_some_and(|fill| contains_secret_key(&fill.provider_venue_metadata))
    {
        warnings.push(warning(
            FillQualityWarningCode::RawSecretRejected,
            WarningSeverity::Error,
            "Execution contract contains raw secret-shaped metadata.",
            event,
        ));
    }
}

fn warning(
    code: FillQualityWarningCode,
    severity: WarningSeverity,
    message: &str,
    event: &ExecutionEventContract,
) -> FillQualityWarning {
    FillQualityWarning {
        code,
        severity,
        message: message.to_string(),
        replay_ref: event.replay_ref.replay_ref.clone(),
    }
}

fn quote_midpoint(quote: &QuoteSnapshot) -> Option<f64> {
    match (quote.bid, quote.ask, quote.last) {
        (Some(bid), Some(ask), _) => Some((bid + ask) / 2.0),
        (_, _, Some(last)) => Some(last),
        _ => None,
    }
}

fn quote_is_stale(quote: Option<&QuoteSnapshot>, compare_at_unix_ms: u64) -> bool {
    quote
        .and_then(|quote| quote.stale_after_unix_ms)
        .is_some_and(|stale_after| compare_at_unix_ms > stale_after)
}

fn warnings_by_code(
    warnings: &[FillQualityWarning],
    codes: &[FillQualityWarningCode],
) -> Vec<FillQualityWarning> {
    warnings
        .iter()
        .filter(|warning| codes.contains(&warning.code))
        .cloned()
        .collect()
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
    fn execution_event_contract_serializes_deterministically() {
        let event = complete_paper_fill_event();

        let serialized = serde_json::to_string(&event).expect("serialize event");

        assert!(serialized.contains("\"schemaVersion\":\"tradeassembly.execution_event.v1\""));
        assert!(serialized.contains("\"eventKind\":\"paper_fill\""));
        assert!(serialized.contains("\"accountScopeRef\":\"acct_scope_redacted\""));
        assert!(!serialized.contains("api_secret"));
        assert!(!serialized.contains("brokerAccountId\":\""));
    }

    #[test]
    fn fill_quality_report_calculates_slippage_spread_and_time_to_fill() {
        let report = analyze_fill_quality(&complete_paper_fill_event());

        assert_eq!(report.schema_version, FILL_QUALITY_SCHEMA_VERSION);
        assert_eq!(report.benchmark_reference.benchmark_price, Some(10.0));
        assert_close(report.slippage_amount, 0.1);
        assert_close(report.spread_capture, -0.05);
        assert_eq!(report.time_to_fill_ms, Some(400));
        assert!(report.warnings.is_empty());
        assert_eq!(report.replay_refs[0].replay_ref, "replay://fills/fill-1");
    }

    #[test]
    fn missing_and_stale_quotes_fail_closed_with_structured_warnings() {
        let mut event = complete_paper_fill_event();
        event.decision_quote = None;
        event.fill_quote = Some(QuoteSnapshot {
            stale_after_unix_ms: Some(1_000),
            ..quote("quote-fill", 9.95, 10.05, 1_500)
        });

        let report = analyze_fill_quality(&event);

        assert!(report.slippage_amount.is_none());
        assert_eq!(report.missing_quote_warnings.len(), 1);
        assert_eq!(
            report.missing_quote_warnings[0].code,
            FillQualityWarningCode::MissingDecisionQuote
        );
        assert_eq!(report.stale_quote_warnings.len(), 1);
        assert_eq!(
            report.stale_quote_warnings[0].code,
            FillQualityWarningCode::StaleFillQuote
        );
        assert!(!event.validate().ok);
    }

    #[test]
    fn rejects_ambiguous_instrument_incomplete_fill_and_raw_secret_metadata() {
        let mut event = complete_paper_fill_event();
        event.order_intent.instrument_id = "ambiguous:SPY".to_string();
        event.fill.as_mut().expect("fill").fill_price = None;
        event
            .order_intent
            .venue_metadata
            .insert("api_secret".to_string(), json!("SHOULD_NOT_LEAK"));

        let validation = event.validate();
        let codes = validation
            .warnings
            .iter()
            .map(|warning| warning.code.clone())
            .collect::<Vec<_>>();

        assert!(!validation.ok);
        assert!(codes.contains(&FillQualityWarningCode::AmbiguousInstrumentIdentity));
        assert!(codes.contains(&FillQualityWarningCode::IncompleteFillEvent));
        assert!(codes.contains(&FillQualityWarningCode::RawSecretRejected));
        assert!(!serde_json::to_string(&validation)
            .expect("serialize validation")
            .contains("SHOULD_NOT_LEAK"));
    }

    #[test]
    fn cancel_and_reject_reasons_are_reported_without_trade_advice() {
        let mut event = complete_paper_fill_event();
        event.event_kind = ExecutionEventKind::Reject;
        event.fill = Some(ExecutionFill {
            status: FillStatus::Rejected,
            filled_quantity: None,
            fill_price: None,
            filled_at_unix_ms: None,
            reject_reason: Some("provider_rejected_buying_power".to_string()),
            ..fill()
        });

        let report = analyze_fill_quality(&event);
        let rendered = serde_json::to_string(&report).expect("serialize report");

        assert_eq!(
            report.reject_reason.as_deref(),
            Some("provider_rejected_buying_power")
        );
        assert!(rendered.contains("did not recommend"));
        for forbidden in [
            "you should",
            "we recommend",
            "best broker",
            "buy now",
            "sell now",
            "increase size",
        ] {
            assert!(
                !rendered.to_ascii_lowercase().contains(forbidden),
                "fill quality report emitted advice phrase: {forbidden}"
            );
        }
    }

    fn complete_paper_fill_event() -> ExecutionEventContract {
        ExecutionEventContract {
            schema_version: EXECUTION_EVENT_SCHEMA_VERSION.to_string(),
            event_id: "event-fill-1".to_string(),
            event_kind: ExecutionEventKind::PaperFill,
            mode: ExecutionMode::Paper,
            order_intent: order_intent(),
            fill: Some(fill()),
            decision_quote: Some(quote("quote-decision", 9.95, 10.05, 1_000)),
            fill_quote: Some(quote("quote-fill", 10.00, 10.10, 1_400)),
            replay_ref: ReplayRef {
                replay_ref: "replay://fills/fill-1".to_string(),
                deterministic: true,
            },
        }
    }

    fn order_intent() -> ExecutionOrderIntent {
        ExecutionOrderIntent {
            order_intent_id: "intent-1".to_string(),
            strategy_id: "strategy-1".to_string(),
            instrument_id: "ul:equity:SPY".to_string(),
            side: OrderSide::Buy,
            order_type: OrderType::Limit,
            quantity: 2.0,
            limit_price: Some(10.25),
            created_at_unix_ms: 1_000,
            provider_ref: Some("alpaca-paper".to_string()),
            user_activation_ref: Some("activation-user-approved".to_string()),
            account_scope: ExecutionAccountScope {
                mode: ExecutionMode::Paper,
                account_scope_ref: "acct_scope_redacted".to_string(),
                broker_account_id: None,
                raw_account_identifier_exposed: false,
            },
            venue_metadata: BTreeMap::from([("venue".to_string(), json!("paper"))]),
            source_provenance: provenance("order_intent"),
        }
    }

    fn fill() -> ExecutionFill {
        ExecutionFill {
            fill_id: "fill-1".to_string(),
            status: FillStatus::Filled,
            filled_quantity: Some(2.0),
            fill_price: Some(10.1),
            filled_at_unix_ms: Some(1_400),
            fee_amount: Some(0.02),
            fee_currency: Some("USD".to_string()),
            reject_reason: None,
            cancel_reason: None,
            provider_venue_metadata: BTreeMap::from([("venue".to_string(), json!("paper"))]),
            source_provenance: provenance("fill"),
        }
    }

    fn quote(ref_id: &str, bid: f64, ask: f64, captured_at_unix_ms: u64) -> QuoteSnapshot {
        QuoteSnapshot {
            quote_ref: ref_id.to_string(),
            instrument_id: "ul:equity:SPY".to_string(),
            bid: Some(bid),
            ask: Some(ask),
            last: None,
            captured_at_unix_ms,
            stale_after_unix_ms: Some(captured_at_unix_ms + 1_000),
            provider_ref: "alpaca-paper".to_string(),
            venue_metadata: BTreeMap::from([("feed".to_string(), json!("iex"))]),
            source_provenance: provenance(ref_id),
        }
    }

    fn provenance(source_ref: &str) -> SourceProvenance {
        SourceProvenance {
            source: "fixture".to_string(),
            source_ref: source_ref.to_string(),
            collected_at_unix_ms: 1_000,
            raw_secrets_exposed: false,
        }
    }

    fn assert_close(actual: Option<f64>, expected: f64) {
        let actual = actual.expect("numeric value");
        assert!(
            (actual - expected).abs() < 0.000_001,
            "expected {expected}, got {actual}"
        );
    }
}
