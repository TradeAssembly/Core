use clap::Parser;
use serde_json::json;
use tradeassembly_runtime::cli::{execute_command, Cli};
use tradeassembly_runtime::service::TradeAssemblyService;

fn owner_service() -> TradeAssemblyService {
    let directory = tempfile::tempdir().expect("isolated state");
    let database = directory
        .keep()
        .join("runtime.db")
        .to_string_lossy()
        .into_owned();
    TradeAssemblyService::test_local(database).for_authenticated_invocation(
        "https://local-owner.example",
        "owner-cli-test",
        None,
        Some("CLI owner".to_string()),
    )
}

fn publish_args<'a>(strategy_id: &'a str, hash: &'a str, key: &'a str) -> Vec<&'a str> {
    vec![
        "tradeassembly",
        "strategy",
        "publish",
        strategy_id,
        "--expected-draft-hash",
        hash,
        "--acknowledge-publication",
        "--idempotency-key",
        key,
    ]
}

#[test]
fn publication_cli_requires_ack_before_mutation_and_accepts_exact_contract() {
    let service = owner_service();
    let created = service.handle_http(
        "POST",
        "/product/strategies/create",
        json!({
            "id":"owner-cli-strategy", "name":"Owner CLI test",
            "spec": tradeassembly_runtime::spec::btc_exit_demo_spec_payload(),
        }),
    );
    assert_eq!(created.status, 201, "{:#}", created.body);
    let hash = created.body["body"]["draft"]["draftHash"]
        .as_str()
        .expect("draft hash")
        .to_string();
    let before = service
        .runtime()
        .storage
        .list_json("strategy_versions")
        .expect("versions before");

    let mut missing_ack = publish_args("owner-cli-strategy", &hash, "owner-cli-no-ack");
    missing_ack.retain(|arg| *arg != "--acknowledge-publication");
    let parsed = Cli::try_parse_from(missing_ack).expect("ack is runtime protocol validation");
    let rejected = execute_command(&service, parsed.command);
    assert_eq!(
        rejected["error"]["code"],
        "strategy_publication_acknowledgement_required"
    );
    assert_eq!(
        service
            .runtime()
            .storage
            .list_json("strategy_versions")
            .unwrap(),
        before
    );

    let parsed = Cli::try_parse_from(publish_args(
        "owner-cli-strategy",
        &hash,
        "owner-cli-publish",
    ))
    .expect("publication CLI parses");
    let published = execute_command(&service, parsed.command);
    assert_eq!(published["body"]["published"], true, "{published:#}");
    assert_eq!(published["body"]["activation"]["started"], false);
    assert_eq!(
        service
            .runtime()
            .storage
            .list_json("strategy_versions")
            .unwrap()
            .len(),
        before.len() + 1
    );
    let replay = execute_command(
        &service,
        Cli::try_parse_from(publish_args(
            "owner-cli-strategy",
            &hash,
            "owner-cli-publish",
        ))
        .unwrap()
        .command,
    );
    assert_eq!(replay["duplicate"], true);
    assert_eq!(
        service
            .runtime()
            .storage
            .list_json("strategy_versions")
            .unwrap()
            .len(),
        before.len() + 1
    );
    let stale = execute_command(
        &service,
        Cli::try_parse_from(publish_args(
            "owner-cli-strategy",
            "sha256:stale",
            "owner-cli-stale",
        ))
        .unwrap()
        .command,
    );
    assert_eq!(stale["error"]["code"], "strategy_draft_changed");
    assert_eq!(
        service
            .runtime()
            .storage
            .list_json("strategy_versions")
            .unwrap()
            .len(),
        before.len() + 1
    );
    assert!(service
        .runtime()
        .journal
        .events()
        .iter()
        .any(|event| event.event_type == "strategy.version_published"));
}

#[test]
fn publication_cli_rejects_empty_hash_and_requires_idempotency_key() {
    let empty_hash = Cli::try_parse_from([
        "tradeassembly",
        "strategy",
        "publish",
        "strategy-1",
        "--expected-draft-hash",
        "",
        "--acknowledge-publication",
        "--idempotency-key",
        "key",
    ])
    .expect("empty hash reaches runtime validation");
    let service = owner_service();
    let response = execute_command(&service, empty_hash.command);
    assert_eq!(response["error"]["code"], "strategy_draft_hash_required");
    let empty_key = Cli::try_parse_from([
        "tradeassembly",
        "strategy",
        "publish",
        "strategy-1",
        "--expected-draft-hash",
        "hash",
        "--acknowledge-publication",
        "--idempotency-key",
        "",
    ])
    .expect("empty key reaches runtime validation");
    let response = execute_command(&service, empty_key.command);
    assert_eq!(response["error"]["code"], "idempotency_key_required");
    assert!(Cli::try_parse_from([
        "tradeassembly",
        "strategy",
        "publish",
        "strategy-1",
        "--acknowledge-publication",
        "--idempotency-key",
        "key",
    ])
    .is_err());
    assert!(Cli::try_parse_from([
        "tradeassembly",
        "strategy",
        "publish",
        "strategy-1",
        "--expected-draft-hash",
        "hash",
        "--acknowledge-publication",
    ])
    .is_err());
}
