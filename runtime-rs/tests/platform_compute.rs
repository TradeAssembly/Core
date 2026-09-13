// Copyright (c) 2026 OptionLab LLC. All rights reserved.

use tradeassembly_runtime::platform_compute::{
    create_compute_adapter, live_cloud_preflight_report, resolve_adapter_repository_refs,
    AdapterRepositoryRef, DeploymentProfile, RuntimeProfileName, DEFAULT_FOSS_DEPLOYMENT,
    REQUIRED_RUNTIME_ENV,
};

#[test]
fn default_foss_deployment_uses_nats_jetstream_and_stateless_standard_compute() {
    let adapter = create_compute_adapter(Some("container")).expect("adapter");
    let plan = adapter.plan(&DEFAULT_FOSS_DEPLOYMENT).expect("plan");

    assert_eq!(DEFAULT_FOSS_DEPLOYMENT.database_provider, "postgres");
    assert_eq!(DEFAULT_FOSS_DEPLOYMENT.event_provider, "nats-jetstream");
    assert_eq!(DEFAULT_FOSS_DEPLOYMENT.compute_provider, "container");
    assert!(DEFAULT_FOSS_DEPLOYMENT.requires_external_state());
    assert!(plan.stateless());
    assert_eq!(plan.roles().len(), 4);
    assert!(!plan
        .worker_specs
        .iter()
        .any(|worker| worker.command.iter().any(|arg| arg == "stream")));
    assert!(plan
        .worker_specs
        .iter()
        .all(|worker| worker.env_required == REQUIRED_RUNTIME_ENV));
}

#[test]
fn live_cloud_preflight_requires_external_state_and_stateless_workers() {
    let local_report = live_cloud_preflight_report(&DEFAULT_FOSS_DEPLOYMENT);
    assert_eq!(local_report["ready"], false);
    assert!(local_report["blockedReasons"]
        .as_array()
        .unwrap()
        .iter()
        .any(|reason| reason.as_str() == Some("external_object_store")));
    assert!(local_report["blockedReasons"]
        .as_array()
        .unwrap()
        .iter()
        .any(|reason| reason.as_str() == Some("external_credential_backend")));

    let production = DeploymentProfile {
        name: "self-hosted-production".to_string(),
        runtime_profile: RuntimeProfileName::Production,
        database_provider: "postgres".to_string(),
        event_provider: "nats-jetstream".to_string(),
        compute_provider: "container".to_string(),
        object_store_provider: "s3-compatible".to_string(),
        credential_backend: "vault".to_string(),
        adapter_repositories: Vec::new(),
    };
    let production_report = live_cloud_preflight_report(&production);
    assert_eq!(production_report["ready"], true);
    assert!(production_report["checks"]
        .as_array()
        .unwrap()
        .iter()
        .all(|check| check["status"] == "pass"));

    let hosted = DeploymentProfile {
        name: "hosted-only".to_string(),
        runtime_profile: RuntimeProfileName::Production,
        database_provider: "aurora-serverless".to_string(),
        event_provider: "sqs".to_string(),
        compute_provider: "lambda".to_string(),
        object_store_provider: "s3".to_string(),
        credential_backend: "aws-secrets-manager".to_string(),
        adapter_repositories: Vec::new(),
    };
    let hosted_report = live_cloud_preflight_report(&hosted);
    assert_eq!(hosted_report["ready"], false);
    assert!(hosted_report["blockedReasons"]
        .as_array()
        .unwrap()
        .iter()
        .any(|reason| reason.as_str() == Some("public_core_adapter_boundary")));
}

#[test]
fn kubernetes_compatible_compute_plan_renders_worker_deployments() {
    let adapter = create_compute_adapter(Some("k8s")).expect("adapter");
    let workloads = adapter
        .kubernetes_workloads(&DEFAULT_FOSS_DEPLOYMENT)
        .expect("workloads");

    let names = workloads
        .iter()
        .map(|workload| workload["metadata"]["name"].as_str().expect("name"))
        .collect::<Vec<_>>();
    assert!(names.contains(&"tradeassembly-runner"));
    assert!(names.contains(&"tradeassembly-oms"));
    assert!(workloads
        .iter()
        .all(|workload| workload["kind"] == "Deployment"));
}

#[test]
fn adapter_repository_refs_accept_github_slugs_and_reject_urls_or_paths() {
    let reference = AdapterRepositoryRef::new("compute", "acme/tradeassembly-k8s", Some("v1.2.3"))
        .expect("valid ref");
    assert_eq!(reference.kind, "compute");
    assert_eq!(reference.slug, "acme/tradeassembly-k8s");
    assert_eq!(reference.reference, "v1.2.3");

    for slug in [
        "https://github.com/acme/repo",
        "../repo",
        "acme/repo/path",
        "missing-owner",
    ] {
        assert!(AdapterRepositoryRef::new("compute", slug, None).is_err());
    }
}

