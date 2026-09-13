// Copyright (c) 2026 OptionLab LLC. All rights reserved.

use regex::Regex;
use serde_json::json;
use serde_yaml::Value as YamlValue;
use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::fs;
use std::path::{Component, Path, PathBuf};
use std::sync::OnceLock;
use walkdir::WalkDir;

const REQUIRED_CONTEXTS: &[&str] = &[
    "strategy",
    "execution",
    "risk",
    "marketdata",
    "broker",
    "research",
    "plugins",
    "journal",
];
const REQUIRED_CONTEXT_LAYERS: &[&str] = &["domain", "commands", "ports", "adapters"];
const CANONICAL_RUNTIME_SURFACES: &[&str] = &[
    "runtime/src/tradeassembly/acp",
    "runtime/src/tradeassembly/application",
    "runtime/src/tradeassembly/application_commands.py",
    "runtime/src/tradeassembly/architecture",
    "runtime/src/tradeassembly/cli.py",
    "runtime/src/tradeassembly/contexts",
    "runtime/src/tradeassembly/mcp",
    "runtime/src/tradeassembly/platform",
];
const PROHIBITED_DOMAIN_IMPORT_ROOTS: &[&str] = &[
    "aiohttp",
    "aiokafka",
    "click",
    "axum",
    "graphql",
    "httpx",
    "kafka",
    "nats",
    "os",
    "psycopg",
    "requests",
    "sqlalchemy",
    "sqlite3",
    "strawberry",
];
const PROHIBITED_CONTEXT_COMMAND_PREFIXES: &[&str] = &[
    "tradeassembly.bus",
    "tradeassembly.engine",
    "tradeassembly.persistence",
    "tradeassembly.platform.events",
    "tradeassembly.platform.storage",
    "tradeassembly.store",
];
const PROHIBITED_CONTEXT_ADAPTER_PREFIXES: &[&str] = &[
    "tradeassembly.bus",
    "tradeassembly.bus.base",
    "tradeassembly.bus.envelope",
    "tradeassembly.bus.factory",
    "tradeassembly.engine",
    "tradeassembly.store",
    "tradeassembly.persistence",
    "tradeassembly.persistence.postgres",
];
const PROHIBITED_CONTEXT_ADAPTER_NAMES: &[&str] = &[
    "BusDelivery",
    "DurableBus",
    "LocalStore",
    "LocalRuntimeService",
    "MessageEnvelope",
    "PostgresStore",
    "create_bus",
];
const TRANSPORT_FILES: &[&str] = &[
    "runtime/src/tradeassembly/api.py",
    "runtime/src/tradeassembly/cli.py",
    "runtime/src/tradeassembly/acp/tools.py",
    "runtime/src/tradeassembly/mcp/tools.py",
];
const ROUTE_FILES: &[&str] = &["runtime/src/tradeassembly/api.py"];
const FORBIDDEN_ROUTE_RECEIVERS: &[&str] = &["service", "control", "runtime"];
const FORBIDDEN_ROUTE_PARAMETERS: &[&str] = &["service", "control", "runtime"];
const ROUTE_GROUPS_MIGRATED_OFF_SERVICE_COMMANDS: &[&str] = &[
    "local",
    "journal",
    "strategy",
    "marketdata",
    "provider",
    "plugins",
    "research",
    "risk",
    "execution",
    "scheduler",
    "product",
];
const UI_MARKERS: &[&str] = &["TODO", "FIXME", "placeholder", "stub", "mock", "local-data"];
const UI_SUFFIXES: &[&str] = &[".ts", ".tsx", ".js", ".jsx"];
const UI_EXCLUDED_PARTS: &[&str] = &[
    "node_modules",
    ".next",
    ".next-e2e",
    "test-results",
    "playwright-report",
];
const ACTIVE_DOCS: &[&str] = &[
    "AGENTS.md",
    "README.md",
    "docs/architecture.md",
    "docs/decisions.md",
    "docs/principles.md",
    "docs/audits/investigations.md",
    "docs/rust-rewrite-harness.md",
    "docs/source-parity-map.md",
];
const CURRENT_STATE_DOCS: &[&str] = &[
    "docs/source-parity-map.md",
    "docs/reference/current-system-design-and-data-model.md",
    "docs/reference/current-product-capabilities.md",
    "docs/audits/ui-parity-map.md",
];
const STALE_ACCEPTED_OWNER_CLAIMS: &[&str] = &[
    "partial plugin/capability/entitlement resolver",
    "execution and result workflows remain incomplete",
    "full run workflow is not ready",
    "continuous-operation and recovery acceptance remain open",
    "hosting and installation remain GC-37 work",
    "GC-37 still owns standalone executable Alpaca extraction and installation",
    "GC-11 still owns",
    "GC-11 through GC-13 still own",
    "GC-10 through GC-12 own",
    "GC-13 through GC-16",
    "GC-05 and GC-07 through GC-16 own",
    "GC-05 plus GC-07 through GC-16",
    "continuous execution and recovery remain unproved",
    "accepted cross-activation monitoring remains roadmap work",
];
const STALE_ACTIVE_DOC_TERMS: &[&str] = &[
    "runtime/src/tradeassembly",
    "runtime/.venv",
    "FastAPI",
    "uvicorn",
    "make check.ci",
    "make dev.api",
    "make dev.start",
    "cargo xtask legacy",
];
const HISTORICAL_DOCS: &[&str] = &[
    "docs/planning/roadmaps/tradeassembly-architecture-production-readiness-plan.md",
    "docs/planning/plans/2026-05-23-tradeassembly-production-runtime-source-parity.md",
    "docs/planning/plans/2026-06-24-rust-backend-full-parity-closure.md",
];
const HISTORICAL_MIGRATION_BANNER: &str =
    "Historical migration evidence; not an active runtime owner map.";
const ACTIVE_DOC_STALE_POLICY: &str = "active docs must not use Python-era runtime ownership language; move historical evidence behind the historical migration banner";
const SHELL_WRAPPER_FORBIDDEN_TERMS: &[&str] = &[
    "python3 -",
    "python -",
    "<<'PY'",
    "<<PY",
    "subprocess.Popen",
];
const GENERIC_REPOSITORY_METHODS: &[&str] = &[
    "get",
    "put",
    "list",
    "append_event",
    "list_events",
    "connect",
];

#[derive(Debug, PartialEq)]
pub struct ArchitectureReport {
    pub ok: bool,
    pub failures: Vec<String>,
}

#[derive(Debug)]
struct ReportBuilder {
    failures: Vec<String>,
}

impl ReportBuilder {
    fn new() -> Self {
        Self {
            failures: Vec::new(),
        }
    }

    fn require(&mut self, condition: bool, message: impl Into<String>) {
        if !condition {
            self.failures.push(message.into());
        }
    }

    fn finish(self) -> ArchitectureReport {
        ArchitectureReport {
            ok: self.failures.is_empty(),
            failures: self.failures,
        }
    }
}

#[derive(Clone, Debug)]
struct CompatibilityShim {
    shim_path: String,
    canonical_path: String,
}

#[derive(Clone, Debug, PartialEq)]
struct ImportViolation {
    path: String,
    line: usize,
    imported: String,
    rule: &'static str,
}

impl ImportViolation {
    fn message(&self) -> String {
        format!(
            "{}:{}: {}: {}",
            self.path, self.line, self.rule, self.imported
        )
    }
}

#[derive(Clone, Debug, PartialEq)]
struct InterfaceViolation {
    path: String,
    line: usize,
    rule: &'static str,
    detail: String,
}

impl InterfaceViolation {
    fn message(&self) -> String {
        format!(
            "{}:{}: {}: {}",
            self.path, self.line, self.rule, self.detail
        )
    }
}

#[derive(Clone, Debug, PartialEq)]
struct UiStubGap {
    path: String,
    line: usize,
    marker: &'static str,
    text: String,
}

pub fn run_architecture_check_command(args: &[String], root: &Path) -> i32 {
    let mut write_ui_stubs = false;
    for arg in args {
        match arg.as_str() {
            "--write-ui-stubs" | "--write" => write_ui_stubs = true,
            "--format" | "json" | "--json" => {}
            other => {
                eprintln!("unknown architecture-check option: {other}");
                return 1;
            }
        }
    }

    if write_ui_stubs {
        let target = root.join("docs/ui-stub-gaps.md");
        if let Err(error) = fs::write(&target, expected_ui_stub_gap_text(root)) {
            eprintln!("failed to write {}: {error}", target.display());
            return 1;
        }
    }

    let report = run_architecture_check(root);
    let payload = json!({
        "failures": report.failures,
        "ok": report.ok,
    });
    println!(
        "{}",
        serde_json::to_string_pretty(&payload).expect("serialize architecture report")
    );
    if report.ok {
        0
    } else {
        1
    }
}

pub fn run_contract_check_command(args: &[String], root: &Path) -> i32 {
    let checks = if args.is_empty() {
        vec!["deployment", "render-iac", "cloud-iac", "tooling"]
    } else {
        args.iter().map(String::as_str).collect()
    };

    let mut report = ReportBuilder::new();
    for check in checks {
        match check {
            "deployment" | "deployment-contract" => {
                check_deployment_artifact_contract(root, &mut report)
            }
            "render-iac" | "render-iac-contract" => {
                check_render_blueprint_contract(root, &mut report)
            }
            "cloud-iac" | "cloud-iac-contract" => check_cloud_iac_contract(root, &mut report),
            "tooling" | "tooling-contract" => check_tooling_recipes(root, &mut report),
            other => report.require(false, format!("unknown contract check: {other}")),
        }
    }
    let report = report.finish();
    let payload = json!({
        "failures": report.failures,
        "ok": report.ok,
    });
    println!(
        "{}",
        serde_json::to_string_pretty(&payload).expect("serialize contract report")
    );
    if report.ok {
        0
    } else {
        1
    }
}

pub fn run_architecture_check(root: &Path) -> ArchitectureReport {
    let mut report = ReportBuilder::new();
    check_docs(root, &mut report);
    check_future_state_canonical_docs(root, &mut report);
    check_identity_and_credential_contract_docs(root, &mut report);
    check_deployment_artifact_contract(root, &mut report);
    check_render_blueprint_contract(root, &mut report);
    check_docs_contract(root, &mut report);
    check_iac_contract(root, &mut report);
    check_cloud_iac_contract(root, &mut report);
    check_contexts(root, &mut report);
    check_codex_maker_checker_protocol(root, &mut report);
    check_local_agent_harness(root, &mut report);
    check_runtime_inventory(root, &mut report);
    check_platform(root, &mut report);
    check_application_command_surface(root, &mut report);
    check_cli_command_surface(root, &mut report);
    check_storage_repository_surface(root, &mut report);
    check_kafka_real_adapter_contract(root, &mut report);
    check_interface_rules(root, &mut report);
    check_graphql_operation_map(root, &mut report);
    check_compatibility_shim_ledger(root, &mut report);
    check_import_boundaries(root, &mut report);
    check_static_forbidden_patterns(root, &mut report);
    check_ui_stub_scan(root, &mut report);
    check_full_parity_gap_closure(root, &mut report);
    check_active_docs_do_not_claim_python_runtime(root, &mut report);
    check_historical_docs_are_classified(root, &mut report);
    check_shell_wrappers_are_thin(root, &mut report);
    check_tooling_recipes(root, &mut report);
    report.finish()
}

/// Portable runtime checks; Studio keeps the full mixed-repository profile.
pub(crate) fn run_core_architecture(root: &Path) -> i32 {
    let mut report = ReportBuilder::new();
    report.require(
        rust_rewrite_runtime_active(root),
        "Core requires the Rust runtime",
    );
    report.require(
        !root.join("runtime/src").exists(),
        "Core must not contain the retired runtime",
    );
    check_contexts(root, &mut report);
    check_runtime_inventory(root, &mut report);
    check_platform(root, &mut report);
    check_application_command_surface(root, &mut report);
    check_cli_command_surface(root, &mut report);
    check_storage_repository_surface(root, &mut report);
    check_kafka_real_adapter_contract(root, &mut report);
    check_import_boundaries(root, &mut report);
    check_interface_rules(root, &mut report);
    check_static_forbidden_patterns(root, &mut report);
    check_shell_wrappers_are_thin(root, &mut report);
    let report = report.finish();
    println!(
        "{}",
        serde_json::to_string_pretty(
            &json!({"profile":"core","ok":report.ok,"failures":report.failures})
        )
        .expect("architecture report")
    );
    if report.ok {
        0
    } else {
        1
    }
}

fn check_docs(root: &Path, report: &mut ReportBuilder) {
    for name in [
        "architecture.md",
        "decisions.md",
        "principles.md",
        "technical-design-guide.md",
        "planning/roadmaps/tradeassembly-architecture-standards-roadmap.md",
    ] {
        report.require(
            root.join("docs").join(name).exists(),
            format!("missing docs/{name}"),
        );
    }
    for name in [
        "architecture.md",
        "decisions.md",
        "principles.md",
        "technical-design-guide.md",
    ] {
        let rel = format!("docs/{name}");
        let Some(text) = read_rel(root, &rel) else {
            continue;
        };
        report.require(
            text.contains("NATS"),
            format!("docs/{name} must mention NATS JetStream"),
        );
        report.require(
            text.contains("Kafka"),
            format!("docs/{name} must mention optional Kafka compatibility"),
        );
    }
}

fn check_codex_maker_checker_protocol(root: &Path, report: &mut ReportBuilder) {
    let protocol_rel = ".sdlc/codex-review.md";
    let checks_rel = ".sdlc/checks.yaml";
    let wrapper_rel = "scripts/sdlc/codex-review";

    let Some(protocol) = read_rel(root, protocol_rel) else {
        report.require(false, format!("missing {protocol_rel}"));
        return;
    };
    let Some(checks) = read_rel(root, checks_rel) else {
        report.require(false, format!("missing {checks_rel}"));
        return;
    };
    let Some(agents) = read_rel(root, "AGENTS.md") else {
        report.require(
            false,
            "missing AGENTS.md for maker-checker contract".to_string(),
        );
        return;
    };
    let Some(wrapper) = read_rel(root, wrapper_rel) else {
        report.require(false, format!("missing {wrapper_rel}"));
        return;
    };
    let Some(xtask_main) = read_rel(root, "xtask/src/main.rs") else {
        report.require(false, "missing xtask/src/main.rs".to_string());
        return;
    };

    for term in [
        "No Codex concern may be silently dismissed.",
        "one initial review and one targeted re-review",
        "explicit human authorization",
        "Codex subprocess is unavailable or fails",
        "- resolution_status: resolved",
    ] {
        report.require(
            protocol.contains(term),
            format!("{protocol_rel} missing maker-checker term {term:?}"),
        );
    }
    for term in [
        "codex_maker_checker_required_for_authored_code: false",
        "automatic_review_disabled: true",
        "explicit_human_request_required: true",
        "resolution_record_required: true",
        "fresh_rereview_after_material_remediation: true",
        "unresolved_material_concern_blocks_slice: true",
        "maximum_review_rounds: 2",
        "third_round_requires_explicit_human_authorization_for_p0_p1: true",
        "default_review_mode: routine",
        "shared_review_limit_7d: 20",
        "shared_routine_review_limit_7d: 16",
        "reserved_high_risk_release_reviews_7d: 4",
        "deterministic_exemption_requires_nonbehavioral_proof: true",
    ] {
        report.require(
            checks.contains(term),
            format!("{checks_rel} missing maker-checker policy {term:?}"),
        );
    }
    report.require(
        agents.contains("Codex subprocess review is manual-only"),
        "AGENTS.md missing manual-only Codex review policy".to_string(),
    );
    for term in [
        "model-context.sh",
        "sdlc_export_model_context",
        "codex exec review",
        "--ephemeral",
        "--ignore-user-config",
        "--ignore-rules",
        "--base \"$base_ref\"",
        "native_output",
        "No actionable findings",
        "gpt-5.6-terra",
        "model_reasoning_effort",
        "model_context_window",
        "tool_output_token_limit",
        "CODEX_REVIEW_DAILY_LIMIT",
        "CODEX_REVIEW_WEEKLY_LIMIT",
        "CODEX_REVIEW_RESERVED_LIMIT",
        "CODEX_REVIEW_BUDGET_DIR",
        "tradeassembly-review-usage.lock",
        "routine_weekly_limit",
        "recorded_status=124",
        "cut -c1-500",
        "--mode",
        "--dry-run",
        "--manual",
        "env -u CODEX_THREAD_ID -u CODEX_MODEL -u AGENT_MODEL",
        "--intent",
        "--evidence",
        "--output",
        "reviewed_base:",
        "reviewed_diff_hash:",
    ] {
        report.require(
            wrapper.contains(term),
            format!("{wrapper_rel} missing wrapper safeguard {term:?}"),
        );
    }
    for term in [
        "mod review_evidence;",
        "review-evidence",
        "review_evidence::run_review_evidence",
    ] {
        report.require(
            xtask_main.contains(term),
            format!("xtask/src/main.rs missing manual review-evidence command {term:?}"),
        );
    }
    report.require(
        !xtask_main.contains("fn check_ci() -> i32 {\n    if review_evidence::run_review_evidence"),
        "cargo xtask verify must not invoke manual Codex review evidence".to_string(),
    );
}

fn check_local_agent_harness(root: &Path, report: &mut ReportBuilder) {
    let required_agents = [
        "goal-coordinator",
        "spec-architect",
        "ticket-implementer",
        "mechanical-transformer",
        "explorer",
        "invariant-tester",
        "high-risk-engineer",
        "debugger",
        "routine-reviewer",
        "closure-auditor",
    ];
    let required_files = [
        ".codex/README.md",
        ".codex/workflows/modes.md",
        ".codex/workflows/routing.md",
        ".codex/skills/tradeassembly-sdlc/SKILL.md",
        "scripts/sdlc/check-agent-harness",
    ];

    let Some(agents) = read_rel(root, "AGENTS.md") else {
        report.require(
            false,
            "missing AGENTS.md for local agent harness".to_string(),
        );
        return;
    };
    for term in [
        "Local agent harness:",
        "Select an operating mode before acting:",
        ".codex/README.md",
        ".codex/agents/",
        ".codex/workflows/",
        ".codex/skills/",
    ] {
        report.require(
            agents.contains(term),
            format!("AGENTS.md missing local harness term {term:?}"),
        );
    }

    for rel in required_files {
        report.require(root.join(rel).is_file(), format!("missing {rel}"));
    }
    for name in required_agents {
        let rel = format!(".codex/agents/{name}.md");
        let Some(role) = read_rel(root, &rel) else {
            report.require(false, format!("missing pinned agent role {rel}"));
            continue;
        };
        report.require(
            role.contains("model: gpt-5.6-"),
            format!("{rel} must pin a gpt-5.6 model"),
        );
        report.require(
            role.contains("model_reasoning_effort:"),
            format!("{rel} must pin reasoning effort"),
        );
    }

    for alias in ["CLAUDE.md", "GEMINI.md"] {
        match fs::read_link(root.join(alias)) {
            Ok(target) => report.require(
                target == Path::new("AGENTS.md"),
                format!("{alias} must symlink to AGENTS.md"),
            ),
            Err(error) => {
                report.require(false, format!("{alias} must symlink to AGENTS.md: {error}"))
            }
        }
    }

    if let Some(routing) = read_rel(root, ".codex/workflows/routing.md") {
        for term in [
            "Long-running Goal coordinator",
            "Sol Medium",
            "Final closure audit",
        ] {
            report.require(
                routing.contains(term),
                format!(".codex/workflows/routing.md missing {term:?}"),
            );
        }
    } else {
        report.require(false, "missing .codex/workflows/routing.md".to_string());
    }
    if let Some(verify) = read_rel(root, "scripts/sdlc/verify") {
        report.require(
            verify.contains("check-agent-harness"),
            "scripts/sdlc/verify must run check-agent-harness".to_string(),
        );
    } else {
        report.require(false, "missing scripts/sdlc/verify".to_string());
    }
}

