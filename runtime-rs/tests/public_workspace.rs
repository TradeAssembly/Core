use serde_json::json;
use tempfile::NamedTempFile;
use tradeassembly_runtime::backtest_contracts::canonical_hash;
use tradeassembly_runtime::ports::{AuthorityContext, IdempotencyKey, SideEffectContext};
use tradeassembly_runtime::service::{public_workspace as public, TradeAssemblyService};

fn service_with_owner(subject: &str) -> (NamedTempFile, TradeAssemblyService) {
    let file = NamedTempFile::new().unwrap();
    let base = TradeAssemblyService::test_local(file.path().to_str().unwrap());
    let owner = base.for_authenticated_invocation("local-owner", subject, None, None);
    (file, owner)
}
fn service() -> (NamedTempFile, TradeAssemblyService) {
    service_with_owner("owner")
}
fn source() -> (NamedTempFile, TradeAssemblyService) {
    let (file, service) = service();
    let created = service.handle_http_from_source(
        "cli",
        "POST",
        "/product/strategies/create",
        json!({"id":"safe-1","name":"Local research","templateId":"blank"}),
    );
    assert_eq!(created.status, 201, "{}", created.body);
    assert!(service
        .runtime()
        .storage
        .get_json("strategies", "safe-1")
        .unwrap()
        .is_some());
    (file, service)
}
fn context() -> SideEffectContext {
    SideEffectContext::new(
        AuthorityContext::local_cli(),
        IdempotencyKey::new("test-write").unwrap(),
    )
}
fn bytes(bundle: &public::PublicWorkspaceBundle) -> Vec<u8> {
    serde_json::to_vec(bundle).unwrap()
}
fn rehash(bundle: &mut public::PublicWorkspaceBundle) {
    for entry in &mut bundle.entries {
        entry.content_hash = canonical_hash(&entry.value, "hash").unwrap();
    }
    bundle.bundle_hash=canonical_hash(&json!({"schema":bundle.schema,"entries":bundle.entries,"settings":bundle.settings,"legacyBacktestsExcluded":bundle.legacy_backtests_excluded}),"hash").unwrap();
}

#[test]
fn durable_roundtrip_rebinds_owner_restores_stopped_and_retries_exactly() {
    let (_source, source) = source();
    let mut record = source
        .runtime()
        .storage
        .get_json("strategies", "safe-1")
        .unwrap()
        .unwrap();
    record["status"] = json!("active");
    source
        .runtime()
        .storage
        .put_json("strategies", "safe-1", record, &context())
        .unwrap();
    // Actual local research dataset/configuration, whose raw rows are excluded.
    let dataset = source.handle_http(
        "POST",
        "/product/strategies/research/datasets/create",
        json!({"strategyId":"safe-1","rows":[{"close":123.0}],"symbol":"BTC/USD"}),
    );
    // Route support is independently asserted; never invent stored fixture rows.
    assert_eq!(dataset.status, 200, "{}", dataset.body);
    let exported = public::export(&source).unwrap();
    assert!(!bytes(&exported).windows(7).any(|w| w == b"records"));
    let (target_file, target) = service_with_owner("target-owner");
    let imported = public::import(&target, &bytes(&exported)).unwrap();
    assert_eq!(imported["restoredState"], "stopped");
    assert_eq!(
        public::import(&target, &bytes(&exported)).unwrap()["duplicate"],
        true
    );
    let reopened = TradeAssemblyService::test_local(target_file.path().to_str().unwrap())
        .for_authenticated_invocation("local-owner", "target-owner", None, None);
    let again = public::export(&reopened).unwrap();
    assert_eq!(
        serde_json::to_value(exported).unwrap(),
        serde_json::to_value(again).unwrap()
    );
    for namespace in [
        "execution_activations",
        "execution_runs",
        "execution_configs",
        "execution_orders",
        "positions",
        "credentials",
        "dataset_snapshots_v1",
    ] {
        assert!(
            reopened
                .runtime()
                .storage
                .list_json(namespace)
                .unwrap()
                .is_empty(),
            "{namespace}"
        );
    }
    assert_eq!(
        reopened
            .runtime()
            .storage
            .get_json("strategies", "safe-1")
            .unwrap()
            .unwrap()["status"],
        "stopped"
    );
}

