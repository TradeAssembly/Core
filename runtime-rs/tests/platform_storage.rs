// Copyright (c) 2026 OptionLab LLC. All rights reserved.

use tradeassembly_runtime::platform_storage::{
    create_olap_storage, create_oltp_storage, RepositoryBundle, RuntimeProfileName,
};

#[test]
fn sqlite_storage_adapter_exposes_context_repositories() {
    let storage = create_oltp_storage(None, RuntimeProfileName::Local).expect("storage");
    let health = storage.health();
    assert!(health.ok);
    assert_eq!(health.adapter, "sqlite");

    let repositories = storage.repositories();
    assert_eq!(
        repositories
            .for_context("journal")
            .expect("journal")
            .context,
        "journal"
    );
    assert_eq!(
        repositories
            .for_context("execution")
            .expect("execution")
            .context,
        "execution"
    );
    assert_ne!(
        repositories.for_context("journal").unwrap().context,
        repositories.for_context("execution").unwrap().context
    );
}

#[test]
fn distributed_storage_rejects_local_path() {
    let error = create_oltp_storage(
        Some(".tradeassembly/local.db"),
        RuntimeProfileName::Production,
    )
    .expect_err("local path rejected");

    assert_eq!(error.code, "configuration_error");
    assert_eq!(error.details["profile"], "production");
}

#[test]
fn storage_config_errors_redact_credential_bearing_urls() {
    let error = create_oltp_storage(
        Some("postgres://user:SUPERSECRET@db.internal/tradeassembly"),
        RuntimeProfileName::Production,
    )
    .expect_err("external profile rejects direct path");

    assert_eq!(error.details["path"], serde_json::json!("***redacted***"));
    assert!(!error.details.to_string().contains("SUPERSECRET"));
}

#[test]
fn olap_storage_exports_rows_and_projections() {
    let mut olap = create_olap_storage();
    olap.append_row("runs", serde_json::json!({"run_id": "run-1"}));
    olap.append_projection("runs", serde_json::json!({"run_id": "run-2"}));

    assert_eq!(olap.export_rows("runs").len(), 2);
    assert_eq!(olap.export_projection("runs").len(), 2);
    assert_eq!(olap.health().adapter, "olap-memory");
}

#[test]
fn repository_bundle_has_named_contexts_without_generic_passthrough() {
    let bundle = RepositoryBundle::new();
    let contexts = bundle.context_names();

    assert_eq!(
        contexts,
        vec![
            "broker",
            "execution",
            "journal",
            "marketdata",
            "plugins",
            "research",
            "risk",
            "strategy",
        ]
    );
    assert!(bundle.for_context("__getattr__").is_err());
}