fn check_future_state_canonical_docs(root: &Path, report: &mut ReportBuilder) {
    require_doc_terms(
        root,
        report,
        "docs/architecture.md",
        &[
            "Trust Fabric",
            "Warden Core",
            "Finance Profile",
            "Sightline",
            "signed receipt",
            "compliance-support",
            "Warden Core is a standalone",
        ],
    );
    require_doc_terms(
        root,
        report,
        "docs/decisions.md",
        &[
            "Warden Core Is Standalone",
            "Finance Profile",
            "Sightline",
            "compliance claims",
            "interface plus plugin/adapter",
        ],
    );
    require_doc_terms(
        root,
        report,
        "docs/sdlc.md",
        &[
            "Phase 10",
            "APF",
            "Sightline",
            "compliance-claim",
            "release evidence",
        ],
    );
    require_doc_terms(
        root,
        report,
        "docs/rust-rewrite-harness.md",
        &[
            "APF",
            "Sightline",
            "live/cloud",
            "compliance-claim",
            "Alpaca paper smoke",
        ],
    );
    require_doc_terms(
        root,
        report,
        "docs/repo-boundaries.md",
        &[
            "TradeAssemblyHQ/warden",
            "Warden Platform",
            "TradeAssemblyHQ/sightline",
            "signed-receipt references",
        ],
    );
    require_doc_terms(
        root,
        report,
        "docs/reference/current-product-capabilities.md",
        &[
            "Current V1 Coverage",
            "Backtesting V1",
            "Continuous Execution V1",
            "Sightline",
            "Blocked Until",
        ],
    );
    require_doc_terms(
        root,
        report,
        "docs/reference/current-system-design-and-data-model.md",
        &[
            "Current V1 Coverage",
            "Warden",
            "Sightline",
            "live/cloud",
            "service/execution.rs",
        ],
    );
}

fn check_identity_and_credential_contract_docs(root: &Path, report: &mut ReportBuilder) {
    require_doc_terms(
        root,
        report,
        "docs/reference/contracts/identity-install-contracts.md",
        &[
            "Issuer discovery",
            "Token validation",
            "Login",
            "Status",
            "Logout",
            "Install claim",
            "Metrics opt-out",
            "No broker/data credential custody",
            "Graceful hosted-service failure",
            "WorkOS/AuthKit",
        ],
    );
    require_doc_terms(
        root,
        report,
        "docs/reference/contracts/credential-custody-contracts.md",
        &[
            "Credential Handles",
            "Backend Policies",
            "Rotation",
            "Revocation",
            "Audit Events",
            "Alpaca Paper MVP",
            "macOS Keychain",
            "AWS Secrets Manager",
            "no hardcoded API keys",
            "no hosted TradeAssembly.org credential custody",
        ],
    );
}

fn require_doc_terms(root: &Path, report: &mut ReportBuilder, rel: &str, terms: &[&str]) {
    let Some(text) = read_rel(root, rel) else {
        report.require(false, format!("missing {rel}"));
        return;
    };
    for term in terms {
        report.require(text.contains(term), format!("{rel} must mention {term}"));
    }
}

fn check_deployment_artifact_contract(root: &Path, report: &mut ReportBuilder) {
    require_file_terms(
        root,
        report,
        "docker-compose.distributed.yml",
        &[
            "postgres:",
            "nats:",
            "api:",
            "scheduler:",
            "runner:",
            "oms:",
            "order-status:",
            "TRADEASSEMBLY_RUNTIME_PROFILE: ${TRADEASSEMBLY_RUNTIME_PROFILE:-self_hosted}",
            "TRADEASSEMBLY_POSTGRES_URL_REF: env://TRADEASSEMBLY_POSTGRES_URL",
            "TRADEASSEMBLY_POSTGRES_URL: postgresql://tradeassembly@postgres:5432/tradeassembly",
            "TRADEASSEMBLY_NATS_URL_REF: env://TRADEASSEMBLY_NATS_URL",
            "TRADEASSEMBLY_NATS_URL: nats://nats:4222",
            "tradeassembly-worker-loop",
            "http://127.0.0.1:8090/ready",
        ],
    );
    require_file_terms(
        root,
        report,
        "runtime/Dockerfile",
        &[
            "FROM rust:",
            "cargo build --release --bin tradeassembly",
            "COPY --from=build /tmp/tradeassembly /usr/local/bin/tradeassembly",
            "--mount=type=cache,target=/app/target",
            "COPY --from=build /app/deploy/worker-loop.sh /usr/local/bin/tradeassembly-worker-loop",
            "CMD [\"tradeassembly\", \"api\"",
        ],
    );
    require_file_terms(
        root,
        report,
        "deploy/production.env.example",
        &[
            "TRADEASSEMBLY_RUNTIME_PROFILE=self_hosted",
            "TRADEASSEMBLY_POSTGRES_URL_REF=env://TRADEASSEMBLY_POSTGRES_URL",
            "TRADEASSEMBLY_NATS_URL_REF=env://TRADEASSEMBLY_NATS_URL",
            "TRADEASSEMBLY_CREDENTIAL_BACKEND=env",
            "TRADEASSEMBLY_ENABLE_LIVE_TRADING=0",
        ],
    );
    if let Some(text) = read_rel(root, "deploy/production.env.example") {
        report.require(
            !text.contains("change-me"),
            "deploy/production.env.example must not contain change-me",
        );
    }
    require_file_terms(
        root,
        report,
        "deploy/worker-loop.sh",
        &[
            "scheduler)",
            "runner)",
            "oms)",
            "status)",
            "tradeassembly scheduler run",
            "tradeassembly workers run",
            "tradeassembly orders reconcile",
            "tradeassembly orders attempts",
        ],
    );
    for rel in [
        ".dockerignore",
        "docker-compose.distributed.yml",
        "deploy/production.env.example",
        "deploy/README.md",
        "deploy/worker-loop.sh",
        "runtime/Dockerfile",
        "render.yaml",
        "infra/render/production.env.example",
    ] {
        let Some(text) = read_rel(root, rel) else {
            report.require(false, format!("missing {rel}"));
            continue;
        };
        for secret in ["ALPACA_API_SECRET=", "ALPACA_API_KEY=", "AKIA", "ghp_"] {
            report.require(
                !text.contains(secret),
                format!("{rel} must not contain committed secret marker {secret}"),
            );
        }
    }
    require_file_terms(
        root,
        report,
        ".dockerignore",
        &[
            ".env",
            ".env.*",
            ".tradeassembly",
            "target",
            "runtime/.venv-research",
            "webapp/node_modules",
        ],
    );
    check_self_hosted_container_contract(root, report);
}

fn check_render_blueprint_contract(root: &Path, report: &mut ReportBuilder) {
    let Some(text) = read_rel(root, "render.yaml") else {
        report.require(false, "missing render.yaml");
        return;
    };
    let blueprint = match serde_yaml::from_str::<YamlValue>(&text) {
        Ok(value) => value,
        Err(error) => {
            report.require(false, format!("render.yaml is not valid YAML: {error}"));
            return;
        }
    };
    let service_names = yaml_seq(yaml_get(&blueprint, "services"))
        .into_iter()
        .filter_map(|service| yaml_string(yaml_get(service, "name")))
        .collect::<BTreeSet<_>>();
    for name in [
        "tradeassembly-api",
        "tradeassembly-studio",
        "tradeassembly-scheduler",
        "tradeassembly-runner",
        "tradeassembly-oms",
        "tradeassembly-order-status",
    ] {
        report.require(
            service_names.contains(name),
            format!("render.yaml missing service {name}"),
        );
    }
    let database_name = yaml_seq(yaml_get(&blueprint, "databases"))
        .first()
        .and_then(|database| yaml_string(yaml_get(database, "name")));
    report.require(
        database_name.as_deref() == Some("tradeassembly-postgres"),
        "render.yaml must define tradeassembly-postgres database",
    );

    for service in yaml_seq(yaml_get(&blueprint, "services")) {
        let name = yaml_string(yaml_get(service, "name")).unwrap_or_default();
        if name != "tradeassembly-studio" {
            let env = render_env_vars(service);
            for key in [
                "TRADEASSEMBLY_RUNTIME_PROFILE",
                "TRADEASSEMBLY_AUTH_ISSUER",
                "TRADEASSEMBLY_AUTH_AUDIENCE",
                "TRADEASSEMBLY_AUTH_CLIENT_ID",
                "TRADEASSEMBLY_POSTGRES_URL_REF",
                "TRADEASSEMBLY_POSTGRES_URL",
                "TRADEASSEMBLY_NATS_URL_REF",
                "TRADEASSEMBLY_NATS_URL",
                "TRADEASSEMBLY_OBJECT_STORE_ENDPOINT",
                "TRADEASSEMBLY_CREDENTIAL_BACKEND",
                "TRADEASSEMBLY_ENABLE_LIVE_TRADING",
            ] {
                report.require(
                    env.contains_key(key),
                    format!("render service {name} missing env var {key}"),
                );
            }
            report.require(
                env.get("TRADEASSEMBLY_RUNTIME_PROFILE")
                    .and_then(|value| value.as_deref())
                    == Some("self_hosted"),
                format!("render service {name} must use self_hosted runtime profile"),
            );
            report.require(
                env.get("TRADEASSEMBLY_CREDENTIAL_BACKEND")
                    .and_then(|value| value.as_deref())
                    == Some("env"),
                format!("render service {name} must use env credential backend"),
            );
            report.require(
                env.get("TRADEASSEMBLY_ENABLE_LIVE_TRADING")
                    .and_then(|value| value.as_deref())
                    == Some("0"),
                format!("render service {name} must disable live trading by default"),
            );
        }
        let command = ["dockerCommand", "startCommand", "buildCommand"]
            .into_iter()
            .filter_map(|key| yaml_string(yaml_get(service, key)))
            .collect::<Vec<_>>()
            .join("\n");
        for forbidden in ["npm run dev", "kafka-memory", "sqlite"] {
            report.require(
                !command.to_lowercase().contains(&forbidden.to_lowercase()),
                format!("render service {name} command contains dev-only token {forbidden}"),
            );
        }
        if name == "tradeassembly-studio" {
            report.require(
                yaml_string(yaml_get(service, "buildCommand"))
                    .is_some_and(|command| command.contains("npm run build")),
                "render studio must build with npm run build",
            );
            report.require(
                yaml_string(yaml_get(service, "startCommand"))
                    .is_some_and(|command| command.contains("npm run start")),
                "render studio must start with npm run start",
            );
        }
    }

    for rel in [
        "infra/render/README.md",
        "infra/render/production.env.example",
    ] {
        report.require(root.join(rel).exists(), format!("missing {rel}"));
    }
    if let Some(justfile) = read_rel(root, "Justfile") {
        report.require(
            justfile.contains("iac-render-validate:"),
            "Justfile missing iac-render-validate",
        );
        report.require(
            just_recipe_block(&justfile, "iac-render-validate").contains("contract render-iac"),
            "Justfile must route Render validation through the Rust xtask contract",
        );
    } else {
        report.require(false, "missing Justfile");
    }
}

fn require_file_terms(root: &Path, report: &mut ReportBuilder, rel: &str, terms: &[&str]) {
    let Some(text) = read_rel(root, rel) else {
        report.require(false, format!("missing {rel}"));
        return;
    };
    for term in terms {
        report.require(text.contains(term), format!("{rel} must contain {term}"));
    }
}

fn check_self_hosted_container_contract(root: &Path, report: &mut ReportBuilder) {
    require_file_terms(
        root,
        report,
        "runtime/Dockerfile",
        &["ca-certificates", "curl"],
    );
    require_file_terms(
        root,
        report,
        "docker-compose.distributed.yml",
        &[
            "[\"CMD\", \"curl\", \"--fail\"",
            "command: [\"tradeassembly\", \"seed-btc-demo\"]",
        ],
    );
    require_file_terms(
        root,
        report,
        "infra/ansible/templates/docker-compose.generated.yml.j2",
        &[
            "[\"CMD\", \"curl\", \"--fail\"",
            "command: [\"tradeassembly\", \"seed-btc-demo\"]",
        ],
    );
    require_file_terms(
        root,
        report,
        "infra/scripts/iac-doctor.sh",
        &["TRADEASSEMBLY_NATS_PORT"],
    );

    for (rel, forbidden) in [
        (
            "docker-compose.distributed.yml",
            "seed-btc-demo\", \"--provider-ref",
        ),
        ("infra/scripts/iac-doctor.sh", "TRADEASSEMBLY_KAFKA_PORT"),
        ("Makefile", "Postgres/Kafka/API/workers"),
    ] {
        if let Some(text) = read_rel(root, rel) {
            report.require(
                !text.contains(forbidden),
                format!("{rel} contains stale self-hosted token {forbidden}"),
            );
        }
    }
}

fn yaml_get<'a>(value: &'a YamlValue, key: &str) -> Option<&'a YamlValue> {
    value.as_mapping()?.get(YamlValue::String(key.to_string()))
}

fn yaml_seq(value: Option<&YamlValue>) -> Vec<&YamlValue> {
    value
        .and_then(YamlValue::as_sequence)
        .map(|items| items.iter().collect())
        .unwrap_or_default()
}

fn yaml_string(value: Option<&YamlValue>) -> Option<String> {
    value?.as_str().map(str::to_string)
}

fn rust_rewrite_runtime_active(root: &Path) -> bool {
    root.join("runtime-rs/src/lib.rs").exists()
}

fn render_env_vars(service: &YamlValue) -> BTreeMap<String, Option<String>> {
    let mut vars = BTreeMap::new();
    for item in yaml_seq(yaml_get(service, "envVars")) {
        let Some(key) = yaml_string(yaml_get(item, "key")) else {
            continue;
        };
        vars.insert(key, yaml_string(yaml_get(item, "value")));
    }
    vars
}

fn check_contexts(root: &Path, report: &mut ReportBuilder) {
    if rust_rewrite_runtime_active(root) {
        for module in [
            "runtime-rs/src/auth.rs",
            "runtime-rs/src/engine.rs",
            "runtime-rs/src/journal.rs",
            "runtime-rs/src/marketdata.rs",
            "runtime-rs/src/mcp.rs",
            "runtime-rs/src/risk.rs",
            "runtime-rs/src/spec.rs",
            "runtime-rs/src/storage.rs",
        ] {
            report.require(root.join(module).exists(), format!("missing {module}"));
        }
        return;
    }
    let base = root.join("runtime/src/tradeassembly/contexts");
    report.require(base.exists(), "missing runtime/src/tradeassembly/contexts");
    for context in REQUIRED_CONTEXTS {
        for layer in REQUIRED_CONTEXT_LAYERS {
            let rel = format!("runtime/src/tradeassembly/contexts/{context}/{layer}");
            report.require(
                root.join(&rel).is_dir(),
                format!("missing context layer {context}/{layer}"),
            );
        }
    }
}

