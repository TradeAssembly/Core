use std::fs;
use std::path::PathBuf;
use tradeassembly_runtime::http::documented_routes;
use tradeassembly_runtime::mcp::tool_names;
use tradeassembly_runtime::service::GRAPHQL_OPERATIONS;
use tradeassembly_runtime::surfaces::cli_command_inventory;

#[test]
fn cli_manifest_matches_live_inventory() {
    assert_manifest("cli_commands.json", cli_command_inventory());
}

#[test]
fn http_manifest_matches_route_registry() {
    assert_manifest("http_routes.json", documented_routes());
}

#[test]
fn graphql_manifest_matches_operation_registry() {
    assert_manifest("graphql_operations.json", GRAPHQL_OPERATIONS.to_vec());
}

#[test]
fn mcp_manifest_matches_tool_registry() {
    assert_manifest("mcp_tools.json", tool_names());
}

fn assert_manifest(fixture_name: &str, actual: Vec<&'static str>) {
    let mut expected = fixture_array(fixture_name);
    expected.sort();
    let mut actual = actual.into_iter().map(str::to_string).collect::<Vec<_>>();
    actual.sort();
    assert_eq!(actual, expected, "parity fixture drifted: {fixture_name}");
}

fn fixture_array(name: &str) -> Vec<String> {
    let path = fixture_path(name);
    let text = fs::read_to_string(&path).unwrap_or_else(|error| {
        panic!("read parity fixture {}: {error}", path.display());
    });
    serde_json::from_str(&text).unwrap_or_else(|error| {
        panic!("parse parity fixture {}: {error}", path.display());
    })
}

fn fixture_path(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("tests")
        .join("fixtures")
        .join("parity")
        .join(name)
}
