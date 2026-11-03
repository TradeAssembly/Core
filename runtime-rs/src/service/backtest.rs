//! Disabled legacy constructed research. Canonical engine results are owned by
//! backtest_lifecycle and its integrity-checked repository, never this namespace.
use super::{strategy_id_from, TradeAssemblyService, LEGAL_BOUNDARY};
use serde_json::{json, Value};

pub(crate) const LEGACY_RESEARCH_DISABLED: &str = "legacy_constructed_research_disabled";

/// Retain the old call boundary so no caller can revive the constructed engine.
/// This performs no dataset/provider/graph acquisition and writes no result.
pub(crate) fn run_backtest(service: &TradeAssemblyService, body: Value) -> Value {
    let strategy_id = strategy_id_from(&body);
    if service.require_object("strategy", &strategy_id).is_err() {
        return json!({"id":null,"status":"not_found","error":{"code":"object_not_available"}});
    }
    json!({"schemaVersion":"tradeassembly.legacy_research_unavailable.v1","kind":"BacktestRun",
        "id":null,"strategyId":strategy_id,"status":"blocked","ok":false,
        "error":{"code":LEGACY_RESEARCH_DISABLED,"message":"Legacy constructed research is disabled. Create a canonical durable backtest with verified immutable inputs."},
        "artifacts":[],"researchEvidenceQualified":false,"nextAction":canonical_action(),"noAdvice":LEGAL_BOUNDARY})
}

/// Refuse before generic C3 response replay: old completed fixture command bodies
/// remain historical records but cannot re-enable a retired result constructor.
/// Callers perform their ordinary current invocation/object checks first.
pub(crate) fn preflight(
    service: &TradeAssemblyService,
    method: &str,
    path: &str,
    body: &Value,
) -> Option<super::ServiceResponse> {
    if !method.eq_ignore_ascii_case("POST") {
        return None;
    }
    match path {
        "/research/jobs" | "/product/strategies/research/jobs/create" => {
            Some(super::research::create_job(service, body.clone()))
        }
        "/product/strategies/research/jobs/status" => Some(super::ServiceResponse::ok(
            super::api_result(super::research::job_status(service, body.clone())),
        )),
        "/product/reports/envelope"
            if crate::report_envelope::requires_verified_report(
                body["kind"].as_str().unwrap_or("backtest"),
            ) =>
        {
            Some(super::ServiceResponse {
                status: 501,
                body: crate::report_envelope::report_envelope(
                    body["kind"].as_str().unwrap_or("backtest"),
                    &strategy_id_from(body),
                    body["backtestId"]
                        .as_str()
                        .or_else(|| body["backtest_id"].as_str())
                        .unwrap_or("backtest-local"),
                    "http://127.0.0.1:3001",
                ),
            })
        }
        _ => None,
    }
}

fn canonical_action() -> Value {
    json!({"tool":"tradeassembly.backtest.run","method":"POST","path":"/backtests",
        "requires":["immutable_strategy_version","verified_dataset_snapshot","explicit_backtest_configuration","idempotency_key"]})
}

/// Historical rows remain untouched. Return only an owned metadata tombstone,
/// never their constructed figures, purported report URI, export or replay refs.
pub(crate) fn historical_summary(record: &Value, kind: &str) -> Value {
    let mut summary = json!({"schemaVersion":"tradeassembly.legacy_research_unavailable.v1",
        "kind":kind,"status":"unavailable","error":{"code":LEGACY_RESEARCH_DISABLED},
        "historicalRecord":{"stored":true,"source":"legacy_constructed_research","engineVerified":false},
        "researchEvidenceQualified":false,"artifacts":[],"nextAction":canonical_action(),"noAdvice":LEGAL_BOUNDARY});
    for field in [
        "id",
        "strategyId",
        "strategy_id",
        "datasetId",
        "dataset_id",
        "backtestId",
        "backtest_id",
        "runId",
    ] {
        if let Some(value) = record
            .get(field)
            .and_then(Value::as_str)
            .filter(|v| !v.is_empty() && v.len() <= 256 && !v.chars().any(char::is_control))
        {
            summary[field] = json!(value);
        }
    }
    if let Some(status) = record["status"].as_str().filter(|s| {
        matches!(
            *s,
            "draft" | "queued" | "running" | "completed" | "failed" | "canceled" | "blocked"
        )
    }) {
        summary["historicalRecord"]["storedStatus"] = json!(status);
    }
    summary
}

