// Copyright (c) 2026 OptionLab LLC. All rights reserved.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};

pub const PORTFOLIO_RISK_SCHEMA_VERSION: &str = "tradeassembly.portfolio_risk.contract.v1";
pub const PORTFOLIO_RISK_NOTICE: &str =
    "Research-only risk evidence. TradeAssembly does not tell users what to trade, when to trade, or how much to trade.";

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PortfolioRiskScopeKind {
    Strategy,
    Account,
    Portfolio,
    Watchlist,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PortfolioRiskScope {
    pub kind: PortfolioRiskScopeKind,
    pub id: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PositionEvidence {
    pub position_id: String,
    pub symbol: String,
    pub instrument_id: String,
    pub quantity: f64,
    pub notional: f64,
    pub provider_ref: Option<String>,
    pub strategy_id: Option<String>,
    pub approved_evidence_ref: String,
    #[serde(default)]
    pub exposure_inputs: BTreeMap<String, Value>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InstrumentSupport {
    Supported,
    Unsupported,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InstrumentMetadata {
    pub instrument_id: String,
    pub symbol: String,
    pub asset_class: String,
    pub provider: String,
    pub multiplier: f64,
    pub support: InstrumentSupport,
    #[serde(default)]
    pub groups: BTreeMap<String, String>,
    #[serde(default)]
    pub source_refs: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GroupingDimension {
    pub id: String,
    pub label: String,
    pub metadata_field: String,
    pub source_ref: String,
    pub required: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExposureFormula {
    pub id: String,
    pub label: String,
    pub expression: String,
    pub output_unit: String,
    #[serde(default)]
    pub required_inputs: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConcentrationThreshold {
    pub id: String,
    pub metric: String,
    pub group_by: String,
    pub max_share: Option<f64>,
    pub max_notional: Option<f64>,
    pub severity: String,
    pub user_defined: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StressScenario {
    pub id: String,
    pub label: String,
    #[serde(default)]
    pub shocks: BTreeMap<String, f64>,
    pub source_ref: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StaleDataPolicy {
    pub market_data_as_of_epoch: u64,
    pub evaluation_epoch: u64,
    pub max_market_data_age_seconds: u64,
    pub fail_closed: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelProvenance {
    pub model_id: String,
    pub model_version: String,
    #[serde(default)]
    pub source_refs: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PortfolioRiskSnapshotRequest {
    pub snapshot_id: String,
    pub scope: PortfolioRiskScope,
    #[serde(default)]
    pub positions: Vec<PositionEvidence>,
    #[serde(default)]
    pub instruments: BTreeMap<String, InstrumentMetadata>,
    #[serde(default)]
    pub grouping_dimensions: Vec<GroupingDimension>,
    #[serde(default)]
    pub exposure_formulas: Vec<ExposureFormula>,
    #[serde(default)]
    pub concentration_thresholds: Vec<ConcentrationThreshold>,
    #[serde(default)]
    pub stress_scenarios: Vec<StressScenario>,
    pub stale_data_policy: StaleDataPolicy,
    pub model_provenance: ModelProvenance,
    #[serde(default)]
    pub replay_refs: Vec<String>,
    #[serde(default)]
    pub export_refs: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExposureTable {
    pub table_id: String,
    pub metric: String,
    pub group_by: String,
    #[serde(default)]
    pub rows: Vec<ExposureRow>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExposureRow {
    pub group: String,
    pub notional: f64,
    pub share: f64,
    pub position_count: usize,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConcentrationWarning {
    pub threshold_id: String,
    pub group: String,
    pub metric: String,
    pub observed: f64,
    pub limit: f64,
    pub severity: String,
    pub message: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PortfolioRiskWarning {
    pub code: String,
    pub message: String,
    pub fail_closed: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PortfolioRiskSnapshotContract {
    pub ok: bool,
    pub schema_version: String,
    pub snapshot_hash: String,
    pub snapshot_id: String,
    pub scope: PortfolioRiskScope,
    #[serde(default)]
    pub exposure_tables: Vec<ExposureTable>,
    #[serde(default)]
    pub concentration_warnings: Vec<ConcentrationWarning>,
    #[serde(default)]
    pub scenario_refs: Vec<String>,
    #[serde(default)]
    pub missing_data_warnings: Vec<PortfolioRiskWarning>,
    pub stale_data_policy: StaleDataPolicy,
    pub model_provenance: ModelProvenance,
    #[serde(default)]
    pub replay_refs: Vec<String>,
    #[serde(default)]
    pub export_refs: Vec<String>,
    pub notice: String,
}

pub fn build_portfolio_risk_snapshot_contract(
    request: &PortfolioRiskSnapshotRequest,
) -> PortfolioRiskSnapshotContract {
    let snapshot_hash = canonical_hash(request);
    let mut warnings = validate_request(request);
    let exposure_tables = build_exposure_tables(request, &mut warnings);
    let concentration_warnings = build_concentration_warnings(request, &exposure_tables);
    let ok = warnings.iter().all(|warning| !warning.fail_closed);
    PortfolioRiskSnapshotContract {
        ok,
        schema_version: PORTFOLIO_RISK_SCHEMA_VERSION.to_string(),
        snapshot_hash,
        snapshot_id: request.snapshot_id.clone(),
        scope: request.scope.clone(),
        exposure_tables,
        concentration_warnings,
        scenario_refs: request
            .stress_scenarios
            .iter()
            .map(|scenario| scenario.id.clone())
            .collect(),
        missing_data_warnings: warnings,
        stale_data_policy: request.stale_data_policy.clone(),
        model_provenance: request.model_provenance.clone(),
        replay_refs: request.replay_refs.clone(),
        export_refs: request.export_refs.clone(),
        notice: PORTFOLIO_RISK_NOTICE.to_string(),
    }
}

fn validate_request(request: &PortfolioRiskSnapshotRequest) -> Vec<PortfolioRiskWarning> {
    let mut warnings = Vec::new();
    if request.positions.is_empty() {
        warnings.push(fail_closed(
            "missing_positions",
            "Portfolio risk snapshots require user-approved position evidence.",
        ));
    }
    if request.concentration_thresholds.is_empty()
        || request
            .concentration_thresholds
            .iter()
            .any(|threshold| !threshold.user_defined)
    {
        warnings.push(fail_closed(
            "missing_user_thresholds",
            "Portfolio risk snapshots require user-defined concentration thresholds.",
        ));
    }
    if request
        .stale_data_policy
        .evaluation_epoch
        .saturating_sub(request.stale_data_policy.market_data_as_of_epoch)
        > request.stale_data_policy.max_market_data_age_seconds
    {
        warnings.push(PortfolioRiskWarning {
            code: "stale_market_data".to_string(),
            message: "Market data age exceeds the user-approved stale-data policy.".to_string(),
            fail_closed: request.stale_data_policy.fail_closed,
        });
    }
    if contains_secret_material(request) {
        warnings.push(fail_closed(
            "raw_secret_material",
            "Portfolio risk contracts cannot contain raw credentials, tokens, passwords, or secret values.",
        ));
    }

    let grouping_ids = request
        .grouping_dimensions
        .iter()
        .map(|dimension| dimension.id.as_str())
        .collect::<BTreeSet<_>>();
    for threshold in &request.concentration_thresholds {
        if !grouping_ids.contains(threshold.group_by.as_str()) {
            warnings.push(fail_closed(
                "unknown_grouping_dimension",
                format!(
                    "Threshold '{}' references unknown grouping dimension '{}'.",
                    threshold.id, threshold.group_by
                ),
            ));
        }
    }

    for position in &request.positions {
        match request.instruments.get(&position.instrument_id) {
            Some(instrument) if instrument.support == InstrumentSupport::Unsupported => {
                warnings.push(fail_closed(
                    "unsupported_instrument",
                    format!(
                        "Instrument '{}' is not supported by the portfolio risk contract.",
                        position.instrument_id
                    ),
                ));
            }
            Some(instrument) => {
                for dimension in request
                    .grouping_dimensions
                    .iter()
                    .filter(|dimension| dimension.required)
                {
                    if !instrument.groups.contains_key(&dimension.metadata_field) {
                        warnings.push(fail_closed(
                            "missing_grouping_metadata",
                            format!(
                                "Instrument '{}' is missing required grouping metadata '{}'.",
                                position.instrument_id, dimension.metadata_field
                            ),
                        ));
                    }
                }
            }
            None => warnings.push(fail_closed(
                "missing_instrument_metadata",
                format!(
                    "Position '{}' references instrument '{}' without metadata.",
                    position.position_id, position.instrument_id
                ),
            )),
        }
    }
    warnings
}

fn build_exposure_tables(
    request: &PortfolioRiskSnapshotRequest,
    warnings: &mut Vec<PortfolioRiskWarning>,
) -> Vec<ExposureTable> {
    let total_notional: f64 = request
        .positions
        .iter()
        .map(|position| position.notional)
        .sum();
    if total_notional <= 0.0 {
        warnings.push(fail_closed(
            "missing_notional",
            "Portfolio risk snapshots require positive position notionals.",
        ));
        return Vec::new();
    }

    let mut tables = Vec::new();
    for dimension in &request.grouping_dimensions {
        let mut grouped: BTreeMap<String, (f64, usize)> = BTreeMap::new();
        for position in &request.positions {
            let Some(instrument) = request.instruments.get(&position.instrument_id) else {
                continue;
            };
            let Some(group) = instrument.groups.get(&dimension.metadata_field) else {
                continue;
            };
            let entry = grouped.entry(group.clone()).or_insert((0.0, 0));
            entry.0 += position.notional;
            entry.1 += 1;
        }
        let rows = grouped
            .into_iter()
            .map(|(group, (notional, position_count))| ExposureRow {
                group,
                notional: round6(notional),
                share: round6(notional / total_notional),
                position_count,
            })
            .collect();
        tables.push(ExposureTable {
            table_id: format!("exposure_by_{}", dimension.id),
            metric: "notional_share".to_string(),
            group_by: dimension.id.clone(),
            rows,
        });
    }
    tables
}

fn build_concentration_warnings(
    request: &PortfolioRiskSnapshotRequest,
    exposure_tables: &[ExposureTable],
) -> Vec<ConcentrationWarning> {
    let table_by_group = exposure_tables
        .iter()
        .map(|table| (table.group_by.as_str(), table))
        .collect::<BTreeMap<_, _>>();
    let mut warnings = Vec::new();
    for threshold in &request.concentration_thresholds {
        let Some(table) = table_by_group.get(threshold.group_by.as_str()) else {
            continue;
        };
        for row in &table.rows {
            if let Some(max_share) = threshold.max_share {
                if row.share > max_share {
                    warnings.push(ConcentrationWarning {
                        threshold_id: threshold.id.clone(),
                        group: row.group.clone(),
                        metric: threshold.metric.clone(),
                        observed: row.share,
                        limit: max_share,
                        severity: threshold.severity.clone(),
                        message: format!(
                            "Observed {} concentration for '{}' is above the user-defined threshold.",
                            threshold.metric, row.group
                        ),
                    });
                }
            }
            if let Some(max_notional) = threshold.max_notional {
                if row.notional > max_notional {
                    warnings.push(ConcentrationWarning {
                        threshold_id: threshold.id.clone(),
                        group: row.group.clone(),
                        metric: "notional".to_string(),
                        observed: row.notional,
                        limit: max_notional,
                        severity: threshold.severity.clone(),
                        message: format!(
                            "Observed notional exposure for '{}' is above the user-defined threshold.",
                            row.group
                        ),
                    });
                }
            }
        }
    }
    warnings
}

fn canonical_hash<T: Serialize>(value: &T) -> String {
    let bytes = serde_json::to_vec(value).expect("portfolio risk contract is serializable");
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    format!("{:x}", hasher.finalize())
}

fn fail_closed(code: impl Into<String>, message: impl Into<String>) -> PortfolioRiskWarning {
    PortfolioRiskWarning {
        code: code.into(),
        message: message.into(),
        fail_closed: true,
    }
}

fn contains_secret_material<T: Serialize>(value: &T) -> bool {
    serde_json::to_value(value)
        .map(|value| contains_secret_value(&value))
        .unwrap_or(true)
}

fn contains_secret_value(value: &Value) -> bool {
    match value {
        Value::Object(object) => object.iter().any(|(key, value)| {
            let key = key.to_ascii_lowercase();
            matches!(
                key.as_str(),
                "api_key"
                    | "apikey"
                    | "api-secret"
                    | "api_secret"
                    | "password"
                    | "token"
                    | "secret"
            ) || contains_secret_value(value)
        }),
        Value::Array(values) => values.iter().any(contains_secret_value),
        _ => false,
    }
}

fn round6(value: f64) -> f64 {
    (value * 1_000_000.0).round() / 1_000_000.0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn contract_accepts_strategy_account_portfolio_and_watchlist_scopes() {
        for kind in [
            PortfolioRiskScopeKind::Strategy,
            PortfolioRiskScopeKind::Account,
            PortfolioRiskScopeKind::Portfolio,
            PortfolioRiskScopeKind::Watchlist,
        ] {
            let mut request = fixture_request();
            request.scope = PortfolioRiskScope {
                kind,
                id: "scope-1".to_string(),
            };
            let contract = build_portfolio_risk_snapshot_contract(&request);
            assert!(contract.ok, "{:?}", contract.missing_data_warnings);
            assert_eq!(contract.schema_version, PORTFOLIO_RISK_SCHEMA_VERSION);
            assert_eq!(contract.scope.id, "scope-1");
        }
    }

    #[test]
    fn contract_returns_exposure_tables_warnings_refs_and_provenance() {
        let contract = build_portfolio_risk_snapshot_contract(&fixture_request());
        assert!(contract.ok);
        assert_eq!(contract.exposure_tables.len(), 1);
        assert_eq!(contract.exposure_tables[0].group_by, "sector");
        assert_eq!(contract.concentration_warnings.len(), 1);
        assert_eq!(contract.concentration_warnings[0].group, "Technology");
        assert_eq!(contract.scenario_refs, vec!["scenario-rate-up"]);
        assert_eq!(
            contract.replay_refs,
            vec!["replay://portfolio-risk/snapshot-1"]
        );
        assert_eq!(contract.export_refs, vec!["file://exports/snapshot-1.json"]);
        assert_eq!(
            contract.model_provenance.model_id,
            "tradeassembly-portfolio-risk"
        );
    }

    #[test]
    fn contract_fails_closed_without_user_thresholds_or_positions() {
        let mut request = fixture_request();
        request.positions.clear();
        request.concentration_thresholds.clear();
        let contract = build_portfolio_risk_snapshot_contract(&request);
        assert!(!contract.ok);
        let codes = warning_codes(&contract);
        assert!(codes.contains("missing_positions"));
        assert!(codes.contains("missing_user_thresholds"));
    }

    #[test]
    fn contract_fails_closed_for_stale_data_unsupported_instruments_and_unknown_groups() {
        let mut request = fixture_request();
        request.stale_data_policy.market_data_as_of_epoch = 1_000;
        request.stale_data_policy.evaluation_epoch = 9_000;
        request.instruments.get_mut("SPY").unwrap().support = InstrumentSupport::Unsupported;
        request.concentration_thresholds[0].group_by = "unknown".to_string();
        let contract = build_portfolio_risk_snapshot_contract(&request);
        assert!(!contract.ok);
        let codes = warning_codes(&contract);
        assert!(codes.contains("stale_market_data"));
        assert!(codes.contains("unsupported_instrument"));
        assert!(codes.contains("unknown_grouping_dimension"));
    }

    #[test]
    fn contract_serializes_deterministically_and_rejects_raw_secret_material() {
        let request = fixture_request();
        let contract_a = build_portfolio_risk_snapshot_contract(&request);
        let contract_b = build_portfolio_risk_snapshot_contract(&request);
        assert_eq!(contract_a.snapshot_hash, contract_b.snapshot_hash);
        assert_eq!(
            serde_json::to_string(&contract_a).unwrap(),
            serde_json::to_string(&contract_b).unwrap()
        );

        let mut request = fixture_request();
        request.positions[0]
            .exposure_inputs
            .insert("api_key".to_string(), serde_json::json!("not-allowed"));
        let contract = build_portfolio_risk_snapshot_contract(&request);
        assert!(!contract.ok);
        assert!(warning_codes(&contract).contains("raw_secret_material"));
        assert!(!serde_json::to_string(&contract)
            .unwrap()
            .contains("not-allowed"));
    }

    #[test]
    fn contract_copy_contains_no_trade_advice() {
        let contract = build_portfolio_risk_snapshot_contract(&fixture_request());
        let rendered = serde_json::to_string(&contract)
            .unwrap()
            .to_ascii_lowercase();
        for phrase in [
            "should buy",
            "should sell",
            "you should trade",
            "enter this trade",
            "exit this trade",
            "increase position",
            "decrease position",
            "rebalance now",
        ] {
            assert!(
                !rendered.contains(phrase),
                "portfolio risk contract contained advice phrase: {phrase}"
            );
        }
    }

    fn warning_codes(contract: &PortfolioRiskSnapshotContract) -> BTreeSet<&str> {
        contract
            .missing_data_warnings
            .iter()
            .map(|warning| warning.code.as_str())
            .collect()
    }

    fn fixture_request() -> PortfolioRiskSnapshotRequest {
        let mut groups = BTreeMap::new();
        groups.insert("sector".to_string(), "Technology".to_string());
        let mut instruments = BTreeMap::new();
        instruments.insert(
            "SPY".to_string(),
            InstrumentMetadata {
                instrument_id: "SPY".to_string(),
                symbol: "SPY".to_string(),
                asset_class: "etf".to_string(),
                provider: "fixture".to_string(),
                multiplier: 1.0,
                support: InstrumentSupport::Supported,
                groups,
                source_refs: vec!["fixture://instrument-master/SPY".to_string()],
            },
        );
        PortfolioRiskSnapshotRequest {
            snapshot_id: "snapshot-1".to_string(),
            scope: PortfolioRiskScope {
                kind: PortfolioRiskScopeKind::Strategy,
                id: "strategy-1".to_string(),
            },
            positions: vec![PositionEvidence {
                position_id: "position-1".to_string(),
                symbol: "SPY".to_string(),
                instrument_id: "SPY".to_string(),
                quantity: 10.0,
                notional: 5_000.0,
                provider_ref: Some("provider://paper/account-1".to_string()),
                strategy_id: Some("strategy-1".to_string()),
                approved_evidence_ref: "journal://evidence/position-1".to_string(),
                exposure_inputs: BTreeMap::new(),
            }],
            instruments,
            grouping_dimensions: vec![GroupingDimension {
                id: "sector".to_string(),
                label: "Sector".to_string(),
                metadata_field: "sector".to_string(),
                source_ref: "fixture://instrument-master".to_string(),
                required: true,
            }],
            exposure_formulas: vec![ExposureFormula {
                id: "notional_share".to_string(),
                label: "Notional Share".to_string(),
                expression: "position_notional / portfolio_notional".to_string(),
                output_unit: "share".to_string(),
                required_inputs: vec![
                    "position_notional".to_string(),
                    "portfolio_notional".to_string(),
                ],
            }],
            concentration_thresholds: vec![ConcentrationThreshold {
                id: "sector-share".to_string(),
                metric: "notional_share".to_string(),
                group_by: "sector".to_string(),
                max_share: Some(0.5),
                max_notional: None,
                severity: "warning".to_string(),
                user_defined: true,
            }],
            stress_scenarios: vec![StressScenario {
                id: "scenario-rate-up".to_string(),
                label: "Rates Up".to_string(),
                shocks: BTreeMap::from([("rate_parallel_shift_bp".to_string(), 100.0)]),
                source_ref: "scenario://rates-up".to_string(),
            }],
            stale_data_policy: StaleDataPolicy {
                market_data_as_of_epoch: 2_000,
                evaluation_epoch: 2_300,
                max_market_data_age_seconds: 600,
                fail_closed: true,
            },
            model_provenance: ModelProvenance {
                model_id: "tradeassembly-portfolio-risk".to_string(),
                model_version: "v1".to_string(),
                source_refs: vec!["docs/reference/contracts/portfolio-risk-contract.md".to_string()],
            },
            replay_refs: vec!["replay://portfolio-risk/snapshot-1".to_string()],
            export_refs: vec!["file://exports/snapshot-1.json".to_string()],
        }
    }
}
