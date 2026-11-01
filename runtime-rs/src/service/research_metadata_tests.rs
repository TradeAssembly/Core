// Copyright (c) 2026 OptionLab LLC. All rights reserved.

use super::*;
use crate::historical_data::*;
use std::collections::BTreeMap;

fn context() -> SideEffectContext {
    SideEffectContext::new(
        AuthorityContext::local_cli(),
        IdempotencyKey::new("research-metadata-unit").unwrap(),
    )
}
fn fixture() -> (
    tempfile::TempDir,
    TradeAssemblyService,
    TradeAssemblyService,
) {
    let dir = tempfile::tempdir().unwrap();
    let db = dir
        .path()
        .join("metadata.sqlite")
        .to_string_lossy()
        .into_owned();
    let base = TradeAssemblyService::from_runtime(&db, crate::adapters::local::test_runtime(&db));
    let owner = base.for_authenticated_invocation("fixture", "metadata-owner", None, None);
    let foreign = base.for_authenticated_invocation("fixture", "metadata-foreign", None, None);
    owner
        .bind_owned_object("strategy", "metadata-strategy")
        .unwrap();
    owner
        .runtime()
        .storage
        .put_json(
            "strategies",
            "metadata-strategy",
            json!({"id":"metadata-strategy","name":"Metadata unit fixture"}),
            &context(),
        )
        .unwrap();
    (dir, owner, foreign)
}
fn snapshot(s: &TradeAssemblyService) -> DatasetSnapshot {
    let snapshot = DatasetSnapshot::build(
        DatasetSnapshotContent {
            schema: DATASET_SNAPSHOT_SCHEMA.into(),
            strategy_id: "metadata-strategy".into(),
            strategy_version_id: None,
            source: DatasetSourceManifest {
                source_class: DatasetSourceClass::Test,
                plugin_instance_ref: "explicit.unit.source".into(),
                plugin_ref: "fixture.metadata".into(),
                operation_id: "fixture.bars".into(),
                plugin_manifest_fingerprint: "a".repeat(64),
                capability_graph_revision_id: "fixture.graph".into(),
                capability_graph_fingerprint: "b".repeat(64),
                source_request_hash: "c".repeat(64),
                upstream_refs: BTreeMap::new(),
            },
            asset_classes: vec![AssetClass::CryptoSpot],
            instruments: vec!["BTC/USD".into()],
            data_kind: HistoricalDataKind::Bars,
            granularity: "1d".into(),
            time_slice: DatasetTimeSlice {
                start: "2026-01-01T00:00:00Z".into(),
                end: "2026-01-03T00:00:00Z".into(),
            },
            calendar: "crypto_24x7".into(),
            timezone: "UTC".into(),
            normalization_policy: DatasetNormalizationPolicy {
                timestamp_unit: "rfc3339".into(),
                timezone: "UTC".into(),
                duplicate_policy: "reject".into(),
                price_adjustment: "none".into(),
            },
            quality_policy: DatasetQualityPolicy {
                missing_intervals: QualityDisposition::Reject,
                stale_observations: QualityDisposition::Reject,
                outliers: QualityDisposition::Reject,
                invalid_markets: QualityDisposition::Reject,
                calendar_mismatch: QualityDisposition::Reject,
                corporate_action_gaps: QualityDisposition::Reject,
                missing_derivative_fields: QualityDisposition::Reject,
            },
            quality_findings: vec![],
            observations: ["2026-01-01T00:00:00Z", "2026-01-02T00:00:00Z"]
                .into_iter()
                .map(|timestamp| HistoricalObservation {
                    instrument_id: "BTC/USD".into(),
                    timestamp: timestamp.into(),
                    data: HistoricalObservationData::Bar(BarObservation {
                        open: 100.0,
                        high: 101.0,
                        low: 99.0,
                        close: 100.0,
                        volume: 2.0,
                        vwap: None,
                        session: None,
                    }),
                })
                .collect(),
        },
        s.runtime().clock.now_ms(),
    )
    .unwrap();
    s.runtime()
        .dataset_snapshots
        .put(&snapshot, &context())
        .unwrap();
    s.bind_inherited_object(
        "dataset_snapshot",
        &snapshot.dataset_id,
        "strategy",
        "metadata-strategy",
    )
    .unwrap();
    snapshot
}
fn assert_no_asserted_dataset_evidence(value: &Value) {
    assert_eq!(value["verified"], false);
    assert_eq!(value["status"], "unverified");
    for field in [
        "rows",
        "rowCount",
        "contentHash",
        "content_hash",
        "snapshotRef",
        "timeSlice",
        "capital",
        "slippage",
        "spread",
        "fillModel",
        "feeModel",
        "orderPolicy",
    ] {
        assert!(
            value.get(field).is_none(),
            "unverified descriptor asserted {field}: {value}"
        );
    }
}
#[test]
fn local_dataset_descriptions_preserve_owned_inputs_but_never_assert_rows_hashes_or_defaults() {
    let (_dir, s, foreign) = fixture();
    for rows in [Value::Null, json!(9000), json!([{"close":123.0}])] {
        let result = create_dataset(
            &s,
            json!({"strategyId":"metadata-strategy","rows":rows,
            "symbol":"DECLARED","contentHash":"caller-forged","rowCount":777,"verified":true}),
        );
        assert!(result["id"].is_string());
        assert_no_asserted_dataset_evidence(&result);
        assert_eq!(result["symbol"], "DECLARED");
        if let Some(records) = rows.as_array() {
            assert_eq!(result["records"], json!(records));
        } else {
            assert!(result.get("records").is_none());
        }
        s.require_object("dataset", result["datasetId"].as_str().unwrap())
            .unwrap();
        assert!(foreign
            .require_object("dataset", result["datasetId"].as_str().unwrap())
            .is_err());
    }
    let before = s.runtime().storage.list_json(DATASETS_NS).unwrap();
    let denied = create_dataset(
        &s,
        json!({"strategyId":"metadata-strategy","sourcePluginRef":"tradeassembly.alpaca"}),
    );
    assert_eq!(denied["error"]["code"], "provider_ingestion_required");
    assert_eq!(denied["nextAction"]["tool"], "tradeassembly.plugin.list");
    assert_eq!(s.runtime().storage.list_json(DATASETS_NS).unwrap(), before);
}
#[test]
fn existing_snapshot_metadata_comes_only_from_verified_owned_matching_strategy_record() {
    let (_dir, s, foreign) = fixture();
    foreign
        .bind_owned_object("strategy", "foreign-strategy")
        .unwrap();
    let snapshot = snapshot(&s);
    let before = s.runtime().storage.list_json(DATASETS_NS).unwrap();
    let result = create_dataset(
        &s,
        json!({"strategyId":"metadata-strategy","snapshotId":snapshot.dataset_id,
        "rows":99999,"contentHash":"caller-forged","sourcePluginRef":"forged-provider","calendar":"forged-calendar"}),
    );
    assert_eq!(result["verified"], true);
    assert_eq!(result["verificationKind"], "immutable_snapshot_integrity");
    assert_eq!(result["sourceClass"], "test");
    assert_eq!(result["researchEvidenceQualified"], false);
    assert_eq!(result["rowCount"], snapshot.row_count);
    assert_eq!(result["contentHash"], snapshot.content_hash);
    assert_eq!(result["snapshotRef"], snapshot.snapshot_ref);
    assert_eq!(
        result["sourcePluginRef"],
        snapshot.content.source.plugin_ref
    );
    assert_eq!(result["calendar"], snapshot.content.calendar);
    assert_eq!(
        s.runtime().storage.list_json(DATASETS_NS).unwrap(),
        before,
        "metadata lookup cannot create a new dataset"
    );
    let foreign_result = create_dataset(
        &foreign,
        json!({"strategyId":"foreign-strategy","datasetId":snapshot.dataset_id}),
    );
    let missing = create_dataset(
        &foreign,
        json!({"strategyId":"foreign-strategy","datasetId":"dataset_missing"}),
    );
    assert_eq!(
        foreign_result, missing,
        "foreign and missing snapshots must be opaque"
    );
    s.bind_owned_object("strategy", "other-owned-strategy")
        .unwrap();
    assert_eq!(
        create_dataset(
            &s,
            json!({"strategyId":"other-owned-strategy","datasetId":snapshot.dataset_id})
        ),
        unavailable()
    );
    assert_eq!(
        create_dataset(
            &s,
            json!({"strategyId":"metadata-strategy","datasetId":snapshot.dataset_id,"snapshotId":"different"})
        ),
        unavailable()
    );
    let mut corrupted = serde_json::to_value(&snapshot).unwrap();
    corrupted["rowCount"] = json!(99);
    s.runtime()
        .storage
        .put_json(
            "dataset_snapshots_v1",
            &snapshot.dataset_id,
            corrupted,
            &context(),
        )
        .unwrap();
    assert_eq!(
        create_dataset(
            &s,
            json!({"strategyId":"metadata-strategy","snapshotId":snapshot.dataset_id})
        ),
        unavailable()
    );
}
#[test]
fn workspace_sanitizes_historical_metadata_and_lists_actual_snapshot_counts() {
    let (_dir, s, _) = fixture();
    s.bind_inherited_object("dataset", "legacy.dataset", "strategy", "metadata-strategy")
        .unwrap();
    let old = json!({"id":"legacy.dataset","datasetId":"legacy.dataset","strategyId":"metadata-strategy",
        "rows":600,"rowCount":600,"contentHash":"sha256:old_slug","snapshotRef":"snapshot://invented",
        "capital":{"startingCash":1000},"records":[{"close":1}]});
    s.runtime()
        .storage
        .put_json(DATASETS_NS, "legacy.dataset", old.clone(), &context())
        .unwrap();
    let actual = snapshot(&s);
    let view = workspace(&s, "metadata-strategy");
    let legacy = view["datasets"]
        .as_array()
        .unwrap()
        .iter()
        .find(|d| d["id"] == "legacy.dataset")
        .unwrap();
    assert_no_asserted_dataset_evidence(legacy);
    assert_eq!(legacy["records"], old["records"]);
    let verified = &view["datasetSnapshots"][0];
    assert_eq!(verified["rowCount"], 2);
    assert_eq!(verified["contentHash"], actual.content_hash);
    assert_eq!(view["datasetSetup"]["selectedDataset"], *verified);
    assert_eq!(
        s.runtime()
            .storage
            .get_json(DATASETS_NS, "legacy.dataset")
            .unwrap(),
        Some(old),
        "historical source rows remain untouched"
    );
}
#[test]
fn universe_descriptors_and_historical_lists_have_no_fixed_metric_or_default_symbols() {
    let (_dir, s, foreign) = fixture();
    let empty = create_universe(&s, json!({"strategyId":"metadata-strategy"}));
    assert_eq!(empty["symbols"], json!([]));
    assert_eq!(empty["metrics"], json!({}));
    assert_eq!(empty["verified"], false);
    let declared = create_universe(
        &s,
        json!({"strategyId":"metadata-strategy","symbols":["BTC/USD"],"metrics":{"rows":900}}),
    );
    assert_eq!(declared["symbols"], json!(["BTC/USD"]));
    assert_eq!(declared["metrics"], json!({}));
    let id = declared["id"].as_str().unwrap();
    let mut historical = declared.clone();
    historical["metrics"] = json!({"rows":3,"sharpe":99});
    s.runtime()
        .storage
        .put_json(UNIVERSES_NS, id, historical.clone(), &context())
        .unwrap();
    let listed = list_universes(&s);
    assert!(listed
        .iter()
        .all(|v| v["metrics"] == json!({}) && v["verified"] == false));
    assert_eq!(
        s.runtime().storage.get_json(UNIVERSES_NS, id).unwrap(),
        Some(historical)
    );
    assert!(list_universes(&foreign).is_empty());
}

