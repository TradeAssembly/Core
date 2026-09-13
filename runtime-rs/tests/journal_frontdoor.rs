use clap::Parser;
use serde_json::{json, Value};
use tempfile::NamedTempFile;
use tradeassembly_runtime::cli::{execute_command, Cli, Command, JournalCommand};
use tradeassembly_runtime::mcp;
use tradeassembly_runtime::service::TradeAssemblyService;

fn owner_service() -> TradeAssemblyService {
    let db = NamedTempFile::new().expect("temporary journal database");
    let path = db.into_temp_path().keep().expect("keep fixture database");
    let base = TradeAssemblyService::test_local(path.to_string_lossy().to_string());
    base.for_authenticated_invocation(
        "https://issuer.test",
        "owner-journal-frontdoor",
        Some("owner@example.test".to_string()),
        None,
    )
}

fn create_strategy(service: &TradeAssemblyService) {
    let response = service.handle_http_from_source(
        "test",
        "POST",
        "/product/strategies/create",
        json!({"mode": "blank", "name": "Journal frontdoor"}),
    );
    assert_eq!(response.status, 201, "{:#?}", response.body);
}

#[test]
fn cli_journal_commands_parse_and_execute_shared_handlers() {
    for (subcommand, expected_path) in [
        ("list", "/journal/events"),
        ("export", "/journal/export"),
        ("replay", "/journal/replay-report"),
    ] {
        let cli = Cli::try_parse_from(["tradeassembly", "journal", subcommand])
            .expect("journal CLI parses");
        let Command::Journal { command } = cli.command else {
            panic!("journal command did not parse");
        };
        match (expected_path, command) {
            ("/journal/events", JournalCommand::List)
            | ("/journal/export", JournalCommand::Export)
            | ("/journal/replay-report", JournalCommand::Replay) => {}
            _ => panic!("wrong journal subcommand"),
        }
    }
}

#[test]
fn authenticated_cli_and_mcp_export_match_and_have_contract_fields() {
    let service = owner_service();
    create_strategy(&service);

    let cli = execute_command(
        &service,
        Command::Journal {
            command: JournalCommand::Export,
        },
    );
    let mcp_result = service.call_mcp_tool("tradeassembly.journal.export", json!({}));
    assert_eq!(mcp_result["isError"], false);
    assert_eq!(cli, mcp_result["structuredContent"]);
    assert_eq!(cli["schemaVersion"], 1);
    assert_eq!(cli["scope"], "authenticated_owner");
    assert_eq!(cli["legacyProjection"], "owned_strategy_events_only");
    assert!(cli["events"].is_array());
    assert!(cli["eventsSha256"]
        .as_str()
        .is_some_and(|digest| digest.len() == 64));

    let list = execute_command(
        &service,
        Command::Journal {
            command: JournalCommand::List,
        },
    );
    assert!(list.is_array());
    let replay = execute_command(
        &service,
        Command::Journal {
            command: JournalCommand::Replay,
        },
    );
    assert_eq!(replay["mode"], "stored_journal_inspection");
    assert_eq!(replay["strategyEvaluationPerformed"], false);
}

#[test]
fn journal_mcp_discovery_is_read_only_and_generic_fallback_fails_closed() {
    let definitions = mcp::tool_definitions();
    for name in [
        "tradeassembly.journal.list",
        "tradeassembly.journal.export",
        "tradeassembly.journal.replay",
    ] {
        let spec = definitions
            .as_array()
            .unwrap()
            .iter()
            .find(|item| item["name"] == name)
            .expect("journal tool advertised");
        assert_eq!(spec["annotations"]["readOnlyHint"], true);
    }
    let fallback = mcp::call_tool("tradeassembly.journal.export", json!({}));
    assert_eq!(fallback["isError"], true);
    assert_eq!(
        fallback["structuredContent"]["error"]["code"],
        "service_required"
    );
}

#[test]
fn unauthenticated_journal_reads_are_denied() {
    let dir = tempfile::tempdir().unwrap();
    let service = TradeAssemblyService::test_local(dir.path().join("runtime.db").to_string_lossy());
    for (method, path) in [
        ("GET", "/journal/events"),
        ("GET", "/journal/export"),
        ("POST", "/journal/replay-report"),
    ] {
        let response = service.handle_http(method, path, Value::Object(Default::default()));
        assert_eq!(response.status, 401, "{path}: {:#?}", response.body);
    }
    let mcp_result = service.call_mcp_tool("tradeassembly.journal.export", json!({}));
    assert_eq!(mcp_result["isError"], true);
    assert_eq!(
        mcp_result["structuredContent"]["error"]["code"],
        "journal_owner_required"
    );
}
