//! Trusted local receipt consumption; never accepts a caller-supplied price.
use super::{read, required_value, BrokerCurrentState, BrokerSubmissionDependencies};
use crate::ports::PluginOperationResponse;
use serde_json::{json, Value};

/// Technical freshness policy, not a trading recommendation. Caller arguments
/// cannot extend it. Source and retrieval timestamps must both meet this bound.
const MAX_QUOTE_AGE_MS: i64 = 30_000;

pub(super) fn load_price_evidence(
    deps: &BrokerSubmissionDependencies,
    state: &BrokerCurrentState,
    symbol: &str,
    receipt_id: &str,
) -> Result<Value, String> {
    if receipt_id.is_empty() || receipt_id.len() > 256 {
        return Err("price_receipt_reference_invalid".into());
    }
    let request = read(
        deps.storage.as_ref(),
        "plugin_operation_requests",
        receipt_id,
    )?;
    let binding = &request["binding"];
    if binding["schemaVersion"] != "tradeassembly.plugin_request_binding.v1"
        || binding["operationId"] != "marketdata.quote.read"
        || binding["symbolHash"] != crate::spec::canonical_hash(&json!(symbol))?
        || binding["capabilityGraphRevisionId"] != state.run["capabilityGraphRevisionId"]
        || binding["capabilityGraphFingerprint"] != state.run["capabilityGraphFingerprint"]
        || binding["mode"] != "live"
    {
        return Err("price_request_binding_invalid".into());
    }
    let selected = state.capability_revision["graph"]["nodes"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|node| node["blockers"].as_array().is_some_and(Vec::is_empty))
        .find_map(|node| {
            let selected = &node["selected"];
            ([
                "pluginInstanceRef",
                "pluginRef",
                "manifestFingerprint",
                "operationId",
                "accountRef",
            ]
            .iter()
            .all(|key| selected[*key] == binding[*key])
                && node["requirement"]["capability"] == binding["capability"])
                .then_some(selected)
        })
        .ok_or("price_selection_not_current")?;
    let instance = deps
        .plugins
        .get_instance(required_value(selected, "pluginInstanceRef")?)?
        .ok_or("price_instance_missing")?;
    if instance["enabled"] != true || instance["accountRef"] != selected["accountRef"] {
        return Err("price_instance_binding_invalid".into());
    }
    let credential_ref = required_value(&instance, "credentialRef")?.to_string();
    let credential_status = deps.credentials.status(&credential_ref);
    if !credential_status.configured {
        return Err("price_credential_unavailable".into());
    }
    let instance =
        crate::live_execution_checks::apply_current_credential_status(instance, &credential_status);
    let package = deps
        .plugin_packages
        .get(required_value(&instance, "activePackageSha256")?)?
        .ok_or("price_package_missing")?;
    let manifest = deps
        .plugins
        .get_manifest(required_value(selected, "pluginRef")?)?
        .ok_or("price_manifest_missing")?;
    if package.manifest_sha256 != manifest["manifestDigest"]
        || manifest["manifestDigest"] != selected["manifestFingerprint"]
        || instance["activePackageSha256"] != manifest["package"]["packageSha256"]
        || package.plugin_ref != selected["pluginRef"]
        || !manifest["manifest"]["operations"]
            .as_array()
            .into_iter()
            .flatten()
            .any(|op| {
                op["id"] == "marketdata.quote.read" && op["capability"] == binding["capability"]
            })
    {
        return Err("price_package_binding_invalid".into());
    }
    let prepared = read(
        deps.storage.as_ref(),
        "plugin_operation_prepared_bindings",
        receipt_id,
    )?;
    let expected = json!({
        "pluginInstanceRef": selected["pluginInstanceRef"],
        "pluginRef": selected["pluginRef"],
        "packageSha256": package.package_sha256,
        "manifestFingerprint": selected["manifestFingerprint"],
        "configurationDigest": crate::spec::canonical_hash(&instance["configuration"])?,
        "credentialRef": credential_ref,
        "credentialGeneration": crate::live_execution_checks::credential_revision(&instance),
    });
    if prepared != expected {
        return Err("price_prepared_binding_stale".into());
    }
    let receipt_value = read(
        deps.storage.as_ref(),
        "plugin_operation_receipts",
        receipt_id,
    )?;
    let now = deps.clock.trusted_now_ms()?;
    let mut evidence = validate_quote_receipt(
        &receipt_value,
        required_value(&request, "correlationId")?,
        symbol,
        now,
    )?;
    evidence["receiptId"] = json!(receipt_id);
    evidence["preparedBinding"] = prepared;
    Ok(evidence)
}