fn check_runtime_inventory(root: &Path, report: &mut ReportBuilder) {
    if rust_rewrite_runtime_active(root) {
        for module in [
            "runtime-rs/src/admin_control.rs",
            "runtime-rs/src/agent_surfaces.rs",
            "runtime-rs/src/conformance.rs",
            "runtime-rs/src/demos.rs",
            "runtime-rs/src/engine.rs",
            "runtime-rs/src/platform_compute.rs",
            "runtime-rs/src/platform_events.rs",
            "runtime-rs/src/platform_storage.rs",
            "runtime-rs/src/strategy_lifecycle.rs",
        ] {
            report.require(root.join(module).exists(), format!("missing {module}"));
        }
        return;
    }
    for failure in runtime_surface_inventory_failures(root) {
        report.require(false, failure);
    }
    let engine_rel = "runtime/src/tradeassembly/engine/__init__.py";
    if let Some(engine_text) = read_rel(root, engine_rel) {
        let runtime_methods = class_methods(&engine_text, "LocalRuntimeService");
        let local_defs = top_level_functions(&engine_text);
        for name in [
            "_normalize_broker_position_symbol",
            "_infer_position_asset_class",
            "_adjust_buying_power",
            "_position_id",
            "_apply_position_fill",
            "_build_position_fill",
            "_reservation_for_order",
            "_apply_order_fill_delta",
            "_reservation_for_order_in_transaction",
            "_apply_order_fill_delta_in_transaction",
        ] {
            report.require(
                !runtime_methods.contains(name),
                format!("risk helper wrapper must not be exposed by tradeassembly.engine: {name}"),
            );
        }
        for name in [
            "_normalize_order_status",
            "_order_fill_quantities",
            "_resolve_order_status_state",
            "_record_order_attempt",
            "_record_order_status_transition",
            "_record_submit_failure",
            "_receipt_from_broker_order_payload",
            "_reconcile_submit_after_ambiguous_error",
            "_submit_order_with_retry",
            "_record_order",
            "_emit_order_status_side_effect_events",
            "_broker_for",
            "_idempotency_key",
            "_put_position_record_in_transaction",
            "_sql_dialect",
            "_get_record_in_transaction",
            "_list_records_in_transaction",
            "_append_outbox_in_transaction",
            "_rule_rows",
            "_assert_live_submit_allowed",
        ] {
            report.require(
                !runtime_methods.contains(name),
                format!("order submit/status helper wrapper must not be exposed by tradeassembly.engine: {name}"),
            );
        }
        let live_submit_text = read_rel(
            root,
            "runtime/src/tradeassembly/contexts/execution/commands/live_submit.py",
        )
        .unwrap_or_default();
        for token in [
            "live.execution_blocked",
            "TRADEASSEMBLY_ENABLE_LIVE_TRADING=1",
        ] {
            report.require(
                !engine_text.contains(token),
                format!("live-submit guard behavior must be owned by execution context, not tradeassembly.engine: {token}"),
            );
            report.require(
                live_submit_text.contains(token),
                format!(
                    "live-submit guard behavior must be implemented by execution context: {token}"
                ),
            );
        }
        for name in ["demo_spec", "btc_exit_demo_spec", "strategy_template_spec"] {
            report.require(
                !local_defs.contains(name),
                format!("strategy template behavior must be owned by strategy context, not tradeassembly.engine: {name}"),
            );
        }
        for token in [
            "AiDraftSession",
            "strategy.ai_draft.created",
            "draft_strategy(",
        ] {
            report.require(
                !engine_text.contains(token),
                format!("strategy AI draft behavior must be owned by strategy context, not tradeassembly.engine: {token}"),
            );
        }
        for token in ["total_order_notional", "submitted_orders", "unrealized_pnl"] {
            report.require(
                !engine_text.contains(token),
                format!("performance summary behavior must be owned by journal context, not tradeassembly.engine: {token}"),
            );
        }
        for token in [
            "ExecutionWorkspaceReportBuilder",
            "versions=deps.store.list",
            "all_events=deps.store.list_events",
            "_execution_resolved_strategy_legs",
            "_execute_builtin_spread_template_stage",
            "_execute_provider_stage",
        ] {
            report.require(
                !engine_text.contains(token),
                format!("execution workspace report behavior must be owned by execution context, not tradeassembly.engine: {token}"),
            );
        }
        for token in [
            "ensure_options_demo",
            "ensure_btc_exit_demo",
            "self.store.get(\"config\", seed[\"config_id\"]",
        ] {
            report.require(
                !engine_text.contains(token),
                format!("strategy seed/config fallback behavior must be owned by strategy context, not tradeassembly.engine: {token}"),
            );
        }
        for token in ["missingCapabilities", "provider.enabled", "version missing"] {
            report.require(
                !engine_text.contains(token),
                format!("strategy compatibility behavior must be owned by strategy context, not tradeassembly.engine: {token}"),
            );
        }
        for token in [
            "research universe requires",
            "research.universe.created",
            "universe_snapshot",
            "research.dataset.created",
        ] {
            report.require(
                !engine_text.contains(token),
                format!("research dataset/universe behavior must be owned by research context, not tradeassembly.engine: {token}"),
            );
        }
        for name in ["_research_service", "_create_research_artifact"] {
            report.require(
                !runtime_methods.contains(name),
                format!("research service/artifact helper must not be exposed by tradeassembly.engine: {name}"),
            );
        }
        for token in [
            "portfolio.position_closed",
            "portfolio.position_opened",
            "broker position requires symbol",
            "source=\"local_order\"",
        ] {
            report.require(
                !engine_text.contains(token),
                format!("risk portfolio mutation behavior must be owned by risk context, not tradeassembly.engine: {token}"),
            );
        }
        for token in [
            "previous_remaining = max(0.0",
            "reservation_notional_delta",
            "entry_fill_delta",
            "\"consumed_reservation\"",
            "\"adjusted_reservation\"",
        ] {
            report.require(
                !engine_text.contains(token),
                format!("order-fill reservation behavior must be owned by risk context, not tradeassembly.engine: {token}"),
            );
        }
        for token in [
            "risk.reservation_broker_status_reconciled",
            "failed_run_missing_order",
            "order_still_open",
            "risk.reservation_reconcile_deferred",
            "risk.reconciliation_completed",
        ] {
            report.require(
                !engine_text.contains(token),
                format!("risk reservation reconciliation behavior must be owned by risk context, not tradeassembly.engine: {token}"),
            );
        }
        for token in [
            "for key in (\"exit_policy\", \"exit\")",
            "config.get(\"exit_policy\")",
        ] {
            report.require(
                !engine_text.contains(token),
                format!("exit policy extraction must be owned by execution domain, not tradeassembly.engine: {token}"),
            );
        }
        for token in [
            "attempts = self.orders.attempts.list()",
            "self.store.list(\"exit_watch\", SyntheticExitWatch)",
            "self.store.list(\"run\", StrategyRun)",
        ] {
            report.require(
                !engine_text.contains(token),
                format!("execution query filtering must be owned by execution repository, not tradeassembly.engine: {token}"),
            );
        }
        let demo_text = read_rel(
            root,
            "runtime/src/tradeassembly/contexts/execution/commands/demo.py",
        )
        .unwrap_or_default();
        for token in [
            "self.store.seed_providers()",
            "self.run_backtest(seed[\"strategy_id\"])",
            "self.run_once(activation.id, submit_exit=True)",
        ] {
            report.require(
                !engine_text.contains(token),
                format!("BTC exit demo orchestration must be owned by execution context, not tradeassembly.engine: {token}"),
            );
        }
        for token in [
            "self.store.seed_providers()",
            "self.run_once(activation.id, submit_exit=True)",
            "\"no_advice\"",
        ] {
            report.require(
                demo_text.contains(token),
                format!(
                    "BTC exit demo orchestration must be implemented by execution context: {token}"
                ),
            );
        }
        let replay_text = read_rel(
            root,
            "runtime/src/tradeassembly/contexts/journal/commands/replay_service.py",
        )
        .unwrap_or_default();
        for token in [
            "ReplayHarness(self.store).verify",
            "ReplayExpectation.from_payload",
        ] {
            report.require(
                !engine_text.contains(token),
                format!("journal replay harness behavior must be owned by journal context, not tradeassembly.engine: {token}"),
            );
            report.require(
                replay_text.contains(token),
                format!("journal replay harness behavior must be implemented by journal context: {token}"),
            );
        }
        let reset_text = read_rel(
            root,
            "runtime/src/tradeassembly/application/local_workspace.py",
        )
        .unwrap_or_default();
        report.require(
            !runtime_methods.contains("_assert_reset_allowed"),
            "local workspace reset policy must not be exposed by tradeassembly.engine: _assert_reset_allowed",
        );
        for token in [
            "reset is local-only",
            "external runtime profiles require explicit migration",
            "local_artifacts_allowed",
        ] {
            report.require(
                !engine_text.contains(token),
                format!("local workspace reset policy must be owned by application workspace service, not tradeassembly.engine: {token}"),
            );
            report.require(
                reset_text.contains(token),
                format!("local workspace reset policy must be implemented by application workspace service: {token}"),
            );
        }
    }
    for path in unclassified_runtime_surfaces(root) {
        report.require(false, format!("unclassified runtime surface: {path}"));
    }
    for path in active_business_compatibility_shims(root) {
        report.require(false, format!("active business compatibility shim: {path}"));
    }
    for path in context_stub_layers(root) {
        report.require(false, format!("context layer is still a stub: {path}"));
    }
}

fn check_docs_contract(root: &Path, report: &mut ReportBuilder) {
    for stale in stale_kafka_default_references(root) {
        report.require(
            false,
            format!("stale Kafka-default docs reference: {stale}"),
        );
    }
}

fn check_iac_contract(root: &Path, report: &mut ReportBuilder) {
    for rel in [
        "docs/planning/roadmaps/tradeassembly-infrastructure-as-code-readiness-plan.md",
        "docs/planning/roadmaps/tradeassembly-cloud-iac-production-readiness-plan.md",
        "infra/README.md",
        "infra/ansible/ansible.cfg",
        "infra/ansible/requirements.yml",
        "infra/ansible/inventories/local.yml",
        "infra/ansible/inventories/local-container.yml",
        "infra/ansible/inventories/single-host.example.yml",
        "infra/ansible/inventories/single-host-external.example.yml",
        "infra/ansible/group_vars/all.yml",
        "infra/ansible/group_vars/local.yml",
        "infra/ansible/group_vars/single_host.yml",
        "infra/ansible/playbooks/local-up.yml",
        "infra/ansible/playbooks/local-down.yml",
        "infra/ansible/playbooks/server-apply.yml",
        "infra/ansible/playbooks/verify.yml",
        "infra/ansible/roles/preflight/tasks/main.yml",
        "infra/ansible/roles/container_backend/tasks/main.yml",
        "infra/ansible/roles/healthcheck/tasks/main.yml",
        "infra/ansible/templates/tradeassembly.env.j2",
        "infra/ansible/templates/docker-compose.generated.yml.j2",
        "infra/tofu/README.md",
        "infra/tofu/scripts/bootstrap-tofu.sh",
        "infra/tofu/scripts/validate-provider.sh",
        "infra/tofu/aws/main.tf",
        "infra/tofu/aws/outputs.tf",
        "infra/tofu/gcp/main.tf",
        "infra/tofu/gcp/outputs.tf",
        "infra/tofu/azure/main.tf",
        "infra/tofu/azure/outputs.tf",
        "render.yaml",
        "infra/render/README.md",
        "webapp/Dockerfile",
    ] {
        report.require(
            root.join(rel).exists(),
            format!("missing IaC contract file: {rel}"),
        );
    }

    let combined_docs = [
        "README.md",
        "docs/architecture.md",
        "docs/technical-design-guide.md",
        "docs/history/architecture-decisions.md",
        "deploy/README.md",
    ]
    .iter()
    .filter_map(|rel| read_rel(root, rel))
    .collect::<Vec<_>>()
    .join("\n");
    for token in [
        "ARCH-IAC-001",
        "Ansible",
        "OpenTofu",
        "Render.com",
        "AWS",
        "GCP",
        "Azure",
        "render.yaml",
        "just iac-local-up",
    ] {
        report.require(
            combined_docs.contains(token),
            format!("IaC docs must mention {token}"),
        );
    }
    for forbidden in [
        "Docker Desktop required",
        "Docker is required",
        "Compose is the canonical",
        "compose is the canonical",
    ] {
        report.require(
            !combined_docs.contains(forbidden),
            format!("docs present Docker/Compose as mandatory infrastructure: {forbidden}"),
        );
    }

    if let Some(justfile) = read_rel(root, "Justfile") {
        report.require(
            justfile.contains("CONNECTION"),
            "server IaC plan must support non-SSH local dry-run validation",
        );
        report.require(
            justfile.contains("EXTRA_VARS"),
            "server IaC plan must allow explicit external-service variables",
        );
        if rust_rewrite_runtime_active(root) {
            report.require(
                just_recipe_block(&justfile, "iac-prod-audit").contains("cloud-iac"),
                "Rust-first IaC audit must run cloud-iac contract checks",
            );
        } else {
            report.require(
                justfile.contains("single-host-external.example.yml"),
                "IaC audit must syntax-check the external-service single-host inventory",
            );
        }
        for target in [
            "iac-bootstrap:",
            "iac-local-plan:",
            "iac-local-up:",
            "iac-local-down:",
            "iac-local-status:",
            "iac-local-logs:",
            "iac-local-verify:",
            "iac-server-plan:",
            "iac-server-apply:",
            "iac-cloud-validate:",
            "iac-cloud-plan:",
            "iac-render-validate:",
            "iac-prod-audit:",
            "iac-audit:",
        ] {
            report.require(
                justfile.contains(target),
                format!("missing IaC Just recipe {target}"),
            );
        }
        report.require(
            just_recipe_block(&justfile, "verify").contains("cargo xtask verify"),
            "verify must delegate to the Rust aggregate gate",
        );
        for target in [
            "iac-local-plan",
            "iac-local-up",
            "iac-local-down",
            "iac-local-status",
            "iac-local-logs",
            "iac-local-verify",
            "iac-server-plan",
            "iac-server-apply",
        ] {
            let block = just_recipe_block(&justfile, target);
            report.require(
                block.contains("ansible-playbook"),
                format!("{target} must route through Ansible"),
            );
            report.require(
                !block.contains("docker compose -f docker-compose.distributed.yml"),
                format!("{target} bypasses IaC with raw Compose"),
            );
            report.require(
                !block.contains("dev/distributed-lifecycle.sh"),
                format!("{target} bypasses IaC with raw shell lifecycle"),
            );
        }
        for target in [
            "dev-distributed-compose-seed",
            "dev-distributed-compose-up",
            "dev-distributed-compose-down",
            "dev-distributed-compose-logs",
        ] {
            let block = just_recipe_block(&justfile, target);
            report.require(
                block.contains("just iac-local-"),
                format!("{target} must route through IaC container backend"),
            );
            report.require(
                !block.contains("docker compose -f docker-compose.distributed.yml"),
                format!("{target} bypasses IaC with raw Compose"),
            );
        }
    }

    if let Some(text) = read_rel(root, "infra/ansible/group_vars/all.yml") {
        for token in [
            "TRADEASSEMBLY_RUNTIME_PROFILE",
            "TRADEASSEMBLY_AUTH_ISSUER",
            "TRADEASSEMBLY_AUTH_AUDIENCE",
            "TRADEASSEMBLY_AUTH_CLIENT_ID",
            "TRADEASSEMBLY_POSTGRES_URL_REF",
            "TRADEASSEMBLY_POSTGRES_URL",
            "TRADEASSEMBLY_NATS_URL_REF",
            "TRADEASSEMBLY_NATS_URL",
            "TRADEASSEMBLY_OBJECT_STORE_ENDPOINT",
            "TRADEASSEMBLY_CREDENTIAL_BACKEND",
            "TRADEASSEMBLY_ENABLE_LIVE_TRADING",
            "tradeassembly_enable_live_trading: \"0\"",
            "tradeassembly_service_path:",
            "tradeassembly_container_skip_without_engine:",
            "tradeassembly_container_engine_path:",
            "tradeassembly_container_required_services:",
            "tradeassembly_provision_postgres:",
            "tradeassembly_provision_nats:",
            "tradeassembly_runtime_postgres_url:",
            "tradeassembly_runtime_nats_url:",
            "tradeassembly_nats_url_ref:",
            "tradeassembly_nats_url:",
        ] {
            report.require(
                text.contains(token),
                format!("IaC variable model missing {token}"),
            );
        }
        report.require(
            !text.contains(" --db "),
            "IaC-managed services must use configured adapters, not CLI --db SQLite paths",
        );
        report.require(
            !text.to_ascii_lowercase().contains("kafka"),
            "active Ansible defaults must use NATS JetStream, not Kafka",
        );
    }
    if let Some(text) = read_rel(root, "infra/ansible/roles/preflight/tasks/main.yml") {
        report.require(
            text.contains("lsof -nP -iTCP"),
            "preflight port conflicts must report listener process details",
        );
        report.require(
            text.contains("Port {{ item.item.item }} is already in use"),
            "preflight port conflict failure must include the port",
        );
    }
    if let Some(text) = read_rel(root, "infra/ansible/playbooks/verify.yml") {
        report.require(
            text.contains("tradeassembly_port_conflict_check_enabled: false"),
            "verify playbook must not treat expected running services as port conflicts",
        );
    }
    if let Some(text) = read_rel(root, "infra/ansible/group_vars/single_host.yml") {
        let lowered = text.to_lowercase();
        report.require(
            !lowered.contains("sqlite"),
            "single-host IaC inventory must not use SQLite",
        );
        report.require(
            !lowered.contains("memory"),
            "single-host IaC inventory must not use memory bus",
        );
        report.require(
            lowered.contains("self_hosted"),
            "single-host IaC must use the self_hosted runtime profile",
        );
        report.require(
            !lowered.contains("external-required"),
            "single-host IaC must not use external-required credential sentinel",
        );
    }
    if let Some(text) = read_rel(root, "infra/ansible/roles/healthcheck/tasks/main.yml") {
        for token in [
            "tradeassembly_api_host",
            "tradeassembly_api_port",
            "tradeassembly_studio_host",
            "tradeassembly_studio_port",
        ] {
            report.require(
                text.contains(token),
                format!("self-host health checks must use configured endpoints: {token}"),
            );
        }
    }
    for rel in [
        "infra/ansible/templates/tradeassembly.env.j2",
        "infra/ansible/templates/docker-compose.generated.yml.j2",
    ] {
        if let Some(text) = read_rel(root, rel) {
            report.require(
                !text.contains("TRADEASSEMBLY_ENABLE_LIVE_TRADING=1"),
                format!("IaC template enables live trading by default: {rel}"),
            );
            for token in [
                "TRADEASSEMBLY_RUNTIME_PROFILE",
                "TRADEASSEMBLY_POSTGRES_URL_REF",
                "TRADEASSEMBLY_POSTGRES_URL",
                "TRADEASSEMBLY_NATS_URL_REF",
                "TRADEASSEMBLY_NATS_URL",
                "TRADEASSEMBLY_OBJECT_STORE_ENDPOINT",
            ] {
                report.require(
                    text.contains(token),
                    format!("IaC template missing {token}: {rel}"),
                );
            }
            if rel.ends_with("docker-compose.generated.yml.j2") {
                for token in [
                    "tradeassembly_provision_postgres",
                    "tradeassembly_provision_nats",
                    "tradeassembly_runtime_postgres_url",
                    "tradeassembly_runtime_nats_url",
                ] {
                    report.require(
                        text.contains(token),
                        format!("generated Compose must honor adapter selection: {token}"),
                    );
                }
            }
        }
    }
    if let Some(text) = read_rel(root, "infra/ansible/roles/container_backend/tasks/main.yml") {
        for token in [
            "docker info",
            "podman info",
            "tradeassembly_container_skipped",
            "container-skip.json",
            "Seed container BTC demo",
            "tradeassembly_container_required_services | difference",
        ] {
            report.require(
                text.contains(token),
                format!("container IaC backend missing {token}"),
            );
        }
    }
    for provider in ["aws", "gcp", "azure"] {
        for rel in [
            format!("infra/tofu/{provider}/outputs.tf"),
            format!("infra/tofu/{provider}/main.tf"),
            format!("infra/tofu/{provider}/variables.tf"),
        ] {
            if let Some(text) = read_rel(root, &rel) {
                report.require(
                    !text.contains("local-file"),
                    format!("{provider} IaC must not default to local-file credentials"),
                );
                report.require(
                    !text.contains("kafka-memory"),
                    format!("{provider} IaC must not default to memory events"),
                );
            }
        }
        let rel = format!("infra/tofu/{provider}/outputs.tf");
        if let Some(text) = read_rel(root, &rel) {
            for token in [
                "ansible_inventory",
                "tradeassembly_runtime_profile",
                "tradeassembly_db_url",
                "tradeassembly_nats_url",
            ] {
                report.require(
                    text.contains(token),
                    format!("{provider} IaC must export {token}"),
                );
            }
        }
    }
    if let Some(text) = read_rel(root, "render.yaml") {
        for token in [
            "tradeassembly-api",
            "tradeassembly-studio",
            "tradeassembly-scheduler",
            "tradeassembly-runner",
            "tradeassembly-oms",
            "tradeassembly-order-status",
            "TRADEASSEMBLY_POSTGRES_URL_REF",
            "TRADEASSEMBLY_NATS_URL_REF",
            "TRADEASSEMBLY_NATS_URL",
        ] {
            report.require(
                text.contains(token),
                format!("Render blueprint missing {token}"),
            );
        }
        for forbidden in ["kafka-memory", "sqlite://", "local-file", "npm run dev"] {
            report.require(
                !text.contains(forbidden),
                format!("Render blueprint contains local/dev-only value: {forbidden}"),
            );
        }
    }
}

