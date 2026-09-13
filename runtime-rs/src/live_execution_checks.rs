//! Portable predicates shared by service and broker-boundary admission paths.
//!
//! This module deliberately has no dependency on `TradeAssemblyService`.

use crate::ports::CredentialStatus;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

pub(crate) const LIVE_ACCOUNT_OBSERVATION_MAX_AGE_MS: i64 = 120_000;

pub(crate) fn within_execution_limits(run: &Value, quantity: f64, price_micros: i64) -> bool {
    let limits = &run["riskLimits"];
    // Omitted limits do not invent a risk policy. A configured but malformed
    // limit must not silently become unlimited, however.
    if run.get("riskLimits").is_some() && !limits.is_object() {
        return false;
    }
    let limit = |key: &str| match limits.get(key) {
        None => Some(f64::INFINITY),
        Some(value) => value.as_f64().filter(|n| n.is_finite() && *n >= 0.0),
    };
    let (Some(maximum_quantity), Some(maximum_notional)) =
        (limit("max_order_quantity"), limit("max_notional"))
    else {
        return false;
    };
    let price = price_micros as f64 / 1_000_000.0;
    let notional = quantity * price;
    quantity.is_finite()
        && price_micros > 0
        && quantity > 0.0
        && quantity <= maximum_quantity
        && notional.is_finite()
        && notional <= maximum_notional
        && price.is_finite()
}

pub(crate) fn live_account_observation_matches(
    record: &Value,
    selected: &Value,
    now_ms: i64,
) -> bool {
    let Some(checked_at) = record["health"]["checkedAtMs"].as_i64() else {
        return false;
    };
    let Some(age) = now_ms.checked_sub(checked_at) else {
        return false;
    };
    let Some(account_id) = record["health"]["account"]["id"]
        .as_str()
        .filter(|id| !id.is_empty())
    else {
        return false;
    };
    let Some(instance_ref) = record["instanceRef"].as_str().filter(|id| !id.is_empty()) else {
        return false;
    };
    (0..=LIVE_ACCOUNT_OBSERVATION_MAX_AGE_MS).contains(&age)
        && record["health"]["evidenceCurrent"] == true
        && record["credentialStatus"]["configured"] == true
        && record["accountMode"] == "live"
        && record["health"]["account"]["mode"] == "live"
        && record["health"]["account"]["status"] == "ACTIVE"
        && ["tradingBlocked", "accountBlocked", "tradeSuspendedByUser"]
            .iter()
            .all(|field| record["health"]["account"][field] == false)
        && selected["pluginInstanceRef"] == record["instanceRef"]
        && selected["pluginRef"] == record["pluginRef"]
        && selected["accountRef"] == record["accountRef"]
        && record["accountRef"] == format!("account://{instance_ref}/{account_id}")
}

pub(crate) fn credential_status(status: &CredentialStatus, revision: u64) -> Value {
    json!({
        "configured": status.configured,
        "custody": status.custody,
        "redactedDisplay": status.redacted_display,
        "revisionRef": format!("credential-generation:{revision}"),
    })
}

pub(crate) fn credential_revision(record: &Value) -> u64 {
    record["credentialRevision"].as_u64().unwrap_or(0)
}

/// Apply the current non-secret credential posture and recompute evidence
/// currentness using the instance's already-persisted health state.
pub(crate) fn apply_current_credential_status(
    mut record: Value,
    status: &CredentialStatus,
) -> Value {
    record["credentialStatus"] = credential_status(status, credential_revision(&record));
    record["health"]["evidenceCurrent"] = json!(live_health_evidence_current(&record));
    record
}

pub(crate) fn live_health_evidence_binding(record: &Value) -> Value {
    json!({
        "instanceRef": record["instanceRef"],
        "pluginRef": record["pluginRef"],
        "packageSha256": record["activePackageSha256"],
        "configurationDigest": serde_json_canonicalizer::to_vec(&record["configuration"])
            .ok()
            .map(|bytes| format!("sha256:{:x}", Sha256::digest(bytes))),
        "credentialRevision": credential_revision(record),
        "accountRef": record["accountRef"],
        "accountMode": record["accountMode"],
    })
}

pub(crate) fn live_health_evidence_current(record: &Value) -> bool {
    record["enabled"] == true
        && !record
            .get("removedAtMs")
            .is_some_and(|value| !value.is_null())
        && record["credentialStatus"]["configured"] == true
        && record["health"]["state"] == "ready"
        && record["health"]["connectivityChecked"] == true
        && record["health"]["binding"].is_object()
        && record["health"]["binding"] == live_health_evidence_binding(record)
}

#[cfg(test)]
mod tests {
    use super::within_execution_limits;
    use serde_json::json;

    #[test]
    fn execution_limits_require_finite_positive_quantity_and_price() {
        let run = json!({"riskLimits": {"max_order_quantity": 10.0, "max_notional": 1000.0}});
        assert!(within_execution_limits(&run, 2.0, 100_000_000));
        assert!(!within_execution_limits(&run, f64::INFINITY, 100_000_000));
        assert!(!within_execution_limits(&run, f64::NAN, 100_000_000));
        assert!(!within_execution_limits(&run, 2.0, 0));
        assert!(!within_execution_limits(&run, 2.0, -100_000_000));
        assert!(!within_execution_limits(&run, 11.0, 100_000_000));
        assert!(!within_execution_limits(&run, 2.0, 600_000_000));
        let no_limits = json!({});
        assert!(within_execution_limits(&no_limits, 2.0, 100_000_000));
        assert!(!within_execution_limits(
            &no_limits,
            f64::INFINITY,
            100_000_000
        ));
        assert!(!within_execution_limits(&no_limits, f64::MAX, 2_000_000));
    }

    #[test]
    fn configured_invalid_limits_never_become_unlimited() {
        for invalid in [
            json!(null),
            json!("100"),
            json!(true),
            json!([]),
            json!({}),
            json!(-1),
        ] {
            for field in ["max_order_quantity", "max_notional"] {
                let mut run = json!({"riskLimits":{}});
                run["riskLimits"][field] = invalid.clone();
                assert!(!within_execution_limits(&run, 1.0, 1_000_000), "{run}");
            }
        }
        for invalid in [json!(null), json!("unlimited"), json!([])] {
            assert!(!within_execution_limits(
                &json!({"riskLimits":invalid}),
                1.0,
                1_000_000
            ));
        }
        assert!(within_execution_limits(
            &json!({"riskLimits":{}}),
            1.0,
            1_000_000
        ));
        assert!(!within_execution_limits(
            &json!({"riskLimits":{"max_notional":0}}),
            1.0,
            1_000_000
        ));
        assert!(within_execution_limits(
            &json!({"riskLimits":{"max_notional":1}}),
            1.0,
            1_000_000
        ));
    }
}
