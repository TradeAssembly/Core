use aes_gcm::aead::{Aead, Payload};
use aes_gcm::{Aes256Gcm, KeyInit, Nonce};
use axum::{
    extract::{Path as AxumPath, State},
    routing::{get, post},
    Json, Router,
};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use clap::Parser;
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};
use tempfile::{NamedTempFile, TempDir};
use tradeassembly_runtime::backtest_contracts::canonical_hash;
use tradeassembly_runtime::cli::{
    execute_backtests_command, execute_command, execute_comparison_command,
    execute_derivatives_command, execute_robustness_command, BacktestsCommand, Cli,
    Command as CliCommand, ComparisonCommand, DerivativesCommand, RobustnessCommand,
};
use tradeassembly_runtime::historical_data::{
    AssetClass, BarObservation, DatasetNormalizationPolicy, DatasetQualityPolicy, DatasetSnapshot,
    DatasetSnapshotContent, DatasetSourceClass, DatasetSourceManifest, DatasetTimeSlice,
    HistoricalDataKind, HistoricalObservation, HistoricalObservationData, QualityDisposition,
    DATASET_SNAPSHOT_SCHEMA,
};
use tradeassembly_runtime::mcp;
use tradeassembly_runtime::ports::{AuthorityContext, IdempotencyKey, SideEffectContext};
use tradeassembly_runtime::robustness_engine::{MonteCarloInput, MonteCarloMode, StudyInput};
use tradeassembly_runtime::service::TradeAssemblyService;
use tradeassembly_runtime::surfaces::cli_command_inventory;

const TEST_WARDEN_TOKEN: &str = "cli-test-warden-token-with-at-least-32-bytes";
const UNIT: i64 = 1_000_000;

#[test]
fn plugins_cli_parses_the_generic_package_and_instance_lifecycle() {
    for arguments in [
        vec!["plugins", "status"],
        vec!["plugins", "install-default", "--offline"],
        vec!["plugins", "install-default", "--skip-default-plugins"],
        vec![
            "plugins",
            "install",
            "--source",
            "/tmp/plugin.tar.gz",
            "--package-sha256",
            "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            "--manifest-sha256",
            "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
            "--offline",
        ],
        vec![
            "plugins",
            "create",
            "--plugin-ref",
            "example.plugin",
            "--instance-ref",
            "example-instance",
        ],
        vec![
            "plugins",
            "configure",
            "example-instance",
            "--configuration-file",
            "configuration.json",
        ],
        vec!["plugins", "enable", "example-instance"],
        vec!["plugins", "disable", "example-instance"],
        vec!["plugins", "refresh-health", "example-instance"],
        vec![
            "plugins",
            "upgrade",
            "example-instance",
            "--source",
            "/tmp/plugin.tar.gz",
            "--package-sha256",
            "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            "--manifest-sha256",
            "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
        ],
        vec!["plugins", "rollback", "example-instance"],
        vec!["plugins", "remove", "example-instance"],
        vec![
            "plugins",
            "invoke",
            "example-instance",
            "account.health",
            "--request-file",
            "request.json",
        ],
    ] {
        let mut argv = vec!["tradeassembly"];
        argv.extend(arguments);
        let parsed = Cli::try_parse_from(argv).expect("plugins command parses");
        assert!(matches!(parsed.command, CliCommand::Plugins { .. }));
    }
}

#[test]
fn plugins_cli_routes_instance_lifecycle_through_shared_application_commands() {
    let service = TradeAssemblyService::test_local(temp_db("plugins-cli").display().to_string());
    let disable = Cli::try_parse_from(["tradeassembly", "plugins", "disable", "sim"])
        .expect("disable command parses");
    let response = execute_command(&service, disable.command);
    assert_eq!(response["ok"], true);
    assert_eq!(response["instance"]["enabled"], false);

    let status =
        Cli::try_parse_from(["tradeassembly", "plugins", "status"]).expect("status command parses");
    let response = execute_command(&service, status.command);
    let sim = response["instances"]
        .as_array()
        .expect("instances")
        .iter()
        .find(|instance| instance["instanceRef"] == "sim")
        .expect("sim instance");
    assert_eq!(sim["enabled"], false);

    let offline = Cli::try_parse_from(["tradeassembly", "plugins", "install-default", "--offline"])
        .expect("offline default install parses");
    let response = execute_command(&service, offline.command);
    assert_eq!(
        response["error"]["code"], "plugin_package_install_failed",
        "{response}"
    );
    assert_eq!(
        response["error"]["details"]["message"], "plugin_package_offline_cache_miss",
        "{response}"
    );

    let deselected = Cli::try_parse_from([
        "tradeassembly",
        "plugins",
        "install-default",
        "--skip-default-plugins",
    ])
    .expect("deselected default install parses");
    let response = execute_command(&service, deselected.command);
    assert_eq!(response["ok"], true);
    assert_eq!(response["status"], "skipped");
    assert_eq!(response["reason"], "not_selected");
}

#[test]
fn backtests_cli_parses_each_lifecycle_verb() {
    for arguments in [
        vec!["backtests", "create", "--request-file", "request.json"],
        vec!["backtests", "get", "run-1"],
        vec!["backtests", "list"],
        vec!["backtests", "cancel", "run-1"],
        vec!["backtests", "retry", "run-1"],
        vec!["backtests", "process", "--worker", "worker-1"],
        vec!["backtests", "replay", "run-1"],
    ] {
        let mut argv = vec!["tradeassembly"];
        argv.extend(arguments);
        let parsed = Cli::try_parse_from(argv).expect("backtests command parses");
        assert!(matches!(parsed.command, CliCommand::Backtests { .. }));
    }
}

#[test]
fn backtests_cli_uses_shared_routes_and_preserves_error_statuses() {
    let service =
        TradeAssemblyService::test_local(temp_db("backtests-routes").display().to_string());
    let mut invalid_request = NamedTempFile::new().expect("request file");
    invalid_request
        .write_all(b"{}")
        .expect("write invalid request");

    let create = execute_backtests_command(
        &service,
        parse_backtests(&[
            "create",
            "--request-file",
            invalid_request.path().to_str().expect("request path"),
        ]),
    );
    assert_eq!(create.status, 400);
    assert!(create.body["error"]["code"].is_string());

    let list = execute_backtests_command(&service, BacktestsCommand::List);
    assert_eq!(list.status, 200);
    assert_eq!(list.body["runs"], json!([]));

    for command in [
        BacktestsCommand::Get {
            run_id: "missing".to_string(),
        },
        parse_backtests(&["cancel", "missing"]),
        parse_backtests(&["retry", "missing"]),
        BacktestsCommand::Replay {
            run_id: "missing".to_string(),
        },
    ] {
        let response = execute_backtests_command(&service, command);
        assert!(response.status >= 400);
        assert!(response.body["error"]["code"].is_string());
    }

    let process = execute_backtests_command(
        &service,
        BacktestsCommand::Process {
            worker: "cli-test-worker".to_string(),
        },
    );
    assert_eq!(process.status, 202);
    assert_eq!(process.body["status"], "idle");
}

#[test]
fn backtests_cli_prints_shared_error_json_and_returns_nonzero() {
    assert_backtest_request_error(&["backtests", "create"]);
    assert_backtest_request_error(&["backtest"]);
    let missing = tradeassembly()
        .arg("backtest")
        .output()
        .expect("missing request");
    assert_eq!(missing.status.code(), Some(2));
    assert!(missing.stdout.is_empty());
    assert!(String::from_utf8(missing.stderr)
        .unwrap()
        .contains("--request-file"));
}