fn check_cloud_iac_contract(root: &Path, report: &mut ReportBuilder) {
    let providers = ["aws", "gcp", "azure"];
    let required_provider_files = [
        "provider.tf",
        "variables.tf",
        "main.tf",
        "outputs.tf",
        "README.md",
    ];
    let required_top_level_files = [
        "infra/tofu/README.md",
        "infra/tofu/versions.tf",
        "infra/tofu/scripts/bootstrap-tofu.sh",
        "infra/tofu/scripts/validate-provider.sh",
        "infra/tofu/scripts/render-ansible-inventory.sh",
        "infra/tofu/examples/aws.tfvars.example",
        "infra/tofu/examples/gcp.tfvars.example",
        "infra/tofu/examples/azure.tfvars.example",
    ];

    for provider in providers {
        let provider_root = root.join("infra/tofu").join(provider);
        report.require(
            provider_root.is_dir(),
            format!("missing cloud IaC provider tree: infra/tofu/{provider}"),
        );
        for file_name in required_provider_files {
            let rel = format!("infra/tofu/{provider}/{file_name}");
            report.require(root.join(&rel).exists(), format!("missing {rel}"));
        }
    }
    for rel in required_top_level_files {
        report.require(root.join(rel).exists(), format!("missing {rel}"));
    }

    for provider in providers {
        let outputs_rel = format!("infra/tofu/{provider}/outputs.tf");
        if let Some(outputs) = read_rel(root, &outputs_rel) {
            for output in [
                "ansible_host",
                "ansible_user",
                "tradeassembly_runtime_profile",
                "tradeassembly_db_url",
                "tradeassembly_nats_url",
                "ansible_inventory",
            ] {
                let token = format!("output \"{output}\"");
                report.require(
                    outputs.contains(&token),
                    format!("{outputs_rel} must define {token}"),
                );
            }
            for token in [
                "yamlencode",
                "tradeassembly_backend",
                "tradeassembly_runtime_profile",
                "tradeassembly_credential_backend",
                "tradeassembly_provision_postgres",
                "tradeassembly_provision_nats",
            ] {
                report.require(
                    outputs.contains(token),
                    format!("{outputs_rel} must contain {token}"),
                );
            }
        } else {
            report.require(false, format!("missing {outputs_rel}"));
        }

        let variables_rel = format!("infra/tofu/{provider}/variables.tf");
        if let Some(variables) = read_rel(root, &variables_rel) {
            for token in [
                "variable \"profile\"",
                "default = \"starter\"",
                "variable \"tradeassembly_db_url\"",
                "variable \"tradeassembly_nats_url\"",
                "variable \"tradeassembly_credential_backend\"",
            ] {
                report.require(
                    variables.contains(token),
                    format!("{variables_rel} must contain {token}"),
                );
            }
        } else {
            report.require(false, format!("missing {variables_rel}"));
        }

        let combined = ["provider.tf", "variables.tf", "main.tf", "outputs.tf"]
            .into_iter()
            .filter_map(|file_name| read_rel(root, &format!("infra/tofu/{provider}/{file_name}")))
            .collect::<Vec<_>>()
            .join("\n");
        let lowered = combined.to_lowercase();
        for value in ["local-file", "kafka-memory", "inmemory", "sqlite://"] {
            report.require(
                !lowered.contains(value),
                format!("{provider} cloud IaC contains local-only runtime value: {value}"),
            );
        }
        for token in [
            "default = \"env\"",
            "runtime_profile = \"self_hosted\"",
            "nats://nats:4222",
            "public_api_ingress",
            "public_studio_ingress",
        ] {
            report.require(
                combined.contains(token),
                format!("{provider} cloud IaC must contain {token}"),
            );
        }
    }

    for (provider, tokens) in [
        (
            "aws",
            [
                "aws_instance",
                "aws_ebs_volume",
                "nats://nats:4222",
                "tradeassembly_nats_url",
                "postgresql://tradeassembly@postgres:5432/tradeassembly",
            ],
        ),
        (
            "gcp",
            [
                "google_compute_instance",
                "nats://nats:4222",
                "tradeassembly_nats_url",
                "postgresql://tradeassembly@postgres:5432/tradeassembly",
                "google_compute_disk",
            ],
        ),
        (
            "azure",
            [
                "azurerm_linux_virtual_machine",
                "nats://nats:4222",
                "tradeassembly_nats_url",
                "postgresql://tradeassembly@postgres:5432/tradeassembly",
                "azurerm_managed_disk",
            ],
        ),
    ] {
        let rel = format!("infra/tofu/{provider}/main.tf");
        if let Some(text) = read_rel(root, &rel) {
            for token in tokens.into_iter().filter(|token| !token.is_empty()) {
                report.require(text.contains(token), format!("{rel} must contain {token}"));
            }
        } else {
            report.require(false, format!("missing {rel}"));
        }
    }

    if let Some(aws) = read_rel(root, "infra/tofu/aws/main.tf") {
        for token in [
            "data \"aws_ssm_parameter\" \"amazon_linux_2023_ami\"",
            "/aws/service/ami-amazon-linux-latest/al2023-ami-kernel-default-x86_64",
            "var.ami_id != \"\"",
        ] {
            report.require(
                aws.contains(token),
                format!("AWS starter must contain {token}"),
            );
        }
        report.require(
            !aws.contains("ami-00000000000000000"),
            "AWS starter must not use a placeholder AMI",
        );
    }

    if let Some(justfile) = read_rel(root, "Justfile") {
        for target in [
            "iac-cloud-bootstrap",
            "iac-cloud-validate",
            "iac-cloud-plan",
            "iac-cloud-inventory",
            "iac-cloud-apply",
            "iac-cloud-destroy",
            "iac-prod-audit",
        ] {
            let pattern = Regex::new(&format!(r"(?m)^{}:", regex::escape(target)))
                .expect("valid Just recipe regex");
            report.require(
                pattern.is_match(&justfile),
                format!("Justfile missing guarded cloud IaC recipe {target}"),
            );
        }
        for token in [
            "CONFIRM_CLOUD_APPLY",
            "CONFIRM_DESTROY_TRADEASSEMBLY_CLOUD",
            "infra/tofu/scripts/validate-provider.sh",
        ] {
            report.require(
                justfile.contains(token),
                format!("Justfile cloud IaC recipes must contain {token}"),
            );
        }
        report.require(
            just_recipe_block(&justfile, "verify").contains("cargo xtask verify"),
            "Just verify must delegate to cargo xtask verify",
        );
    } else {
        report.require(false, "missing Justfile");
    }

    let tofu_root = root.join("infra/tofu");
    if tofu_root.exists() {
        for entry in WalkDir::new(&tofu_root).into_iter().filter_map(Result::ok) {
            if !entry.file_type().is_file() {
                continue;
            }
            let path = entry.path();
            let file_name = path
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or_default();
            if !file_name.contains(".tf") {
                continue;
            }
            if let Some(text) = read_path(path) {
                let rel = rel_path(root, path);
                for secret in ["AKIA", "ghp_", "BEGIN PRIVATE KEY", "change-me"] {
                    report.require(
                        !text.contains(secret),
                        format!("{rel} must not contain committed secret marker {secret}"),
                    );
                }
            }
        }
    }
}

fn check_platform(root: &Path, report: &mut ReportBuilder) {
    if rust_rewrite_runtime_active(root) {
        for rel in [
            "runtime-rs/src/platform_storage.rs",
            "runtime-rs/src/platform_events.rs",
            "runtime-rs/src/platform_compute.rs",
            "runtime-rs/src/bus.rs",
            "runtime-rs/src/outbox.rs",
        ] {
            report.require(root.join(rel).exists(), format!("missing {rel}"));
        }
        return;
    }
    for rel in [
        "runtime/src/tradeassembly/platform/storage/contracts.py",
        "runtime/src/tradeassembly/platform/storage/sqlite.py",
        "runtime/src/tradeassembly/platform/storage/postgres.py",
        "runtime/src/tradeassembly/platform/storage/olap.py",
        "runtime/src/tradeassembly/platform/storage/repositories.py",
        "runtime/src/tradeassembly/platform/events/contracts.py",
        "runtime/src/tradeassembly/platform/events/adapters.py",
        "runtime/src/tradeassembly/platform/events/outbox.py",
        "runtime/src/tradeassembly/platform/events/dead_letters.py",
        "runtime/src/tradeassembly/platform/events/memory.py",
        "runtime/src/tradeassembly/platform/events/factory.py",
        "runtime/src/tradeassembly/platform/events/kafka.py",
        "runtime/src/tradeassembly/platform/events/nats.py",
        "runtime/src/tradeassembly/bus/kafka.py",
        "runtime/src/tradeassembly/contexts/plugins/domain/runtime.py",
        "runtime/src/tradeassembly/contexts/plugins/ports/runtime.py",
        "runtime/src/tradeassembly/contexts/plugins/commands/invoke.py",
        "runtime/src/tradeassembly/contexts/plugins/adapters/subprocess_runtime.py",
    ] {
        report.require(root.join(rel).exists(), format!("missing {rel}"));
    }
    if let Some(text) = read_rel(root, "runtime/src/tradeassembly/platform/events/kafka.py") {
        report.require(
            text.contains("AiokafkaEventAdapter"),
            "Kafka platform adapter must include dependency-gated real adapter",
        );
        report.require(
            text.contains("InMemoryKafkaEventAdapter"),
            "Kafka test adapter must be explicitly named as in-memory",
        );
    }
    if let Some(text) = read_rel(root, "runtime/src/tradeassembly/persistence/postgres.py") {
        report.require(
            text.contains("pg_advisory_xact_lock"),
            "Postgres schema bootstrap must be concurrency-safe",
        );
    }
}

fn check_application_command_surface(root: &Path, report: &mut ReportBuilder) {
    if rust_rewrite_runtime_active(root) {
        for rel in [
            "runtime-rs/src/admin_control.rs",
            "runtime-rs/src/agent_surfaces.rs",
            "runtime-rs/src/mcp.rs",
            "runtime-rs/src/report_envelope.rs",
        ] {
            report.require(root.join(rel).exists(), format!("missing {rel}"));
        }
        return;
    }
    let rel = "runtime/src/tradeassembly/application_commands.py";
    let Some(text) = read_rel(root, rel) else {
        report.require(
            false,
            "missing runtime/src/tradeassembly/application_commands.py",
        );
        return;
    };
    for token in [
        "from tradeassembly.bus",
        "import tradeassembly.bus",
        "from tradeassembly.engine",
        "LocalRuntimeService",
        "RuntimeCommandDispatcher",
        "ProductCommandDispatcher",
        "__getattr__",
        "from tradeassembly.messages",
        "from tradeassembly.store",
        "import sqlite3",
        "import psycopg",
        "import aiokafka",
        "import nats",
    ] {
        report.require(
            !text.contains(token),
            format!("application command facade imports raw infrastructure: {token}"),
        );
    }
    for rel in [
        "runtime/src/tradeassembly/application/contracts.py",
        "runtime/src/tradeassembly/application/registry.py",
        "runtime/src/tradeassembly/application/bootstrap.py",
    ] {
        report.require(root.join(rel).exists(), format!("missing {rel}"));
    }
    if let Some(bootstrap) = read_rel(root, "runtime/src/tradeassembly/application/bootstrap.py") {
        report.require(
            bootstrap.contains("contract_service: StrategyContractService"),
            "strategy application command group must use the strategy contract service",
        );
        report.require(
            !bootstrap.contains("strategy=StrategyCommands(service_factory)"),
            "strategy validation still builds the legacy runtime service",
        );
    }
}

fn check_cli_command_surface(root: &Path, report: &mut ReportBuilder) {
    if rust_rewrite_runtime_active(root) {
        for rel in [
            "xtask/src/main.rs",
            "scripts/sdlc/verify",
            "scripts/language-invariant.sh",
        ] {
            report.require(root.join(rel).exists(), format!("missing {rel}"));
        }
        return;
    }
    let rel = "runtime/src/tradeassembly/cli.py";
    let Some(text) = read_rel(root, rel) else {
        report.require(false, "missing runtime/src/tradeassembly/cli.py");
        return;
    };
    for token in [
        "from tradeassembly.engine",
        "LocalRuntimeService",
        "from tradeassembly.store",
        "LocalStore",
        "def _create_service",
    ] {
        report.require(
            !text.contains(token),
            format!("CLI bypasses application command registry: {token}"),
        );
    }
    for (name, block) in click_command_blocks(&text) {
        if name == "api" {
            continue;
        }
        report.require(
            block.contains("application_commands."),
            format!("CLI command does not call application_commands handler: {name}"),
        );
    }
}

fn check_storage_repository_surface(root: &Path, report: &mut ReportBuilder) {
    if rust_rewrite_runtime_active(root) {
        for rel in [
            "runtime-rs/src/storage.rs",
            "runtime-rs/src/platform_storage.rs",
            "runtime-rs/src/journal.rs",
            "runtime-rs/src/outbox.rs",
        ] {
            report.require(root.join(rel).exists(), format!("missing {rel}"));
        }
        return;
    }
    if let Some(text) = read_rel(
        root,
        "runtime/src/tradeassembly/platform/storage/contracts.py",
    ) {
        report.require(
            !text.contains("class RepositoryPort"),
            "platform storage exposes generic RepositoryPort passthrough",
        );
    }
    if let Some(text) = read_rel(
        root,
        "runtime/src/tradeassembly/platform/storage/repositories.py",
    ) {
        for token in [
            "class StoreRepository",
            "class BaseContextRepository",
            "class BrokerContextRepository",
            "class ExecutionContextRepository",
            "class JournalContextRepository",
            "class MarketDataContextRepository",
            "class PluginContextRepository",
            "class ResearchContextRepository",
            "class RiskContextRepository",
            "class StrategyContextRepository",
            "__getattr__",
            "broker=StoreRepository",
            "execution=StoreRepository",
        ] {
            report.require(
                !text.contains(token),
                format!("platform storage exposes generic repository passthrough: {token}"),
            );
        }
    }
    for context in [
        "broker",
        "execution",
        "journal",
        "marketdata",
        "plugins",
        "research",
        "risk",
        "strategy",
    ] {
        let rel = format!("runtime/src/tradeassembly/contexts/{context}/ports/repositories.py");
        report.require(
            root.join(&rel).exists(),
            format!("missing context repository ports: {rel}"),
        );
        if let Some(text) = read_rel(root, &rel) {
            let methods = protocol_methods(&text);
            let has_specific = methods
                .iter()
                .any(|method| !GENERIC_REPOSITORY_METHODS.contains(&method.as_str()));
            report.require(
                has_specific,
                format!("context repository ports are generic only: {rel}"),
            );
        }
    }
    for path in python_files_under(root, "runtime/src/tradeassembly/contexts") {
        let rel = rel_path(root, &path);
        if rel.ends_with("/adapters/repositories.py") {
            continue;
        }
        if let Some(text) = read_path(&path) {
            if text.contains("from tradeassembly.persistence")
                || text.contains("import tradeassembly.persistence")
            {
                report.require(
                    false,
                    format!(
                        "context imports persistence internals outside repository adapter: {rel}"
                    ),
                );
            }
        }
    }
}

fn check_kafka_real_adapter_contract(root: &Path, report: &mut ReportBuilder) {
    if let Some(text) = read_rel(root, "runtime/src/tradeassembly/platform/events/kafka.py") {
        report.require(
            text.contains("publish_dead_letter"),
            "Kafka adapter must publish durable dead-letter records",
        );
        report.require(
            text.contains("original_offset"),
            "Kafka dead letters must include original offset metadata",
        );
        report.require(
            !text.contains("delivery_count += 1"),
            "Kafka retry count must not be in-memory handle mutation only",
        );
    }
    for rel in [
        "runtime/src/tradeassembly/contexts/execution/adapters/runner.py",
        "runtime/src/tradeassembly/contexts/execution/adapters/oms_worker.py",
        "runtime/src/tradeassembly/contexts/execution/adapters/order_status_worker.py",
        "runtime/src/tradeassembly/contexts/execution/adapters/order_stream_publisher.py",
    ] {
        if let Some(text) = read_rel(root, rel) {
            report.require(
                !text.contains("from tradeassembly.engine")
                    && !text.contains("LocalRuntimeService"),
                format!("execution worker imports runtime facade: {rel}"),
            );
        }
    }
}

fn check_import_boundaries(root: &Path, report: &mut ReportBuilder) {
    for violation in collect_import_boundary_violations(root) {
        report.require(
            false,
            format!("import boundary violation: {}", violation.message()),
        );
    }
}

fn check_interface_rules(root: &Path, report: &mut ReportBuilder) {
    for violation in collect_interface_violations(root) {
        report.require(
            false,
            format!("interface boundary violation: {}", violation.message()),
        );
    }
}

fn check_graphql_operation_map(root: &Path, report: &mut ReportBuilder) {
    for rel in [
        "webapp/lib/schema.ts",
        "webapp/lib/product/operation-map.ts",
        "webapp/lib/product/graphql.ts",
    ] {
        report.require(root.join(rel).exists(), format!("missing {rel}"));
    }
    let Some(schema_text) = read_rel(root, "webapp/lib/schema.ts") else {
        return;
    };
    let Some(operation_text) = read_rel(root, "webapp/lib/product/operation-map.ts") else {
        return;
    };
    let Some(graphql_text) = read_rel(root, "webapp/lib/product/graphql.ts") else {
        return;
    };
    let schema_keys = typescript_object_keys(&schema_text, "operationMap");
    let route_keys = typescript_object_keys(&operation_text, "productOperationMap");
    report.require(
        schema_keys == route_keys,
        format!(
            "GraphQL product operation map mismatch: missing={:?} extra={:?}",
            schema_keys.difference(&route_keys).collect::<Vec<_>>(),
            route_keys.difference(&schema_keys).collect::<Vec<_>>()
        ),
    );
    for key in &route_keys {
        let entry = typescript_object_entry(&operation_text, key);
        report.require(
            entry.contains("commandGroup:"),
            format!("GraphQL operation {key} missing commandGroup"),
        );
        report.require(
            entry.contains("endpoints:"),
            format!("GraphQL operation {key} missing REST endpoint mapping"),
        );
    }
    report.require(
        graphql_text.contains("/graphql"),
        "GraphQL resolver must proxy to Rust /graphql",
    );
    report.require(
        graphql_text.contains("fetch("),
        "GraphQL resolver must call the Rust API over HTTP",
    );
    report.require(
        !graphql_text.contains("includes(op)"),
        "GraphQL resolver must not use ad hoc operation allowlists",
    );
}

fn check_compatibility_shim_ledger(root: &Path, report: &mut ReportBuilder) {
    let ledger_rel = "docs/reference/runtime/compatibility-shims.md";
    let Some(ledger_text) = read_rel(root, ledger_rel) else {
        report.require(
            false,
            "missing docs/reference/runtime/compatibility-shims.md",
        );
        return;
    };
    let legacy_tokens = [
        "from tradeassembly.engine",
        "LocalRuntimeService",
        "from tradeassembly.store",
        "import tradeassembly.store",
        "from tradeassembly.bus",
        "import tradeassembly.bus",
    ];
    for path in python_files_under(root, "runtime/src/tradeassembly") {
        let rel = rel_path(root, &path);
        if rel.starts_with("runtime/src/tradeassembly/architecture/") {
            continue;
        }
        let Some(text) = read_path(&path) else {
            continue;
        };
        if !legacy_tokens.iter().any(|token| text.contains(token)) {
            continue;
        }
        let parts = rel.split('/').collect::<Vec<_>>();
        let listed = ledger_text.contains(&rel)
            || (1..parts.len()).any(|index| {
                let ancestor = format!("{}/", parts[..index].join("/"));
                ledger_text.contains(&ancestor)
            });
        report.require(
            listed,
            format!(
                "legacy compatibility import is not listed in docs/reference/runtime/compatibility-shims.md: {rel}"
            ),
        );
    }
}

fn check_static_forbidden_patterns(root: &Path, report: &mut ReportBuilder) {
    let checks = [
        (
            "runtime/src/tradeassembly/contexts",
            &["runtime: Any", "self.runtime"][..],
            "context command runtime facade dependency",
        ),
        (
            "runtime/src/tradeassembly/contexts",
            &[
                "from tradeassembly.bus",
                "import tradeassembly.bus",
                "DurableBus",
                "MessageEnvelope",
                "BusDelivery",
                "create_bus",
            ][..],
            "context raw event dependency",
        ),
        (
            "runtime/src/tradeassembly/contexts",
            &[
                "from tradeassembly.store",
                "import tradeassembly.store",
                "LocalStore",
                "PostgresStore",
                "from tradeassembly.persistence.postgres",
            ][..],
            "context raw storage dependency",
        ),
        (
            "runtime/src/tradeassembly/platform/events",
            &[
                "from tradeassembly.bus",
                "import tradeassembly.bus",
                "create_bus",
                "KafkaBus",
            ][..],
            "platform event legacy bus dependency",
        ),
    ];
    for (base, tokens, rule) in checks {
        for path in python_files_under(root, base) {
            let Some(text) = read_path(&path) else {
                continue;
            };
            let rel = rel_path(root, &path);
            for token in tokens {
                if text.contains(token) {
                    report.require(false, format!("{rule}: {rel} contains {token}"));
                }
            }
        }
    }
}