fn validate_quote_receipt(
    receipt_value: &Value,
    correlation_id: &str,
    symbol: &str,
    now: i64,
) -> Result<Value, String> {
    let receipt: PluginOperationResponse = serde_json::from_value(receipt_value.clone())
        .map_err(|_| "price_receipt_invalid".to_string())?;
    if receipt.correlation_id.is_empty()
        || receipt.correlation_id != correlation_id
        || receipt.content_hash.is_empty()
        || receipt.reconciliation_required
        || receipt.freshness_state != "fresh"
        || !fresh(receipt.observed_at_ms, now)
    {
        return Err("price_receipt_not_current".into());
    }
    let quote = &receipt.payload["quote"];
    if quote["symbol"] != symbol {
        return Err("price_instrument_mismatch".into());
    }
    let source_time = chrono::DateTime::parse_from_rfc3339(required_value(quote, "timestamp")?)
        .map_err(|_| "price_source_time_invalid".to_string())?
        .timestamp_millis();
    if !fresh(source_time, now) {
        return Err("price_source_stale".into());
    }
    let bid = crate::broker_order_intent::parse_quantity(&quote["bid"])?;
    let ask = crate::broker_order_intent::parse_quantity(&quote["ask"])?;
    if bid > ask {
        return Err("price_quote_crossed".into());
    }
    Ok(
        json!({"receiptDigest":crate::spec::canonical_hash(receipt_value)?,
        "priceMicros":ask,"sourceAtMs":source_time,"observedAtMs":receipt.observed_at_ms,
        "policy":"bound-quote-30s-v1"}),
    )
}

fn fresh(timestamp: i64, now: i64) -> bool {
    now.checked_sub(timestamp)
        .is_some_and(|age| (0..=MAX_QUOTE_AGE_MS).contains(&age))
}

#[cfg(test)]
mod tests {
    use super::{fresh, validate_quote_receipt};
    use serde_json::{json, Value};

    fn receipt() -> Value {
        json!({"correlationId":"quote-correlation","schemaRef":"schema://fixture/quote@1",
            "payload":{"quote":{"symbol":"FIXTURE","bid":"1.234567","ask":"1.234568","timestamp":"1970-01-01T00:00:01Z"}},
            "observedAtMs":1000,"sourceEventId":"fixture-source","contentHash":"fixture-content-hash",
            "freshnessState":"fresh","evidenceRefs":[],"deterministic":false,"replayable":true,
            "providerOutcomeId":null,"reconciliationRequired":false})
    }

    #[test]
    fn quote_validation_binds_exact_price_and_entire_receipt_digest() {
        let value = receipt();
        let evidence =
            validate_quote_receipt(&value, "quote-correlation", "FIXTURE", 31_000).unwrap();
        assert_eq!(evidence["priceMicros"], 1_234_568);
        assert_eq!(
            evidence["receiptDigest"],
            crate::spec::canonical_hash(&value).unwrap()
        );
    }

    #[test]
    fn rejects_wrong_identity_stale_future_ambiguous_or_invalid_prices() {
        for (pointer, replacement) in [
            ("/correlationId", json!("other")),
            ("/contentHash", json!("")),
            ("/freshnessState", json!("stale")),
            ("/reconciliationRequired", json!(true)),
            ("/observedAtMs", json!(999)),
            ("/observedAtMs", json!(31_001)),
            ("/payload/quote/symbol", json!("OTHER")),
            (
                "/payload/quote/timestamp",
                json!("1970-01-01T00:00:00.999Z"),
            ),
            (
                "/payload/quote/timestamp",
                json!("1970-01-01T00:00:31.001Z"),
            ),
            ("/payload/quote/bid", json!("2")),
            ("/payload/quote/ask", json!("0")),
            ("/payload/quote/ask", json!("1.0000001")),
            ("/payload/quote/ask", json!("9223372036854.775808")),
        ] {
            let mut value = receipt();
            *value.pointer_mut(pointer).unwrap() = replacement;
            assert!(
                validate_quote_receipt(&value, "quote-correlation", "FIXTURE", 31_000).is_err(),
                "accepted mutation {pointer}: {value}"
            );
        }
    }
    #[test]
    fn freshness_rejects_future_stale_and_overflow() {
        assert!(fresh(1_000, 31_000));
        assert!(!fresh(1_000, 31_001));
        assert!(!fresh(1_001, 1_000));
        assert!(!fresh(i64::MIN, i64::MAX));
    }
}
