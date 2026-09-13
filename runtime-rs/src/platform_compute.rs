// Copyright (c) 2026 OptionLab LLC. All rights reserved.

use crate::bus::ConfigError;
pub use crate::bus::RuntimeProfileName;
use crate::platform_events::WorkerRole;
use serde_json::{json, Value};
use std::sync::LazyLock;

pub const REQUIRED_RUNTIME_ENV: [&str; 11] = [
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
];

const PUBLIC_CORE_WORKER_ROLES: [WorkerRole; 4] = [
    WorkerRole::Scheduler,
    WorkerRole::Runner,
    WorkerRole::Oms,
    WorkerRole::OrderStatus,
];

pub static DEFAULT_FOSS_DEPLOYMENT: LazyLock<DeploymentProfile> =
    LazyLock::new(|| DeploymentProfile {
        name: "foss-default".to_string(),
        runtime_profile: RuntimeProfileName::DistributedDev,
        database_provider: "postgres".to_string(),
        event_provider: "nats-jetstream".to_string(),
        compute_provider: "container".to_string(),
        object_store_provider: "filesystem".to_string(),
        credential_backend: "local-file".to_string(),
        adapter_repositories: Vec::new(),
    });

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AdapterRepositoryRef {
    pub kind: String,
    pub slug: String,
    pub reference: String,
    pub package: Option<String>,
}

impl AdapterRepositoryRef {
    pub fn new(kind: &str, slug: &str, reference: Option<&str>) -> Result<Self, ConfigError> {
        let normalized_kind = normalize_adapter_kind(kind);
        let normalized_slug = slug.trim();
        let normalized_ref = reference.unwrap_or("main").trim();
        if !is_allowed_kind(&normalized_kind) {
            return Err(configuration_error(
                "unsupported deployment adapter kind",
                json!({"kind": kind}),
            ));
        }
        if !valid_github_slug(normalized_slug) {
            return Err(configuration_error(
                "adapter repository must be a GitHub owner/repo slug",
                json!({"slug": slug}),
            ));
        }
        if normalized_ref.is_empty()
            || normalized_ref.starts_with('-')
            || normalized_ref.contains("..")
            || normalized_ref.contains("://")
            || normalized_ref.contains('\\')
        {
            return Err(configuration_error(
                "adapter repository ref must be a branch, tag, or commit-ish value",
                json!({"ref": normalized_ref}),
            ));
        }
        Ok(Self {
            kind: normalized_kind,
            slug: normalized_slug.to_string(),
            reference: normalized_ref.to_string(),
            package: None,
        })
    }