fn check_ui_stub_scan(root: &Path, report: &mut ReportBuilder) {
    let rel = "docs/ui-stub-gaps.md";
    report.require(root.join(rel).exists(), "missing docs/ui-stub-gaps.md");
    if let Some(actual) = read_rel(root, rel) {
        let expected = expected_ui_stub_gap_text(root);
        report.require(
            actual == expected,
            "docs/ui-stub-gaps.md is stale; run `tradeassembly architecture scan-ui-stubs --write`",
        );
    }
}

fn check_full_parity_gap_closure(root: &Path, report: &mut ReportBuilder) {
    let gaps = scan_ui_stubs(root);
    report.require(
        gaps.is_empty(),
        format!(
            "full Rust parity requires empty docs/ui-stub-gaps.md; {} UI stub/local-data markers remain",
            gaps.len()
        ),
    );

    let parity_rel = "docs/source-parity-map.md";
    let Some(parity_map) = read_rel(root, parity_rel) else {
        report.require(false, format!("missing {parity_rel}"));
        return;
    };
    for forbidden in [
        "rust parity in progress",
        "rust-bridge",
        "| gap |",
        "| investigation |",
        "runtime/src/tradeassembly/",
        "runtime/src/tradeassembly.",
    ] {
        report.require(
            !parity_map.contains(forbidden),
            format!("{parity_rel} contains stale full-parity language: {forbidden}"),
        );
    }

    for rel in [
        "docs/audits/runtime-parity-audit-2026-05-29.md",
        "docs/audits/runtime-demo-test-port-audit-2026-06-02.md",
    ] {
        let Some(text) = read_rel(root, rel) else {
            report.require(false, format!("missing {rel}"));
            continue;
        };
        report.require(
            text.lines()
                .take(12)
                .any(|line| line.contains("Superseded by full Rust parity closure")),
            format!("{rel} must be marked as historical evidence, not current parity status"),
        );
    }

    check_current_state_docs(root, report);
}

fn check_current_state_docs(root: &Path, report: &mut ReportBuilder) {
    for rel in CURRENT_STATE_DOCS {
        let Some(text) = read_rel(root, rel) else {
            report.require(false, format!("missing {rel}"));
            continue;
        };
        for stale_claim in STALE_ACCEPTED_OWNER_CLAIMS {
            report.require(
                !text.contains(stale_claim),
                format!("{rel} assigns accepted behavior to a future owner: {stale_claim}"),
            );
        }
    }
}

fn check_active_docs_do_not_claim_python_runtime(root: &Path, report: &mut ReportBuilder) {
    for rel in ACTIVE_DOCS {
        let Some(text) = read_rel(root, rel) else {
            report.require(false, format!("missing active doc {rel}"));
            continue;
        };
        for (line_index, line) in text.lines().enumerate() {
            for term in STALE_ACTIVE_DOC_TERMS {
                if line.contains(term) && !is_explicit_stale_policy_line(line) {
                    report.require(
                        false,
                        format!(
                            "{ACTIVE_DOC_STALE_POLICY}: {rel}:{} contains {term}",
                            line_index + 1
                        ),
                    );
                }
            }
        }
    }
}

fn is_explicit_stale_policy_line(line: &str) -> bool {
    line.contains("must not describe")
        || line.contains("as current")
        || line.contains("current runtime owners or canonical gates")
        || line.contains("only with the exact historical migration banner")
}

fn check_historical_docs_are_classified(root: &Path, report: &mut ReportBuilder) {
    for rel in HISTORICAL_DOCS {
        let Some(text) = read_rel(root, rel) else {
            report.require(false, format!("missing historical migration doc {rel}"));
            continue;
        };
        report.require(
            text.lines()
                .take(20)
                .any(|line| line.contains(HISTORICAL_MIGRATION_BANNER)),
            format!("{rel} must include historical migration banner in first 20 lines"),
        );
    }
}

fn check_shell_wrappers_are_thin(root: &Path, report: &mut ReportBuilder) {
    for entry in WalkDir::new(root).into_iter().filter_map(Result::ok) {
        let path = entry.path();
        if !entry.file_type().is_file()
            || path.extension().and_then(|ext| ext.to_str()) != Some("sh")
        {
            continue;
        }
        let rel = rel_path(root, path);
        if rel.starts_with(".git/") || rel.contains("/target/") || rel.contains("/node_modules/") {
            continue;
        }
        let Ok(text) = fs::read_to_string(path) else {
            continue;
        };
        for (line_index, line) in text.lines().enumerate() {
            for term in SHELL_WRAPPER_FORBIDDEN_TERMS {
                if line.contains(term) {
                    report.require(
                        false,
                        format!(
                            "shell wrapper must stay thin and avoid embedded Python: {rel}:{} contains {term}",
                            line_index + 1
                        ),
                    );
                }
            }
        }
    }
}

fn check_tooling_recipes(root: &Path, report: &mut ReportBuilder) {
    report.require(
        !root.join("Makefile").exists(),
        "Makefile must be deleted after Rust-native tooling migration",
    );
    let migration_path = "docs/reference/tooling-command-migration.md";
    report.require(
        root.join(migration_path).exists(),
        format!("missing {migration_path}"),
    );

    let Some(text) = read_rel(root, "Justfile") else {
        report.require(false, "missing Justfile");
        return;
    };
    let recipes = Regex::new(r"(?m)^([A-Za-z0-9_-]+):")
        .expect("valid Just recipe regex")
        .captures_iter(&text)
        .filter_map(|capture| capture.get(1).map(|matched| matched.as_str().to_string()))
        .collect::<BTreeSet<_>>();
    for recipe in [
        "verify",
        "setup",
        "dev",
        "dev-start",
        "dev-stop",
        "dev-status",
        "seed",
        "seed-btc",
        "reset",
        "run",
        "run-btc-demo",
        "demo",
        "backtest",
        "backtest-btc",
        "creds",
        "scheduler",
        "test",
        "build",
        "architecture-check",
        "deploy-gate",
        "iac-prod-audit",
        "release-evidence",
        "scan-public",
        "clean",
    ] {
        report.require(
            recipes.contains(recipe),
            format!("missing canonical Just recipe {recipe}"),
        );
    }
    for forbidden in ["make ", "$(MAKE)", "cargo xtask legacy", "legacy *args"] {
        report.require(
            !text.contains(forbidden),
            format!("Justfile retains forbidden Make compatibility token {forbidden}"),
        );
    }

    if let Some(migration) = read_rel(root, migration_path) {
        check_tooling_migration_inventory(&migration, &recipes, report);
        check_tooling_compatibility_disclosures(&migration, report);
    }

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;

        let lifecycle = root.join("dev/lifecycle.sh");
        match fs::metadata(&lifecycle) {
            Ok(metadata) => report.require(
                metadata.permissions().mode() & 0o111 != 0,
                "dev/lifecycle.sh must be executable",
            ),
            Err(_) => report.require(false, "missing dev/lifecycle.sh"),
        }
    }

    for rel in [
        "README.md",
        "AGENTS.md",
        "docs/sdlc.md",
        "docs/architecture.md",
        "docs/deployment-ownership.md",
        "docs/rust-rewrite-harness.md",
        "docs/reference/contracts/release-evidence-contract.md",
        ".sdlc/release-evidence.json",
        "scripts/sdlc/setup",
        "scripts/sdlc/verify",
        "xtask/src/main.rs",
    ] {
        let Some(active) = read_rel(root, rel) else {
            continue;
        };
        for forbidden in [
            "`make ",
            "make check",
            "make dev",
            "make test",
            "make build",
            "make iac",
            "make release",
            "make architecture",
            "`make` targets remain",
            "$(MAKE)",
            "cargo xtask legacy",
        ] {
            report.require(
                !active.contains(forbidden),
                format!("active command surface {rel} retains Make invocation {forbidden}"),
            );
        }
    }
}

fn check_tooling_compatibility_disclosures(migration: &str, report: &mut ReportBuilder) {
    let normalized_migration = migration.split_whitespace().collect::<Vec<_>>().join(" ");
    for disclosure in [
        "## Accepted Behavior Corrections",
        "`BACKEND=native` is not a supported `iac-local-*` mode",
        "had no native Ansible role",
        "both local inventories selected `container`",
        "preflight rejected every backend except `container`",
        "`iac-local-*` defaults to `BACKEND=container`",
        "thin local single-process development remains available through `just dev`",
    ] {
        report.require(
            normalized_migration.contains(disclosure),
            format!(
                "tooling migration must disclose native IaC compatibility correction: {disclosure}"
            ),
        );
    }
}

fn check_tooling_migration_inventory(
    migration: &str,
    recipes: &BTreeSet<String>,
    report: &mut ReportBuilder,
) {
    const SOURCE_HASH: &str = "c9dc9c3f53d625acdac087b8285d5d4c3f213b30bc32541cf05aa1fc05ac375e";
    const EXPECTED_TARGETS: &[&str] = &[
        "architecture-check",
        "architecture.gate",
        "architecture.gate.legacy",
        "backtest",
        "backtest.btc",
        "build",
        "build.studio",
        "check",
        "check.ci",
        "clean",
        "clean.all",
        "clean.deps",
        "creds",
        "creds.store",
        "creds.test",
        "deploy.gate",
        "dev",
        "dev.api",
        "dev.distributed",
        "dev.distributed.compose.down",
        "dev.distributed.compose.logs",
        "dev.distributed.compose.seed",
        "dev.distributed.compose.up",
        "dev.distributed.logs",
        "dev.distributed.restart",
        "dev.distributed.start",
        "dev.distributed.status",
        "dev.distributed.stop",
        "dev.logs",
        "dev.restart",
        "dev.start",
        "dev.status",
        "dev.stop",
        "dev.studio",
        "distributed.lifecycle.smoke",
        "distributed.smoke",
        "e2e-exit-demo-clean",
        "e2e.alpaca-options-exercise",
        "e2e.alpaca-options-strategies",
        "e2e.alpaca-paper",
        "gate-ci",
        "help",
        "iac.audit",
        "iac.bootstrap",
        "iac.cloud.apply",
        "iac.cloud.bootstrap",
        "iac.cloud.destroy",
        "iac.cloud.inventory",
        "iac.cloud.plan",
        "iac.cloud.validate",
        "iac.local.destroy",
        "iac.local.down",
        "iac.local.logs",
        "iac.local.plan",
        "iac.local.status",
        "iac.local.up",
        "iac.local.verify",
        "iac.prod.audit",
        "iac.render.validate",
        "iac.server.apply",
        "iac.server.plan",
        "lifecycle.smoke",
        "marketdata.bars",
        "marketdata.conformance",
        "marketdata.indicators",
        "marketdata.quote",
        "options.chain",
        "options.select",
        "orders.attempts",
        "orders.reconcile",
        "orders.status-event",
        "orders.status-stream",
        "orders.stream-watch",
        "plugin-contract",
        "positions",
        "positions.sync",
        "preview",
        "provider.policy",
        "provider.policy.set",
        "release.evidence",
        "research.artifacts",
        "research.backtest",
        "research.job",
        "research.jobs",
        "research.sweep",
        "research.universe",
        "research.universes",
        "reset",
        "risk.account-set",
        "risk.account-sync",
        "risk.margin-preview",
        "risk.status",
        "risk.stress",
        "run",
        "run.btc-demo",
        "runtime.demo-gate",
        "runtime.ensure",
        "scan.public",
        "scheduler",
        "scheduler.run",
        "scheduler.start",
        "scheduler.stop",
        "sdlc.observe",
        "sdlc.retrospective",
        "seed",
        "seed.btc",
        "setup",
        "setup.research",
        "studio.ensure",
        "test",
        "test.e2e",
        "test.runtime",
        "test.studio",
        "validate",
        "worker.oms",
        "worker.runner",
        "worker.scheduler",
        "worker.status",
        "worker.stream",
    ];
    const REMOVED_TARGETS: &[&str] = &[
        "architecture.gate.legacy",
        "creds.store",
        "e2e.alpaca-paper",
        "worker.stream",
    ];

    report.require(
        migration.contains(SOURCE_HASH),
        "tooling migration inventory must bind to the deleted Makefile SHA-256",
    );

    let row_pattern =
        Regex::new(r"(?m)^\| `([^`]+)` \| (.+) \|$").expect("valid tooling migration row regex");
    let just_pattern =
        Regex::new(r"`just ([A-Za-z0-9_-]+)(?: [^`]*)?`").expect("valid Just command regex");
    let private_pattern =
        Regex::new(r"private `([A-Za-z0-9_-]+)` recipe").expect("valid private recipe regex");
    let mut targets = BTreeSet::new();

    for capture in row_pattern.captures_iter(migration) {
        let target = capture.get(1).expect("target capture").as_str();
        let destination = capture.get(2).expect("destination capture").as_str();
        report.require(
            targets.insert(target.to_string()),
            format!("duplicate tooling migration target {target}"),
        );

        if destination.starts_with("removed;") {
            report.require(
                REMOVED_TARGETS.contains(&target),
                format!("tooling target {target} is removed without an accepted disposition"),
            );
            continue;
        }

        let recipe = just_pattern
            .captures(destination)
            .or_else(|| private_pattern.captures(destination))
            .and_then(|captures| captures.get(1))
            .map(|matched| matched.as_str());
        match recipe {
            Some(recipe) => report.require(
                recipes.contains(recipe),
                format!("tooling migration target {target} maps to unknown recipe {recipe}"),
            ),
            None => report.require(
                false,
                format!("tooling migration target {target} has no canonical recipe"),
            ),
        }
    }

    report.require(
        targets.len() == EXPECTED_TARGETS.len(),
        format!(
            "tooling migration inventory must contain {} unique targets, found {}",
            EXPECTED_TARGETS.len(),
            targets.len()
        ),
    );
    let expected_targets = EXPECTED_TARGETS
        .iter()
        .map(|target| (*target).to_string())
        .collect::<BTreeSet<_>>();
    let missing = expected_targets
        .difference(&targets)
        .cloned()
        .collect::<Vec<_>>();
    let unexpected = targets
        .difference(&expected_targets)
        .cloned()
        .collect::<Vec<_>>();
    report.require(
        missing.is_empty() && unexpected.is_empty(),
        format!(
            "tooling migration target set drifted: missing=[{}] unexpected=[{}]",
            missing.join(","),
            unexpected.join(",")
        ),
    );
    for removed in REMOVED_TARGETS {
        report.require(
            targets.contains(*removed),
            format!("tooling migration inventory must disposition removed target {removed}"),
        );
    }
}

fn collect_import_boundary_violations(root: &Path) -> Vec<ImportViolation> {
    let runtime = root.join("runtime/src/tradeassembly");
    if !runtime.exists() {
        if root.join("runtime-rs/src/lib.rs").exists() {
            return Vec::new();
        }
        return vec![ImportViolation {
            path: "runtime/src/tradeassembly".to_string(),
            line: 1,
            imported: "<missing>".to_string(),
            rule: "runtime package is missing",
        }];
    }
    let mut violations = Vec::new();
    for path in python_files_under(root, "runtime/src/tradeassembly") {
        let rel = rel_path(root, &path);
        let imports = imports_for(&path);
        let imported_names = imported_symbol_names(&path);
        if let Some((context, layer)) = context_layer(root, &path) {
            match layer.as_str() {
                "domain" => violations.extend(domain_import_violations(&rel, &imports)),
                "commands" => {
                    violations.extend(command_import_violations(&rel, &imports, &context))
                }
                "ports" => violations.extend(port_import_violations(&rel, &imports, &context)),
                "adapters" => {
                    violations.extend(adapter_import_violations(&rel, &imports, &imported_names))
                }
                _ => {}
            }
        }
        if rel.starts_with("runtime/src/tradeassembly/platform/") {
            violations.extend(platform_import_violations(&rel, &imports));
        }
        if rel.ends_with("_api.py") || TRANSPORT_FILES.contains(&rel.as_str()) {
            violations.extend(transport_import_violations(&rel, &imports));
        }
    }
    violations
}

fn imports_for(path: &Path) -> Vec<(usize, String)> {
    let mut imports = Vec::new();
    let Some(text) = read_path(path) else {
        return imports;
    };
    for (index, line) in text.lines().enumerate() {
        let trimmed = line.trim_start();
        if let Some(rest) = trimmed.strip_prefix("import ") {
            for item in rest.split(',') {
                let imported = item.split_whitespace().next().unwrap_or_default();
                if !imported.is_empty() {
                    imports.push((index + 1, imported.to_string()));
                }
            }
        } else if let Some(rest) = trimmed.strip_prefix("from ") {
            if let Some((module, _)) = rest.split_once(" import ") {
                imports.push((index + 1, module.trim().to_string()));
            }
        }
    }
    imports
}

fn imported_symbol_names(path: &Path) -> Vec<(usize, String, String)> {
    let mut names = Vec::new();
    let Some(text) = read_path(path) else {
        return names;
    };
    for (index, line) in text.lines().enumerate() {
        let trimmed = line.trim_start();
        let Some(rest) = trimmed.strip_prefix("from ") else {
            continue;
        };
        let Some((module, names_part)) = rest.split_once(" import ") else {
            continue;
        };
        for name in names_part.split(',') {
            let name = name.split_whitespace().next().unwrap_or_default();
            if !name.is_empty() && name != "(" {
                names.push((
                    index + 1,
                    module.trim().to_string(),
                    name.trim_matches('(').to_string(),
                ));
            }
        }
    }
    names
}

fn context_layer(root: &Path, path: &Path) -> Option<(String, String)> {
    let rel = path.strip_prefix(root).ok()?;
    let parts = rel
        .components()
        .filter_map(|component| match component {
            Component::Normal(value) => Some(value.to_string_lossy().to_string()),
            _ => None,
        })
        .collect::<Vec<_>>();
    let index = parts.iter().position(|part| part == "contexts")?;
    if parts.len() <= index + 2 {
        return None;
    }
    let context = parts[index + 1].clone();
    let layer = parts[index + 2].clone();
    if REQUIRED_CONTEXT_LAYERS.contains(&layer.as_str()) {
        Some((context, layer))
    } else {
        None
    }
}

fn domain_import_violations(path: &str, imports: &[(usize, String)]) -> Vec<ImportViolation> {
    let mut violations = Vec::new();
    for (line, imported) in imports {
        let root = imported.split('.').next().unwrap_or_default();
        if PROHIBITED_DOMAIN_IMPORT_ROOTS.contains(&root) {
            violations.push(ImportViolation {
                path: path.to_string(),
                line: *line,
                imported: imported.clone(),
                rule: "context domain imports infrastructure or environment dependency",
            });
            continue;
        }
        if starts_with_any(
            imported,
            &[
                "tradeassembly.platform",
                "tradeassembly.bus",
                "tradeassembly.engine",
                "tradeassembly.persistence",
                "tradeassembly.store",
            ],
        ) {
            violations.push(ImportViolation {
                path: path.to_string(),
                line: *line,
                imported: imported.clone(),
                rule: "context domain imports storage/event infrastructure",
            });
        }
    }
    violations
}

fn command_import_violations(
    path: &str,
    imports: &[(usize, String)],
    context: &str,
) -> Vec<ImportViolation> {
    let allowed = [
        format!("tradeassembly.contexts.{context}.domain"),
        format!("tradeassembly.contexts.{context}.ports"),
        format!("tradeassembly.contexts.{context}.commands"),
    ];
    let mut violations = Vec::new();
    for (line, imported) in imports {
        if allowed.iter().any(|prefix| imported.starts_with(prefix)) {
            continue;
        }
        if starts_with_any(imported, PROHIBITED_CONTEXT_COMMAND_PREFIXES) {
            violations.push(ImportViolation {
                path: path.to_string(),
                line: *line,
                imported: imported.clone(),
                rule: "context command imports concrete infrastructure",
            });
        }
    }
    violations
}