#[test]
fn adapter_repo_resolver_accepts_required_port_kinds_and_mapping_shapes() {
    let refs = resolve_adapter_repository_refs(serde_json::json!({
        "event_stream": "acme/tradeassembly-kafka@v1",
        "command_queue": {"slug": "acme/tradeassembly-command-queue", "ref": "main", "package": "queue"},
        "scheduler-trigger": {"repository": "acme/tradeassembly-scheduler", "version": "v2"},
        "database": "acme/tradeassembly-postgres",
        "compute": "acme/tradeassembly-container",
        "object_store": "acme/tradeassembly-object-store",
        "credential": "acme/tradeassembly-vault",
        "iac": "acme/tradeassembly-iac"
    }))
    .expect("refs");

    let kinds = refs
        .iter()
        .map(|reference| reference.kind.as_str())
        .collect::<Vec<_>>();
    assert!(kinds.contains(&"event-stream"));
    assert!(kinds.contains(&"command-queue"));
    assert!(kinds.contains(&"scheduler-trigger"));
    let event = refs
        .iter()
        .find(|reference| reference.kind == "event-stream")
        .unwrap();
    assert_eq!(event.slug, "acme/tradeassembly-kafka");
    assert_eq!(event.reference, "v1");
}

#[test]
fn public_core_rejects_hosted_only_compute_and_event_profiles() {
    assert!(create_compute_adapter(Some("fargate")).is_err());

    let hosted = DeploymentProfile {
        name: "hosted-lean".to_string(),
        runtime_profile: RuntimeProfileName::Production,
        database_provider: "postgres".to_string(),
        event_provider: "sqs".to_string(),
        compute_provider: "fargate".to_string(),
        object_store_provider: "s3".to_string(),
        credential_backend: "aws-secrets-manager".to_string(),
        adapter_repositories: Vec::new(),
    };

    assert!(hosted.validate_public_core().is_err());
}

#[test]
fn production_profile_requires_external_artifact_and_credential_adapters() {
    let local_state = DeploymentProfile {
        name: "serverless-local-state".to_string(),
        runtime_profile: RuntimeProfileName::Production,
        database_provider: "postgres".to_string(),
        event_provider: "nats-jetstream".to_string(),
        compute_provider: "container".to_string(),
        object_store_provider: "filesystem".to_string(),
        credential_backend: "local-file".to_string(),
        adapter_repositories: Vec::new(),
    };

    let error = local_state
        .validate_public_core()
        .expect_err("local state rejected");
    assert_eq!(error.code, "configuration_error");
    assert_eq!(
        error.details["object_store_provider"],
        serde_json::json!("filesystem")
    );

    let external_artifacts = DeploymentProfile {
        object_store_provider: "s3-compatible".to_string(),
        ..local_state
    };
    let credential_error = external_artifacts
        .validate_public_core()
        .expect_err("local credentials rejected");
    assert_eq!(
        credential_error.details["credential_backend"],
        serde_json::json!("local-file")
    );
}

#[test]
fn kinesis_style_events_fail_closed_without_an_external_adapter_repository() {
    let kinesis = DeploymentProfile {
        name: "serverless-kinesis".to_string(),
        runtime_profile: RuntimeProfileName::Production,
        database_provider: "postgres".to_string(),
        event_provider: "kinesis".to_string(),
        compute_provider: "container".to_string(),
        object_store_provider: "s3-compatible".to_string(),
        credential_backend: "vault".to_string(),
        adapter_repositories: Vec::new(),
    };

    let error = kinesis
        .validate_public_core()
        .expect_err("kinesis needs adapter");
    assert_eq!(error.code, "configuration_error");
    assert_eq!(
        error.details["event_provider"],
        serde_json::json!("kinesis")
    );
}

#[test]
fn kafka_requires_an_explicit_external_event_adapter() {
    let kafka = DeploymentProfile {
        name: "external-kafka".to_string(),
        runtime_profile: RuntimeProfileName::Production,
        database_provider: "postgres".to_string(),
        event_provider: "kafka".to_string(),
        compute_provider: "container".to_string(),
        object_store_provider: "s3-compatible".to_string(),
        credential_backend: "vault".to_string(),
        adapter_repositories: Vec::new(),
    };

    let error = kafka
        .validate_public_core()
        .expect_err("kafka needs an explicit adapter");
    assert_eq!(error.code, "configuration_error");
    assert_eq!(error.details["event_provider"], serde_json::json!("kafka"));

    let configured = DeploymentProfile {
        adapter_repositories: vec![AdapterRepositoryRef::new(
            "event-stream",
            "acme/tradeassembly-kafka",
            Some("v1"),
        )
        .expect("event adapter")],
        ..kafka
    };
    configured
        .validate_public_core()
        .expect("explicit external Kafka adapter is accepted");
}

#[test]
fn adapter_config_errors_redact_credential_bearing_urls() {
    let error = AdapterRepositoryRef::new(
        "compute",
        "https://user:SUPERSECRET@example.invalid/repo",
        None,
    )
    .expect_err("credential URL rejected");

    assert_eq!(error.details["slug"], serde_json::json!("***redacted***"));
    assert!(!error.details.to_string().contains("SUPERSECRET"));
}