#[test]
fn invalid_bundles_never_mutate_target() {
    let (_source, source) = source();
    let bundle = public::export(&source).unwrap();
    let mut wrong = bundle.clone();
    wrong.schema = "future".to_string();
    let mut tampered = bundle.clone();
    tampered.entries[0].value["name"] = json!("changed");
    let mut traversal = bundle.clone();
    traversal.entries[0].key = "../outside".to_string();
    rehash(&mut traversal);
    let mut secret = bundle.clone();
    let strategy = secret
        .entries
        .iter_mut()
        .find(|e| e.namespace == "strategies")
        .unwrap();
    strategy.value["spec"]["api_key"] = json!("private");
    rehash(&mut secret);
    let mut hidden = bundle.clone();
    hidden.entries[0].value["brokerAccountId"] = json!("private-account");
    rehash(&mut hidden);
    let mut path = bundle.clone();
    path.entries
        .iter_mut()
        .find(|e| e.namespace == "strategies")
        .unwrap()
        .value["spec"]["path"] = json!("file:///private/data");
    rehash(&mut path);
    let mut oversized = bundle.clone();
    oversized.entries[0].value["name"] = json!("x".repeat(2 * 1024 * 1024 + 1));
    rehash(&mut oversized);
    for rejected in [wrong, tampered, traversal, secret, hidden, path, oversized] {
        let (_target, target) = service();
        assert!(public::import(&target, &bytes(&rejected)).is_err());
        assert!(target
            .runtime()
            .storage
            .list_json("strategies")
            .unwrap()
            .is_empty());
        assert!(target
            .runtime()
            .storage
            .list_json("public_workspace_imports_v1")
            .unwrap()
            .is_empty());
    }
    let (_target, target) = service();
    assert_eq!(
        public::import(&target, &vec![b' '; public::MAX_BUNDLE_BYTES + 1]).unwrap_err(),
        "workspace_size_limit"
    );
}

#[test]
fn nonempty_or_modified_restore_fails_closed() {
    let (_source, source) = source();
    let bundle = public::export(&source).unwrap();
    let (_target, target) = service();
    target
        .runtime()
        .storage
        .put_json(
            "execution_activations",
            "hidden",
            json!({"state":"active"}),
            &context(),
        )
        .unwrap();
    assert_eq!(
        public::import(&target, &bytes(&bundle)).unwrap_err(),
        "workspace_not_empty"
    );
    let (_clean, target) = service();
    public::import(&target, &bytes(&bundle)).unwrap();
    let mut strategy = target
        .runtime()
        .storage
        .get_json("strategies", "safe-1")
        .unwrap()
        .unwrap();
    strategy["name"] = json!("edited");
    target
        .runtime()
        .storage
        .put_json("strategies", "safe-1", strategy, &context())
        .unwrap();
    assert_eq!(
        public::import(&target, &bytes(&bundle)).unwrap_err(),
        "workspace_import_replay_conflict"
    );
}

#[cfg(unix)]
#[test]
fn file_io_is_bounded_no_overwrite_no_traversal_or_symlink() {
    let (_source, source) = source();
    let dir = tempfile::tempdir().unwrap();
    let canonical = dir.path().canonicalize().unwrap();
    let path = canonical.join("bundle.json");
    public::export_file(&source, &path).unwrap();
    assert!(public::export_file(&source, &path).is_err());
    let (_target, target) = service();
    public::import(&target, &public::read_file(&path).unwrap()).unwrap();
    let link = canonical.join("link");
    std::os::unix::fs::symlink(&path, &link).unwrap();
    assert!(public::read_file(&link).is_err());
    let parent_link = canonical.join("parent-link");
    std::os::unix::fs::symlink(&canonical, &parent_link).unwrap();
    assert!(public::read_file(&parent_link.join("bundle.json")).is_err());
    assert!(public::export_file(&source, &parent_link.join("other.json")).is_err());
    assert!(public::read_file(&dir.path().join("../outside")).is_err());
}