fn assert_backtest_request_error(command: &[&str]) {
    let mut invalid_request = NamedTempFile::new().expect("request file");
    invalid_request
        .write_all(b"not json")
        .expect("write malformed request");
    let output = tradeassembly()
        .args(command)
        .args([
            "--request-file",
            invalid_request.path().to_str().expect("request path"),
        ])
        .output()
        .expect("backtests create");

    assert_eq!(output.status.code(), Some(1));
    let response = json_stdout(output.stdout);
    assert_eq!(response["error"]["code"], "backtest_request_invalid");
    assert!(output.stderr.is_empty());
}

#[test]
fn robustness_cli_parses_each_lifecycle_verb() {
    for arguments in [
        vec!["robustness", "create", "--request-file", "request.json"],
        vec!["robustness", "run", "--request-file", "request.json"],
        vec!["robustness", "get", "run-1"],
        vec!["robustness", "list"],
        vec!["robustness", "cancel", "run-1"],
        vec!["robustness", "retry", "run-1"],
        vec!["robustness", "process", "--worker", "worker-1"],
    ] {
        let mut argv = vec!["tradeassembly"];
        argv.extend(arguments);
        let parsed = Cli::try_parse_from(argv).expect("robustness command parses");
        assert!(matches!(parsed.command, CliCommand::Robustness { .. }));
    }
}

#[test]
fn robustness_typed_create_requires_explicit_research_inputs() {
    let output = tradeassembly()
        .args(["robustness", "create", "--source-run-id", "backtest-1"])
        .output()
        .expect("robustness create");
    assert_eq!(output.status.code(), Some(1));
    let response = json_stdout(output.stdout);
    assert_eq!(response["error"]["code"], "robustness_study_kind_required");
}

#[test]
fn robustness_cli_uses_canonical_routes_and_preserves_statuses() {
    let service =
        TradeAssemblyService::test_local(temp_db("robustness-routes").display().to_string());
    let mut invalid_request = NamedTempFile::new().expect("request file");
    invalid_request
        .write_all(b"{}")
        .expect("write invalid request");

    let create = execute_robustness_command(
        &service,
        parse_robustness(&[
            "create",
            "--request-file",
            invalid_request.path().to_str().expect("request path"),
        ]),
    );
    assert_eq!(create.status, 400);
    assert_eq!(create.body["error"]["code"], "robustness_request_invalid");

    let list = execute_robustness_command(&service, RobustnessCommand::List);
    assert_eq!(list.status, 200);
    assert_eq!(list.body["runs"], json!([]));

    for command in [
        RobustnessCommand::Get {
            run_id: "missing".to_string(),
        },
        parse_robustness(&["cancel", "missing"]),
        parse_robustness(&["retry", "missing"]),
    ] {
        let response = execute_robustness_command(&service, command);
        assert!(response.status >= 400);
        assert!(response.body["error"]["code"].is_string());
    }

    let process = execute_robustness_command(
        &service,
        parse_robustness(&["process", "--worker", "cli-test-worker"]),
    );
    assert_eq!(process.status, 202);
}

#[test]
fn robustness_cli_report_replay_and_export_use_durable_routes_and_fail_closed() {
    for arguments in [
        vec!["robustness", "get-report", "run-1"],
        vec![
            "robustness",
            "replay",
            "run-1",
            "--idempotency-key",
            "replay-1",
        ],
        vec![
            "robustness",
            "export",
            "run-1",
            "--format",
            "json",
            "--idempotency-key",
            "export-1",
        ],
    ] {
        let mut argv = vec!["tradeassembly"];
        argv.extend(arguments);
        let parsed = Cli::try_parse_from(argv).expect("robustness artifact command parses");
        assert!(matches!(parsed.command, CliCommand::Robustness { .. }));
    }

    for arguments in [
        vec!["tradeassembly", "robustness", "replay", "run-1"],
        vec![
            "tradeassembly",
            "robustness",
            "export",
            "run-1",
            "--format",
            "xml",
            "--idempotency-key",
            "export-invalid",
        ],
    ] {
        assert!(
            Cli::try_parse_from(arguments).is_err(),
            "invalid robustness command must fail closed"
        );
    }

    let service =
        TradeAssemblyService::test_local(temp_db("robustness-artifacts").display().to_string());
    for command in [
        parse_robustness(&["get-report", "missing"]),
        parse_robustness(&[
            "replay",
            "missing",
            "--idempotency-key",
            "cli-replay-missing",
        ]),
        parse_robustness(&[
            "export",
            "missing",
            "--format",
            "csv",
            "--idempotency-key",
            "cli-export-missing",
        ]),
    ] {
        let response = execute_robustness_command(&service, command);
        assert!(response.status >= 400, "{response:#?}");
        assert_eq!(
            response.body["error"]["code"], "robustness_not_found",
            "{response:#?}"
        );
        assert!(!response.body.to_string().contains("resultHash"));
    }
}

#[test]
fn robustness_cli_and_mcp_match_successful_durable_http_artifacts() {
    let fixture = completed_robustness_fixture("robustness-transport-parity");
    let report_path = format!("/robustness-runs/{}/report", fixture.run_id);
    let replay_path = format!("/robustness-runs/{}/replay", fixture.run_id);
    let export_path = format!("/robustness-runs/{}/export", fixture.run_id);

    let direct_report =
        fixture
            .service
            .handle_http_from_source("http", "GET", &report_path, json!({}));
    let cli_report = execute_robustness_command(
        &fixture.service,
        parse_robustness(&["get-report", &fixture.run_id]),
    );
    assert_eq!(cli_report.status, direct_report.status);
    assert_eq!(cli_report.body, direct_report.body);
    assert_eq!(cli_report.body["result"]["resultHash"], fixture.result_hash);

    let direct_replay = fixture.service.handle_http_from_source(
        "http",
        "POST",
        &replay_path,
        json!({
            "idempotencyKey": "direct-replay-parity",
            "authorityContext": {"actor": "test-user", "surface": "http"}
        }),
    );
    let cli_replay = execute_robustness_command(
        &fixture.service,
        parse_robustness(&[
            "replay",
            &fixture.run_id,
            "--idempotency-key",
            "cli-replay-parity",
            "--actor",
            "test-user",
        ]),
    );
    assert_eq!(cli_replay.status, direct_replay.status);
    assert_eq!(cli_replay.body, direct_replay.body);
    assert_eq!(cli_replay.body["status"], "verified");

    let direct_export = fixture.service.handle_http_from_source(
        "http",
        "POST",
        &export_path,
        json!({
            "format": "json",
            "idempotencyKey": "direct-export-parity",
            "authorityContext": {"actor": "test-user", "surface": "http"}
        }),
    );
    let cli_export = execute_robustness_command(
        &fixture.service,
        parse_robustness(&[
            "export",
            &fixture.run_id,
            "--format",
            "json",
            "--idempotency-key",
            "cli-export-parity",
            "--actor",
            "test-user",
        ]),
    );
    assert_eq!(cli_export.status, direct_export.status);
    assert_eq!(cli_export.body, direct_export.body);
    assert_eq!(cli_export.body["format"], "json");

    let mcp_report = mcp::call_service_tool(
        &fixture.service,
        "tradeassembly.robustness.report",
        json!({"run_id": fixture.run_id}),
    );
    assert_eq!(mcp_report["isError"], false, "{mcp_report:#}");
    assert_eq!(mcp_report["structuredContent"], direct_report.body);

    let mcp_replay = mcp::call_service_tool(
        &fixture.service,
        "tradeassembly.robustness.replay",
        json!({
            "run_id": fixture.run_id,
            "idempotency_key": "mcp-replay-parity",
            "actor": "test-user"
        }),
    );
    assert_eq!(mcp_replay["isError"], false, "{mcp_replay:#}");
    assert_eq!(mcp_replay["structuredContent"], direct_replay.body);

    let direct_csv_export = fixture.service.handle_http_from_source(
        "http",
        "POST",
        &export_path,
        json!({
            "format": "csv",
            "idempotencyKey": "direct-csv-export-parity",
            "authorityContext": {"actor": "test-user", "surface": "http"}
        }),
    );
    let mcp_export = mcp::call_service_tool(
        &fixture.service,
        "tradeassembly.robustness.export",
        json!({
            "run_id": fixture.run_id,
            "format": "csv",
            "idempotency_key": "mcp-export-parity",
            "actor": "test-user"
        }),
    );
    assert_eq!(mcp_export["isError"], false, "{mcp_export:#}");
    assert_eq!(mcp_export["structuredContent"], direct_csv_export.body);
}

