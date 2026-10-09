// Copyright (c) 2026 OptionLab LLC. All rights reserved.

#[path = "strategy_spec/mod.rs"]
mod strategy_spec;

pub use strategy_spec::*;

use serde_json::{json, Value};
use std::fs;
use std::path::Path;

pub fn validate_strategy_spec_report(payload: &Value) -> ValidationReport {
    validate(payload)
}

pub fn validate_strategy_spec_payload(payload: Value) -> (bool, Vec<String>) {
    let report = validate_strategy_spec_report(&payload);
    (
        report.valid,
        report
            .diagnostics
            .iter()
            .map(Diagnostic::display_line)
            .collect(),
    )
}

pub fn strategy_spec_runtime_boundary_violations(payload: &Value) -> Vec<String> {
    runtime_boundary_diagnostics(payload)
        .iter()
        .map(Diagnostic::display_line)
        .collect()
}

pub fn strategy_spec_schema() -> Value {
    serde_json::from_str(include_str!(
        "../../docs/reference/contracts/strategy-spec.schema.json"
    ))
    .expect("checked-in StrategySpec schema must be valid JSON")
}

pub fn generated_strategy_spec_schema() -> Value {
    serde_json::to_value(schemars::schema_for!(StrategySpec))
        .expect("StrategySpec schema must serialize")
}

pub fn validate_cli_file(path: impl AsRef<Path>) -> Result<String, String> {
    let bytes = fs::read(path.as_ref()).map_err(|error| error.to_string())?;
    let report = validate_bytes(&bytes);
    if report.valid {
        Ok(format!(
            "VALID {}\n",
            report.spec_hash.as_deref().unwrap_or("sha256:unavailable")
        ))
    } else {
        Err(report
            .diagnostics
            .iter()
            .map(Diagnostic::display_line)
            .collect::<Vec<_>>()
            .join("\n"))
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct StrategySpecMigration {
    pub payload: Value,
    pub changed: bool,
    pub events: Vec<&'static str>,
    pub review: MigrationResult,
}

pub fn migrate_strategy_spec_payload(payload: &Value) -> StrategySpecMigration {
    let review = migrate_value(payload);
    let proposed = review.proposed_v3_draft.clone();
    StrategySpecMigration {
        payload: proposed.clone().unwrap_or_else(|| payload.clone()),
        changed: proposed.as_ref().is_some_and(|value| value != payload),
        events: if proposed.as_ref().is_some_and(|value| value != payload) {
            vec!["strategy.spec.migration_proposed"]
        } else {
            Vec::new()
        },
        review,
    }
}

pub fn normalize_strategy_market_requirements_payload(payload: &Value) -> Value {
    let mut normalized = payload.clone();
    normalize_provable_aliases(&mut normalized);
    normalized
}

pub fn validate_market_requirements(payload: &Value) -> Result<Value, String> {
    let normalized = normalize_strategy_market_requirements_payload(payload);
    let report = validate_strategy_spec_report(&normalized);
    if report.valid {
        Ok(normalized)
    } else {
        Err(report
            .diagnostics
            .iter()
            .map(Diagnostic::display_line)
            .collect::<Vec<_>>()
            .join("\n"))
    }
}

pub fn btc_exit_demo_spec_payload() -> Value {
    fixture(include_str!(
        "../../examples/strategy-spec/v3/valid/crypto-spot-24x7.json"
    ))
}

pub fn strategy_template_spec_payload(template: &str) -> Value {
    let mut payload = if template == "option-spread" {
        fixture(include_str!(
            "../../examples/strategy-spec/v3/valid/multi-leg-option-spread.json"
        ))
    } else {
        fixture(include_str!(
            "../../examples/strategy-spec/v3/valid/static-equity.json"
        ))
    };
    payload["strategy_id"] = json!(format!("template_{}", template.replace('-', "_")));
    payload["name"] = json!(if template == "option-spread" {
        "Option spread"
    } else {
        "Blank strategy"
    });
    payload
}

fn fixture(source: &str) -> Value {
    serde_json::from_str(source).expect("checked-in StrategySpec fixture must be valid JSON")
}

fn normalize_provable_aliases(value: &mut Value) {
    match value {
        Value::Object(map) => {
            for (key, child) in map.iter_mut() {
                if key == "asset_class" && child.as_str() == Some("options") {
                    *child = json!("option");
                }
                if key == "instrument_family" && child.as_str() == Some("equity_option") {
                    *child = json!("option_contract");
                }
                normalize_provable_aliases(child);
            }
        }
        Value::Array(items) => {
            for child in items {
                normalize_provable_aliases(child);
            }
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn checked_in_v3_fixture_is_publishable() {
        let payload = strategy_template_spec_payload("blank");
        let report = validate_strategy_spec_report(&payload);
        assert!(
            report.valid,
            "{}",
            report
                .diagnostics
                .iter()
                .map(Diagnostic::display_line)
                .collect::<Vec<_>>()
                .join("\n")
        );
        assert_eq!(report.source_family.as_deref(), Some("strategy_spec_v3"));
        assert!(report
            .spec_hash
            .as_deref()
            .is_some_and(|hash| hash.starts_with("sha256:")));
        let bars = &payload["capability_requirements"]["required"][0];
        assert_eq!(bars["capability"], "market_data.bars.read@1");
        assert_eq!(bars["constraints"]["deterministic"], false);
        assert_eq!(bars["constraints"]["replayable"], false);
        assert!(!bars["required_for"]
            .as_array()
            .expect("required_for array")
            .iter()
            .any(|mode| mode == "backtest"));
        assert_eq!(
            payload["capability_requirements"]["required"][1]["dependency_refs"],
            json!([])
        );
    }

    #[test]
    fn checked_in_schema_matches_closed_rust_model() {
        assert_eq!(strategy_spec_schema(), generated_strategy_spec_schema());
    }

    #[test]
    fn legacy_facade_does_not_convert_v2_into_publishable_payload() {
        let v2 = json!({
            "spec_version": "2.0",
            "market_requirements": {},
            "capability_requirements": {"required": []},
            "stages": {"universe": {}}
        });
        let migration = migrate_strategy_spec_payload(&v2);
        assert_eq!(migration.payload, v2);
        assert!(!migration.changed);
        assert!(!validate_strategy_spec_report(&migration.payload).valid);
    }
}
