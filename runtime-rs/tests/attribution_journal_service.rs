// Copyright (c) 2026 OptionLab LLC. All rights reserved.

use serde_json::{json, Value};
use tempfile::TempDir;
use tradeassembly_runtime::service::TradeAssemblyService;

#[test]
fn attribution_journal_service_computes_deterministic_local_analytics_and_evidence() {
    let database = test_db("happy");
    let service = TradeAssemblyService::test_local(database.path.clone());
    let request = sample_request();

    let first = service.attribution_journal_analysis(request.clone());
    let second = service.attribution_journal_analysis(request);

    assert_eq!(first["ok"], json!(true));
    assert_eq!(
        first["schemaVersion"],
        json!("tradeassembly.attribution_journal.service.v1")
    );
    assert_eq!(first["kind"], json!("attribution_journal"));
    assert_eq!(first["summary"]["closedCount"], json!(2));
    assert_eq!(first["summary"]["wins"], json!(1));
    assert_eq!(first["summary"]["losses"], json!(1));
    assert_eq!(first["summary"]["netPnl"], json!(65.0));
    assert_eq!(first["summary"]["skipRejectCount"], json!(2));
    assert_eq!(first["summary"]["warningCount"], json!(0));
    assert_eq!(
        first["analytics"]["winLossSummary"],
        json!({"totalClosed": 2, "wins": 1, "losses": 1, "flat": 0})
    );
    assert_eq!(first["analytics"]["pnlInputs"].as_array().unwrap().len(), 2);
    assert_eq!(
        first["analytics"]["holdingPeriodSummaries"][0]["bucket"],
        json!("0-1d")
    );
    assert_eq!(
        first["analytics"]["skipRejectSummaries"][0]["reasonKind"],
        json!("skip")
    );
    assert_eq!(
        first["exportRefs"]["csv"]["uri"],
        json!("tradeassembly://artifact/attribution-journal/review-001/journal.csv")
    );
    assert_eq!(
        first["journalEvidence"]["eventType"],
        json!("attribution_journal.review.completed")
    );
    assert_eq!(first["sideEffects"]["strategyLogicChanged"], json!(false));
    assert_eq!(first["sideEffects"]["credentialsTouched"], json!(false));
    assert_eq!(first["sideEffects"]["brokerStateChanged"], json!(false));
    assert_eq!(first["sideEffects"]["activationChanged"], json!(false));
    assert_eq!(first["contractHash"], second["contractHash"]);
    assert_eq!(first["exportRefs"], second["exportRefs"]);
    assert_eq!(first["replayReview"], second["replayReview"]);
    assert!(service.latest_attribution_journal().is_some());
}

#[test]
fn attribution_journal_service_fails_closed_for_hash_mismatch_and_incomplete_refs_without_secret_echo(
) {
    let database = test_db("warnings");
    let service = TradeAssemblyService::test_local(database.path.clone());
    let mut request = sample_request();
    request["reviewId"] = json!("review-warning");
    request["artifactRefs"][0]["observedHash"] = json!("sha256:different");
    request["dataSnapshots"][0]["verifiedDataSource"] = json!(false);
    request["dataSnapshots"][0]["asOfUnixMs"] = json!(1_786_000_000_000u64);
    request["userNotes"][0]["explicitlySaved"] = json!(false);
    request["metadata"] = json!({"api_secret": "DO_NOT_ECHO"});

    let result = service.attribution_journal_analysis(request);
    let rendered = serde_json::to_string(&result).expect("serialize attribution result");

    assert_eq!(result["ok"], json!(false));
    assert!(rendered.contains("hash_mismatch"));
    assert!(rendered.contains("unverified_data_source"));
    assert!(rendered.contains("stale_artifact"));
    assert!(rendered.contains("incomplete_journal_state"));
    assert!(rendered.contains("raw_secret_rejected"));
    assert!(!rendered.contains("DO_NOT_ECHO"));
    assert!(result["summary"]["warningCount"].as_u64().unwrap() > 0);
}

#[test]
fn attribution_journal_service_output_contains_no_advice_copy() {
    let database = test_db("no-advice");
    let service = TradeAssemblyService::test_local(database.path.clone());
    let rendered = serde_json::to_string(&service.attribution_journal_analysis(sample_request()))
        .expect("serialize attribution result")
        .to_ascii_lowercase();

    for forbidden in [
        "should buy",
        "should sell",
        "should enter",
        "should exit",
        "increase size",
        "reduce size",
        "must trade",
        "recommend trade",
        "predicts profit",
        "forecast profit",
        "guaranteed",
    ] {
        assert!(!rendered.contains(forbidden), "{forbidden}");
    }
}