pub(crate) fn list_artifacts(service: &TradeAssemblyService) -> Vec<Value> {
    service
        .runtime()
        .storage
        .list_json("research_artifacts")
        .unwrap_or_default()
        .into_iter()
        .filter(|(key, _)| service.object_is_visible("backtest_artifact", key))
        .map(|(_, value)| historical_summary(&value, "LegacyResearchArtifact"))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ports::{AuthorityContext, IdempotencyKey, SideEffectContext};

    fn rig() -> (
        tempfile::TempDir,
        TradeAssemblyService,
        TradeAssemblyService,
    ) {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("research.sqlite");
        let db = db.to_str().unwrap();
        let base = TradeAssemblyService::from_runtime(db, crate::adapters::local::test_runtime(db));
        let alice = base.for_authenticated_invocation("fixture", "alice", None, None);
        let bob = base.for_authenticated_invocation("fixture", "bob", None, None);
        alice
            .bind_owned_object("strategy", "owned-research")
            .unwrap();
        (dir, alice, bob)
    }
    fn ctx() -> SideEffectContext {
        SideEffectContext::new(
            AuthorityContext::local_cli(),
            IdempotencyKey::new("legacy-research-history-test").unwrap(),
        )
    }
    fn research_rows(s: &TradeAssemblyService) -> Vec<Vec<(String, Value)>> {
        [
            "backtests",
            "research_jobs",
            "research_artifacts",
            "capability_graph_revisions",
            "backtest_runs_v2",
            "backtest_results_v1",
        ]
        .iter()
        .map(|ns| s.runtime().storage.list_json(ns).unwrap())
        .collect()
    }
    #[test]
    fn legacy_research_create_is_unconditionally_blocked_before_any_result_or_acquisition() {
        let (_dir, s, foreign) = rig();
        let before = research_rows(&s);
        let request = json!({"strategyId":"owned-research","datasetId":"looks-valid", "engine":"real",
            "demo":true,"allowLegacy":true,"rows":[{"close":1}],"idempotencyKey":"must-not-create"});
        let direct = run_backtest(&s, request.clone());
        assert_eq!(direct["status"], "blocked");
        assert_eq!(direct["error"]["code"], LEGACY_RESEARCH_DISABLED);
        assert!(direct["id"].is_null());
        assert_eq!(direct["artifacts"], json!([]));
        assert!(direct.get("result").is_none() && direct.get("metrics").is_none());
        for (index, path) in ["/research/jobs", "/product/strategies/research/jobs/create"]
            .iter()
            .enumerate()
        {
            let mut body = request.clone();
            body["idempotencyKey"] = json!(format!("blocked-legacy-{index}"));
            let response = s.handle_http("POST", path, body);
            assert_eq!(response.status, 501, "{}", response.body);
            assert_eq!(response.body["ok"], false);
            assert_eq!(response.body["error"]["code"], LEGACY_RESEARCH_DISABLED);
            assert_eq!(response.body["body"]["nextAction"]["path"], "/backtests");
        }
        assert_eq!(research_rows(&s), before);
        let foreign = run_backtest(&foreign, request);
        assert_eq!(foreign["error"]["code"], "object_not_available");
        assert!(!foreign.to_string().contains("owned-research"));
    }
    #[test]
    fn historical_constructed_jobs_and_artifacts_are_owned_metadata_only_and_never_requalified() {
        let (_dir, s, foreign) = rig();
        let job = json!({"id":"old-job","strategyId":"owned-research","status":"completed","backtestId":"old-backtest",
            "metrics":{"return":987654321},"result":{"trades":["fabricated-row-marker"]},
            "artifacts":[{"uri":"artifact://do-not-reemit"}],"exportRefs":{"uri":"artifact://do-not-reemit"}});
        let artifact = json!({"id":"old-artifact","backtestId":"old-backtest","strategyId":"owned-research", "status":"completed",
            "uri":"artifact://do-not-reemit","content_hash":"purported-result-hash","result":{"trades":["fabricated-row-marker"]}});
        s.bind_inherited_object("backtest", "old-backtest", "strategy", "owned-research")
            .unwrap();
        s.bind_inherited_object("research_job", "old-job", "strategy", "owned-research")
            .unwrap();
        s.bind_inherited_object(
            "backtest_artifact",
            "old-artifact",
            "backtest",
            "old-backtest",
        )
        .unwrap();
        s.runtime()
            .storage
            .put_json("research_jobs", "old-job", job.clone(), &ctx())
            .unwrap();
        s.runtime()
            .storage
            .put_json(
                "research_artifacts",
                "old-artifact",
                artifact.clone(),
                &ctx(),
            )
            .unwrap();
        s.runtime().storage.put_json("backtests","old-backtest",json!({"id":"old-backtest","status":"completed","result":{"trades":["fabricated-row-marker"]}}),&ctx()).unwrap();
        let before = research_rows(&s);
        for path in ["/research/jobs", "/research/artifacts"] {
            let own = s.handle_http("GET", path, json!({}));
            assert_eq!(own.status, 200, "{}", own.body);
            let rows = own.body.as_array().unwrap();
            assert_eq!(rows.len(), 1);
            assert_eq!(rows[0]["status"], "unavailable");
            assert_eq!(rows[0]["historicalRecord"]["storedStatus"], "completed");
            assert_eq!(rows[0]["historicalRecord"]["engineVerified"], false);
            assert_eq!(rows[0]["researchEvidenceQualified"], false);
            let rendered = own.body.to_string();
            for marker in [
                "987654321",
                "fabricated-row-marker",
                "artifact://do-not-reemit",
                "purported-result-hash",
            ] {
                assert!(!rendered.contains(marker), "{rendered}");
            }
            let other = foreign.handle_http("GET", path, json!({}));
            assert_eq!(other.body, json!([]));
        }
        let read = s.handle_http(
            "POST",
            "/product/strategies/research/jobs/status",
            json!({"jobId":"old-job"}),
        );
        assert_eq!(read.status, 200, "{}", read.body);
        assert_eq!(read.body["body"]["job"]["status"], "unavailable");
        assert_eq!(read.body["body"]["artifacts"], json!([]));
        let other = foreign.handle_http(
            "POST",
            "/product/strategies/research/jobs/status",
            json!({"jobId":"old-job"}),
        );
        let absent = foreign.handle_http(
            "POST",
            "/product/strategies/research/jobs/status",
            json!({"jobId":"missing"}),
        );
        assert_eq!(other.body, absent.body);
        assert_eq!(research_rows(&s), before, "history must remain unchanged");
        assert_eq!(
            s.runtime()
                .storage
                .get_json("research_jobs", "old-job")
                .unwrap(),
            Some(job)
        );
        assert_eq!(
            s.runtime()
                .storage
                .get_json("research_artifacts", "old-artifact")
                .unwrap(),
            Some(artifact)
        );
        // Canonical list/get/report/export/replay use the durable repository,
        // never the old namespace merely because its ID looks like a run.
        let list = s.handle_http("GET", "/backtests", json!({}));
        assert!(!list.body.to_string().contains("old-backtest"));
        for (method, path) in [
            ("GET", "/backtests/old-backtest"),
            ("GET", "/backtests/old-backtest/report"),
            ("POST", "/backtests/old-backtest/export"),
            ("POST", "/backtests/old-backtest/replay"),
        ] {
            let response = s.handle_http(method, path, json!({}));
            assert_ne!(response.status, 200, "{path}: {}", response.body);
            assert!(!response.body.to_string().contains("fabricated-row-marker"));
        }
        for tool in [
            "tradeassembly.backtest.get",
            "tradeassembly.backtest.report",
            "tradeassembly.backtest.export",
            "tradeassembly.backtest.replay",
        ] {
            let response=s.call_mcp_tool(tool,json!({"run_id":"old-backtest","backtest_id":"old-backtest","export_kind":"reportJson","idempotency_key":format!("legacy-{tool}")}));
            assert!(!response.to_string().contains("fabricated-row-marker"));
            assert_ne!(response["structuredContent"]["status"], "completed");
        }
    }
    #[test]
    fn retired_research_never_replays_a_preexisting_completed_control_plane_body() {
        let (_dir, s, _) = rig();
        let historical = json!({"ok":true,"body":{"status":"completed","result":{"trades":["old-cached-constructed-row"]}}});
        for path in [
            "/research/jobs",
            "/product/strategies/research/jobs/create",
            "/product/reports/envelope",
        ] {
            let request = json!({"strategyId":"owned-research","kind":"backtest","backtestId":"old","idempotencyKey":format!("cached:{path}")});
            let bound = super::super::bind_authenticated_invocation_value(
                request.clone(),
                s.invocation_principal.as_ref().unwrap(),
                "http",
                s.invocation_actor_kind,
                false,
                None,
            );
            let bound = s.enrich_control_plane_body(path, bound);
            let envelope =
                crate::control_plane::http_envelope("http", "POST", path, &bound).unwrap();
            // Explicit historical control-plane record, not current engine evidence.
            let record = crate::control_plane::ControlPlaneCommandRecord {
                envelope: envelope.clone(),
                duplicate: false,
                response_status: Some(200),
                response_body: Some(historical.clone()),
            };
            let stored = serde_json::to_value(record).unwrap();
            s.runtime()
                .storage
                .put_json(
                    crate::control_plane::COMMAND_NS,
                    &envelope.command_id,
                    stored.clone(),
                    &ctx(),
                )
                .unwrap();
            let response = s.handle_http("POST", path, request);
            assert_eq!(response.status, 501, "{path}: {}", response.body);
            assert!(!response
                .body
                .to_string()
                .contains("old-cached-constructed-row"));
            assert_eq!(
                s.runtime()
                    .storage
                    .get_json(crate::control_plane::COMMAND_NS, &envelope.command_id)
                    .unwrap(),
                Some(stored),
                "refusal preserves the exact old record"
            );
        }
        let request = json!({"operationName":"QueueStrategyResearchJob","query":"mutation QueueStrategyResearchJob { queueStrategyResearchJob }","variables":{"strategyId":"owned-research","idempotencyKey":"old-graphql-job"}});
        let enriched = s.enrich_graphql_request(&request, "QueueStrategyResearchJob");
        let envelope = crate::control_plane::graphql_envelope(&enriched).unwrap();
        assert_ne!(
            envelope.side_effect_class, "read",
            "exercise actual mutation response replay"
        );
        let record = crate::control_plane::ControlPlaneCommandRecord {
            envelope: envelope.clone(),
            duplicate: false,
            response_status: Some(200),
            response_body: Some(json!({"data":{"queueStrategyResearchJob":historical}})),
        };
        let stored = serde_json::to_value(record).unwrap();
        s.runtime()
            .storage
            .put_json(
                crate::control_plane::COMMAND_NS,
                &envelope.command_id,
                stored.clone(),
                &ctx(),
            )
            .unwrap();
        let response = s.execute_graphql(request);
        assert_eq!(
            response["errors"][0]["message"], LEGACY_RESEARCH_DISABLED,
            "{response}"
        );
        assert!(!response.to_string().contains("old-cached-constructed-row"));
        assert_eq!(
            s.runtime()
                .storage
                .get_json(crate::control_plane::COMMAND_NS, &envelope.command_id)
                .unwrap(),
            Some(stored)
        );
    }
    #[test]
    fn customer_generic_report_and_workspace_do_not_emit_placeholder_research_metrics() {
        let (_dir, s, _) = rig();
        let http = s.handle_http(
            "POST",
            "/product/reports/envelope",
            json!({"kind":"backtest","strategyId":"owned-research","backtestId":"old-or-missing"}),
        );
        assert_eq!(http.status, 501, "{}", http.body);
        assert_eq!(
            http.body["error"]["code"],
            "durable_backtest_report_required"
        );
        assert_eq!(http.body["charts"], json!([]));
        let mcp=s.call_mcp_tool("tradeassembly.report.envelope",json!({"kind":"backtest","strategy_id":"owned-research","backtest_id":"old-or-missing"}));
        assert_eq!(mcp["isError"], true, "{mcp}");
        assert_eq!(
            mcp["structuredContent"]["error"]["code"],
            "durable_backtest_report_required"
        );
        assert_eq!(
            mcp["structuredContent"]["details"]["summary"]["status"],
            "unavailable"
        );
        assert_eq!(
            mcp["structuredContent"]["details"]["artifactRefs"],
            json!([])
        );
        let workspace = super::super::workspace::workspace(&s);
        assert_eq!(workspace["researchMetrics"]["status"], "unavailable");
        for metric in ["returnPct", "maxDrawdownPct", "datasetRows", "replayable"] {
            assert!(workspace["researchMetrics"].get(metric).is_none());
        }
    }
}