#[test]
fn summaries_do_not_manufacture_metrics_or_read_other_owners() {
    let (_source, source) = source();
    let result = public::result_summary(&source, "safe-1").unwrap();
    assert_eq!(result["schema"], public::RESULT_SUMMARY_SCHEMA);
    assert_eq!(result["paper"]["completeness"], "unavailable");
    assert_eq!(result["backtests"], json!([]));
    assert!(result["source"]["contentHash"]
        .as_str()
        .unwrap()
        .starts_with("sha256:"));
    let other = source.for_authenticated_invocation("local-owner", "other", None, None);
    assert_eq!(
        public::result_summary(&other, "safe-1").unwrap_err(),
        "object_not_available"
    );
    assert_eq!(public::export(&other).unwrap().entries.len(), 0);
    let remote = source.for_authenticated_invocation("provider", "owner", None, None);
    assert!(public::export(&remote).is_err());
    assert!(public::import(&remote, &bytes(&public::export(&source).unwrap())).is_err());
}

#[test]
fn failed_second_write_rolls_back_all_import_records_and_receipt() {
    let (_source, source) = source();
    let bundle = public::export(&source).unwrap();
    let (target_file, target) = service();
    let connection = rusqlite::Connection::open(target_file.path()).unwrap();
    connection.execute_batch("CREATE TRIGGER fail_import BEFORE INSERT ON tradeassembly_kv WHEN NEW.namespace='strategy_drafts' BEGIN SELECT RAISE(ABORT,'bounded_import_fault'); END;").unwrap();
    assert!(public::import(&target, &bytes(&bundle)).is_err());
    assert!(target
        .runtime()
        .storage
        .list_json("strategies")
        .unwrap()
        .is_empty());
    assert!(target
        .runtime()
        .storage
        .list_json("public_workspace_imports_v1")
        .unwrap()
        .is_empty());
    assert_eq!(
        target
            .runtime()
            .storage
            .list_json("object_authorization_scopes_v1")
            .unwrap()
            .len(),
        8
    );
    connection
        .execute_batch("DROP TRIGGER fail_import")
        .unwrap();
    let restarted = TradeAssemblyService::test_local(target_file.path().to_str().unwrap())
        .for_authenticated_invocation("local-owner", "owner", None, None);
    assert_eq!(
        public::import(&restarted, &bytes(&bundle)).unwrap()["duplicate"],
        false
    );
    assert_eq!(
        public::import(&restarted, &bytes(&bundle)).unwrap()["duplicate"],
        true
    );
}

#[test]
fn mcp_parameters_advertise_preassigned_ids_and_bounded_scheduler_interval() {
    let definitions = tradeassembly_runtime::mcp::tool_definitions();
    let tools = definitions.as_array().unwrap();
    let config = tools
        .iter()
        .find(|tool| tool["name"] == "tradeassembly.execution.config.save")
        .unwrap();
    assert_eq!(
        config["inputSchema"]["properties"]["config_id"]["type"],
        "string"
    );
    assert_eq!(
        config["inputSchema"]["properties"]["scheduler_interval_seconds"]["minimum"],
        1
    );
    let activate = tools
        .iter()
        .find(|tool| tool["name"] == "tradeassembly.execution.activate")
        .unwrap();
    assert_eq!(
        activate["inputSchema"]["properties"]["activation_id"]["type"],
        "string"
    );
}