fn sample_request() -> Value {
    json!({
        "reviewId": "review-001",
        "strategyId": "strategy-1",
        "generatedAtUnixMs": 1_785_000_000_000u64,
        "strategyVersion": {
            "refId": "strategy-v1",
            "refUri": "tradeassembly://strategy/strategy-1/versions/v1",
            "contentHash": "sha256:strategyv1",
            "asOfUnixMs": 1_785_000_000_000u64
        },
        "selectorRuns": [{
            "refId": "selector-001",
            "refUri": "tradeassembly://selector/selector-001",
            "contentHash": "sha256:selector001",
            "asOfUnixMs": 1_785_000_000_000u64
        }],
        "backtestReports": [{
            "refId": "backtest-001",
            "refUri": "tradeassembly://report/backtest-001",
            "contentHash": "sha256:backtest001",
            "asOfUnixMs": 1_785_000_000_000u64
        }],
        "ledgerCheckpoints": [{
            "refId": "ledger-001",
            "refUri": "tradeassembly://ledger/checkpoint-001",
            "contentHash": "sha256:ledger001",
            "asOfUnixMs": 1_785_000_000_000u64
        }],
        "valuationSnapshots": [{
            "refId": "valuation-001",
            "refUri": "tradeassembly://valuation/valuation-001",
            "contentHash": "sha256:valuation001",
            "asOfUnixMs": 1_785_000_000_000u64
        }],
        "fillQualityReports": [{
            "refId": "fill-quality-001",
            "refUri": "tradeassembly://report/fill-quality-001",
            "contentHash": "sha256:fillquality001",
            "asOfUnixMs": 1_785_000_000_000u64
        }],
        "artifactRefs": [{
            "refId": "report-001",
            "refUri": "tradeassembly://report/review-001",
            "contentHash": "sha256:report001",
            "observedHash": "sha256:report001",
            "asOfUnixMs": 1_785_000_000_000u64
        }],
        "userNotes": [{
            "noteId": "note-001",
            "noteRef": "tradeassembly://notes/note-001",
            "contentHash": "sha256:note001",
            "explicitlySaved": true,
            "createdAtUnixMs": 1_785_000_000_000u64
        }],
        "executionEvents": [
            {
                "eventId": "exec-win",
                "refUri": "tradeassembly://execution/exec-win",
                "contentHash": "sha256:execwin",
                "underlying": "SPY",
                "instrumentId": "occ:SPY260717C00550000",
                "legId": "long-call",
                "entryAtUnixMs": 1_785_000_000_000u64,
                "exitAtUnixMs": 1_785_036_000_000u64,
                "grossPnl": 100.0,
                "fees": 2.0,
                "netPnl": 98.0,
                "quantity": 1.0,
                "outcome": "win",
                "fillQualityRef": "fill-quality-001"
            },
            {
                "eventId": "exec-loss",
                "refUri": "tradeassembly://execution/exec-loss",
                "contentHash": "sha256:execloss",
                "underlying": "SPY",
                "instrumentId": "occ:SPY260717P00530000",
                "legId": "short-put",
                "entryAtUnixMs": 1_785_000_000_000u64,
                "exitAtUnixMs": 1_785_172_800_000u64,
                "grossPnl": -30.0,
                "fees": 3.0,
                "netPnl": -33.0,
                "quantity": 1.0,
                "outcome": "loss",
                "fillQualityRef": "fill-quality-001"
            }
        ],
        "skips": [{"reasonCode": "confidence_below_threshold", "sourceRef": "selector-001"}],
        "rejects": [{"reasonCode": "provider_rejected_buying_power", "sourceRef": "exec-loss"}],
        "groupingDimensions": ["strategy_version", "underlying", "outcome", "skip_reason", "reject_reason"],
        "dataSnapshots": [{
            "snapshotRef": "snapshot://spy-2026-06-01",
            "contentHash": "sha256:snapshot001",
            "asOfUnixMs": 1_785_000_000_000u64,
            "staleAfterUnixMs": 1_785_086_400_000u64,
            "verifiedDataSource": true
        }],
        "codeRefs": [{
            "codeRef": "git://TradeAssembly/runtime-rs/src/service/attribution_journal.rs",
            "commitSha": "085bad039d4821510e75b16c450fd2df04f47f5c",
            "contentHash": "sha256:code001"
        }],
        "settingsHash": "sha256:settings001"
    })
}

struct TestDatabase {
    _directory: TempDir,
    path: String,
}

fn test_db(label: &str) -> TestDatabase {
    let directory = tempfile::Builder::new()
        .prefix(&format!(
            "tradeassembly-attribution-journal-service-{label}-"
        ))
        .tempdir()
        .expect("temporary database directory");
    let path = directory.path().join("runtime.db");
    TestDatabase {
        _directory: directory,
        path: path.to_string_lossy().into_owned(),
    }
}
