use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use tradeassembly_runtime::ports::{AuthorityContext, IdempotencyKey};
use tradeassembly_runtime::service::TradeAssemblyService;

fn owner(base: &TradeAssemblyService, issuer: &str) -> TradeAssemblyService {
    base.for_authenticated_invocation(issuer, "shared-subject", None, None)
}

#[test]
fn non_strategy_events_bind_full_owner_and_export_recomputable_digest() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("runtime.db").to_string_lossy().into_owned();
    let base = TradeAssemblyService::test_local(&db);
    let alice = owner(&base, "https://alice.example");
    let bob = owner(&alice, "https://bob.example");
    assert!(base.runtime().journal_owner.is_none());
    assert_eq!(
        alice.runtime().journal_owner.as_ref().unwrap().issuer,
        "https://alice.example"
    );
    let payload = json!({"note":"User-authored observation", "api_secret":"fixture-private-secret", "owner":{"issuer":"https://forged.example"}});
    let first = alice
        .record_side_effect(
            "journal.observation",
            payload.clone(),
            Some(AuthorityContext::local_cli()),
            Some(IdempotencyKey::new("same-key").unwrap()),
        )
        .unwrap();
    let second = bob
        .record_side_effect(
            "journal.observation",
            payload,
            Some(AuthorityContext::local_cli()),
            Some(IdempotencyKey::new("same-key").unwrap()),
        )
        .unwrap();
    assert_ne!(first, second);
    assert_eq!(base.runtime().journal.try_events().unwrap().len(), 2);
    for (service, issuer) in [
        (&alice, "https://alice.example"),
        (&bob, "https://bob.example"),
    ] {
        let export = service.handle_http("GET", "/journal/export", json!({"owner":"forged"}));
        assert_eq!(export.status, 200);
        assert_eq!(export.body["events"].as_array().unwrap().len(), 1);
        assert_eq!(export.body["events"][0]["owner"]["issuer"], issuer);
        assert!(!export.body.to_string().contains("fixture-private-secret"));
        assert_eq!(
            export.body["events"][0]["payload"]["api_secret"],
            "***redacted***"
        );
        let digest = format!(
            "{:x}",
            Sha256::digest(serde_json_canonicalizer::to_vec(&export.body["events"]).unwrap())
        );
        assert_eq!(export.body["eventsSha256"], digest);
    }
    drop(alice);
    drop(bob);
    drop(base);
    let reopened = owner(
        &TradeAssemblyService::test_local(&db),
        "https://alice.example",
    );
    let rows = reopened.handle_http("GET", "/journal/events", json!({}));
    assert_eq!(rows.body.as_array().unwrap().len(), 1);
    assert_eq!(rows.body[0]["event_type"], "journal.observation");
    assert_eq!(rows.body[0]["owner"]["issuer"], "https://alice.example");
}

#[test]
fn real_journal_reopens_and_isolates_equal_subjects_from_different_issuers() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("runtime.db").to_string_lossy().into_owned();
    let base = TradeAssemblyService::test_local(&db);
    let alice = owner(&base, "https://alice.example");
    assert_eq!(
        alice.handle_http("GET", "/journal/events", json!({})).body,
        json!([])
    );
    let created = alice.handle_http(
        "POST",
        "/product/strategies/create",
        json!({"id":"journal-owned", "name":"Journal fixture"}),
    );
    assert_eq!(created.status, 201, "{}", created.body);
    let rows = alice.handle_http("GET", "/journal/events", json!({}));
    assert_eq!(rows.status, 200);
    assert!(!rows.body.as_array().unwrap().is_empty());
    assert!(rows
        .body
        .as_array()
        .unwrap()
        .iter()
        .any(|event| event["event_type"] == "strategy.created"));
    let bob = owner(&base, "https://bob.example");
    assert_eq!(
        bob.handle_http("GET", "/journal/events", json!({"actor":"shared-subject"}))
            .body,
        json!([])
    );
    assert_eq!(
        base.handle_http("GET", "/journal/events", json!({})).status,
        401
    );
    drop(alice);
    drop(bob);
    drop(base);
    let reopened = owner(
        &TradeAssemblyService::test_local(&db),
        "https://alice.example",
    );
    assert_eq!(
        reopened
            .handle_http("GET", "/journal/events", json!({}))
            .body,
        rows.body
    );
    let report = reopened.handle_http("POST", "/journal/replay-report", json!({}));
    assert_eq!(report.status, 200);
    assert_eq!(report.body["events"], rows.body.as_array().unwrap().len());
    assert_eq!(report.body["strategyEvaluationPerformed"], false);
    assert!(report.body["deterministic"].is_null());
    let matching = reopened.handle_http(
        "POST",
        "/journal/replay-harness",
        json!({"expected":{"counts":report.body["counts"]}}),
    );
    assert_eq!(matching.status, 200, "{}", matching.body);
    let wrong = reopened.handle_http(
        "POST",
        "/journal/replay-harness",
        json!({"expected":{"counts":{}}}),
    );
    assert_eq!(wrong.status, 409, "{}", wrong.body);
    assert_eq!(wrong.body["ok"], false);
    assert_eq!(
        reopened
            .handle_http(
                "POST",
                "/journal/replay-harness",
                json!({"deterministic":true})
            )
            .status,
        400
    );
}

#[test]
fn workspace_journal_aliases_share_authenticated_owner_projection() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("runtime.db").to_string_lossy().into_owned();
    let base = TradeAssemblyService::test_local(&db);
    let alice = owner(&base, "https://alice.example");
    let created = alice.handle_http(
        "POST",
        "/product/strategies/create",
        json!({"id":"workspace-journal-owned", "name":"Workspace journal fixture"}),
    );
    assert_eq!(created.status, 201, "{}", created.body);

    let workspace = alice.handle_http("GET", "/workspace", json!({}));
    assert_eq!(workspace.status, 200, "{}", workspace.body);
    let events = alice.handle_http("GET", "/journal/events", json!({}));
    assert_eq!(events.status, 200, "{}", events.body);
    assert_eq!(workspace.body["journalEvents"], events.body);
    assert_eq!(workspace.body["journal_events"], events.body);
    assert_eq!(
        workspace.body["journalEvents"],
        workspace.body["journal_events"]
    );
    assert!(events
        .body
        .as_array()
        .unwrap()
        .iter()
        .any(|event| event["payload"]["id"] == "workspace-journal-owned"));
}

#[test]
fn corrupted_durable_journal_is_not_an_empty_success() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("runtime.db").to_string_lossy().into_owned();
    let service = owner(
        &TradeAssemblyService::test_local(&db),
        "https://owner.example",
    );
    let created = service.handle_http(
        "POST",
        "/product/strategies/create",
        json!({"id":"corruption-fixture", "name":"Corruption fixture"}),
    );
    assert_eq!(created.status, 201);
    let conn = rusqlite::Connection::open(&db).unwrap();
    assert!(
        conn.execute(
            "UPDATE runtime_events SET payload_json = ?1 WHERE stream_name='local.journal'",
            ["private-corruption-marker"]
        )
        .unwrap()
            > 0
    );
    for (method, path) in [
        ("GET", "/journal/events"),
        ("POST", "/journal/replay"),
        ("POST", "/journal/replay-report"),
    ] {
        let response = service.handle_http(method, path, Value::Null);
        assert_eq!(response.status, 503, "{}", response.body);
        assert!(!response
            .body
            .to_string()
            .contains("private-corruption-marker"));
        assert!(response
            .body
            .to_string()
            .contains("journal_read_unavailable"));
    }
}