#[test]
fn advertised_mcp_ids_reach_real_durable_activation_and_one_bounded_paper_tick() {
    let (_file, service) = service();
    let saved=service.call_mcp_tool("tradeassembly.execution.config.save",json!({
        "strategy_id":"strat_local_btc_demo","provider_ref":"sim","mode":"paper",
        "config_id":"cfg_bounded-public","scheduler_interval_seconds":86400,
        "risk_limits":{"max_notional":25,"max_order_quantity":0.0001,"max_daily_loss":25,"max_concurrent_positions":1},
        "idempotency_key":"cfg-bounded-public"
    }));
    assert_ne!(saved["isError"], true, "{saved}");
    let config = service
        .runtime()
        .storage
        .get_json("execution_configs", "cfg_bounded-public")
        .unwrap()
        .unwrap();
    assert_eq!(config["schedulerIntervalSeconds"], 86400);
    let activated=service.call_mcp_tool("tradeassembly.execution.activate",json!({
        "strategy_id":"strat_local_btc_demo","config_id":"cfg_bounded-public","activation_id":"activation_bounded-public",
        "idempotency_key":"activate-bounded-public","acknowledgement_ids":["user_logic","user_risk","no_advice"]
    }));
    assert_ne!(activated["isError"], true, "{activated}");
    assert!(
        service
            .runtime()
            .storage
            .get_json("execution_runs", "run_activation_bounded-public")
            .unwrap()
            .is_some(),
        "{activated}"
    );
    let tick=service.call_mcp_tool("tradeassembly.execution.scheduler.run",json!({"activation_id":"activation_bounded-public","worker_id":"worker-bounded-public","max_cycles":1,"idempotency_key":"one-bounded-tick"}));
    // Close the activation before assertions. The scheduler interval also keeps
    // any background worker parked while this deterministic test owns one tick.
    let stopped=service.call_mcp_tool("tradeassembly.execution.deactivate",json!({"activation_id":"activation_bounded-public","idempotency_key":"stop-bounded-public"}));
    assert_ne!(tick["isError"], true, "{tick}");
    assert_ne!(stopped["isError"], true, "{stopped}");
    let run = service
        .runtime()
        .storage
        .get_json("execution_runs", "run_activation_bounded-public")
        .unwrap()
        .unwrap();
    assert_eq!(run["cycle"], 1, "{tick}");
    assert_eq!(run["state"], "stopped");
    assert_eq!(
        service
            .runtime()
            .storage
            .list_json("execution_attempts")
            .unwrap()
            .len(),
        1
    );
    let summary = public::result_summary(&service, "strat_local_btc_demo").unwrap();
    assert_eq!(summary["operational"]["runs"][0]["state"], "stopped");
    assert_eq!(
        summary["paper"]["runs"][0]["metrics"]["fees"],
        serde_json::Value::Null
    );
}

#[test]
fn summary_cursor_uses_real_verified_owner_sequence_not_event_count() {
    let (_file, source) = source();
    let before = public::result_summary(&source, "safe-1").unwrap();
    let cursor = before["source"]["localArchiveCursor"].clone();
    assert_eq!(cursor["schema"], "tradeassembly.local-archive-cursor.v1");
    assert!(cursor["sequence"].as_i64().unwrap() > 0);
    assert_eq!(before["source"]["cursorCompleteness"], "available");
    // An event from another owner advances the durable stream but must not
    // advance the first owner's cursor or disclose the other owner's event.
    let other = source.for_authenticated_invocation("local-owner", "other", None, None);
    let created = other.handle_http_from_source(
        "cli",
        "POST",
        "/product/strategies/create",
        json!({"id":"other-only","name":"Other","templateId":"blank"}),
    );
    assert_eq!(created.status, 201);
    assert_eq!(
        public::result_summary(&source, "safe-1").unwrap()["source"]["localArchiveCursor"],
        cursor
    );
    let stream = source
        .runtime()
        .events
        .replay("local.journal", 0, 256)
        .unwrap();
    assert!(stream.last().unwrap().sequence > cursor["sequence"].as_i64().unwrap());
}

#[test]
fn duplicate_json_fields_and_unknown_envelopes_fail_before_mutation() {
    let (_source, source) = source();
    let bundle = public::export(&source).unwrap();
    let raw = String::from_utf8(bytes(&bundle)).unwrap();
    let duplicate = raw.replacen(
        "\"name\":\"Local research\"",
        "\"name\":\"shadow\",\"name\":\"Local research\"",
        1,
    );
    assert_ne!(raw, duplicate);
    let (_target, target) = service();
    assert_eq!(
        public::import(&target, duplicate.as_bytes()).unwrap_err(),
        "workspace_bundle_invalid"
    );
    let mut unknown = serde_json::to_value(bundle).unwrap();
    unknown["activeAuthority"] = json!({});
    assert_eq!(
        public::import(&target, &serde_json::to_vec(&unknown).unwrap()).unwrap_err(),
        "workspace_bundle_invalid"
    );
    assert!(target
        .runtime()
        .storage
        .list_json("strategies")
        .unwrap()
        .is_empty());
}

#[test]
fn owning_wire_schemas_validate_real_exports_and_summaries() {
    let (_file, source) = source();
    for (raw, value) in [
        (
            include_str!("../../contracts/public-workspace.v1.schema.json"),
            serde_json::to_value(public::export(&source).unwrap()).unwrap(),
        ),
        (
            include_str!("../../contracts/strategy-result-summary.v1.schema.json"),
            public::result_summary(&source, "safe-1").unwrap(),
        ),
    ] {
        let schema: serde_json::Value = serde_json::from_str(raw).unwrap();
        let validator = jsonschema::validator_for(&schema).unwrap();
        assert!(
            validator.is_valid(&value),
            "{}",
            validator.validate(&value).unwrap_err()
        );
    }
}