fn port_import_violations(
    path: &str,
    imports: &[(usize, String)],
    context: &str,
) -> Vec<ImportViolation> {
    let allowed_domain = format!("tradeassembly.contexts.{context}.domain");
    let allowed_ports = format!("tradeassembly.contexts.{context}.ports");
    let mut violations = Vec::new();
    for (line, imported) in imports {
        if !imported.starts_with("tradeassembly.") {
            continue;
        }
        if imported.starts_with(&allowed_domain) || imported.starts_with(&allowed_ports) {
            continue;
        }
        violations.push(ImportViolation {
            path: path.to_string(),
            line: *line,
            imported: imported.clone(),
            rule: "context port imports outside its domain boundary",
        });
    }
    violations
}

fn adapter_import_violations(
    path: &str,
    imports: &[(usize, String)],
    imported_names: &[(usize, String, String)],
) -> Vec<ImportViolation> {
    let mut violations = Vec::new();
    for (line, imported) in imports {
        if path.ends_with("/adapters/repositories.py")
            && imported.starts_with("tradeassembly.persistence")
        {
            continue;
        }
        if starts_with_any(imported, PROHIBITED_CONTEXT_ADAPTER_PREFIXES) {
            violations.push(ImportViolation {
                path: path.to_string(),
                line: *line,
                imported: imported.clone(),
                rule: "context adapter imports raw bus/storage infrastructure",
            });
        }
    }
    for (line, module, name) in imported_names {
        if PROHIBITED_CONTEXT_ADAPTER_NAMES.contains(&name.as_str()) {
            violations.push(ImportViolation {
                path: path.to_string(),
                line: *line,
                imported: format!("{module}.{name}"),
                rule: "context adapter imports raw bus/storage infrastructure",
            });
        }
    }
    violations
}

fn platform_import_violations(path: &str, imports: &[(usize, String)]) -> Vec<ImportViolation> {
    let mut violations = Vec::new();
    for (line, imported) in imports {
        if path == "runtime/src/tradeassembly/platform/storage/repositories.py"
            && imported.starts_with("tradeassembly.contexts.")
            && imported.contains(".adapters.repositories")
        {
            continue;
        }
        if imported.starts_with("tradeassembly.contexts.") && imported.contains(".adapters") {
            violations.push(ImportViolation {
                path: path.to_string(),
                line: *line,
                imported: imported.clone(),
                rule: "platform imports a context adapter",
            });
        }
    }
    violations
}

fn transport_import_violations(path: &str, imports: &[(usize, String)]) -> Vec<ImportViolation> {
    let mut violations = Vec::new();
    for (line, imported) in imports {
        if imported.starts_with("tradeassembly.contexts.")
            && [".domain", ".ports", ".adapters"]
                .iter()
                .any(|part| imported.contains(part))
        {
            violations.push(ImportViolation {
                path: path.to_string(),
                line: *line,
                imported: imported.clone(),
                rule: "transport imports context internals instead of command handlers",
            });
        }
        if starts_with_any(
            imported,
            &[
                "tradeassembly.engine",
                "tradeassembly.store",
                "tradeassembly.persistence",
                "tradeassembly.bus",
            ],
        ) {
            violations.push(ImportViolation {
                path: path.to_string(),
                line: *line,
                imported: imported.clone(),
                rule: "transport imports runtime facade or raw infrastructure",
            });
        }
    }
    violations
}

fn collect_interface_violations(root: &Path) -> Vec<InterfaceViolation> {
    let runtime = root.join("runtime/src/tradeassembly");
    if !runtime.exists() {
        return Vec::new();
    }
    let mut violations = Vec::new();
    if let Ok(entries) = fs::read_dir(runtime) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|value| value.to_str()) != Some("py") {
                continue;
            }
            let rel = rel_path(root, &path);
            if !(rel.ends_with("_api.py") || ROUTE_FILES.contains(&rel.as_str())) {
                continue;
            }
            let Some(text) = read_path(&path) else {
                continue;
            };
            let imports_application_commands = imports_application_commands(&text);
            let receives_command_group = receives_command_group(&text);
            if defines_axum_routes(&text)
                && !(imports_application_commands || receives_command_group)
            {
                violations.push(InterfaceViolation {
                    path: rel.clone(),
                    line: 1,
                    rule: "REST route module does not use application commands",
                    detail: "import tradeassembly.application_commands or receive an explicit injected command group"
                        .to_string(),
                });
            }
            violations.extend(register_function_interface_violations(&rel, &text));
            violations.extend(dynamic_dispatcher_violations(&rel, &text));
            violations.extend(app_state_runtime_exposure_violations(&rel, &text));
        }
    }
    violations.extend(route_command_registry_exposure_violations(root));
    violations
}

fn register_function_interface_violations(path: &str, text: &str) -> Vec<InterfaceViolation> {
    let mut violations = Vec::new();
    for function in function_blocks(text) {
        if !function.name.starts_with("register_") {
            continue;
        }
        let mut tainted = HashSet::new();
        for arg in parse_function_args(function.signature) {
            if FORBIDDEN_ROUTE_PARAMETERS.contains(&arg.as_str()) {
                violations.push(InterfaceViolation {
                    path: path.to_string(),
                    line: function.line,
                    rule: "REST route module receives runtime service/control instead of explicit command group",
                    detail: arg.clone(),
                });
            }
            if FORBIDDEN_ROUTE_RECEIVERS.contains(&arg.as_str()) {
                tainted.insert(arg);
            }
        }
        for (line_offset, line) in function.block.lines().enumerate() {
            if let Some((target, value)) = simple_assignment(line) {
                if tainted.contains(value) || FORBIDDEN_ROUTE_RECEIVERS.contains(&value) {
                    tainted.insert(target.to_string());
                }
            }
            for (receiver, attr) in dotted_calls(line) {
                if tainted.contains(receiver) {
                    let direct = FORBIDDEN_ROUTE_RECEIVERS.contains(&receiver);
                    violations.push(InterfaceViolation {
                        path: path.to_string(),
                        line: function.line + line_offset,
                        rule: if direct {
                            "REST route calls business service/control directly"
                        } else {
                            "REST route calls aliased business service/control directly"
                        },
                        detail: format!("{receiver}.{attr}"),
                    });
                }
            }
        }
    }
    violations
}

fn dynamic_dispatcher_violations(path: &str, text: &str) -> Vec<InterfaceViolation> {
    let mut violations = Vec::new();
    for (line_number, line) in text.lines().enumerate() {
        for dispatcher in ["RuntimeCommandDispatcher", "ProductCommandDispatcher"] {
            if line.contains(&format!("{dispatcher}(")) || line.contains(&format!(".{dispatcher}("))
            {
                violations.push(InterfaceViolation {
                    path: path.to_string(),
                    line: line_number + 1,
                    rule: "REST route constructs dynamic application command dispatcher",
                    detail: dispatcher.to_string(),
                });
            }
        }
        for (receiver, attr) in dotted_calls(line) {
            if FORBIDDEN_ROUTE_RECEIVERS.contains(&receiver) {
                violations.push(InterfaceViolation {
                    path: path.to_string(),
                    line: line_number + 1,
                    rule: "REST route calls business service/control directly",
                    detail: format!("{receiver}.{attr}"),
                });
            }
        }
    }
    violations
}

fn app_state_runtime_exposure_violations(path: &str, text: &str) -> Vec<InterfaceViolation> {
    let mut violations = Vec::new();
    for (line_number, line) in text.lines().enumerate() {
        for attr in ["service", "control", "runtime"] {
            if line.contains(&format!(".state.{attr}")) && line.contains('=') {
                violations.push(InterfaceViolation {
                    path: path.to_string(),
                    line: line_number + 1,
                    rule: "Rust axum HTTP API app state exposes runtime service/control instead of command registry",
                    detail: format!("app.state.{attr}"),
                });
            }
        }
    }
    violations
}

fn route_command_registry_exposure_violations(root: &Path) -> Vec<InterfaceViolation> {
    let rel = "runtime/src/tradeassembly/application/route_commands.py";
    let Some(text) = read_rel(root, rel) else {
        return Vec::new();
    };
    let mut violations = Vec::new();
    for class in class_blocks(&text) {
        if class.name != "FastApiRouteCommandRegistry" && class.name != "ProductRouteCommands" {
            continue;
        }
        for (line_offset, line) in class.block.lines().enumerate() {
            let trimmed = line.trim();
            let Some((field, annotation)) = trimmed.split_once(':') else {
                continue;
            };
            let field = field.trim();
            if field.contains(' ') || field.is_empty() {
                continue;
            }
            if ["service", "control", "runtime"].contains(&field) {
                violations.push(InterfaceViolation {
                    path: rel.to_string(),
                    line: class.line + line_offset,
                    rule: "route command registry exposes runtime service/control instead of command groups",
                    detail: format!("{}.{}", class.name, field),
                });
            }
            if class.name == "FastApiRouteCommandRegistry"
                && ROUTE_GROUPS_MIGRATED_OFF_SERVICE_COMMANDS.contains(&field)
                && annotation.trim().starts_with("ServiceRouteCommands")
            {
                violations.push(InterfaceViolation {
                    path: rel.to_string(),
                    line: class.line + line_offset,
                    rule: "migrated REST route group is still backed by legacy service route commands",
                    detail: format!("FastApiRouteCommandRegistry.{field}"),
                });
            }
        }
    }
    violations
}

fn stale_kafka_default_references(root: &Path) -> Vec<String> {
    let mut stale = Vec::new();
    for rel in [
        "AGENTS.md",
        "README.md",
        "docs/architecture.md",
        "docs/decisions.md",
        "docs/principles.md",
        "docs/technical-design-guide.md",
        "docs/reference/runtime/runtime-failure-model.md",
        "docs/rust-rewrite-harness.md",
        "docs/source-parity-map.md",
        "deploy/README.md",
        "infra/README.md",
    ] {
        let path = root.join(rel);
        if path.exists() {
            add_stale_kafka_lines(root, &path, &mut stale);
        }
    }
    stale
}

fn add_stale_kafka_lines(root: &Path, path: &Path, stale: &mut Vec<String>) {
    let Some(text) = read_path(path) else {
        return;
    };
    let rel = rel_path(root, path);
    for (line_number, line) in text.lines().enumerate() {
        if is_stale_kafka_default_claim(line) {
            stale.push(format!("{rel}:{}: {}", line_number + 1, line.trim()));
        }
    }
}

fn is_stale_kafka_default_claim(line: &str) -> bool {
    let lower = line.to_lowercase();
    if [
        "no docs surface tells users",
        "test that fails if docs mention",
        "remove statements saying",
        "stale kafka-default docs reference",
        "historical decision only",
    ]
    .iter()
    .any(|phrase| lower.contains(phrase))
    {
        return false;
    }
    if [
        "kafka remains the source-decided default",
        "kafka default for distributed-dev",
        "defaults non-local profiles to kafka",
        "compose runs postgres, kafka",
    ]
    .iter()
    .any(|pattern| lower.contains(pattern))
    {
        return true;
    }
    if !lower.contains("kafka") || !lower.contains("default") {
        return false;
    }
    if lower.contains("kafka") && (lower.contains("optional") || lower.contains("compat")) {
        return false;
    }
    if lower.contains("kafka-default") {
        return false;
    }
    true
}

fn compatibility_shims(root: &Path) -> Vec<CompatibilityShim> {
    let Some(text) = read_rel(root, "docs/reference/runtime/compatibility-shims.md") else {
        return Vec::new();
    };
    let mut shims = Vec::new();
    for line in text.lines() {
        if !line.starts_with("| `") {
            continue;
        }
        let cells = line
            .trim()
            .trim_matches('|')
            .split('|')
            .map(|cell| cell.trim().trim_matches('`').to_string())
            .collect::<Vec<_>>();
        if cells.len() != 6 {
            continue;
        }
        shims.push(CompatibilityShim {
            shim_path: cells[0].clone(),
            canonical_path: cells[2].clone(),
        });
    }
    shims
}

fn unclassified_runtime_surfaces(root: &Path) -> Vec<String> {
    let base = root.join("runtime/src/tradeassembly");
    if !base.exists() {
        return vec!["runtime/src/tradeassembly".to_string()];
    }
    let shim_paths = compatibility_shims(root)
        .into_iter()
        .map(|shim| trim_trailing_slash(&shim.shim_path).to_string())
        .collect::<HashSet<_>>();
    let mut unclassified = Vec::new();
    let canonical = CANONICAL_RUNTIME_SURFACES
        .iter()
        .copied()
        .collect::<HashSet<_>>();
    if let Ok(entries) = fs::read_dir(base) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.file_name().and_then(|value| value.to_str()) == Some("__pycache__") {
                continue;
            }
            if path.is_file() && path.extension().and_then(|value| value.to_str()) != Some("py") {
                continue;
            }
            let rel = trim_trailing_slash(&rel_path(root, &path)).to_string();
            if canonical.contains(rel.as_str()) || shim_paths.contains(&rel) {
                continue;
            }
            unclassified.push(rel);
        }
    }
    unclassified.sort();
    unclassified
}

fn active_business_compatibility_shims(root: &Path) -> Vec<String> {
    compatibility_shims(root)
        .into_iter()
        .filter(|shim| {
            (shim
                .canonical_path
                .contains("runtime/src/tradeassembly/contexts/")
                || shim.canonical_path == "context command packages")
                && !is_compatibility_shim_only(root, &shim.shim_path)
        })
        .map(|shim| shim.shim_path)
        .collect()
}

fn context_stub_layers(root: &Path) -> Vec<String> {
    let mut stubs = Vec::new();
    for context in REQUIRED_CONTEXTS {
        for layer in REQUIRED_CONTEXT_LAYERS {
            let rel = format!("runtime/src/tradeassembly/contexts/{context}/{layer}/__init__.py");
            let path = root.join(&rel);
            if path.exists() && is_docstring_only(&path) {
                stubs.push(rel);
            }
        }
    }
    stubs
}

fn is_docstring_only(path: &Path) -> bool {
    let Some(text) = read_path(path) else {
        return false;
    };
    let stripped = text.trim();
    stripped.is_empty()
        || (stripped.starts_with("\"\"\"") && stripped.ends_with("\"\"\""))
        || (stripped.starts_with("'''") && stripped.ends_with("'''"))
}

fn is_compatibility_shim_only(root: &Path, shim_path: &str) -> bool {
    let path = root.join(trim_trailing_slash(shim_path));
    if path.is_dir() {
        let python_files = python_files_under_path(&path);
        return !python_files.is_empty() && python_files.iter().all(|path| is_marked_shim(path));
    }
    path.is_file() && is_marked_shim(&path)
}

fn is_marked_shim(path: &Path) -> bool {
    let Some(text) = read_path(path) else {
        return false;
    };
    let stripped = strip_leading_copyright_comments(&text);
    if !stripped.starts_with("# tradeassembly-compat-shim") {
        return false;
    }
    ![
        "tradeassembly.store",
        "tradeassembly.persistence",
        "tradeassembly.bus",
        "sqlite3",
        "psycopg",
        "sqlalchemy",
    ]
    .iter()
    .any(|token| text.contains(token))
}

fn strip_leading_copyright_comments(text: &str) -> String {
    let mut lines = text.lines().peekable();
    while let Some(line) = lines.peek() {
        let trimmed = line.trim();
        if trimmed.is_empty()
            || trimmed == "# Copyright (c) 2026 OptionLab LLC. All rights reserved."
        {
            lines.next();
            continue;
        }
        break;
    }
    lines
        .collect::<Vec<_>>()
        .join("\n")
        .trim_start()
        .to_string()
}

fn thin_runtime_shim_failures(root: &Path) -> Vec<String> {
    let rel = "runtime/src/tradeassembly/engine/__init__.py";
    let Some(text) = read_rel(root, rel) else {
        return vec![format!("missing {rel}")];
    };
    let mut failures = Vec::new();
    if text.lines().count() > 250 {
        failures
            .push("tradeassembly.engine compatibility shim must be 250 LOC or less".to_string());
    }
    let class = class_blocks(&text)
        .into_iter()
        .find(|block| block.name == "LocalRuntimeService");
    let Some(class) = class else {
        failures.push(
            "tradeassembly.engine must expose LocalRuntimeService compatibility shim".to_string(),
        );
        return failures;
    };
    let methods = class_methods(&text, "LocalRuntimeService");
    if !methods.is_empty() {
        failures.push(format!(
            "LocalRuntimeService compatibility shim defines methods instead of inheriting/delegating: {:?}",
            methods
        ));
    }
    if !class.signature.contains("_LegacyRuntimeService") {
        failures.push(
            "LocalRuntimeService compatibility shim must inherit _LegacyRuntimeService".to_string(),
        );
    }
    if text.contains("ApplicationCommandRegistry") {
        failures.push(
            "tradeassembly.engine shim must not construct application command registry directly"
                .to_string(),
        );
    }
    failures
}

fn runtime_surface_inventory_failures(root: &Path) -> Vec<String> {
    thin_runtime_shim_failures(root)
}

fn scan_ui_stubs(root: &Path) -> Vec<UiStubGap> {
    let webapp = root.join("webapp");
    if !webapp.exists() {
        return Vec::new();
    }
    let mut gaps = Vec::new();
    for entry in WalkDir::new(webapp).into_iter().filter_map(Result::ok) {
        if !entry.file_type().is_file() {
            continue;
        }
        let path = entry.path();
        let rel = rel_path(root, path);
        if !UI_SUFFIXES.iter().any(|suffix| rel.ends_with(suffix)) {
            continue;
        }
        if path_components(path)
            .iter()
            .any(|part| UI_EXCLUDED_PARTS.contains(&part.as_str()))
        {
            continue;
        }
        if rel.contains("/tests/") {
            continue;
        }
        let Some(text) = read_path(path) else {
            continue;
        };
        let mut jsx_state = JsxTagScanState::default();
        let scans_jsx = rel.ends_with(".tsx") || rel.ends_with(".jsx");
        for (line_number, line) in text.lines().enumerate() {
            let lowered = line.to_lowercase();
            let marker_source = if scans_jsx {
                mask_jsx_placeholder_attributes(&lowered, &mut jsx_state)
            } else {
                lowered.clone()
            };
            if let Some(marker) = UI_MARKERS
                .iter()
                .copied()
                .find(|marker| marker_source.contains(&marker.to_lowercase()))
            {
                // `local-data` is the stable identifier of TradeAssembly's
                // bundled local-data provider. A quoted identifier is a
                // runtime binding, not a UI stub marker.
                if marker == "local-data" && contains_quoted_token(line, marker) {
                    continue;
                }
                gaps.push(UiStubGap {
                    path: rel.clone(),
                    line: line_number + 1,
                    marker,
                    text: line.trim().to_string(),
                });
            }
        }
    }
    gaps.sort_by(|a, b| {
        let a_parts = a.path.split('/').collect::<Vec<_>>();
        let b_parts = b.path.split('/').collect::<Vec<_>>();
        a_parts.cmp(&b_parts).then(a.line.cmp(&b.line))
    });
    gaps
}

fn contains_quoted_token(line: &str, token: &str) -> bool {
    ["\"", "'", "`"].iter().any(|quote| {
        let quoted = format!("{quote}{token}{quote}");
        line.contains(&quoted)
    })
}

#[derive(Default)]
struct JsxTagScanState {
    inside_tag: bool,
    quote: Option<char>,
    escaped: bool,
    expression_depth: usize,
}