#[test]
fn derivatives_cli_parses_and_uses_canonical_routes() {
    for arguments in [
        vec!["derivatives", "create", "--request-file", "request.json"],
        vec!["derivatives", "list"],
        vec!["derivatives", "get", "analysis-1"],
        vec!["derivatives", "export", "analysis-1", "--format", "json"],
    ] {
        let mut argv = vec!["tradeassembly"];
        argv.extend(arguments);
        let parsed = Cli::try_parse_from(argv).expect("derivatives command parses");
        assert!(matches!(parsed.command, CliCommand::Derivatives { .. }));
    }

    let service =
        TradeAssemblyService::test_local(temp_db("derivatives-routes").display().to_string());
    let mut malformed = NamedTempFile::new().expect("request file");
    malformed.write_all(b"[]").expect("write malformed request");
    let create = execute_derivatives_command(
        &service,
        parse_derivatives(&[
            "create",
            "--request-file",
            malformed.path().to_str().expect("request path"),
        ]),
    );
    assert_eq!(create.status, 400);
    assert_eq!(create.body["error"]["code"], "derivatives_request_invalid");

    let list = execute_derivatives_command(&service, DerivativesCommand::List);
    assert_eq!(list.status, 200);
    assert_eq!(list.body["analyses"], json!([]));

    for command in [
        DerivativesCommand::Get {
            analysis_id: "missing".to_string(),
        },
        DerivativesCommand::Export {
            analysis_id: "missing".to_string(),
            format: "json".to_string(),
        },
    ] {
        let response = execute_derivatives_command(&service, command);
        assert!(response.status >= 400);
        assert!(response.body["error"]["code"].is_string());
    }
}

#[test]
fn comparison_cli_parses_and_uses_canonical_routes() {
    for arguments in [
        vec!["comparison", "create", "--request-file", "request.json"],
        vec!["comparison", "list"],
        vec!["comparison", "get", "comparison-1"],
        vec!["comparison", "export", "comparison-1", "--format", "csv"],
    ] {
        let mut argv = vec!["tradeassembly"];
        argv.extend(arguments);
        let parsed = Cli::try_parse_from(argv).expect("comparison command parses");
        assert!(matches!(parsed.command, CliCommand::Comparison { .. }));
    }

    let service =
        TradeAssemblyService::test_local(temp_db("comparison-routes").display().to_string());
    let mut malformed = NamedTempFile::new().expect("request file");
    malformed.write_all(b"[]").expect("write malformed request");
    let create = execute_comparison_command(
        &service,
        parse_comparison(&[
            "create",
            "--request-file",
            malformed.path().to_str().expect("request path"),
        ]),
    );
    assert_eq!(create.status, 400);
    assert_eq!(create.body["error"]["code"], "comparison_request_invalid");

    let list = execute_comparison_command(&service, ComparisonCommand::List);
    assert_eq!(list.status, 200);
    assert_eq!(list.body["comparisons"], json!([]));

    for command in [
        ComparisonCommand::Get {
            comparison_id: "missing".to_string(),
        },
        ComparisonCommand::Export {
            comparison_id: "missing".to_string(),
            format: "json".to_string(),
        },
    ] {
        let response = execute_comparison_command(&service, command);
        assert!(response.status >= 400);
        assert!(response.body["error"]["code"].is_string());
    }
}

#[test]
fn tradeassembly_help_exposes_manifest_command_groups() {
    let output = tradeassembly()
        .arg("--help")
        .output()
        .expect("tradeassembly help");
    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).expect("utf8 help");
    for command in cli_command_inventory() {
        let top_level = command.split_whitespace().next().expect("command");
        assert!(
            stdout.contains(top_level),
            "help missing top-level command {top_level}: {stdout}"
        );
    }
}

#[test]
fn representative_commands_emit_report_envelopes() {
    for args in [
        vec!["readiness"],
        vec!["strategy", "list"],
        vec!["providers", "list"],
        vec!["credentials", "status"],
        vec!["marketdata", "quote"],
        vec!["scheduler", "status"],
        vec!["report"],
        vec!["scenario", "valuation"],
        vec!["monte-carlo", "report"],
    ] {
        let output = tradeassembly()
            .args(&args)
            .output()
            .expect("run tradeassembly");
        let envelope = json_stdout(output.stdout);
        let ok = envelope["ok"]
            .as_bool()
            .expect("canonical envelope ok must be boolean");
        assert_eq!(output.status.success(), ok, "{args:?}: {envelope:#}");
        assert!(envelope["data"].is_object() || envelope["data"].is_array());
        assert!(envelope["warnings"].is_array());
        assert!(envelope["errors"].is_array());
        assert_eq!(envelope["authority"]["surface"], "cli");
        assert!(
            envelope["idempotency_key"].as_str().is_some(),
            "{args:?}: {envelope:#}"
        );
    }
}

#[test]
fn manifest_runtime_commands_emit_canonical_envelopes() {
    for command in cli_command_inventory() {
        if command == "backtest" {
            // Durable commands expose their actual HTTP-status/error contract,
            // not the legacy report envelope or fabricated completion.
            assert_backtest_request_error(&["backtest"]);
            continue;
        }
        if matches!(
            command,
            "agent install"
                | "agent status"
                | "agent uninstall"
                | "agent verify"
                | "api"
                | "mcp serve"
                | "plugins install"
                | "plugins invoke"
        ) {
            assert_help_for_non_runnable_command(command);
            continue;
        }
        let args = runnable_args(command);
        let output = tradeassembly()
            .args(&args)
            .output()
            .expect("run tradeassembly");
        let envelope = json_stdout(output.stdout);
        let ok = envelope["ok"]
            .as_bool()
            .expect("canonical envelope ok must be boolean");
        assert_eq!(output.status.success(), ok, "{command}: {envelope:#}");
        assert!(envelope["data"].is_object() || envelope["data"].is_array());
        assert!(envelope["warnings"].as_array().is_some());
        assert!(envelope["errors"].as_array().is_some());
        assert_eq!(envelope["authority"]["surface"], "cli");
        assert_eq!(envelope["authority"]["account_mode"], "paper");
        assert_eq!(
            envelope["idempotency_key"],
            format!("cli:{}", command.split_whitespace().next().unwrap())
        );
        assert!(!envelope.to_string().contains("api_secret"));
        assert!(!envelope.to_string().contains("Bearer "));
    }
}