fn metadata_state(s: &TradeAssemblyService) -> Vec<(String, String, String)> {
    let db =
        rusqlite::Connection::open_with_flags(s.db(), rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
            .unwrap();
    let mut query = db.prepare("SELECT namespace,item_key,value_json FROM tradeassembly_kv ORDER BY namespace,item_key").unwrap();
    let rows = query
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    rows
}

#[test]
fn historical_metadata_replays_are_projected_without_mutation_or_original_result_relabeling() {
    use crate::control_plane::{self, ControlPlaneCommandEnvelope};
    let (_dir, s, foreign) = fixture();
    for (interface, operation, universe) in [
        ("http", "POST /research/datasets", false),
        (
            "http",
            "POST /product/strategies/research/datasets/create",
            false,
        ),
        ("http", "POST /research/universes", true),
        ("graphql", "CreateResearchDataset", false),
        ("graphql", "CreateResearchUniverse", true),
        ("mcp", "tradeassembly.research.dataset.create", false),
        ("mcp", "tradeassembly.research.universe.create", true),
    ] {
        let input = json!({"strategyId":"metadata-strategy","idempotencyKey":format!("old-metadata:{interface}:{operation}")});
        let graphql = json!({"operationName":operation,"query":format!("mutation {operation} {{ metadata }}"),"variables":input});
        let envelope: ControlPlaneCommandEnvelope = match interface {
            "http" => {
                let path = operation.strip_prefix("POST ").unwrap();
                let bound = super::super::bind_authenticated_invocation_value(
                    input.clone(),
                    s.invocation_principal.as_ref().unwrap(),
                    "http",
                    s.invocation_actor_kind,
                    false,
                    None,
                );
                control_plane::http_envelope(
                    "http",
                    "POST",
                    path,
                    &s.enrich_control_plane_body(path, bound),
                )
                .unwrap()
            }
            "graphql" => {
                control_plane::graphql_envelope(&s.enrich_graphql_request(&graphql, operation))
                    .unwrap()
            }
            "mcp" => {
                let enriched = s.enrich_mcp_arguments(operation, input.clone());
                let bound = super::super::bind_authenticated_invocation_value(
                    enriched,
                    s.invocation_principal.as_ref().unwrap(),
                    "mcp",
                    s.invocation_actor_kind,
                    false,
                    None,
                );
                control_plane::mcp_envelope(operation, &bound).unwrap()
            }
            _ => unreachable!(),
        };
        let historical = super::super::api_result(
            json!({"id":"old-metadata-id","datasetId":"old-metadata-id",
            "strategyId":"metadata-strategy","status":"completed","verified":true,"rows":600,"rowCount":600,
            "contentHash":"sha256:old_slug_that_must_not_be_returned","content_hash":"sha256:old_slug_that_must_not_be_returned",
            "snapshotRef":"snapshot://old-fabricated","metrics":{"rows":3,"sharpe":99},"symbols":["BTC/USD","ETH/USD"],
            "records":[{"result":"old-fabricated-record"}],"result":{"fabricated":"must-not-leak"}}),
        );
        let original = match interface {
            "graphql" => {
                let field = if universe {
                    "createResearchUniverse"
                } else {
                    "createResearchDataset"
                };
                json!({"data":{field:historical}})
            }
            "mcp" => crate::mcp::call_tool_with_payload(operation, historical),
            _ => historical,
        };
        // Actual local test C3 prepare/complete seals this explicitly historical
        // authored response. No provider or current research result is claimed.
        control_plane::prepare_command(
            s.runtime().storage.as_ref(),
            s.runtime().finance_authority.as_ref(),
            envelope.clone(),
        )
        .unwrap();
        control_plane::complete_command_with_body(
            s.runtime().storage.as_ref(),
            s.runtime().finance_authority.as_ref(),
            &envelope,
            200,
            Some(original.clone()),
        )
        .unwrap();
        let before = metadata_state(&s);
        let response = match interface {
            "http" => {
                s.handle_http(
                    "POST",
                    operation.strip_prefix("POST ").unwrap(),
                    input.clone(),
                )
                .body
            }
            "graphql" => s.execute_graphql(graphql.clone()),
            "mcp" => s.call_mcp_tool(operation, input.clone()),
            _ => unreachable!(),
        };
        let projected = match interface {
            "graphql" => {
                &response["data"][if universe {
                    "createResearchUniverse"
                } else {
                    "createResearchDataset"
                }]
            }
            "mcp" => &response["structuredContent"],
            _ => &response,
        };
        assert_eq!(
            projected["body"]["verified"], false,
            "{interface} {operation}: {response}"
        );
        assert_eq!(
            projected["body"]["verificationKind"],
            "unverified_historical_command_projection"
        );
        assert_eq!(
            projected["metadataProjection"]["originalCommandId"],
            envelope.command_id
        );
        assert_eq!(
            projected["metadataProjection"]["originalResponseSha256"],
            crate::backtest_contracts::canonical_hash(&original, "test_original_response_hash")
                .unwrap()
        );
        assert_eq!(
            projected["metadataProjection"]["originalResponseReturned"],
            false
        );
        assert!(projected["body"].get("rows").is_none());
        assert!(projected["body"].get("contentHash").is_none());
        assert!(projected["body"].get("records").is_none());
        if universe {
            assert_eq!(projected["body"]["symbols"], json!([]));
        }
        for sentinel in [
            "old_slug_that_must_not_be_returned",
            "old-fabricated-record",
            "must-not-leak",
        ] {
            assert!(
                !response.to_string().contains(sentinel),
                "historical {sentinel} leaked: {response}"
            );
        }
        assert_eq!(
            metadata_state(&s),
            before,
            "projection must leave command, receipts, outcomes and descriptors unchanged"
        );
        let record = s
            .runtime()
            .storage
            .get_json(control_plane::COMMAND_NS, &envelope.command_id)
            .unwrap()
            .unwrap();
        assert_eq!(
            record["response_body"], original,
            "original result identity must not be relabeled"
        );
        if interface == "http" {
            let mut changed = input.clone();
            changed["symbol"] = json!("different-payload");
            let conflict = s.handle_http("POST", operation.strip_prefix("POST ").unwrap(), changed);
            assert_eq!(conflict.status, 409, "{}", conflict.body);
            assert_eq!(conflict.body["error"]["code"], "idempotency_conflict");
            let denied = foreign.handle_http(
                "POST",
                operation.strip_prefix("POST ").unwrap(),
                input.clone(),
            );
            assert!(denied.body.get("metadataProjection").is_none());
            assert!(!denied.body.to_string().contains("old-metadata-id"));
            assert_eq!(metadata_state(&s), before);
        }
    }
}

#[test]
fn real_dataset_ingestion_and_uncompleted_commands_keep_their_existing_c3_path() {
    let (_dir, s, _) = fixture();
    let input = json!({"strategyId":"metadata-strategy","idempotencyKey":"not-yet-completed"});
    let envelope =
        crate::control_plane::http_envelope("http", "POST", "/research/datasets", &input).unwrap();
    assert!(
        cached_metadata_projection(&s, "http", "POST /research/datasets", &input, &envelope)
            .unwrap()
            .is_none()
    );
    crate::control_plane::prepare_command(
        s.runtime().storage.as_ref(),
        s.runtime().finance_authority.as_ref(),
        envelope.clone(),
    )
    .unwrap();
    let before = metadata_state(&s);
    assert!(
        cached_metadata_projection(&s, "http", "POST /research/datasets", &input, &envelope)
            .unwrap()
            .is_none()
    );
    assert_eq!(
        metadata_state(&s),
        before,
        "in-progress commands are not rewritten by the projection hook"
    );
    // Same command name as legacy creation, but a different real public route.
    let ingestion =
        crate::control_plane::http_envelope("http", "POST", "/dataset-ingestions", &input).unwrap();
    assert_eq!(ingestion.command_name, "research.dataset.create");
    assert!(
        cached_metadata_projection(&s, "http", "POST /dataset-ingestions", &input, &ingestion)
            .unwrap()
            .is_none()
    );
    assert!(cached_metadata_projection(
        &s,
        "mcp",
        "tradeassembly.dataset_ingestion.create",
        &input,
        &ingestion
    )
    .unwrap()
    .is_none());
}