#[cfg(unix)]
#[test]
fn interrupted_import_recovers_without_partial_state_and_can_retry() {
    use std::time::{Duration, Instant};
    const CHILD: &str = "CORE_PUBLIC_IMPORT_CRASH_CHILD";
    if std::env::var_os(CHILD).is_some() {
        let db = std::env::var("CORE_PUBLIC_IMPORT_CRASH_DB").unwrap();
        let handoff = std::env::var("CORE_PUBLIC_IMPORT_CRASH_HANDOFF").unwrap();
        let service = TradeAssemblyService::test_local_with_handoff(db, &handoff)
            .unwrap()
            .for_authenticated_invocation("local-owner", "owner", None, None);
        let raw = public::read_file(std::path::Path::new(
            &std::env::var("CORE_PUBLIC_IMPORT_CRASH_BUNDLE").unwrap(),
        ))
        .unwrap();
        std::fs::write(
            std::env::var("CORE_PUBLIC_IMPORT_CRASH_READY").unwrap(),
            b"ready",
        )
        .unwrap();
        public::import(&service, &raw).unwrap();
        panic!("parent did not interrupt the held import transaction");
    }
    let (_source, source) = source();
    let bundle = public::export(&source).unwrap();
    let (target_file, target) = service();
    let connection = rusqlite::Connection::open(target_file.path()).unwrap();
    // Hold the actual import after its first data insert. This uses no product
    // fault hook and consumes bounded time because the parent kills the child.
    connection.execute_batch("CREATE TRIGGER hold_import AFTER INSERT ON tradeassembly_kv WHEN NEW.namespace='strategies' BEGIN WITH RECURSIVE counter(n) AS (VALUES(0) UNION ALL SELECT n+1 FROM counter WHERE n<1000000000) SELECT sum(n) FROM counter; END;").unwrap();
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path().canonicalize().unwrap();
    let input = root.join("bundle.json");
    std::fs::write(&input, bytes(&bundle)).unwrap();
    let ready = root.join("ready");
    let handoff = TradeAssemblyService::test_local_handoff(target.db()).unwrap();
    let mut child = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "interrupted_import_recovers_without_partial_state_and_can_retry",
            "--nocapture",
        ])
        .env(CHILD, "1")
        .env("CORE_PUBLIC_IMPORT_CRASH_DB", target.db())
        .env("CORE_PUBLIC_IMPORT_CRASH_HANDOFF", handoff)
        .env("CORE_PUBLIC_IMPORT_CRASH_BUNDLE", &input)
        .env("CORE_PUBLIC_IMPORT_CRASH_READY", &ready)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .unwrap();
    connection.busy_timeout(Duration::from_millis(5)).unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut locked = false;
    while Instant::now() < deadline {
        if ready.exists() {
            match connection.execute_batch("BEGIN IMMEDIATE; ROLLBACK;") {
                Ok(()) => {}
                Err(rusqlite::Error::SqliteFailure(error, _))
                    if matches!(
                        error.code,
                        rusqlite::ErrorCode::DatabaseBusy | rusqlite::ErrorCode::DatabaseLocked
                    ) =>
                {
                    locked = true;
                    break;
                }
                Err(error) => panic!("unexpected isolated lock observation: {error}"),
            }
        }
        if child.try_wait().unwrap().is_some() {
            break;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    let _ = child.kill();
    let _ = child.wait();
    assert!(locked, "child import never held its durable transaction");
    connection
        .execute_batch("DROP TRIGGER hold_import")
        .unwrap();
    let recovered = TradeAssemblyService::test_local(target.db()).for_authenticated_invocation(
        "local-owner",
        "owner",
        None,
        None,
    );
    assert!(recovered
        .runtime()
        .storage
        .list_json("strategies")
        .unwrap()
        .is_empty());
    assert!(recovered
        .runtime()
        .storage
        .list_json("public_workspace_imports_v1")
        .unwrap()
        .is_empty());
    assert_eq!(
        public::import(&recovered, &bytes(&bundle)).unwrap()["duplicate"],
        false
    );
    assert_eq!(
        public::import(&recovered, &bytes(&bundle)).unwrap()["duplicate"],
        true
    );
}