#[test]
fn credential_commands_do_not_emit_raw_secret_fields() {
    let output = tradeassembly()
        .args([
            "credentials",
            "status",
            "--provider-ref",
            "external-plugin-test",
        ])
        .output()
        .expect("credential status");
    assert!(!output.status.success());
    let envelope = json_stdout(output.stdout);
    assert_eq!(envelope["ok"].as_bool(), Some(false), "{envelope:#}");
    assert_eq!(envelope["authority"]["surface"], "cli");
    assert!(envelope["idempotency_key"].as_str().is_some());
    assert!(envelope["warnings"].is_array());
    assert!(envelope["errors"].is_array());
    assert_eq!(
        envelope["data"]["error"]["code"], "plugin_instance_not_found",
        "{envelope:#}"
    );
    let serialized = envelope.to_string();
    assert!(!serialized.contains("api_secret"), "{envelope:#}");
    assert!(!serialized.contains("apiKey"), "{envelope:#}");
    assert!(!serialized.contains("Bearer "), "{envelope:#}");
}

#[test]
fn local_account_install_and_telemetry_cli_commands_are_agent_safe() {
    for args in [
        vec!["account", "status", "--profile", "local"],
        vec!["account", "login", "--profile", "local"],
        vec!["install", "claim", "--profile", "local"],
        vec![
            "telemetry",
            "emit",
            "--profile",
            "local",
            "--event",
            "first_backtest",
        ],
        vec!["telemetry", "opt-out", "--profile", "local", "true"],
        vec!["telemetry", "replay", "--profile", "local"],
    ] {
        let output = tradeassembly()
            .args(&args)
            .output()
            .expect("run tradeassembly");
        assert!(
            output.status.success(),
            "{args:?} failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        let envelope = json_stdout(output.stdout);
        assert_eq!(envelope["ok"], true, "{args:?}: {envelope:#}");
        assert_eq!(envelope["authority"]["surface"], "cli");
        assert_eq!(envelope["data"]["profile"], "local");
        assert_eq!(envelope["data"]["no_credential_custody"], true);
        assert!(!envelope.to_string().contains("api_secret"));
        assert!(!envelope.to_string().contains("Bearer "));
        assert!(!envelope.to_string().contains("credential_value"));
    }
}

#[test]
fn legacy_scenario_valuation_cli_fails_closed_without_a_durable_source() {
    let output = tradeassembly()
        .args([
            "scenario",
            "valuation",
            "--strategy-id",
            "strat_local_btc_demo",
            "--checkpoint-id",
            "checkpoint_001",
        ])
        .output()
        .expect("scenario valuation");
    assert!(!output.status.success());
    let envelope = json_stdout(output.stdout);
    let payload = &envelope["data"];

    assert_eq!(
        payload["error"]["code"], "derivatives_request_invalid",
        "{envelope:#}"
    );
    assert!(payload.get("analysis").is_none());
    assert_eq!(envelope["authority"]["surface"], "cli");
    assert!(!envelope.to_string().contains("api_secret"));
    assert!(!envelope.to_string().to_lowercase().contains("should sell"));
}

#[test]
fn legacy_monte_carlo_cli_fails_closed_without_a_durable_run_id() {
    let output = tradeassembly()
        .args([
            "monte-carlo",
            "report",
            "--strategy-id",
            "strat_local_btc_demo",
        ])
        .output()
        .expect("monte carlo report");
    assert!(!output.status.success());
    let envelope = json_stdout(output.stdout);
    let payload = &envelope["data"];

    assert_eq!(
        payload["error"]["code"], "robustness_not_found",
        "{envelope:#}"
    );
    assert!(
        payload.get("result").is_none(),
        "legacy command must not fabricate a robustness result: {envelope:#}"
    );
    assert_eq!(envelope["authority"]["surface"], "cli");
    assert!(!envelope.to_string().contains("api_secret"));
    assert!(!envelope.to_string().to_lowercase().contains("should buy"));
}

#[test]
fn portfolio_risk_cli_emits_shared_report_contract() {
    let output = tradeassembly()
        .args([
            "risk",
            "overlay",
            "--strategy-id",
            "strat_local_btc_demo",
            "--scope-kind",
            "strategy",
        ])
        .output()
        .expect("portfolio risk overlay");
    assert!(
        output.status.success(),
        "portfolio risk overlay failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let envelope = json_stdout(output.stdout);
    let payload = &envelope["data"];

    assert_eq!(payload["ok"], true);
    assert_eq!(payload["kind"], "portfolio_risk");
    assert_eq!(payload["strategyId"], "strat_local_btc_demo");
    assert_eq!(
        payload["reportEnvelope"]["charts"][0]["id"],
        "portfolio_risk_exposure_table"
    );
    assert_eq!(
        payload["reportEnvelope"]["renderContract"]["preferred"],
        "tradeassembly_ui_deep_links"
    );
    assert!(
        payload["reportEnvelope"]["renderTargets"]["tradeassembly"]["href"]
            .as_str()
            .unwrap()
            .contains("view=portfolio-risk")
    );
    assert!(payload["reportEnvelope"]["exportRefs"]["csv"]["uri"]
        .as_str()
        .unwrap()
        .contains("exposures.csv"));
    assert_eq!(envelope["authority"]["surface"], "cli");
    assert!(!envelope.to_string().contains("api_secret"));
    assert!(!envelope.to_string().to_lowercase().contains("should buy"));
}

#[test]
fn fill_quality_cli_emits_shared_report_contract() {
    let output = tradeassembly()
        .args([
            "fill-quality",
            "report",
            "--strategy-id",
            "strat_local_btc_demo",
            "--analysis-id",
            "fill_quality_latest",
        ])
        .output()
        .expect("fill quality report");
    assert!(
        output.status.success(),
        "fill quality report failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let envelope = json_stdout(output.stdout);
    let payload = &envelope["data"];

    assert_eq!(payload["ok"], true);
    assert_eq!(payload["kind"], "fill_quality");
    assert_eq!(payload["strategyId"], "strat_local_btc_demo");
    assert_eq!(payload["summary"]["fillCount"].as_i64(), Some(2));
    assert_eq!(
        payload["reportEnvelope"]["renderContract"]["preferred"],
        "tradeassembly_ui_deep_links"
    );
    assert!(payload["reportEnvelope"]["deepLinks"]["execution"]["href"]
        .as_str()
        .unwrap()
        .contains("fillQuality=fill_quality_latest"));
    assert!(payload["perFill"].as_array().expect("per fill rows").len() >= 2);
    assert_eq!(envelope["authority"]["surface"], "cli");
    assert!(!envelope.to_string().contains("api_secret"));
    assert!(!envelope.to_string().contains("Bearer "));
    assert!(!envelope.to_string().to_lowercase().contains("should buy"));
}

#[test]
fn lifecycle_calendar_cli_emits_shared_calendar_contract() {
    let output = tradeassembly()
        .args([
            "lifecycle-calendar",
            "inspect",
            "--strategy-id",
            "strat_local_btc_demo",
            "--timeline-id",
            "lifecycle_calendar_latest",
        ])
        .output()
        .expect("lifecycle calendar inspect");
    assert!(
        output.status.success(),
        "lifecycle calendar inspect failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let envelope = json_stdout(output.stdout);
    let payload = &envelope["data"];

    assert_eq!(
        payload["schemaVersion"],
        "tradeassembly.lifecycle_calendar.service.v1"
    );
    assert_eq!(payload["strategyId"], "strat_local_btc_demo");
    assert!(payload["summary"]["upcomingCount"].as_i64().unwrap_or(0) >= 1);
    assert!(payload["deepLinks"]["calendar"]["href"]
        .as_str()
        .unwrap()
        .contains("lifecycleCalendar="));
    assert!(payload["exportRefs"]["csv"]["uri"]
        .as_str()
        .unwrap()
        .contains("lifecycle-calendar"));
    assert_eq!(payload["sideEffects"]["brokerStateChanged"], false);
    assert_eq!(envelope["authority"]["surface"], "cli");
    assert!(!envelope.to_string().contains("api_secret"));
    assert!(!envelope.to_string().contains("Bearer "));
    assert!(!envelope.to_string().to_lowercase().contains("should buy"));
}

#[test]
fn research_notebook_cli_emits_shared_notebook_contract() {
    let fixture = completed_robustness_fixture("research-notebook-cli");
    let selector = format!("robustness:{}", fixture.run_id);
    let cli = Cli::try_parse_from([
        "tradeassembly",
        "research-notebook",
        "compose",
        "--strategy-id",
        "strategy-1",
        "--artifact-selector",
        selector.as_str(),
    ])
    .expect("research notebook CLI parses typed selector");
    let payload = execute_command(&fixture.service, cli.command);

    assert_eq!(
        payload["schemaVersion"],
        "tradeassembly.research_notebook.service.v1"
    );
    assert_eq!(payload["kind"], "research_notebook");
    assert_eq!(payload["strategyId"], "strategy-1");
    assert_eq!(payload["summary"]["artifactRefCount"], 1);
    assert_eq!(payload["artifactRefs"][0]["artifactId"], fixture.run_id);
    assert!(!payload["notebook"]["reportRefs"]
        .as_array()
        .expect("report refs")
        .is_empty());
    assert!(payload["deepLinks"]["research"]["href"]
        .as_str()
        .unwrap()
        .contains("notebook="));
    assert!(payload["exportRefs"]["html"]["uri"]
        .as_str()
        .unwrap()
        .contains("research-notebook"));
    assert!(payload["replayRefs"]["commands"][0]
        .as_str()
        .unwrap()
        .contains("research-notebook"));
    assert_eq!(payload["sideEffects"]["brokerStateChanged"], false);
    let commands = fixture
        .service
        .handle_http("GET", "/admin/control-plane", json!({}));
    assert!(
        commands
            .body
            .to_string()
            .contains("\"source_interface\":\"cli\""),
        "{commands:#?}"
    );
    assert!(!payload.to_string().contains("api_secret"));
    assert!(!payload.to_string().contains("Bearer "));
    assert!(!payload.to_string().to_lowercase().contains("should buy"));
}

#[test]
fn attribution_journal_cli_emits_shared_review_contract() {
    let output = tradeassembly()
        .args([
            "attribution-journal",
            "report",
            "--strategy-id",
            "strat_local_btc_demo",
            "--review-id",
            "attribution_journal_latest",
        ])
        .output()
        .expect("attribution journal report");
    assert!(
        output.status.success(),
        "attribution journal report failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let envelope = json_stdout(output.stdout);
    let payload = &envelope["data"];

    assert_eq!(
        payload["schemaVersion"],
        "tradeassembly.attribution_journal.service.v1"
    );
    assert_eq!(payload["kind"], "attribution_journal");
    assert_eq!(payload["strategyId"], "strat_local_btc_demo");
    assert!(payload["summary"]["closedCount"].as_i64().unwrap_or(0) >= 1);
    assert!(!payload["journal"]["tradeRows"]
        .as_array()
        .expect("trade journal rows")
        .is_empty());
    assert!(!payload["attribution"]["tables"]
        .as_array()
        .expect("attribution tables")
        .is_empty());
    assert!(payload["deepLinks"]["journal"]["href"]
        .as_str()
        .unwrap()
        .contains("attributionJournal=attribution_journal_latest"));
    assert!(payload["exportRefs"]["csv"]["uri"]
        .as_str()
        .unwrap()
        .contains("attribution-journal"));
    assert!(payload["replayRefs"]["commands"][0]
        .as_str()
        .unwrap()
        .contains("attribution-journal"));
    assert_eq!(payload["sideEffects"]["brokerStateChanged"], false);
    assert_eq!(envelope["authority"]["surface"], "cli");
    assert!(!envelope.to_string().contains("api_secret"));
    assert!(!envelope.to_string().contains("Bearer "));
    assert!(!envelope.to_string().to_lowercase().contains("should buy"));
}

#[test]
fn unsupported_mcp_transport_fails_with_stable_exit_code() {
    let output = tradeassembly()
        .args(["mcp", "serve", "--transport", "http"])
        .output()
        .expect("mcp unsupported transport");
    assert_eq!(output.status.code(), Some(2));
    let stderr = String::from_utf8(output.stderr).expect("utf8 stderr");
    assert!(stderr.contains("only stdio MCP transport is supported"));
}

#[test]
fn anonymous_cli_mutation_is_denied_and_auth_status_is_redacted() {
    let root = temp_db("anonymous-oidc-session");
    std::fs::create_dir_all(&root).expect("anonymous session directory");
    let session_path = root.join("missing-session.bin");
    let key_path = root.join("missing-session.key");

    let mut mutation = Command::new(tradeassembly_binary());
    configure_warden(&mut mutation);
    configure_test_oidc(&mut mutation);
    let output = mutation
        .env("TRADEASSEMBLY_AUTH_CLI_SESSION_PATH", &session_path)
        .env("TRADEASSEMBLY_AUTH_CLI_SESSION_KEY_PATH", &key_path)
        .arg("--db")
        .arg(temp_db("anonymous-cli"))
        .args(["strategy", "create", "--name", "must-not-exist"])
        .output()
        .expect("anonymous CLI mutation");
    assert_eq!(output.status.code(), Some(1));
    let payload = json_stdout(output.stdout);
    assert_eq!(payload["error"]["code"], "oidc_session_required");
    assert!(!payload.to_string().contains("must-not-exist"));

    let mut status = Command::new(tradeassembly_binary());
    configure_warden(&mut status);
    configure_test_oidc(&mut status);
    let output = status
        .env("TRADEASSEMBLY_AUTH_CLI_SESSION_PATH", &session_path)
        .env("TRADEASSEMBLY_AUTH_CLI_SESSION_KEY_PATH", &key_path)
        .arg("--db")
        .arg(temp_db("anonymous-status"))
        .args(["auth", "status"])
        .output()
        .expect("anonymous auth status");
    assert!(output.status.success());
    let payload = json_stdout(output.stdout);
    assert_eq!(payload["authenticated"], false);
    assert_eq!(payload["reason"], "oidc_session_required");
    assert!(!payload.to_string().contains("token"));
}

#[test]
fn cli_loads_runtime_configuration_file_before_constructing_the_service() {
    let database = temp_db("config-file-database");
    let config = temp_db("config-file").with_extension("json");
    std::fs::write(
        &config,
        serde_json::json!({
            "profile": "local",
            "databasePath": database,
            "artifactRoot": ".tradeassembly/test-exports"
        })
        .to_string(),
    )
    .expect("write runtime config");

    let mut command = Command::new(tradeassembly_binary());
    configure_warden(&mut command);
    let output = command
        .arg("--config")
        .arg(&config)
        .arg("readiness")
        .output()
        .expect("run tradeassembly with config file");

    let _ = std::fs::remove_file(&config);
    assert!(
        output.status.success(),
        "config-backed command failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let envelope = json_stdout(output.stdout);
    assert_eq!(envelope["ok"], true);
}

#[test]
fn incomplete_self_hosted_environment_fails_closed_before_command_execution() {
    let output = Command::new(tradeassembly_binary())
        .env_remove("TRADEASSEMBLY_AUTH_ISSUER")
        .env_remove("TRADEASSEMBLY_AUTH_AUDIENCE")
        .env_remove("TRADEASSEMBLY_AUTH_CLIENT_ID")
        .env_remove("TRADEASSEMBLY_POSTGRES_URL_REF")
        .env_remove("TRADEASSEMBLY_NATS_URL_REF")
        .env_remove("TRADEASSEMBLY_OBJECT_STORE_ENDPOINT")
        .env("TRADEASSEMBLY_RUNTIME_PROFILE", "self_hosted")
        .arg("readiness")
        .output()
        .expect("run incomplete self-hosted command");

    assert_eq!(output.status.code(), Some(2));
    let stderr = String::from_utf8(output.stderr).expect("utf8 stderr");
    assert!(stderr.contains("runtime configuration failed"));
    assert!(!stderr.contains("api_secret"));
    assert!(!stderr.contains("Bearer "));
}

#[test]
fn clap_validation_failures_use_stable_exit_code() {
    let output = tradeassembly()
        .args(["strategy", "get"])
        .output()
        .expect("missing arg failure");
    assert_eq!(output.status.code(), Some(2));
    let stderr = String::from_utf8(output.stderr).expect("utf8 stderr");
    assert!(stderr.contains("required"));
}

fn tradeassembly() -> Command {
    let mut command = Command::new(tradeassembly_binary());
    configure_warden(&mut command);
    configure_verified_oidc_session(&mut command);
    command.arg("--db").arg(temp_db("cli"));
    command
}

fn configure_test_oidc(command: &mut Command) {
    command
        .env("TRADEASSEMBLY_AUTH_PROFILE", "oidc_test")
        .env(
            "TRADEASSEMBLY_AUTH_ISSUER",
            "http://127.0.0.1:8080/realms/tradeassembly-dev",
        )
        .env("TRADEASSEMBLY_AUTH_AUDIENCE", "tradeassembly-local")
        .env("TRADEASSEMBLY_AUTH_CLIENT_ID", "tradeassembly-local");
}

fn configure_verified_oidc_session(command: &mut Command) {
    const AAD: &[u8] = b"tradeassembly.auth-oidc-session.v1";
    let root = temp_db("oidc-session");
    std::fs::create_dir_all(&root).expect("OIDC session directory");
    let key_path = root.join("session.key");
    let session_path = root.join("session.bin");
    let key = [0x42_u8; 32];
    let nonce = [0x24_u8; 12];
    let expires_at_ms = i64::try_from(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_millis(),
    )
    .unwrap()
        + 300_000;
    let plaintext = serde_json::to_vec(&json!({
        "version": 1,
        "identity": {
            "stableIdentityId": "identity_cli_parity",
            "issuer": "http://127.0.0.1:8080/realms/tradeassembly-dev",
            "subject": "cli-parity-user",
            "audience": ["tradeassembly-local"],
            "displayName": "CLI Parity User",
            "email": "cli-parity@example.invalid",
            "assurance": null,
            "expiresAtMs": expires_at_ms
        },
        "refreshToken": null
    }))
    .expect("serialize OIDC session");
    let ciphertext = Aes256Gcm::new_from_slice(&key)
        .expect("session cipher")
        .encrypt(
            Nonce::from_slice(&nonce),
            Payload {
                msg: &plaintext,
                aad: AAD,
            },
        )
        .expect("encrypt OIDC session");
    let mut envelope = vec![1_u8];
    envelope.extend_from_slice(&nonce);
    envelope.extend_from_slice(&ciphertext);
    std::fs::write(&key_path, URL_SAFE_NO_PAD.encode(key)).expect("write OIDC key");
    std::fs::write(&session_path, envelope).expect("write OIDC session");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&key_path, std::fs::Permissions::from_mode(0o600))
            .expect("protect OIDC key");
        std::fs::set_permissions(&session_path, std::fs::Permissions::from_mode(0o600))
            .expect("protect OIDC session");
    }
    command
        .env("TRADEASSEMBLY_AUTH_PROFILE", "oidc_test")
        .env(
            "TRADEASSEMBLY_AUTH_ISSUER",
            "http://127.0.0.1:8080/realms/tradeassembly-dev",
        )
        .env("TRADEASSEMBLY_AUTH_AUDIENCE", "tradeassembly-local")
        .env("TRADEASSEMBLY_AUTH_CLIENT_ID", "tradeassembly-local")
        .env(
            "TRADEASSEMBLY_AUTH_CLI_SESSION_KEY_PATH",
            key_path.as_os_str(),
        )
        .env(
            "TRADEASSEMBLY_AUTH_CLI_SESSION_PATH",
            session_path.as_os_str(),
        );
}

fn parse_backtests(args: &[&str]) -> BacktestsCommand {
    let mut argv = vec!["tradeassembly", "backtests"];
    argv.extend_from_slice(args);
    let parsed = Cli::try_parse_from(argv).expect("backtests command parses");
    let CliCommand::Backtests { command } = parsed.command else {
        panic!("expected backtests command");
    };
    command
}

fn parse_robustness(args: &[&str]) -> RobustnessCommand {
    let mut argv = vec!["tradeassembly", "robustness"];
    argv.extend_from_slice(args);
    let parsed = Cli::try_parse_from(argv).expect("robustness command parses");
    let CliCommand::Robustness { command } = parsed.command else {
        panic!("expected robustness command");
    };
    command
}

fn parse_derivatives(args: &[&str]) -> DerivativesCommand {
    let mut argv = vec!["tradeassembly", "derivatives"];
    argv.extend_from_slice(args);
    let parsed = Cli::try_parse_from(argv).expect("derivatives command parses");
    let CliCommand::Derivatives { command } = parsed.command else {
        panic!("expected derivatives command");
    };
    command
}

fn parse_comparison(args: &[&str]) -> ComparisonCommand {
    let mut argv = vec!["tradeassembly", "comparison"];
    argv.extend_from_slice(args);
    let parsed = Cli::try_parse_from(argv).expect("comparison command parses");
    let CliCommand::Comparison { command } = parsed.command else {
        panic!("expected comparison command");
    };
    command
}

fn configure_warden(command: &mut Command) {
    command
        .env("TRADEASSEMBLY_WARDEN_SIDECAR_URL", test_warden_url())
        .env(
            "TRADEASSEMBLY_WARDEN_TOKEN_REF",
            "env://TRADEASSEMBLY_TEST_WARDEN_TOKEN",
        )
        .env("TRADEASSEMBLY_TEST_WARDEN_TOKEN", TEST_WARDEN_TOKEN);
}

#[derive(Debug, Default)]
struct TestWardenState {
    receipts: Mutex<BTreeMap<String, Value>>,
}

fn test_warden_url() -> &'static str {
    static URL: OnceLock<String> = OnceLock::new();
    URL.get_or_init(|| {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("Warden listener");
        listener
            .set_nonblocking(true)
            .expect("nonblocking Warden listener");
        let address = listener.local_addr().expect("Warden address");
        let state = Arc::new(TestWardenState::default());
        std::thread::spawn(move || {
            let runtime = tokio::runtime::Runtime::new().expect("Warden runtime");
            runtime.block_on(async move {
                let listener =
                    tokio::net::TcpListener::from_std(listener).expect("tokio Warden listener");
                let router = Router::new()
                    .route("/health", get(test_warden_health))
                    .route("/v1/policies", post(test_warden_policy))
                    .route("/v1/actions/authorize", post(test_warden_authorize))
                    .route("/v1/receipts/{receipt_id}", get(test_warden_receipt))
                    .with_state(state);
                axum::serve(listener, router)
                    .await
                    .expect("test Warden serves");
            });
        });
        format!("http://{address}")
    })
}

async fn test_warden_health() -> Json<Value> {
    Json(json!({
        "status": "ok",
        "service": "warden-sidecar",
        "version": "0.1.0"
    }))
}

async fn test_warden_policy(Json(_bundle): Json<Value>) -> Json<Value> {
    Json(json!({
        "policy_version": {
            "bundle_id": "tradeassembly-studio-core",
            "bundle_version": "cli-test",
            "bundle_digest": format!("sha256:{}", "1".repeat(64)),
            "projection_digest": format!("sha256:{}", "2".repeat(64))
        }
    }))
}

async fn test_warden_authorize(
    State(state): State<Arc<TestWardenState>>,
    Json(request): Json<Value>,
) -> Json<Value> {
    let receipt_id = format!(
        "receipt:{}",
        request["request_id"].as_str().unwrap_or("missing")
    );
    let receipt_hash = format!("sha256:{}", "3".repeat(64));
    let receipt = json!({
        "schema_version": "apf.receipt_base.v1",
        "receipt_id": receipt_id,
        "receipt_type": "action_authorization",
        "action": request["action"],
        "resource": request["resource"],
        "decision": "allow",
        "pep_id": request["pep_id"],
        "request_digest": request["context"]["context_digest"],
        "receipt_hash": receipt_hash,
    });
    let signed = json!({
        "schema_version": "warden.signed_receipt.v2",
        "receipt": receipt,
        "signature": {
            "schema_version": "warden.receipt_signature.v2",
            "algorithm": "ed25519",
            "key_id": "cli-test-key",
            "signed_at_epoch_ms": 1,
            "receipt_hash": receipt_hash,
            "signature": "cli-test-signature"
        }
    });
    state
        .receipts
        .lock()
        .expect("receipt lock")
        .insert(receipt_id, signed.clone());
    Json(json!({
        "schema_version": "apf.action_authorization_decision.v1",
        "decision_id": format!("decision:{}", request["request_id"].as_str().unwrap_or("missing")),
        "request_id": request["request_id"],
        "idempotency_key": request["idempotency_key"],
        "decision": "allow",
        "reasons": ["cli_test_policy"],
        "pep_id": request["pep_id"],
        "receipt": signed["receipt"]
    }))
}

async fn test_warden_receipt(
    State(state): State<Arc<TestWardenState>>,
    AxumPath(receipt_id): AxumPath<String>,
) -> Json<Value> {
    Json(
        state
            .receipts
            .lock()
            .expect("receipt lock")
            .get(&receipt_id)
            .cloned()
            .expect("authorized receipt"),
    )
}

fn assert_help_for_non_runnable_command(command: &str) {
    let args = match command {
        "agent install" => vec!["agent", "install", "--help"],
        "agent status" => vec!["agent", "status", "--help"],
        "agent uninstall" => vec!["agent", "uninstall", "--help"],
        "agent verify" => vec!["agent", "verify", "--help"],
        "api" => vec!["api", "--help"],
        "mcp serve" => vec!["mcp", "serve", "--help"],
        "plugins install" => vec!["plugins", "install", "--help"],
        "plugins invoke" => vec!["plugins", "invoke", "--help"],
        _ => unreachable!("not a non-runnable inventory command"),
    };
    let output = tradeassembly().args(args).output().expect("help command");
    assert!(output.status.success(), "{command} help failed");
    let stdout = String::from_utf8(output.stdout).expect("utf8 help");
    assert!(stdout.contains("Usage:"));
}

fn runnable_args(command: &str) -> Vec<&str> {
    match command {
        "strategy get" => vec!["strategy", "get", "strat_local_btc_demo"],
        "demos run" => vec!["demos", "run", "local-simbroker-paper-btc"],
        "dataset-ingestion create" => {
            vec!["dataset-ingestion", "create", "--request-json", "{}"]
        }
        "dataset-ingestion get" => {
            vec!["dataset-ingestion", "get", "ingestion_missing"]
        }
        "dataset-ingestion status" => {
            vec!["dataset-ingestion", "status", "ingestion_missing"]
        }
        "dataset-ingestion cancel" => {
            vec!["dataset-ingestion", "cancel", "ingestion_missing"]
        }
        "dataset-ingestion verify" => {
            vec!["dataset-ingestion", "verify", "ingestion_missing"]
        }
        "execution activate" => vec![
            "execution",
            "activate",
            "--config-id",
            "cfg_cli_manifest",
            "--idempotency-key",
            "cli-manifest-activate",
        ],
        "execution control" => vec![
            "execution",
            "control",
            "--activation-id",
            "activation_cli_manifest",
            "--action",
            "pause_entries",
        ],
        "execution deactivate" => vec![
            "execution",
            "deactivate",
            "--activation-id",
            "activation_cli_manifest",
        ],
        "execution readiness" => vec!["execution", "readiness", "--config-id", "cfg_cli_manifest"],
        "plugins disable" => vec!["plugins", "disable", "sim"],
        "telemetry opt-out" => vec!["telemetry", "opt-out", "true"],
        other => other.split_whitespace().collect(),
    }
}

struct CompletedRobustnessFixture {
    _directory: TempDir,
    service: TradeAssemblyService,
    run_id: String,
    result_hash: Value,
}

fn completed_robustness_fixture(name: &str) -> CompletedRobustnessFixture {
    let directory = tempfile::tempdir().expect("temporary directory");
    let db = directory
        .path()
        .join(format!("{name}.db"))
        .display()
        .to_string();
    let service = TradeAssemblyService::test_local(&db);
    let spec = robustness_strategy_spec();
    let spec_hash = canonical_hash(&spec, "strategy hash").expect("strategy hash");
    service
        .runtime()
        .storage
        .put_json(
            "strategy_versions",
            "strategy-1:1",
            json!({
                "id": "version-1",
                "strategyId": "strategy-1",
                "version": 1,
                "specHash": spec_hash,
                "spec": spec,
                "published": true,
                "immutable": true
            }),
            &robustness_context(&format!("{name}-strategy")),
        )
        .expect("strategy version");
    let snapshot = robustness_snapshot();
    service
        .runtime()
        .dataset_snapshots
        .put(&snapshot, &robustness_context(&format!("{name}-dataset")))
        .expect("dataset snapshot");
    let created_backtest = service.handle_http(
        "POST",
        "/backtests",
        json!({
            "strategyId": "strategy-1",
            "strategyVersionId": "version-1",
            "datasetId": snapshot.dataset_id,
            "startingCashMicros": 1_000 * UNIT,
            "fillTiming": "next_bar",
            "fillPriceSource": "open",
            "endOfData": "mark_to_market",
            "fixedFeeMicros": 0,
            "perUnitFeeMicros": 0,
            "notionalFeeBps": 0,
            "spreadBps": 0,
            "slippageBps": 0,
            "maximumParticipationBps": 10_000,
            "minimumVolumeMicros": UNIT,
            "maximumPositionNotionalMicros": 1_000 * UNIT,
            "maximumOrderQuantityMicros": 10 * UNIT,
            "client": "self",
            "purpose": "strategy_backtest_research",
            "idempotencyKey": format!("{name}-backtest-create")
        }),
    );
    assert_eq!(created_backtest.status, 202, "{created_backtest:#?}");
    let backtest_id = required_text(&created_backtest.body, "runId");
    let completed_backtest = service.handle_http(
        "POST",
        "/backtests:process",
        json!({"worker": format!("{name}-backtest-worker")}),
    );
    assert_eq!(completed_backtest.status, 200, "{completed_backtest:#?}");

    let assumptions = serde_json::to_value(StudyInput::MonteCarlo(MonteCarloInput {
        path_count: 32,
        confidence_level: 0.9,
        ruin_equity_ratio: 0.5,
        mode: MonteCarloMode::TradeBootstrap,
    }))
    .expect("typed assumptions");
    let created = service.handle_http(
        "POST",
        "/robustness-runs",
        json!({
            "sourceRunId": backtest_id,
            "studyKind": "monte_carlo",
            "assumptions": assumptions,
            "budget": {
                "maximumSamples": 32,
                "maximumGridPoints": 10,
                "maximumWindows": 10,
                "maximumScenarios": 10,
                "maximumOutputBytes": 1_000_000,
                "maximumAttempts": 2
            },
            "deterministicSeed": 23,
            "idempotencyKey": format!("{name}-robustness-create"),
            "client": "self",
            "purpose": "strategy_robustness_research"
        }),
    );
    assert_eq!(created.status, 202, "{created:#?}");
    let run_id = required_text(&created.body, "runId");
    let completed = service.handle_http(
        "POST",
        "/robustness-runs:process",
        json!({"worker": format!("{name}-robustness-worker")}),
    );
    assert_eq!(completed.status, 200, "{completed:#?}");
    assert_eq!(completed.body["state"], "completed");
    CompletedRobustnessFixture {
        _directory: directory,
        service,
        run_id,
        result_hash: completed.body["resultHash"].clone(),
    }
}

fn robustness_strategy_spec() -> Value {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../examples/strategy-spec/v3/valid/static-equity.json");
    let mut spec: Value =
        serde_json::from_slice(&fs::read(path).expect("strategy fixture")).expect("strategy JSON");
    spec["stages"]["evaluate"]["substeps"][0]["config"] = json!({"tradeassembly.expression": "bar.close > 101000000", "tradeassembly.quantity": UNIT});
    spec["stages"]["exit_policy"]["substeps"][0]["config"]["tradeassembly.expression"] =
        json!("bar.close < 100000000");
    spec
}

fn robustness_snapshot() -> DatasetSnapshot {
    DatasetSnapshot::build(
        DatasetSnapshotContent {
            schema: DATASET_SNAPSHOT_SCHEMA.to_string(),
            strategy_id: "strategy-1".to_string(),
            strategy_version_id: Some("version-1".to_string()),
            source: DatasetSourceManifest {
                source_class: DatasetSourceClass::Test,
                plugin_instance_ref: "local-data".to_string(),
                plugin_ref: "tradeassembly.local-data".to_string(),
                operation_id: "marketdata.bars.read_v1".to_string(),
                plugin_manifest_fingerprint: "sha256:plugin".to_string(),
                capability_graph_revision_id: "graph-1".to_string(),
                capability_graph_fingerprint: "sha256:graph".to_string(),
                source_request_hash: "sha256:request".to_string(),
                upstream_refs: BTreeMap::new(),
            },
            asset_classes: vec![AssetClass::Equity],
            instruments: vec!["SPY".to_string()],
            data_kind: HistoricalDataKind::Bars,
            granularity: "1h".to_string(),
            time_slice: DatasetTimeSlice {
                start: "2026-01-01T00:00:00Z".to_string(),
                end: "2026-01-01T04:00:00Z".to_string(),
            },
            calendar: "XNYS".to_string(),
            timezone: "UTC".to_string(),
            normalization_policy: DatasetNormalizationPolicy {
                timestamp_unit: "rfc3339".to_string(),
                timezone: "UTC".to_string(),
                duplicate_policy: "reject".to_string(),
                price_adjustment: "none".to_string(),
            },
            quality_policy: DatasetQualityPolicy {
                missing_intervals: QualityDisposition::Reject,
                stale_observations: QualityDisposition::Reject,
                outliers: QualityDisposition::Allow,
                invalid_markets: QualityDisposition::Reject,
                calendar_mismatch: QualityDisposition::Reject,
                corporate_action_gaps: QualityDisposition::Reject,
                missing_derivative_fields: QualityDisposition::Allow,
            },
            quality_findings: vec![],
            observations: vec![
                robustness_bar("2026-01-01T00:00:00Z", 100.0, 102.0),
                robustness_bar("2026-01-01T01:00:00Z", 103.0, 104.0),
                robustness_bar("2026-01-01T02:00:00Z", 99.0, 98.0),
                robustness_bar("2026-01-01T03:00:00Z", 97.0, 97.0),
            ],
        },
        1,
    )
    .expect("snapshot")
}

fn robustness_bar(timestamp: &str, open: f64, close: f64) -> HistoricalObservation {
    HistoricalObservation {
        instrument_id: "SPY".to_string(),
        timestamp: timestamp.to_string(),
        data: HistoricalObservationData::Bar(BarObservation {
            open,
            high: open.max(close),
            low: open.min(close),
            close,
            volume: 100.0,
        }),
    }
}

fn robustness_context(key: &str) -> SideEffectContext {
    SideEffectContext::new(
        AuthorityContext::local_cli(),
        IdempotencyKey::new(key).expect("idempotency key"),
    )
}

fn required_text(value: &Value, key: &str) -> String {
    value[key]
        .as_str()
        .unwrap_or_else(|| panic!("missing {key}: {value:#}"))
        .to_string()
}

fn tradeassembly_binary() -> PathBuf {
    if let Some(binary) = option_env!("CARGO_BIN_EXE_tradeassembly") {
        return PathBuf::from(binary);
    }
    if let Ok(binary) = std::env::var("CARGO_BIN_EXE_tradeassembly") {
        return PathBuf::from(binary);
    }
    let mut binary = std::env::current_exe()
        .expect("test executable path")
        .parent()
        .expect("test executable directory")
        .parent()
        .expect("target debug directory")
        .join("tradeassembly");
    if cfg!(windows) {
        binary.set_extension("exe");
    }
    if !binary.exists() {
        let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml");
        let status = Command::new(std::env::var("CARGO").unwrap_or_else(|_| "cargo".to_string()))
            .arg("build")
            .arg("--manifest-path")
            .arg(manifest)
            .args(["--bin", "tradeassembly"])
            .status()
            .expect("build tradeassembly binary");
        assert!(status.success(), "build tradeassembly binary failed");
    }
    binary
}

fn temp_db(name: &str) -> PathBuf {
    static NEXT_DB: AtomicU64 = AtomicU64::new(0);
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock before unix epoch")
        .as_nanos();
    let sequence = NEXT_DB.fetch_add(1, Ordering::Relaxed);
    std::env::temp_dir().join(format!(
        "tradeassembly-cli-parity-{name}-{}-{stamp}-{sequence}.db",
        std::process::id(),
    ))
}

fn json_stdout(stdout: Vec<u8>) -> Value {
    let text = String::from_utf8(stdout).expect("utf8 stdout");
    serde_json::from_str(&text).unwrap_or_else(|error| panic!("parse CLI JSON: {error}: {text}"))
}