fn mask_jsx_placeholder_attributes(line: &str, state: &mut JsxTagScanState) -> String {
    const PLACEHOLDER: &str = "placeholder";
    let mut masked = String::with_capacity(line.len());
    let mut index = 0;
    let mut previous = None;

    while index < line.len() {
        let ch = line[index..].chars().next().expect("character boundary");

        if let Some(quote) = state.quote {
            masked.push(ch);
            if state.escaped {
                state.escaped = false;
            } else if ch == '\\' {
                state.escaped = true;
            } else if ch == quote {
                state.quote = None;
            }
            index += ch.len_utf8();
            previous = Some(ch);
            continue;
        }

        if state.inside_tag && state.expression_depth == 0 && line[index..].starts_with(PLACEHOLDER)
        {
            let follows_boundary = line[index + PLACEHOLDER.len()..]
                .chars()
                .next()
                .is_some_and(|next| next.is_whitespace() || next == '=');
            if previous.is_none_or(char::is_whitespace) && follows_boundary {
                index += PLACEHOLDER.len();
                previous = Some(' ');
                continue;
            }
        }

        if state.inside_tag {
            match ch {
                '\'' | '"' | '`' => state.quote = Some(ch),
                '{' => state.expression_depth += 1,
                '}' => state.expression_depth = state.expression_depth.saturating_sub(1),
                '>' if state.expression_depth == 0 => state.inside_tag = false,
                _ => {}
            }
        } else if ch == '<' {
            let next = line[index + ch.len_utf8()..].chars().next();
            if next.is_some_and(|next| next == '/' || next.is_ascii_alphabetic()) {
                state.inside_tag = true;
            }
        }

        masked.push(ch);
        index += ch.len_utf8();
        previous = Some(ch);
    }

    masked
}

fn expected_ui_stub_gap_text(root: &Path) -> String {
    render_ui_stub_gaps(&scan_ui_stubs(root))
}

fn render_ui_stub_gaps(gaps: &[UiStubGap]) -> String {
    let mut lines = vec![
        "# UI Stub Gaps".to_string(),
        String::new(),
        "Generated by `tradeassembly architecture scan-ui-stubs`.".to_string(),
        "This file is a soft-enforcement ledger: CI fails on stale output, not on listed gaps."
            .to_string(),
        String::new(),
        "| File | Line | Marker | Evidence |".to_string(),
        "| --- | ---: | --- | --- |".to_string(),
    ];
    for gap in gaps {
        let text = gap.text.replace('|', "\\|");
        lines.push(format!(
            "| `{}` | {} | `{}` | {} |",
            gap.path, gap.line, gap.marker, text
        ));
    }
    if gaps.is_empty() {
        lines.push("| _none_ | 0 | _none_ | No UI stub markers found. |".to_string());
    }
    lines.push(String::new());
    lines.join("\n")
}

fn typescript_object_keys(text: &str, export_name: &str) -> BTreeSet<String> {
    let Some(start) = text.find(&format!("export const {export_name}")) else {
        return BTreeSet::new();
    };
    let Some(brace_offset) = text[start..].find('{') else {
        return BTreeSet::new();
    };
    let brace = start + brace_offset;
    let Some(end) = matching_brace(text, brace) else {
        return BTreeSet::new();
    };
    let mut keys = BTreeSet::new();
    let mut depth = 0usize;
    for line in text[brace + 1..end].lines() {
        if depth == 0 {
            if let Some(capture) = ts_key_regex().captures(line) {
                keys.insert(capture[1].to_string());
            }
        }
        for char in line.chars() {
            match char {
                '{' => depth += 1,
                '}' => depth = depth.saturating_sub(1),
                _ => {}
            }
        }
    }
    keys
}

fn typescript_object_entry(text: &str, key: &str) -> String {
    let Some(match_start) = find_ts_key_line(text, key) else {
        return String::new();
    };
    let Some(brace_offset) = text[match_start..].find('{') else {
        return String::new();
    };
    let brace = match_start + brace_offset;
    let Some(end) = matching_brace(text, brace) else {
        return String::new();
    };
    text[brace + 1..end].to_string()
}

fn matching_brace(text: &str, open: usize) -> Option<usize> {
    let mut depth = 0isize;
    for (offset, char) in text[open..].char_indices() {
        match char {
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    return Some(open + offset);
                }
            }
            _ => {}
        }
    }
    None
}

fn find_ts_key_line(text: &str, key: &str) -> Option<usize> {
    let pattern = Regex::new(&format!(r"(?m)^\s*{}\s*:", regex::escape(key))).ok()?;
    pattern.find(text).map(|matched| matched.start())
}

fn just_recipe_block(text: &str, target: &str) -> String {
    let target_prefix = format!("{target}:");
    let mut block = Vec::new();
    let mut in_target = false;
    for line in text.lines() {
        if line.starts_with(&target_prefix) {
            in_target = true;
            block.push(line);
            continue;
        }
        if in_target {
            if line.starts_with(' ') || line.starts_with('\t') || line.trim().is_empty() {
                block.push(line);
            } else {
                break;
            }
        }
    }
    block.join("\n")
}

#[derive(Debug)]
struct FunctionBlock<'a> {
    name: String,
    line: usize,
    signature: &'a str,
    block: String,
}

#[derive(Debug)]
struct ClassBlock {
    name: String,
    line: usize,
    signature: String,
    block: String,
}

fn function_blocks(text: &str) -> Vec<FunctionBlock<'_>> {
    let lines = text.lines().collect::<Vec<_>>();
    let mut blocks = Vec::new();
    for (index, line) in lines.iter().enumerate() {
        let trimmed = line.trim_start();
        let Some(signature) = trimmed.strip_prefix("def ") else {
            continue;
        };
        let Some(name) = signature.split('(').next() else {
            continue;
        };
        let indent = line.len() - trimmed.len();
        let body_start = signature_body_start(&lines, index);
        let mut body = Vec::new();
        for next in lines.iter().skip(body_start) {
            let next_trimmed = next.trim_start();
            let next_indent = next.len() - next_trimmed.len();
            if !next_trimmed.is_empty() && next_indent <= indent && !next_trimmed.starts_with('@') {
                break;
            }
            body.push(*next);
        }
        blocks.push(FunctionBlock {
            name: name.to_string(),
            line: index + 1,
            signature,
            block: body.join("\n"),
        });
    }
    blocks
}

fn class_blocks(text: &str) -> Vec<ClassBlock> {
    let lines = text.lines().collect::<Vec<_>>();
    let mut blocks = Vec::new();
    for (index, line) in lines.iter().enumerate() {
        let trimmed = line.trim_start();
        let Some(signature) = trimmed.strip_prefix("class ") else {
            continue;
        };
        let name = signature
            .split(['(', ':'])
            .next()
            .unwrap_or_default()
            .trim()
            .to_string();
        let indent = line.len() - trimmed.len();
        let mut body = Vec::new();
        for next in lines.iter().skip(index + 1) {
            let next_trimmed = next.trim_start();
            let next_indent = next.len() - next_trimmed.len();
            if !next_trimmed.is_empty() && next_indent <= indent && !next_trimmed.starts_with('@') {
                break;
            }
            body.push(*next);
        }
        blocks.push(ClassBlock {
            name,
            line: index + 1,
            signature: signature.to_string(),
            block: body.join("\n"),
        });
    }
    blocks
}

fn class_methods(text: &str, class_name: &str) -> HashSet<String> {
    class_blocks(text)
        .into_iter()
        .find(|block| block.name == class_name)
        .map(|block| {
            function_blocks(&block.block)
                .into_iter()
                .map(|function| function.name)
                .filter(|name| !(name.starts_with("__") && name.ends_with("__")))
                .collect()
        })
        .unwrap_or_default()
}

fn top_level_functions(text: &str) -> HashSet<String> {
    text.lines()
        .filter_map(|line| {
            if line.starts_with("def ") {
                line.strip_prefix("def ")?
                    .split('(')
                    .next()
                    .map(str::to_string)
            } else if line.starts_with("async def ") {
                line.strip_prefix("async def ")?
                    .split('(')
                    .next()
                    .map(str::to_string)
            } else {
                None
            }
        })
        .collect()
}

fn protocol_methods(text: &str) -> HashSet<String> {
    let mut methods = HashSet::new();
    for block in class_blocks(text)
        .into_iter()
        .filter(|block| block.signature.contains("Protocol"))
    {
        for function in function_blocks(&block.block) {
            methods.insert(function.name);
        }
    }
    methods
}

fn click_command_blocks(text: &str) -> Vec<(String, String)> {
    let lines = text.lines().collect::<Vec<_>>();
    let mut result = Vec::new();
    let mut decorators = Vec::new();
    for (index, line) in lines.iter().enumerate() {
        let trimmed = line.trim_start();
        if trimmed.starts_with('@') {
            decorators.push(trimmed.to_string());
            continue;
        }
        if let Some(signature) = trimmed.strip_prefix("def ") {
            let name = signature.split('(').next().unwrap_or_default().to_string();
            if decorators
                .iter()
                .any(|decorator| decorator.contains(".command(") || decorator.ends_with(".command"))
            {
                let indent = line.len() - trimmed.len();
                let body_start = signature_body_start(&lines, index);
                let mut body = Vec::new();
                for next in lines.iter().skip(body_start) {
                    let next_trimmed = next.trim_start();
                    let next_indent = next.len() - next_trimmed.len();
                    if !next_trimmed.is_empty()
                        && next_indent <= indent
                        && !next_trimmed.starts_with('@')
                    {
                        break;
                    }
                    body.push(*next);
                }
                result.push((name, body.join("\n")));
            }
            decorators.clear();
        } else if !trimmed.is_empty() {
            decorators.clear();
        }
    }
    result
}

fn signature_body_start(lines: &[&str], start: usize) -> usize {
    for (index, line) in lines.iter().enumerate().skip(start) {
        if line.trim_end().ends_with(':') {
            return index + 1;
        }
    }
    start + 1
}

fn parse_function_args(signature: &str) -> Vec<String> {
    let Some(start) = signature.find('(') else {
        return Vec::new();
    };
    let Some(end) = signature.rfind(')') else {
        return Vec::new();
    };
    signature[start + 1..end]
        .split(',')
        .filter_map(|arg| {
            let name = arg
                .trim()
                .trim_start_matches('*')
                .split([':', '='])
                .next()
                .unwrap_or_default()
                .trim();
            if name.is_empty() {
                None
            } else {
                Some(name.to_string())
            }
        })
        .collect()
}

fn simple_assignment(line: &str) -> Option<(&str, &str)> {
    let trimmed = line.trim();
    if trimmed.starts_with('#') || !trimmed.contains('=') || trimmed.contains("==") {
        return None;
    }
    let (target, value) = trimmed.split_once('=')?;
    let target = target.trim();
    let value = expression_root_name(value.trim())?;
    if is_identifier(target) && is_identifier(value) {
        Some((target, value))
    } else {
        None
    }
}

fn expression_root_name(value: &str) -> Option<&str> {
    value
        .split('.')
        .next()
        .map(str::trim)
        .filter(|value| is_identifier(value))
}

fn dotted_calls(line: &str) -> Vec<(&str, &str)> {
    dotted_call_regex()
        .captures_iter(line)
        .map(|capture| {
            let receiver = capture.get(1).expect("receiver").as_str();
            let attr = capture.get(2).expect("attr").as_str();
            (receiver, attr)
        })
        .collect()
}

fn imports_application_commands(text: &str) -> bool {
    text.contains("import tradeassembly.application_commands")
        || text.contains("from tradeassembly import application_commands")
        || text.contains("from tradeassembly.application_commands import")
}

fn defines_axum_routes(text: &str) -> bool {
    text.lines().any(|line| {
        let trimmed = line.trim_start();
        trimmed.starts_with('@')
            && [".get(", ".post(", ".put(", ".patch(", ".delete("]
                .iter()
                .any(|marker| trimmed.contains(marker))
    })
}

fn receives_command_group(text: &str) -> bool {
    function_blocks(text).into_iter().any(|function| {
        function.name.starts_with("register_")
            && parse_function_args(function.signature)
                .iter()
                .any(|arg| arg == "commands")
    })
}

fn read_rel(root: &Path, rel: &str) -> Option<String> {
    read_path(&root.join(rel))
}

fn read_path(path: &Path) -> Option<String> {
    fs::read_to_string(path).ok()
}

fn python_files_under(root: &Path, rel: &str) -> Vec<PathBuf> {
    python_files_under_path(&root.join(rel))
}

fn python_files_under_path(path: &Path) -> Vec<PathBuf> {
    if !path.exists() {
        return Vec::new();
    }
    WalkDir::new(path)
        .into_iter()
        .filter_map(Result::ok)
        .filter(|entry| entry.file_type().is_file())
        .map(|entry| entry.into_path())
        .filter(|path| path.extension().and_then(|value| value.to_str()) == Some("py"))
        .filter(|path| {
            !path_components(path)
                .iter()
                .any(|part| part == "__pycache__")
        })
        .collect()
}

fn rel_path(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/")
}

fn path_components(path: &Path) -> Vec<String> {
    path.components()
        .filter_map(|component| match component {
            Component::Normal(value) => Some(value.to_string_lossy().to_string()),
            _ => None,
        })
        .collect()
}

fn trim_trailing_slash(value: &str) -> &str {
    value.strip_suffix('/').unwrap_or(value)
}

fn starts_with_any(value: &str, prefixes: &[&str]) -> bool {
    prefixes.iter().any(|prefix| value.starts_with(prefix))
}

fn is_identifier(value: &str) -> bool {
    let mut chars = value.chars();
    match chars.next() {
        Some(first) if first == '_' || first.is_ascii_alphabetic() => {}
        _ => return false,
    }
    chars.all(|char| char == '_' || char.is_ascii_alphanumeric())
}

fn ts_key_regex() -> &'static Regex {
    static REGEX: OnceLock<Regex> = OnceLock::new();
    REGEX.get_or_init(|| {
        Regex::new(r"^\s*([A-Za-z_][A-Za-z0-9_]*)\s*:").expect("valid ts key regex")
    })
}

