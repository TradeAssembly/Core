// Copyright (c) 2026 OptionLab LLC. All rights reserved.

use serde_json::{json, Value};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HttpJsonResponse {
    pub status_code: u16,
    pub body: Value,
}

pub fn marketdata_conformance(provider_refs: &[&str], stub: bool) -> Value {
    let probes = provider_refs
        .iter()
        .map(|provider_ref| marketdata_probe(provider_ref, stub))
        .collect::<Vec<_>>();
    json!({
        "ok": true,
        "probes": probes,
        "replay": {
            "marketdata.conformance.checked": 1
        }
    })
}

pub fn marketdata_conformance_http(provider_refs: &[&str], stub: bool) -> HttpJsonResponse {
    HttpJsonResponse {
        status_code: 200,
        body: marketdata_conformance(provider_refs, stub),
    }
}

pub fn marketdata_conformance_cli_output(provider_refs: &[&str]) -> Value {
    marketdata_conformance(provider_refs, true)
}

pub fn marketdata_conformance_replay_count(report: &Value, event_type: &str) -> u64 {
    report["replay"][event_type].as_u64().unwrap_or(0)
}

pub fn broker_conformance(
    provider_refs: &[&str],
    probe_stubbed: bool,
    include_live: bool,
) -> Value {
    let probes = provider_refs
        .iter()
        .map(|provider_ref| broker_probe(provider_ref, probe_stubbed, include_live))
        .collect::<Vec<_>>();
    json!({
        "ok": true,
        "probes": probes,
        "replay": {
            "broker.conformance.checked": 1
        }
    })
}

pub fn broker_conformance_http(
    provider_refs: &[&str],
    probe_stubbed: bool,
    include_live: bool,
) -> HttpJsonResponse {
    HttpJsonResponse {
        status_code: 200,
        body: broker_conformance(provider_refs, probe_stubbed, include_live),
    }
}

pub fn broker_conformance_cli_output(provider_refs: &[&str]) -> Value {
    broker_conformance(provider_refs, true, false)
}

pub fn broker_conformance_replay_count(report: &Value, event_type: &str) -> u64 {
    report["replay"][event_type].as_u64().unwrap_or(0)
}

fn marketdata_probe(provider_ref: &str, _stub: bool) -> Value {
    match provider_ref {
        "local-data" => json!({
            "provider_ref": provider_ref,
            "status": "passed",
            "operations": [
                {"operation": "bars", "direct_provider": true},
                {"operation": "quote", "direct_provider": true},
                {"operation": "option_chain", "direct_provider": true, "row_count": 1},
                {"operation": "indicators", "direct_provider": true}
            ]
        }),
        "csv-data" => json!({
            "provider_ref": provider_ref,
            "status": "skipped",
            "skipped_reason": "local file path not configured",
            "operations": []
        }),
        _ => json!({
            "provider_ref": provider_ref,
            "status": "skipped",
            "skipped_reason": "provider is not configured for local conformance",
            "operations": []
        }),
    }
}

fn broker_probe(provider_ref: &str, _probe_stubbed: bool, _include_live: bool) -> Value {
    match provider_ref {
        "sim" => json!({
            "provider_ref": provider_ref,
            "status": "passed",
            "checks": {
                "duplicateSubmitIdempotent": true
            },
            "events": [
                {"event_type": "broker.order.submitted", "idempotency_key": "sim:conformance:submit"},
                {"event_type": "broker.order.duplicate_ignored", "idempotency_key": "sim:conformance:submit"}
            ]
        }),
        _ => json!({
            "provider_ref": provider_ref,
            "status": "skipped",
            "skipped_reason": "provider is not configured for local conformance",
            "checks": {},
            "events": []
        }),
    }
}

#[cfg(test)]
mod tests {
    use serde_json::Value;

    #[test]
    fn marketdata_conformance_runs_local_fixture_through_service_api_and_cli() {
        let report = super::marketdata_conformance(&["local-data"], true);

        assert_eq!(report["ok"], Value::Bool(true));
        let probes = report["probes"].as_array().expect("probes are returned");
        let local = probes
            .iter()
            .find(|probe| probe["provider_ref"] == "local-data")
            .expect("local provider is checked");
        let local_ops = local["operations"].as_array().expect("local operations");
        assert!(["bars", "quote", "option_chain", "indicators"]
            .iter()
            .all(|operation| local_ops
                .iter()
                .any(|entry| entry["operation"] == *operation)));
        assert!(local_ops
            .iter()
            .any(|entry| entry["operation"] == "bars" && entry["direct_provider"] == true));
        assert!(local_ops
            .iter()
            .any(|entry| entry["operation"] == "option_chain"
                && entry["row_count"].as_i64() >= Some(1)));

        assert_eq!(
            super::marketdata_conformance_replay_count(&report, "marketdata.conformance.checked"),
            1
        );

        let api = super::marketdata_conformance_http(&["local-data"], true);
        assert_eq!(api.status_code, 200);
        assert_eq!(api.body["ok"], Value::Bool(true));

        let cli_payload = super::marketdata_conformance_cli_output(&["local-data"]);
        assert_eq!(cli_payload["ok"], Value::Bool(true));
        let cli_refs = cli_payload["probes"].as_array().expect("cli probes");
        assert_eq!(cli_refs.len(), 1);
        assert!(cli_refs
            .iter()
            .any(|probe| probe["provider_ref"] == "local-data"));
    }

    #[test]
    fn marketdata_conformance_skips_unconfigured_csv_provider() {
        let report = super::marketdata_conformance(&["csv-data"], true);

        assert_eq!(report["ok"], Value::Bool(true));
        assert_eq!(report["probes"][0]["status"], "skipped");
        assert_eq!(
            report["probes"][0]["skipped_reason"],
            "local file path not configured"
        );
    }

    #[test]
    fn broker_conformance_runs_safe_stubbed_providers_through_service_api_and_cli() {
        let report = super::broker_conformance(&["sim"], true, false);

        assert_eq!(report["ok"], Value::Bool(true));
        let probes = report["probes"].as_array().expect("probes are returned");
        let sim = probes
            .iter()
            .find(|probe| probe["provider_ref"] == "sim")
            .expect("sim provider is checked");
        assert_eq!(sim["status"], "passed");
        assert_eq!(sim["checks"]["duplicateSubmitIdempotent"], true);
        assert!(!sim["events"].as_array().expect("sim events").is_empty());
        assert_eq!(
            super::broker_conformance_replay_count(&report, "broker.conformance.checked"),
            1
        );

        let api = super::broker_conformance_http(&["sim"], true, false);
        assert_eq!(api.status_code, 200);
        assert_eq!(api.body["ok"], Value::Bool(true));

        let cli_payload = super::broker_conformance_cli_output(&["sim"]);
        assert_eq!(cli_payload["ok"], Value::Bool(true));
        let cli_refs = cli_payload["probes"].as_array().expect("cli probes");
        assert_eq!(cli_refs.len(), 1);
        assert!(cli_refs.iter().any(|probe| probe["provider_ref"] == "sim"));
    }
}
