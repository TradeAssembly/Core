use serde_json::{json, Value};
use std::path::Path;
use tradeassembly_runtime::agent_runner::{AgentRun, CodexCliAdapter, MCP_CAPABILITY_ENV};

fn run(resume: bool) -> AgentRun {
    AgentRun {
        run_id: "run".into(),
        deployment_id: "deployment".into(),
        trigger_id: "tick".into(),
        state: "running".into(),
        lease_fence: 1,
        deployment_binding_digest: "binding".into(),
        codex_session_ref: resume.then(|| "session".into()),
    }
}

#[test]
fn binding_requires_absolute_existing_files() {
    assert!(CodexCliAdapter::new(
        Path::new("relative"),
        Path::new("/missing"),
        Path::new("/missing")
    )
    .is_err());
    assert!(CodexCliAdapter::new(Path::new("/tmp"), Path::new("/tmp"), Path::new("/tmp")).is_err());
}

#[test]
fn codex_launcher_resolution_rejects_unrecognized_or_incomplete_packages() {
    use tradeassembly_runtime::agent_runner::resolve_codex_executable;
    let temp = tempfile::tempdir().unwrap();
    std::fs::create_dir(temp.path().join("bin")).unwrap();
    let launcher = temp.path().join("bin/codex.js");
    std::fs::write(&launcher, "#!/usr/bin/env node\n").unwrap();
    assert!(resolve_codex_executable(&launcher).is_err());
    std::fs::write(
        temp.path().join("package.json"),
        r#"{"name":"@openai/codex"}"#,
    )
    .unwrap();
    assert!(resolve_codex_executable(&launcher).is_err());
    let native = temp.path().join("native-codex");
    std::fs::write(&native, "fixture").unwrap();
    assert_eq!(
        resolve_codex_executable(&native).unwrap(),
        native.canonicalize().unwrap()
    );
}

#[test]
fn generic_command_dispatch_cannot_start_an_unbound_agent() {
    use tradeassembly_runtime::cli::{execute_command, AgentCommand, Command};
    let temp = tempfile::tempdir().unwrap();
    let service = tradeassembly_runtime::service::TradeAssemblyService::test_local(
        temp.path().join("runtime.db").to_str().unwrap(),
    );
    let response = execute_command(
        &service,
        Command::Agent {
            command: AgentCommand::Run {
                runner_id: "fixture".into(),
                once: true,
            },
        },
    );
    assert_eq!(response["error"]["code"], "agent_mcp_binding_required");
}

#[test]
fn fresh_and_resume_bind_explicit_server_without_capability_value() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("rig with spaces \"quote\" \\slash");
    std::fs::write(&path, "fixture").unwrap();
    let adapter = CodexCliAdapter::new(&path, &path, &path).unwrap();
    let mut previous = String::new();
    for resume in [false, true] {
        let args = adapter
            .command_args(
                &run(resume),
                &json!({"workspace":"/tmp/data", "tradeAssemblyMcpServer":"stale"}),
            )
            .unwrap();
        assert_eq!(args[0], "-c");
        let prompt: Value = serde_json::from_str(args.last().unwrap()).unwrap();
        let server = prompt["tradeAssemblyMcpServer"].as_str().unwrap();
        assert!(server.starts_with("tradeassembly_runner_"));
        assert_ne!(server, previous);
        previous = server.into();
        assert!(args[1].starts_with(&format!("mcp_servers.{server}=")));
        assert!(args[1].contains(MCP_CAPABILITY_ENV));
        assert!(args[1].contains("required=true"));
        assert_eq!(args[2], "exec");
        assert_eq!(args[3] == "resume", resume);
        assert!(!args
            .iter()
            .any(|arg| arg.contains("bypass") || arg.contains("ignore-user-config")));
    }
}

/// Uses the actual pinned CLI's config parser, but never starts a model or server.
#[test]
#[ignore = "requires F2_TEST_CODEX_BINARY pointing to the supported Codex CLI"]
fn actual_codex_preserves_other_servers_and_decodes_bound_paths() {
    let codex = std::env::var("F2_TEST_CODEX_BINARY").expect("explicit Codex binary required");
    assert!(Path::new(&codex).is_absolute());
    let native =
        tradeassembly_runtime::agent_runner::resolve_codex_executable(Path::new(&codex)).unwrap();
    let version = std::process::Command::new(native)
        .env_clear()
        .arg("--version")
        .output()
        .unwrap();
    assert!(
        version.status.success(),
        "native Codex must start without Node/PATH"
    );
    assert_eq!(
        String::from_utf8(version.stdout).unwrap().trim(),
        "codex-cli 0.152.1"
    );
    let temp = tempfile::tempdir().unwrap();
    let config = "[mcp_servers.tradeassembly]\nurl = \"https://invalid.example/mcp\"\nenabled = false\n[mcp_servers.other]\ncommand = \"/usr/bin/false\"\n";
    std::fs::write(temp.path().join("config.toml"), config).unwrap();
    let path = temp.path().join("rig \"quote\" \\slash é");
    std::fs::write(&path, "fixture").unwrap();
    let expected = path.canonicalize().unwrap();
    let adapter = CodexCliAdapter::new(&path, &path, &path).unwrap();
    for resume in [false, true] {
        let args = adapter.command_args(&run(resume), &json!({})).unwrap();
        let prompt: Value = serde_json::from_str(args.last().unwrap()).unwrap();
        let server = prompt["tradeAssemblyMcpServer"].as_str().unwrap();
        let query = |name: &str| {
            let output = std::process::Command::new(&codex)
                .env("CODEX_HOME", temp.path())
                .current_dir(temp.path())
                .args(&args[..2])
                .args(["mcp", "get", name, "--json"])
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "credential-free config query failed"
            );
            serde_json::from_slice::<Value>(&output.stdout).unwrap()
        };
        let bound = query(server);
        assert_eq!(bound["transport"]["command"], expected.to_str().unwrap());
        assert_eq!(bound["transport"]["args"][1], expected.to_str().unwrap());
        assert_eq!(bound["transport"]["args"][3], expected.to_str().unwrap());
        assert_eq!(bound["transport"]["env_vars"], json!([MCP_CAPABILITY_ENV]));
        assert!(bound["transport"]["env"].is_null());
        assert_eq!(bound["enabled"], true);
        assert_eq!(query("other")["transport"]["command"], "/usr/bin/false");
        assert_eq!(query("tradeassembly")["enabled"], false);
    }
    assert_eq!(
        std::fs::read_to_string(temp.path().join("config.toml")).unwrap(),
        config
    );
}
