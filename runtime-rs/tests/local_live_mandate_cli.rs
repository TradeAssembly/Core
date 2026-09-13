// Copyright (c) 2026 OptionLab LLC. All rights reserved.

use clap::Parser;
use tradeassembly_runtime::cli::{execute_command, Cli};
use tradeassembly_runtime::service::TradeAssemblyService;

#[test]
fn owner_mandate_cli_requires_explicit_target_expiry_and_idempotency() {
    let issue = ["tradeassembly", "execution", "live-mandate", "issue"];
    assert!(Cli::try_parse_from(issue).is_err());
    assert!(Cli::try_parse_from(issue.into_iter().chain([
        "--config-id",
        "user-config",
        "--expires-at-ms",
        "2000000000000",
    ]))
    .is_err());
    for action in ["revoke", "status"] {
        assert!(
            Cli::try_parse_from(["tradeassembly", "execution", "live-mandate", action]).is_err()
        );
    }
}

#[test]
fn cli_issue_reaches_verified_owner_gate_without_activating_anything() {
    let directory = tempfile::tempdir().unwrap();
    let database = directory.path().canonicalize().unwrap().join("runtime.db");
    let service = TradeAssemblyService::test_local(database.to_string_lossy());
    let cli = Cli::try_parse_from([
        "tradeassembly",
        "execution",
        "live-mandate",
        "issue",
        "--config-id",
        "user-config",
        "--expires-at-ms",
        "2000000000000",
        "--idempotency-key",
        "explicit-owner-request",
    ])
    .unwrap();
    let result = execute_command(&service, cli.command);
    assert_eq!(
        result["error"]["code"], "live_mandate_owner_required",
        "{result}"
    );
    assert!(service
        .runtime()
        .storage
        .list_json("local_live_mandates")
        .unwrap()
        .is_empty());
}