    fn with_package(mut self, package: Option<String>) -> Self {
        self.package = package;
        self
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DeploymentProfile {
    pub name: String,
    pub runtime_profile: RuntimeProfileName,
    pub database_provider: String,
    pub event_provider: String,
    pub compute_provider: String,
    pub object_store_provider: String,
    pub credential_backend: String,
    pub adapter_repositories: Vec<AdapterRepositoryRef>,
}

impl DeploymentProfile {
    pub fn requires_external_state(&self) -> bool {
        self.runtime_profile != RuntimeProfileName::Local
    }

    pub fn repository_for(&self, kind: &str) -> Option<&AdapterRepositoryRef> {
        let normalized = normalize_adapter_kind(kind);
        self.adapter_repositories
            .iter()
            .rev()
            .find(|reference| reference.kind == normalized)
    }

    pub fn validate_public_core(&self) -> Result<(), ConfigError> {
        let forbidden = [
            "aurora-serverless",
            "dynamodb",
            "ecs",
            "fargate",
            "gcs",
            "lambda",
            "s3",
            "sqs",
        ];
        let selected = [
            self.database_provider.to_lowercase(),
            self.event_provider.to_lowercase(),
            self.compute_provider.to_lowercase(),
            self.object_store_provider.to_lowercase(),
        ];
        let blocked = selected
            .iter()
            .filter(|provider| forbidden.contains(&provider.as_str()))
            .cloned()
            .collect::<Vec<_>>();
        if !blocked.is_empty() {
            return Err(configuration_error(
                "hosted-only deployment adapters do not belong in public core",
                json!({"profile": self.name, "forbidden": blocked}),
            ));
        }
        if self.requires_external_state()
            && !is_public_durable_event_provider(&self.event_provider)
            && self.repository_for("event").is_none()
            && self.repository_for("event-stream").is_none()
        {
            return Err(configuration_error(
                "FOSS external profiles require NATS JetStream or an explicit durable event adapter",
                json!({"profile": self.name, "event_provider": self.event_provider}),
            ));
        }
        if self.requires_external_state()
            && self.database_provider.to_lowercase() != "postgres"
            && self.repository_for("database").is_none()
        {
            return Err(configuration_error(
                "FOSS external profiles default to Postgres-compatible OLTP storage",
                json!({"profile": self.name, "database_provider": self.database_provider}),
            ));
        }
        if self.runtime_profile == RuntimeProfileName::Production
            && self.object_store_provider.to_lowercase() == "filesystem"
            && self.repository_for("object-store").is_none()
        {
            return Err(configuration_error(
                "production profiles require an external artifact/object-store adapter",
                json!({"profile": self.name, "object_store_provider": self.object_store_provider}),
            ));
        }
        if self.runtime_profile == RuntimeProfileName::Production
            && self.credential_backend.to_lowercase() == "local-file"
            && self.repository_for("credential").is_none()
        {
            return Err(configuration_error(
                "production profiles require an external credential adapter",
                json!({"profile": self.name, "credential_backend": self.credential_backend}),
            ));
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WorkerSpec {
    pub role: WorkerRole,
    pub command: Vec<String>,
    pub env_required: Vec<&'static str>,
    pub replicas: usize,
    pub stateless: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ComputePlan {
    pub profile: DeploymentProfile,
    pub adapter: String,
    pub worker_specs: Vec<WorkerSpec>,
    pub notes: Vec<String>,
}

impl ComputePlan {
    pub fn stateless(&self) -> bool {
        self.worker_specs.iter().all(|worker| worker.stateless)
    }

    pub fn roles(&self) -> Vec<WorkerRole> {
        self.worker_specs.iter().map(|worker| worker.role).collect()
    }
}

pub fn live_cloud_preflight_report(profile: &DeploymentProfile) -> Value {
    let plan = create_compute_adapter(Some(&profile.compute_provider))
        .and_then(|adapter| adapter.plan(profile));
    let stateless_workers = plan.as_ref().map(|plan| plan.stateless()).unwrap_or(false);
    let public_core_valid = profile.validate_public_core().is_ok();
    let checks = vec![
        profile_check(
            "public_core_adapter_boundary",
            "Public-core adapter boundary",
            public_core_valid,
            "deployment_profile",
            &profile.name,
        ),
        profile_check(
            "external_database",
            "External OLTP storage",
            profile.database_provider.eq_ignore_ascii_case("postgres")
                || profile.repository_for("database").is_some(),
            "database",
            &profile.database_provider,
        ),
        profile_check(
            "durable_event_bus",
            "Durable event bus",
            is_public_durable_event_provider(&profile.event_provider)
                || profile.repository_for("event-stream").is_some()
                || profile.repository_for("event").is_some(),
            "event_bus",
            &profile.event_provider,
        ),
        profile_check(
            "external_object_store",
            "External evidence/object storage",
            !profile
                .object_store_provider
                .eq_ignore_ascii_case("filesystem")
                || profile.repository_for("object-store").is_some(),
            "object_store",
            &profile.object_store_provider,
        ),
        profile_check(
            "external_credential_backend",
            "External credential backend",
            !matches!(
                profile.credential_backend.to_ascii_lowercase().as_str(),
                "local-file" | "env" | "environment"
            ) || profile.repository_for("credential").is_some(),
            "credential_backend",
            &profile.credential_backend,
        ),
        profile_check(
            "stateless_workers",
            "Stateless runtime workers",
            stateless_workers,
            "compute",
            &profile.compute_provider,
        ),
        profile_check(
            "bounded_retries",
            "Bounded retries and dead-letter policy",
            true,
            "events",
            "configured_by_runtime_contract",
        ),
        profile_check(
            "duplicate_safe_writes",
            "Idempotent duplicate-safe writes",
            true,
            "control_plane",
            "idempotency_required",
        ),
    ];
    let blocked = checks
        .iter()
        .filter(|check| check["status"] != "pass")
        .filter_map(|check| check["id"].as_str().map(str::to_string))
        .collect::<Vec<_>>();
    json!({
        "schemaVersion": "tradeassembly.live_cloud_preflight.v1",
        "profile": profile.name,
        "runtimeProfile": runtime_profile_name(profile.runtime_profile),
        "publicCoreOnly": true,
        "ready": blocked.is_empty(),
        "checks": checks,
        "blockedReasons": blocked,
        "notes": [
            "Public core exposes ports and self-hostable adapters; hosted-only workers and hosted credential custody stay outside this repository.",
            "Live activation requires external durable state before it can be represented as continuously executable in serverless or stateless profiles."
        ],
    })
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ComputeAdapter {
    pub name: String,
}

impl ComputeAdapter {
    pub fn plan(&self, profile: &DeploymentProfile) -> Result<ComputePlan, ConfigError> {
        profile.validate_public_core()?;
        let worker_specs = PUBLIC_CORE_WORKER_ROLES
            .iter()
            .map(|role| WorkerSpec {
                role: *role,
                command: vec![
                    "deploy/worker-loop.sh".to_string(),
                    worker_loop_arg(*role).to_string(),
                ],
                env_required: REQUIRED_RUNTIME_ENV.to_vec(),
                replicas: 1,
                stateless: true,
            })
            .collect::<Vec<_>>();
        Ok(ComputePlan {
            profile: profile.clone(),
            adapter: self.name.clone(),
            worker_specs,
            notes: vec![
                "Workers are stateless command processes.".to_string(),
                "Durable state lives in external Postgres, NATS JetStream, object storage, and the configured credential backend.".to_string(),
            ],
        })
    }

    pub fn kubernetes_workloads(
        &self,
        profile: &DeploymentProfile,
    ) -> Result<Vec<Value>, ConfigError> {
        let plan = self.plan(profile)?;
        Ok(plan
            .worker_specs
            .iter()
            .map(|worker| {
                json!({
                    "apiVersion": "apps/v1",
                    "kind": "Deployment",
                    "metadata": {"name": format!("tradeassembly-{}", role_name(worker.role))},
                    "spec": {
                        "replicas": worker.replicas,
                        "template": {
                            "spec": {
                                "containers": [{
                                    "name": "worker",
                                    "image": "tradeassembly-runtime",
                                    "command": worker.command,
                                }]
                            }
                        }
                    }
                })
            })
            .collect())
    }
}

fn is_public_durable_event_provider(provider: &str) -> bool {
    matches!(
        provider.trim().to_ascii_lowercase().as_str(),
        "nats" | "jetstream" | "nats-jetstream"
    )
}

pub fn create_compute_adapter(provider: Option<&str>) -> Result<ComputeAdapter, ConfigError> {
    let selected = provider
        .unwrap_or("kubernetes-compatible")
        .trim()
        .to_lowercase();
    if matches!(
        selected.as_str(),
        "kubernetes-compatible" | "kubernetes" | "k8s"
    ) {
        return Ok(ComputeAdapter {
            name: "kubernetes-compatible".to_string(),
        });
    }
    if matches!(selected.as_str(), "container" | "containers" | "compose") {
        return Ok(ComputeAdapter {
            name: "container".to_string(),
        });
    }
    if matches!(selected.as_str(), "fargate" | "ecs" | "lambda") {
        return Err(configuration_error(
            "hosted-only compute adapters are injected outside the public core",
            json!({"provider": selected}),
        ));
    }
    Err(configuration_error(
        "unsupported compute adapter",
        json!({"provider": selected, "supported": ["kubernetes-compatible", "container"]}),
    ))
}

pub fn resolve_adapter_repository_refs(
    value: Value,
) -> Result<Vec<AdapterRepositoryRef>, ConfigError> {
    let object = value.as_object().ok_or_else(|| {
        configuration_error(
            "adapter repositories must be an object",
            json!({"shape": "object"}),
        )
    })?;
    let mut refs = Vec::new();
    for (kind, raw) in object {
        let normalized_kind = normalize_adapter_kind(kind);
        if let Some(text) = raw.as_str() {
            let (slug, reference) = split_slug_ref(text);
            refs.push(AdapterRepositoryRef::new(
                &normalized_kind,
                slug,
                reference,
            )?);
            continue;
        }
        let Some(map) = raw.as_object() else {
            return Err(configuration_error(
                "adapter repository entry must be string or object",
                json!({"kind": kind}),
            ));
        };
        let slug = map
            .get("slug")
            .or_else(|| map.get("repository"))
            .and_then(Value::as_str)
            .ok_or_else(|| {
                configuration_error("adapter repository missing slug", json!({"kind": kind}))
            })?;
        let reference = map
            .get("ref")
            .or_else(|| map.get("version"))
            .and_then(Value::as_str);
        let package = map
            .get("package")
            .and_then(Value::as_str)
            .map(ToString::to_string);
        refs.push(
            AdapterRepositoryRef::new(&normalized_kind, slug, reference)?.with_package(package),
        );
    }
    Ok(refs)
}

fn split_slug_ref(value: &str) -> (&str, Option<&str>) {
    value
        .split_once('@')
        .map(|(slug, reference)| (slug, Some(reference)))
        .unwrap_or((value, None))
}

fn profile_check(id: &str, label: &str, passed: bool, kind: &str, selected: &str) -> Value {
    json!({
        "id": id,
        "label": label,
        "status": if passed { "pass" } else { "blocked" },
        "blocking": true,
        "adapterKind": kind,
        "selected": selected,
        "evidenceRef": format!("deployment.{id}"),
        "repairAction": if passed { Value::Null } else { json!(format!("configure:{kind}")) },
    })
}

fn runtime_profile_name(profile: RuntimeProfileName) -> &'static str {
    match profile {
        RuntimeProfileName::Local => "local",
        RuntimeProfileName::DistributedDev => "distributed_dev",
        RuntimeProfileName::Production => "production",
    }
}

fn normalize_adapter_kind(kind: &str) -> String {
    match kind.replace('_', "-").as_str() {
        "bus" => "event".to_string(),
        "command-queue" => "command-queue".to_string(),
        "event-stream" => "event-stream".to_string(),
        "object-store" => "object-store".to_string(),
        "scheduler-trigger" => "scheduler-trigger".to_string(),
        "storage" => "database".to_string(),
        other => other.to_string(),
    }
}

fn is_allowed_kind(kind: &str) -> bool {
    matches!(
        kind,
        "command-queue"
            | "compute"
            | "credential"
            | "database"
            | "event"
            | "event-stream"
            | "iac"
            | "object-store"
            | "scheduler-trigger"
    )
}

fn valid_github_slug(slug: &str) -> bool {
    let Some((owner, repo)) = slug.split_once('/') else {
        return false;
    };
    !owner.is_empty()
        && !repo.is_empty()
        && !repo.contains('/')
        && !slug.contains("://")
        && !slug.contains("..")
        && slug
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '.' | '-' | '/'))
}

fn worker_loop_arg(role: WorkerRole) -> &'static str {
    match role {
        WorkerRole::Scheduler => "scheduler",
        WorkerRole::Runner => "runner",
        WorkerRole::Oms => "oms",
        WorkerRole::OrderStatus => "status",
        WorkerRole::OrderStream => "stream",
        WorkerRole::OutboxPublisher => "outbox",
    }
}

fn role_name(role: WorkerRole) -> &'static str {
    match role {
        WorkerRole::Scheduler => "scheduler",
        WorkerRole::Runner => "runner",
        WorkerRole::Oms => "oms",
        WorkerRole::OrderStatus => "order-status",
        WorkerRole::OrderStream => "order-stream",
        WorkerRole::OutboxPublisher => "outbox-publisher",
    }
}

fn configuration_error(message: impl Into<String>, details: Value) -> ConfigError {
    ConfigError {
        code: "configuration_error".to_string(),
        message: message.into(),
        details: redact_sensitive_details(details),
    }
}

fn redact_sensitive_details(value: Value) -> Value {
    match value {
        Value::String(text) => Value::String(redact_sensitive_string(&text)),
        Value::Array(items) => {
            Value::Array(items.into_iter().map(redact_sensitive_details).collect())
        }
        Value::Object(map) => Value::Object(
            map.into_iter()
                .map(|(key, value)| (key, redact_sensitive_details(value)))
                .collect(),
        ),
        other => other,
    }
}

fn redact_sensitive_string(value: &str) -> String {
    let lower = value.to_ascii_lowercase();
    if (lower.contains("://") && lower.contains('@'))
        || lower.contains("secret")
        || lower.contains("token")
        || lower.contains("password")
    {
        "***redacted***".to_string()
    } else {
        value.to_string()
    }
}
