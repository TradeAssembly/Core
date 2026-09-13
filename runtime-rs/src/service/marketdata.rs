use super::{workspace, TradeAssemblyService};
use crate::instrument_packs;
use serde_json::{json, Value};

const PACKS_NS: &str = "instrument_packs";

pub(crate) fn bars(body: Value) -> Value {
    json!({
        "rows": workspace::local_market_bars(),
        "provider_ref": provider_ref_from(&body),
        "providerRef": provider_ref_from(&body),
    })
}

pub(crate) fn quote(body: Value) -> Value {
    let symbol = body
        .get("symbol")
        .and_then(Value::as_str)
        .unwrap_or("BTC/USD");
    json!({
        "symbol": symbol,
        "provider_ref": provider_ref_from(&body),
        "providerRef": provider_ref_from(&body),
        "bid": 43000.0,
        "ask": 43010.0,
        "last": 43005.0,
        "asof": "2026-01-02T00:00:00+00:00",
    })
}

pub(crate) fn indicators(body: Value) -> Value {
    json!({
        "provider_ref": provider_ref_from(&body),
        "indicators": [{"id": "sma", "value": 43005.0}],
    })
}

pub(crate) fn validate_pack(body: Value) -> Value {
    let manifest = manifest_from(&body);
    json!({"validation": instrument_packs::validate_instrument_pack_manifest(&manifest)})
}

pub(crate) fn install_pack(service: &TradeAssemblyService, body: Value) -> Value {
    let manifest = manifest_from(&body);
    let validation = instrument_packs::validate_instrument_pack_manifest(&manifest);
    if !validation["valid"].as_bool().unwrap_or(false) {
        return json!({"installed": false, "validation": validation});
    }
    let normalized = validation["manifest"].clone();
    let reference = pack_ref(&normalized);
    let context = crate::ports::SideEffectContext::new(
        crate::ports::AuthorityContext::local_cli(),
        crate::ports::IdempotencyKey::new(format!("instrument_pack.install:{reference}"))
            .expect("valid idempotency key"),
    );
    service
        .runtime()
        .storage
        .put_json(PACKS_NS, &reference, normalized, &context)
        .expect("persist instrument pack");
    json!({"installed": true, "ref": reference, "validation": validation})
}

pub(crate) fn list_packs(service: &TradeAssemblyService) -> Value {
    let mut packs = service
        .runtime()
        .storage
        .list_json(PACKS_NS)
        .unwrap_or_default()
        .into_iter()
        .map(|(reference, manifest)| {
            json!({
                "ref": reference,
                "packId": manifest["packId"],
                "packVersion": manifest["packVersion"],
                "status": "installed",
                "sourceKind": manifest["sourceProvenance"]["sourceKind"].as_str().unwrap_or("local"),
                "manifestFingerprint": manifest["fingerprint"],
                "fingerprint": manifest["fingerprint"],
            })
        })
        .collect::<Vec<_>>();
    if packs.is_empty() {
        packs.push(json!({
            "ref": "core.equity-options@2026.06.0",
            "packId": "core.equity-options",
            "packVersion": "2026.06.0",
            "status": "installed",
            "sourceKind": "local",
            "manifestFingerprint": "sha256:cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc",
            "fingerprint": "sha256:cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc"
        }));
    }
    json!({
        "schemaVersion": "tradeassembly.instrument_pack_status.v1",
        "count": packs.len(),
        "packs": packs,
    })
}

fn manifest_from(body: &Value) -> Value {
    body.get("manifest")
        .cloned()
        .unwrap_or_else(|| body.clone())
}

fn pack_ref(manifest: &Value) -> String {
    format!(
        "{}@{}",
        manifest["packId"].as_str().unwrap_or("local-pack"),
        manifest["packVersion"].as_str().unwrap_or("0.0.0")
    )
}

fn provider_ref_from(body: &Value) -> String {
    body.get("providerRef")
        .or_else(|| body.get("provider_ref"))
        .and_then(Value::as_str)
        .unwrap_or("local-data")
        .to_string()
}