fn dotted_call_regex() -> &'static Regex {
    static REGEX: OnceLock<Regex> = OnceLock::new();
    REGEX.get_or_init(|| {
        Regex::new(r"\b([A-Za-z_][A-Za-z0-9_]*)\.([A-Za-z_][A-Za-z0-9_]*)\s*\(")
            .expect("valid dotted call regex")
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_repo(name: &str) -> PathBuf {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let root =
            std::env::temp_dir().join(format!("tradeassembly-architecture-check-{name}-{stamp}"));
        fs::create_dir_all(&root).expect("create temp root");
        root
    }

    fn write(root: &Path, rel: &str, text: &str) {
        let path = root.join(rel);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).expect("create parent");
        }
        fs::write(path, text).expect("write fixture");
    }

    #[test]
    fn rejects_context_domain_infrastructure_imports() {
        let root = temp_repo("domain-imports");
        write(
            &root,
            "runtime/src/tradeassembly/contexts/strategy/domain/rules.py",
            "import click\nimport sqlite3\n",
        );

        let report = run_architecture_check(&root);

        assert!(
            report
                .failures
                .iter()
                .any(|message| message.contains("domain imports infrastructure")
                    && message.contains("click")),
            "{report:?}"
        );
        assert!(
            report
                .failures
                .iter()
                .any(|message| message.contains("domain imports infrastructure")
                    && message.contains("sqlite3")),
            "{report:?}"
        );
    }

    #[test]
    fn accepts_rust_only_runtime_for_import_boundary_scan() {
        let root = temp_repo("rust-runtime-boundary");
        write(
            &root,
            "runtime-rs/src/lib.rs",
            "pub mod engine;\npub mod platform_events;\n",
        );

        let violations = collect_import_boundary_violations(&root);

        assert!(violations.is_empty(), "{violations:?}");
    }

    #[test]
    fn rejects_aliased_business_service_calls_from_routes() {
        let root = temp_repo("route-service");
        write(
            &root,
            "runtime/src/tradeassembly/product_api.py",
            r#"class App:
    def post(self, path):
        return lambda handler: handler

def register_product_routes(app: App, *, service):
    runtime_commands = service
    @app.post('/product/strategies/create')
    def create(payload: dict):
        return runtime_commands.create_strategy_from_template()
"#,
        );

        let report = run_architecture_check(&root);

        assert!(
            report
                .failures
                .iter()
                .any(|message| message.contains("runtime_commands.create_strategy_from_template")),
            "{report:?}"
        );
    }

    #[test]
    fn reports_stale_ui_stub_gap_ledger() {
        let root = temp_repo("ui-stubs");
        write(&root, "docs/ui-stub-gaps.md", "stale\n");
        write(
            &root,
            "webapp/app/page.tsx",
            "export default function Page() { return <div>TODO replace</div> }\n",
        );

        let report = run_architecture_check(&root);

        assert!(
            report
                .failures
                .iter()
                .any(|message| message.contains("docs/ui-stub-gaps.md is stale")),
            "{report:?}"
        );
    }

    #[test]
    fn rejects_stale_future_owner_claims_in_current_state_docs() {
        for (index, stale_claim) in STALE_ACCEPTED_OWNER_CLAIMS.iter().enumerate() {
            let root = temp_repo(&format!("current-state-docs-{index}"));
            for rel in CURRENT_STATE_DOCS {
                write(&root, rel, "Current accepted implementation status.\n");
            }
            write(
                &root,
                "docs/reference/current-product-capabilities.md",
                &format!("Current claim: {stale_claim}.\n"),
            );

            let mut report = ReportBuilder::new();
            check_current_state_docs(&root, &mut report);

            assert!(
                report.failures.iter().any(|message| {
                    message.contains("current-product-capabilities.md")
                        && message.contains(stale_claim)
                }),
                "stale claim was not rejected: {stale_claim}; {report:?}"
            );
        }
    }

    #[test]
    fn accepts_reconciled_current_state_docs() {
        let root = temp_repo("reconciled-current-state-docs");
        for rel in CURRENT_STATE_DOCS {
            write(
                &root,
                rel,
                "Accepted local behavior is implemented; external release work remains.\n",
            );
        }

        let mut report = ReportBuilder::new();
        check_current_state_docs(&root, &mut report);

        assert!(report.failures.is_empty(), "{report:?}");
    }

    #[test]
    fn ui_stub_scan_ignores_jsx_placeholder_attributes_only() {
        let root = temp_repo("ui-placeholder-attributes");
        write(
            &root,
            "webapp/app/page.tsx",
            r#"export default function Page() {
  return <>
    <input placeholder = "Useful input hint" />
    <input
      placeholder={configured ? "Stored; enter to replace" : ""}
    />
    <div>placeholder implementation</div>
    <div>placeholder = pending</div>
    <div>TODO replace</div>
  </>
}
"#,
        );

        let gaps = scan_ui_stubs(&root);

        assert_eq!(gaps.len(), 3, "{gaps:?}");
        assert_eq!(gaps[0].marker, "placeholder");
        assert_eq!(gaps[0].line, 7);
        assert_eq!(gaps[1].marker, "placeholder");
        assert_eq!(gaps[1].line, 8);
        assert_eq!(gaps[2].marker, "TODO");
        assert_eq!(gaps[2].line, 9);
    }

    #[test]
    fn ui_stub_scan_allows_quoted_local_data_provider_identifier() {
        let root = temp_repo("ui-local-data-provider-identifier");
        write(
            &root,
            "webapp/lib/providers.ts",
            r#"const providerRef = "local-data";
const fallback = 'local-data';
const description = `local-data`;
const unresolved = local-data;
"#,
        );

        let gaps = scan_ui_stubs(&root);

        assert_eq!(gaps.len(), 1, "{gaps:?}");
        assert_eq!(gaps[0].marker, "local-data");
        assert_eq!(gaps[0].line, 4);
    }

    #[test]
    fn requires_identity_and_credential_contract_terms() {
        let root = temp_repo("contract-docs");
        write(
            &root,
            "docs/reference/contracts/identity-install-contracts.md",
            "Issuer discovery\nToken validation\nLogin\nStatus\nLogout\nInstall claim\nMetrics opt-out\nNo broker/data credential custody\nGraceful hosted-service failure\nWorkOS/AuthKit\n",
        );
        write(
            &root,
            "docs/reference/contracts/credential-custody-contracts.md",
            "Credential Handles\nBackend Policies\nRotation\nRevocation\nAudit Events\nAlpaca Paper MVP\nmacOS Keychain\nAWS Secrets Manager\nno hardcoded API keys\nno hosted TradeAssembly.org credential custody\n",
        );

        let mut report = ReportBuilder::new();
        check_identity_and_credential_contract_docs(&root, &mut report);

        assert!(report.finish().ok);
    }

    #[test]
    fn requires_future_state_canonical_doc_terms() {
        let root = temp_repo("future-state-docs");
        write(
            &root,
            "docs/architecture.md",
            "Trust Fabric\nWarden Core is a standalone\nFinance Profile\nSightline\nsigned receipt\ncompliance-support\n",
        );
        write(
            &root,
            "docs/decisions.md",
            "Warden Core Is Standalone\nFinance Profile\nSightline\ncompliance claims\ninterface plus plugin/adapter\n",
        );
        write(
            &root,
            "docs/sdlc.md",
            "Phase 10\nAPF\nSightline\ncompliance-claim\nrelease evidence\n",
        );
        write(
            &root,
            "docs/rust-rewrite-harness.md",
            "APF\nSightline\nlive/cloud\ncompliance-claim\nAlpaca paper smoke\n",
        );
        write(
            &root,
            "docs/repo-boundaries.md",
            "TradeAssemblyHQ/warden\nWarden Platform\nTradeAssemblyHQ/sightline\nsigned-receipt references\n",
        );
        write(
            &root,
            "docs/reference/current-product-capabilities.md",
            "Current V1 Coverage\nBacktesting V1\nContinuous Execution V1\nSightline\nBlocked Until\n",
        );
        write(
            &root,
            "docs/reference/current-system-design-and-data-model.md",
            "Current V1 Coverage\nWarden\nSightline\nlive/cloud\nservice/execution.rs\n",
        );

        let mut report = ReportBuilder::new();
        check_future_state_canonical_docs(&root, &mut report);

        assert!(report.finish().ok);
    }

    #[test]
    fn requires_distributed_deployment_contract_terms() {
        let root = temp_repo("deployment-contract");
        write(
            &root,
            "docker-compose.distributed.yml",
            "postgres:\nnats:\napi:\nscheduler:\nrunner:\noms:\norder-status:\nTRADEASSEMBLY_RUNTIME_PROFILE: ${TRADEASSEMBLY_RUNTIME_PROFILE:-self_hosted}\nTRADEASSEMBLY_POSTGRES_URL_REF: env://TRADEASSEMBLY_POSTGRES_URL\nTRADEASSEMBLY_POSTGRES_URL: postgresql://tradeassembly@postgres:5432/tradeassembly\nTRADEASSEMBLY_NATS_URL_REF: env://TRADEASSEMBLY_NATS_URL\nTRADEASSEMBLY_NATS_URL: nats://nats:4222\ntradeassembly-worker-loop\nhttp://127.0.0.1:8090/ready\n[\"CMD\", \"curl\", \"--fail\"]\ncommand: [\"tradeassembly\", \"seed-btc-demo\"]\n",
        );
        write(
            &root,
            "runtime/Dockerfile",
            "FROM rust:1-bookworm AS build\nRUN --mount=type=cache,target=/app/target cargo build --release --bin tradeassembly && cp /app/target/release/tradeassembly /tmp/tradeassembly\nRUN apt-get install ca-certificates curl\nCOPY --from=build /tmp/tradeassembly /usr/local/bin/tradeassembly\nCOPY --from=build /app/deploy/worker-loop.sh /usr/local/bin/tradeassembly-worker-loop\nCMD [\"tradeassembly\", \"api\", \"--host\", \"0.0.0.0\"]\n",
        );
        write(
            &root,
            "deploy/production.env.example",
            "TRADEASSEMBLY_RUNTIME_PROFILE=self_hosted\nTRADEASSEMBLY_POSTGRES_URL_REF=env://TRADEASSEMBLY_POSTGRES_URL\nTRADEASSEMBLY_NATS_URL_REF=env://TRADEASSEMBLY_NATS_URL\nTRADEASSEMBLY_CREDENTIAL_BACKEND=env\nTRADEASSEMBLY_ENABLE_LIVE_TRADING=0\n",
        );
        write(
            &root,
            "deploy/worker-loop.sh",
            "scheduler)\nrunner)\noms)\nstatus)\ntradeassembly scheduler run\ntradeassembly workers run\ntradeassembly orders reconcile\ntradeassembly orders attempts\n",
        );
        write(
            &root,
            ".dockerignore",
            ".env\n.env.*\n.tradeassembly\ntarget\nruntime/.venv-research\nwebapp/node_modules\n",
        );
        write(
            &root,
            "infra/ansible/templates/docker-compose.generated.yml.j2",
            "[\"CMD\", \"curl\", \"--fail\"]\ncommand: [\"tradeassembly\", \"seed-btc-demo\"]\n",
        );
        write(
            &root,
            "infra/scripts/iac-doctor.sh",
            "TRADEASSEMBLY_NATS_PORT\n",
        );
        write(&root, "Makefile", "Postgres/NATS/API/workers\n");
        for rel in [
            "deploy/README.md",
            "render.yaml",
            "infra/render/production.env.example",
        ] {
            write(&root, rel, "safe\n");
        }

        let mut report = ReportBuilder::new();
        check_deployment_artifact_contract(&root, &mut report);

        assert!(report.finish().ok);
    }

    #[test]
    fn requires_render_blueprint_contract() {
        let root = temp_repo("render-contract");
        write(
            &root,
            "render.yaml",
            r#"
databases:
  - name: tradeassembly-postgres
services:
  - name: tradeassembly-api
    dockerCommand: tradeassembly api
    envVars: &runtime-env
      - key: TRADEASSEMBLY_RUNTIME_PROFILE
        value: self_hosted
      - key: TRADEASSEMBLY_AUTH_ISSUER
        value: https://identity.example.test
      - key: TRADEASSEMBLY_AUTH_AUDIENCE
        value: tradeassembly
      - key: TRADEASSEMBLY_AUTH_CLIENT_ID
        value: tradeassembly
      - key: TRADEASSEMBLY_POSTGRES_URL_REF
        value: env://TRADEASSEMBLY_POSTGRES_URL
      - key: TRADEASSEMBLY_POSTGRES_URL
        fromDatabase:
          name: tradeassembly-postgres
          property: connectionString
      - key: TRADEASSEMBLY_NATS_URL_REF
        value: env://TRADEASSEMBLY_NATS_URL
      - key: TRADEASSEMBLY_NATS_URL
        sync: false
      - key: TRADEASSEMBLY_OBJECT_STORE_ENDPOINT
        value: file:///var/lib/tradeassembly/artifacts
      - key: TRADEASSEMBLY_CREDENTIAL_BACKEND
        value: env
      - key: TRADEASSEMBLY_ENABLE_LIVE_TRADING
        value: "0"
  - name: tradeassembly-scheduler
    dockerCommand: tradeassembly scheduler
    envVars: *runtime-env
  - name: tradeassembly-runner
    dockerCommand: tradeassembly runner
    envVars: *runtime-env
  - name: tradeassembly-oms
    dockerCommand: tradeassembly oms
    envVars: *runtime-env
  - name: tradeassembly-order-status
    dockerCommand: tradeassembly order-status
    envVars: *runtime-env
  - name: tradeassembly-studio
    buildCommand: npm run build
    startCommand: npm run start
"#,
        );
        write(&root, "infra/render/README.md", "Render docs\n");
        write(
            &root,
            "infra/render/production.env.example",
            "TRADEASSEMBLY_RUNTIME_PROFILE=self_hosted\n",
        );
        write(
            &root,
            "Justfile",
            "iac-render-validate:\n    cargo xtask contract render-iac\n",
        );

        let mut report = ReportBuilder::new();
        check_render_blueprint_contract(&root, &mut report);

        assert!(report.finish().ok);
    }

    #[test]
    fn requires_cloud_iac_provider_contract() {
        let root = temp_repo("cloud-iac-contract");
        for provider in ["aws", "gcp", "azure"] {
            write(
                &root,
                &format!("infra/tofu/{provider}/provider.tf"),
                "provider\n",
            );
            write(
                &root,
                &format!("infra/tofu/{provider}/variables.tf"),
                "variable \"profile\" {\n  default = \"starter\"\n}\nvariable \"tradeassembly_db_url\" {}\nvariable \"tradeassembly_nats_url\" {}\nvariable \"tradeassembly_credential_backend\" {\n  default = \"env\"\n}\n",
            );
            write(
                &root,
                &format!("infra/tofu/{provider}/outputs.tf"),
                "output \"ansible_host\" {}\noutput \"ansible_user\" {}\noutput \"tradeassembly_runtime_profile\" {}\noutput \"tradeassembly_db_url\" {}\noutput \"tradeassembly_nats_url\" {}\noutput \"ansible_inventory\" {\n  value = yamlencode({ tradeassembly_backend = \"container\" tradeassembly_credential_backend = var.tradeassembly_credential_backend tradeassembly_runtime_profile = local.runtime_profile tradeassembly_provision_postgres = true tradeassembly_provision_nats = true })\n}\n",
            );
            write(&root, &format!("infra/tofu/{provider}/README.md"), "docs\n");
        }
        write(
            &root,
            "infra/tofu/aws/main.tf",
            "data \"aws_ssm_parameter\" \"amazon_linux_2023_ami\"\n/aws/service/ami-amazon-linux-latest/al2023-ami-kernel-default-x86_64\nvar.ami_id != \"\"\naws_instance\naws_ebs_volume\ntradeassembly_nats_url\nruntime_profile = \"self_hosted\"\nnats://nats:4222\npostgresql://tradeassembly@postgres:5432/tradeassembly\npublic_api_ingress\npublic_studio_ingress\n",
        );
        write(
            &root,
            "infra/tofu/gcp/main.tf",
            "google_compute_instance\ngoogle_compute_disk\ntradeassembly_nats_url\nruntime_profile = \"self_hosted\"\nnats://nats:4222\npostgresql://tradeassembly@postgres:5432/tradeassembly\npublic_api_ingress\npublic_studio_ingress\n",
        );
        write(
            &root,
            "infra/tofu/azure/main.tf",
            "azurerm_linux_virtual_machine\nazurerm_managed_disk\ntradeassembly_nats_url\nruntime_profile = \"self_hosted\"\nnats://nats:4222\npostgresql://tradeassembly@postgres:5432/tradeassembly\npublic_api_ingress\npublic_studio_ingress\n",
        );
        for rel in [
            "infra/tofu/README.md",
            "infra/tofu/versions.tf",
            "infra/tofu/scripts/bootstrap-tofu.sh",
            "infra/tofu/scripts/validate-provider.sh",
            "infra/tofu/scripts/render-ansible-inventory.sh",
            "infra/tofu/examples/aws.tfvars.example",
            "infra/tofu/examples/gcp.tfvars.example",
            "infra/tofu/examples/azure.tfvars.example",
        ] {
            write(&root, rel, "safe\n");
        }
        write(
            &root,
            "Justfile",
            "verify:\n    cargo xtask verify\ncheck: iac-prod-audit\niac-cloud-bootstrap:\niac-cloud-validate:\niac-cloud-plan:\niac-cloud-inventory:\niac-cloud-apply:\n    CONFIRM_CLOUD_APPLY=1 infra/tofu/scripts/validate-provider.sh\niac-cloud-destroy:\n    CONFIRM_DESTROY_TRADEASSEMBLY_CLOUD=1 true\niac-prod-audit:\n    cargo xtask contract cloud-iac\n",
        );

        let mut report = ReportBuilder::new();
        check_cloud_iac_contract(&root, &mut report);

        assert!(report.finish().ok);
    }

    #[test]
    fn accepts_rust_first_check_ci_wrapper_for_cloud_iac_contract() {
        let root = temp_repo("cloud-iac-rust-first-wrapper");
        for provider in ["aws", "gcp", "azure"] {
            write(
                &root,
                &format!("infra/tofu/{provider}/provider.tf"),
                "provider\n",
            );
            write(
                &root,
                &format!("infra/tofu/{provider}/variables.tf"),
                "variable \"profile\" {\n  default = \"starter\"\n}\nvariable \"tradeassembly_db_url\" {}\nvariable \"tradeassembly_nats_url\" {}\nvariable \"tradeassembly_credential_backend\" {\n  default = \"env\"\n}\n",
            );
            write(
                &root,
                &format!("infra/tofu/{provider}/outputs.tf"),
                "output \"ansible_host\" {}\noutput \"ansible_user\" {}\noutput \"tradeassembly_runtime_profile\" {}\noutput \"tradeassembly_db_url\" {}\noutput \"tradeassembly_nats_url\" {}\noutput \"ansible_inventory\" {\n  value = yamlencode({ tradeassembly_backend = \"container\" tradeassembly_credential_backend = var.tradeassembly_credential_backend tradeassembly_runtime_profile = local.runtime_profile tradeassembly_provision_postgres = true tradeassembly_provision_nats = true })\n}\n",
            );
            write(&root, &format!("infra/tofu/{provider}/README.md"), "docs\n");
        }
        write(
            &root,
            "infra/tofu/aws/main.tf",
            "data \"aws_ssm_parameter\" \"amazon_linux_2023_ami\"\n/aws/service/ami-amazon-linux-latest/al2023-ami-kernel-default-x86_64\nvar.ami_id != \"\"\naws_instance\naws_ebs_volume\ntradeassembly_nats_url\nruntime_profile = \"self_hosted\"\nnats://nats:4222\npostgresql://tradeassembly@postgres:5432/tradeassembly\npublic_api_ingress\npublic_studio_ingress\n",
        );
        write(
            &root,
            "infra/tofu/gcp/main.tf",
            "google_compute_instance\ngoogle_compute_disk\ntradeassembly_nats_url\nruntime_profile = \"self_hosted\"\nnats://nats:4222\npostgresql://tradeassembly@postgres:5432/tradeassembly\npublic_api_ingress\npublic_studio_ingress\n",
        );
        write(
            &root,
            "infra/tofu/azure/main.tf",
            "azurerm_linux_virtual_machine\nazurerm_managed_disk\ntradeassembly_nats_url\nruntime_profile = \"self_hosted\"\nnats://nats:4222\npostgresql://tradeassembly@postgres:5432/tradeassembly\npublic_api_ingress\npublic_studio_ingress\n",
        );
        for rel in [
            "infra/tofu/README.md",
            "infra/tofu/versions.tf",
            "infra/tofu/scripts/bootstrap-tofu.sh",
            "infra/tofu/scripts/validate-provider.sh",
            "infra/tofu/scripts/render-ansible-inventory.sh",
            "infra/tofu/examples/aws.tfvars.example",
            "infra/tofu/examples/gcp.tfvars.example",
            "infra/tofu/examples/azure.tfvars.example",
        ] {
            write(&root, rel, "safe\n");
        }
        write(
            &root,
            "Justfile",
            "verify:\n    cargo xtask verify\niac-cloud-bootstrap:\niac-cloud-validate:\niac-cloud-plan:\niac-cloud-inventory:\niac-cloud-apply:\n    CONFIRM_CLOUD_APPLY=1 infra/tofu/scripts/validate-provider.sh\niac-cloud-destroy:\n    CONFIRM_DESTROY_TRADEASSEMBLY_CLOUD=1\niac-prod-audit:\n",
        );

        let mut report = ReportBuilder::new();
        check_cloud_iac_contract(&root, &mut report);

        assert!(report.finish().ok);
    }

    #[test]
    fn rejects_make_reintroduction_and_stale_active_invocations() {
        let root = temp_repo("tooling-contract");
        write(&root, "Makefile", "api:\n");
        write(&root, "Justfile", "verify:\n    cargo xtask verify\n");
        write(&root, "docs/reference/tooling-command-migration.md", "ok\n");
        write(
            &root,
            "README.md",
            "Run `make api`.\n`make` targets remain as compatibility wrappers.\n",
        );

        let mut report = ReportBuilder::new();
        check_tooling_recipes(&root, &mut report);
        let report = report.finish();

        assert!(
            report
                .failures
                .iter()
                .any(|message| message.contains("Makefile must be deleted")),
            "{report:?}"
        );
        assert!(
            report.failures.iter().any(|message| message
                .contains("active command surface README.md retains Make invocation")),
            "{report:?}"
        );
        assert!(
            report
                .failures
                .iter()
                .any(|message| message.contains("`make` targets remain")),
            "{report:?}"
        );
    }

    #[test]
    fn rejects_incomplete_tooling_migration_inventory() {
        let migration = "c9dc9c3f53d625acdac087b8285d5d4c3f213b30bc32541cf05aa1fc05ac375e\n\
| `setup` | `just missing-recipe` |\n";
        let mut report = ReportBuilder::new();

        check_tooling_migration_inventory(migration, &BTreeSet::new(), &mut report);
        let report = report.finish();

        assert!(!report.ok, "{report:?}");
        assert!(
            report
                .failures
                .iter()
                .any(|failure| failure.contains("maps to unknown recipe missing-recipe")),
            "{report:?}"
        );
        assert!(
            report
                .failures
                .iter()
                .any(|failure| failure.contains("must contain 119 unique targets")),
            "{report:?}"
        );
    }

    #[test]
    fn rejects_equal_count_tooling_target_substitution() {
        let migration = include_str!("../../docs/reference/tooling-command-migration.md").replacen(
            "| `setup` |",
            "| `bogus-target` |",
            1,
        );
        let recipes = Regex::new(r"`just ([A-Za-z0-9_-]+)(?: [^`]*)?`")
            .expect("valid Just command regex")
            .captures_iter(&migration)
            .filter_map(|capture| capture.get(1).map(|value| value.as_str().to_string()))
            .collect::<BTreeSet<_>>();
        let mut report = ReportBuilder::new();

        check_tooling_migration_inventory(&migration, &recipes, &mut report);
        let report = report.finish();

        assert!(!report.ok, "{report:?}");
        assert!(
            report.failures.iter().any(|failure| failure.contains(
                "tooling migration target set drifted: missing=[setup] unexpected=[bogus-target]"
            )),
            "{report:?}"
        );
    }

    #[test]
    fn rejects_missing_native_iac_compatibility_disclosure() {
        let migration = include_str!("../../docs/reference/tooling-command-migration.md")
            .replace("had no native", "previously had a native");
        let mut report = ReportBuilder::new();

        check_tooling_compatibility_disclosures(&migration, &mut report);
        let report = report.finish();

        assert!(!report.ok, "{report:?}");
        assert!(
            report
                .failures
                .iter()
                .any(|failure| failure.contains("had no native Ansible role")),
            "{report:?}"
        );
    }
}
