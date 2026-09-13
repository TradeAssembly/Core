// Copyright (c) 2026 OptionLab LLC. All rights reserved.

//! Durable, local-only authorization records for an explicitly owner-selected
//! Live deployment.  This is an authority record, not a legal receipt or
//! caller-authentication mechanism.

use crate::ports::{ImmutablePutOutcome, SideEffectContext, StoragePort};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::fmt;

const GRANTS: &str = "local_live_mandates";
const REQUESTS: &str = "local_live_mandate_requests";
const REVOCATIONS: &str = "local_live_mandate_revocations";
const SCHEMA: u8 = 1;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct VerifiedLiveActor {
    pub actor_kind: String,
    pub issuer: String,
    pub subject: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LocalLiveMandateBinding {
    pub installation_owner_id: String,
    pub owner_issuer: String,
    pub owner_subject: String,
    pub execution_config_id: String,
    pub execution_config_digest: String,
    pub strategy_version: String,
    pub strategy_hash: String,
    pub account_ref: String,
    pub live_mode: bool,
    pub plugin_instance_ref: String,
    pub plugin_ref: String,
    pub capability_graph_revision: String,
    pub capability_graph_fingerprint: String,
    pub credential_generation: String,
    pub credential_health_binding_ref: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LocalLiveMandateRequest {
    pub binding: LocalLiveMandateBinding,
    pub issued_at_ms: i64,
    pub expires_at_ms: i64,
    pub delegate: Option<VerifiedLiveActor>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LocalLiveMandate {
    pub schema: u8,
    pub mandate_id: String,
    pub binding: LocalLiveMandateBinding,
    pub issued_at_ms: i64,
    pub expires_at_ms: i64,
    pub delegate: Option<VerifiedLiveActor>,
    pub request_idempotency_key: String,
    pub digest: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Revocation {
    schema: u8,
    mandate_id: String,
    revoked_at_ms: i64,
    request_idempotency_key: String,
    actor: VerifiedLiveActor,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LocalLiveMandateError {
    Invalid(String),
    NotOwner,
    AgentNotAllowed,
    Conflict,
    Missing,
    Expired,
    NotYetValid,
    Revoked,
    ActorNotAllowed,
    Storage(String),
    Corrupt,
}

impl fmt::Display for LocalLiveMandateError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Invalid(s) => write!(f, "invalid local Live mandate: {s}"),
            Self::NotOwner => f.write_str("local Live mandate owner required"),
            Self::AgentNotAllowed => f.write_str("agent actor is not allowed to manage a mandate"),
            Self::Conflict => {
                f.write_str("local Live mandate request conflicts with an existing request")
            }
            Self::Missing => f.write_str("local Live mandate is missing"),
            Self::Expired => f.write_str("local Live mandate is expired"),
            Self::NotYetValid => f.write_str("local Live mandate is not yet valid"),
            Self::Revoked => f.write_str("local Live mandate is revoked"),
            Self::ActorNotAllowed => {
                f.write_str("actor is not authorized by the local Live mandate")
            }
            Self::Storage(_) => f.write_str("local Live mandate storage failure"),
            Self::Corrupt => f.write_str("local Live mandate storage is corrupt"),
        }
    }
}

impl std::error::Error for LocalLiveMandateError {}

fn json<T: Serialize>(value: &T) -> Result<Value, LocalLiveMandateError> {
    serde_json::to_value(value).map_err(|_| LocalLiveMandateError::Corrupt)
}

fn digest<T: Serialize>(value: &T) -> Result<String, LocalLiveMandateError> {
    let bytes =
        serde_json_canonicalizer::to_vec(value).map_err(|_| LocalLiveMandateError::Corrupt)?;
    Ok(format!("sha256:{:x}", Sha256::digest(bytes)))
}

fn read<T: for<'de> Deserialize<'de>>(
    storage: &dyn StoragePort,
    namespace: &str,
    key: &str,
) -> Result<T, LocalLiveMandateError> {
    let value = storage
        .get_json(namespace, key)
        .map_err(LocalLiveMandateError::Storage)?
        .ok_or(LocalLiveMandateError::Missing)?;
    serde_json::from_value(value).map_err(|_| LocalLiveMandateError::Corrupt)
}

fn owner(
    actor: &VerifiedLiveActor,
    binding: &LocalLiveMandateBinding,
) -> Result<(), LocalLiveMandateError> {
    if actor.actor_kind != "user" {
        return Err(LocalLiveMandateError::AgentNotAllowed);
    }
    if actor.issuer != binding.owner_issuer || actor.subject != binding.owner_subject {
        return Err(LocalLiveMandateError::NotOwner);
    }
    Ok(())
}

fn validate_request(
    request: &LocalLiveMandateRequest,
    now_ms: i64,
) -> Result<(), LocalLiveMandateError> {
    let b = &request.binding;
    let fields = [
        (&b.installation_owner_id, "installation owner"),
        (&b.owner_issuer, "owner issuer"),
        (&b.owner_subject, "owner subject"),
        (&b.execution_config_id, "execution config"),
        (&b.execution_config_digest, "execution config digest"),
        (&b.strategy_version, "strategy version"),
        (&b.strategy_hash, "strategy hash"),
        (&b.account_ref, "account ref"),
        (&b.plugin_instance_ref, "plugin instance"),
        (&b.plugin_ref, "plugin ref"),
        (&b.capability_graph_revision, "capability revision"),
        (&b.capability_graph_fingerprint, "capability fingerprint"),
        (&b.credential_generation, "credential generation"),
        (
            &b.credential_health_binding_ref,
            "credential health binding",
        ),
    ];
    if fields.iter().any(|(v, _)| v.trim().is_empty()) {
        return Err(LocalLiveMandateError::Invalid(
            "binding fields are required".into(),
        ));
    }
    if !b.live_mode {
        return Err(LocalLiveMandateError::Invalid(
            "mandate binding must be Live".into(),
        ));
    }
    if request.issued_at_ms > now_ms {
        return Err(LocalLiveMandateError::NotYetValid);
    }
    if request.expires_at_ms <= request.issued_at_ms {
        return Err(LocalLiveMandateError::Invalid(
            "expiry must be after issued_at".into(),
        ));
    }
    if let Some(delegate) = &request.delegate {
        if !matches!(delegate.actor_kind.as_str(), "user" | "agent" | "runner")
            || delegate.issuer.trim().is_empty()
            || delegate.subject.trim().is_empty()
        {
            return Err(LocalLiveMandateError::Invalid(
                "delegate must be a named user, agent or runner identity".into(),
            ));
        }
        if delegate.issuer == b.owner_issuer && delegate.subject == b.owner_subject {
            return Err(LocalLiveMandateError::Invalid(
                "delegate must differ from owner".into(),
            ));
        }
    }
    Ok(())
}

/// Issue or replay an immutable owner-authorized mandate. `actor` is already
/// verified by the caller; this module only checks the supplied identity.
pub fn issue_local_live_mandate(
    storage: &dyn StoragePort,
    context: &SideEffectContext,
    request: LocalLiveMandateRequest,
    actor: &VerifiedLiveActor,
    now_ms: i64,
) -> Result<LocalLiveMandate, LocalLiveMandateError> {
    validate_request(&request, now_ms)?;
    owner(actor, &request.binding)?;
    let request_id = context.idempotency_key.as_str();
    let mandate_id =
        digest(&(request.binding.clone(), request_id)).map(|d| format!("mandate:{d}"))?;
    let mandate = LocalLiveMandate {
        schema: SCHEMA,
        mandate_id: mandate_id.clone(),
        binding: request.binding,
        issued_at_ms: request.issued_at_ms,
        expires_at_ms: request.expires_at_ms,
        delegate: request.delegate,
        request_idempotency_key: request_id.to_string(),
        digest: String::new(),
    };
    let mandate = LocalLiveMandate {
        digest: digest(&mandate)?,
        ..mandate
    };
    let index = json(&serde_json::json!({"mandateId": mandate_id}))?;
    if let Some(existing) = storage
        .get_json(REQUESTS, request_id)
        .map_err(LocalLiveMandateError::Storage)?
    {
        if existing != index {
            return Err(LocalLiveMandateError::Conflict);
        }
    } else {
        let outcome = storage
            .put_json_if_absent(REQUESTS, request_id, index, context)
            .map_err(LocalLiveMandateError::Storage)?;
        if matches!(outcome, ImmutablePutOutcome::AlreadyPresent) {
            let stored = storage
                .get_json(REQUESTS, request_id)
                .map_err(LocalLiveMandateError::Storage)?
                .ok_or(LocalLiveMandateError::Corrupt)?;
            if stored != serde_json::json!({"mandateId": mandate_id}) {
                return Err(LocalLiveMandateError::Conflict);
            }
        }
    }
    let outcome = storage
        .put_json_if_absent(GRANTS, &mandate_id, json(&mandate)?, context)
        .map_err(|error| {
            if error == "immutable_storage_conflict" {
                LocalLiveMandateError::Conflict
            } else {
                LocalLiveMandateError::Storage(error)
            }
        })?;
    match outcome {
        ImmutablePutOutcome::Created | ImmutablePutOutcome::AlreadyPresent => {
            let stored = load_mandate(storage, &mandate_id)?;
            if stored != mandate {
                return Err(LocalLiveMandateError::Conflict);
            }
            Ok(stored)
        }
    }
}

pub fn revoke_local_live_mandate(
    storage: &dyn StoragePort,
    context: &SideEffectContext,
    mandate_id: &str,
    actor: &VerifiedLiveActor,
    revoked_at_ms: i64,
) -> Result<(), LocalLiveMandateError> {
    let mandate = load_mandate(storage, mandate_id)?;
    owner(actor, &mandate.binding)?;
    let event = Revocation {
        schema: SCHEMA,
        mandate_id: mandate_id.to_string(),
        revoked_at_ms,
        request_idempotency_key: context.idempotency_key.as_str().to_string(),
        actor: actor.clone(),
    };
    let key = mandate_id.to_string();
    storage
        .put_json_if_absent(REVOCATIONS, &key, json(&event)?, context)
        .map(|_| ())
        .map_err(LocalLiveMandateError::Storage)
}

pub fn check_local_live_mandate(
    storage: &dyn StoragePort,
    mandate_id: &str,
    expected: &LocalLiveMandateBinding,
    actor: &VerifiedLiveActor,
    now_ms: i64,
) -> Result<LocalLiveMandate, LocalLiveMandateError> {
    let mandate = load_mandate(storage, mandate_id)?;
    if mandate.schema != SCHEMA || mandate.binding != *expected || !mandate.binding.live_mode {
        return Err(LocalLiveMandateError::ActorNotAllowed);
    }
    if now_ms < mandate.issued_at_ms {
        return Err(LocalLiveMandateError::NotYetValid);
    }
    if now_ms >= mandate.expires_at_ms {
        return Err(LocalLiveMandateError::Expired);
    }
    if let Some(value) = storage
        .get_json(REVOCATIONS, mandate_id)
        .map_err(LocalLiveMandateError::Storage)?
    {
        if serde_json::from_value::<Revocation>(value).is_err() {
            return Err(LocalLiveMandateError::Corrupt);
        }
        return Err(LocalLiveMandateError::Revoked);
    }
    let owner_match = actor.actor_kind == "user"
        && actor.issuer == mandate.binding.owner_issuer
        && actor.subject == mandate.binding.owner_subject;
    let delegate_match = mandate.delegate.as_ref().is_some_and(|d| {
        d.actor_kind == actor.actor_kind && d.issuer == actor.issuer && d.subject == actor.subject
    });
    if !owner_match && !delegate_match {
        return Err(LocalLiveMandateError::ActorNotAllowed);
    }
    Ok(mandate)
}

/// Build the canonical mandate binding from already-verified local observations.
/// Callers must re-resolve capability/health currentness before using this helper.
pub(crate) fn binding_from_observed_state(
    config: &Value,
    revision: &Value,
    selected: &Value,
    instance: &Value,
    owner: &crate::local_owner_identity::LocalOwnerIdentity,
) -> Result<LocalLiveMandateBinding, &'static str> {
    let text = |value: &Value| value.as_str().unwrap_or_default().to_string();
    let health_binding = crate::live_execution_checks::live_health_evidence_binding(instance);
    let binding = LocalLiveMandateBinding {
        installation_owner_id: owner.stable_identity_id.clone(),
        owner_issuer: owner.issuer.clone(),
        owner_subject: owner.subject.clone(),
        execution_config_id: text(
            config
                .get("configId")
                .or_else(|| config.get("id"))
                .unwrap_or(&Value::Null),
        ),
        execution_config_digest: crate::spec::canonical_hash(config)
            .map_err(|_| "config_digest_failed")?,
        strategy_version: text(
            config
                .get("strategyVersionId")
                .or_else(|| config.get("version_id"))
                .unwrap_or(&Value::Null),
        ),
        strategy_hash: text(&config["strategySpecHash"]),
        account_ref: selected["accountRef"]
            .as_str()
            .or_else(|| config["accountRef"].as_str())
            .unwrap_or_default()
            .into(),
        live_mode: config["mode"]
            .as_str()
            .or_else(|| config["accountMode"].as_str())
            == Some("live"),
        plugin_instance_ref: text(&selected["pluginInstanceRef"]),
        plugin_ref: text(&selected["pluginRef"]),
        capability_graph_revision: text(&config["capabilityGraphRevisionId"]),
        capability_graph_fingerprint: revision["graphFingerprint"]
            .as_str()
            .or_else(|| config["capabilityGraphFingerprint"].as_str())
            .unwrap_or_default()
            .into(),
        credential_generation: instance["health"]["binding"]["credentialRevision"]
            .as_u64()
            .map(|value| value.to_string())
            .unwrap_or_default(),
        credential_health_binding_ref: format!(
            "sha256:{:x}",
            Sha256::digest(
                serde_json::to_vec(&health_binding).map_err(|_| "live_binding_incomplete")?
            )
        ),
    };
    if !binding.live_mode {
        return Err("live_config_required");
    }
    if [
        &binding.execution_config_id,
        &binding.strategy_hash,
        &binding.strategy_version,
        &binding.account_ref,
        &binding.plugin_ref,
        &binding.plugin_instance_ref,
        &binding.capability_graph_revision,
        &binding.capability_graph_fingerprint,
        &binding.credential_generation,
    ]
    .iter()
    .any(|value| value.is_empty())
    {
        return Err("live_binding_incomplete");
    }
    Ok(binding)
}

/// A redacted, transport-independent failure from persisted execution authority.
#[derive(Debug, PartialEq, Eq)]
pub enum RunAuthorityError {
    Invalid(&'static str),
    Storage,
    Mandate(LocalLiveMandateError),
}

/// Derive a named deployment actor from installation-owned metadata. This is
/// not caller authentication; callers must obtain these inputs from trusted state.
pub fn deployment_actor(
    installation_owner_id: &str,
    deployment_id: &str,
    binding_digest: &str,
) -> VerifiedLiveActor {
    VerifiedLiveActor {
        actor_kind: "agent".into(),
        issuer: format!("tradeassembly-local-agent:{installation_owner_id}"),
        subject: format!(
            "sha256:{:x}",
            Sha256::digest(format!("{deployment_id}:{binding_digest}").as_bytes())
        ),
    }
}

/// Shared by scheduled evaluation and the final broker guard. `current` must be
/// freshly derived from trusted ports, never deserialized from caller authority.
/// This function neither issues grants nor grants permission to dispatch by itself.
pub fn check_persisted_run_authority(
    storage: &dyn StoragePort,
    run: &Value,
    config: &Value,
    current: &LocalLiveMandateBinding,
    now_ms: i64,
) -> Result<LocalLiveMandate, RunAuthorityError> {
    let invalid = RunAuthorityError::Invalid;
    let authority = &run["localLiveAuthority"];
    if authority["schemaVersion"] != "tradeassembly.local_live_activation_authority.v1" {
        return Err(invalid("local_live_authority_required"));
    }
    let id = authority["mandateId"]
        .as_str()
        .filter(|id| !id.trim().is_empty())
        .ok_or_else(|| invalid("local_live_authority_required"))?;
    let actor: VerifiedLiveActor = serde_json::from_value(authority["actor"].clone())
        .map_err(|_| invalid("local_live_authority_invalid"))?;
    for field in [
        "configId",
        "strategyId",
        "strategyVersionId",
        "strategySpecHash",
        "mode",
        "accountRef",
        "capabilityGraphRevisionId",
        "capabilityGraphFingerprint",
    ] {
        if run[field].is_null() || run[field] != config[field] {
            return Err(invalid("live_mandate_stale_binding"));
        }
    }
    if !current.live_mode
        || config["mode"] != "live"
        || config["configId"] != current.execution_config_id
        || crate::spec::canonical_hash(config).map_err(|_| invalid("live_mandate_stale_binding"))?
            != current.execution_config_digest
    {
        return Err(invalid("live_mandate_stale_binding"));
    }
    if actor.actor_kind == "agent" {
        let deployments = storage
            .list_json(crate::agent_runner::DEPLOYMENTS_NS)
            .map_err(|_| RunAuthorityError::Storage)?;
        let matches = deployments.into_iter().any(|(_, record)| {
            serde_json::from_value::<crate::agent_runner::AgentDeployment>(
                record["deployment"].clone(),
            )
            .is_ok_and(|deployment| {
                deployment.mode == "live"
                    && deployment.execution_config_version_id == current.execution_config_id
                    && deployment_actor(
                        &current.installation_owner_id,
                        &deployment.deployment_id,
                        &crate::agent_runner::deployment_binding_digest(&deployment),
                    ) == actor
            })
        });
        if !matches {
            return Err(invalid("live_mandate_delegate_stale"));
        }
    }
    let mandate = check_local_live_mandate(storage, id, current, &actor, now_ms)
        .map_err(RunAuthorityError::Mandate)?;
    if authority["mandateDigest"] != mandate.digest {
        return Err(invalid("local_live_authority_invalid"));
    }
    Ok(mandate)
}

fn load_mandate(
    storage: &dyn StoragePort,
    mandate_id: &str,
) -> Result<LocalLiveMandate, LocalLiveMandateError> {
    let mandate: LocalLiveMandate = read(storage, GRANTS, mandate_id)?;
    if mandate.schema != SCHEMA || mandate.mandate_id != mandate_id {
        return Err(LocalLiveMandateError::Corrupt);
    }
    let expected_id = digest(&(
        mandate.binding.clone(),
        mandate.request_idempotency_key.as_str(),
    ))
    .map(|d| format!("mandate:{d}"))?;
    if expected_id != mandate_id {
        return Err(LocalLiveMandateError::Corrupt);
    }
    let without_digest = LocalLiveMandate {
        digest: String::new(),
        ..mandate.clone()
    };
    if digest(&without_digest)? != mandate.digest {
        return Err(LocalLiveMandateError::Corrupt);
    }
    Ok(mandate)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapters::local::sqlite::LocalSqliteStorage;
    use crate::ports::{AuthorityContext, IdempotencyKey};
    use tempfile::tempdir;

    fn fixture() -> (LocalLiveMandateRequest, VerifiedLiveActor) {
        let owner = VerifiedLiveActor {
            actor_kind: "user".into(),
            issuer: "issuer".into(),
            subject: "owner".into(),
        };
        let binding = LocalLiveMandateBinding {
            installation_owner_id: "install-1".into(),
            owner_issuer: "issuer".into(),
            owner_subject: "owner".into(),
            execution_config_id: "cfg-1".into(),
            execution_config_digest: "sha256:cfg".into(),
            strategy_version: "v1".into(),
            strategy_hash: "sha256:strat".into(),
            account_ref: "acct-live".into(),
            live_mode: true,
            plugin_instance_ref: "plugin-instance".into(),
            plugin_ref: "alpaca".into(),
            capability_graph_revision: "cap-1".into(),
            capability_graph_fingerprint: "sha256:cap".into(),
            credential_generation: "gen-1".into(),
            credential_health_binding_ref: "health-1".into(),
        };
        (
            LocalLiveMandateRequest {
                binding,
                issued_at_ms: 100,
                expires_at_ms: 200,
                delegate: Some(VerifiedLiveActor {
                    actor_kind: "user".into(),
                    issuer: "issuer".into(),
                    subject: "delegate".into(),
                }),
            },
            owner,
        )
    }

    fn context(key: &str) -> SideEffectContext {
        SideEffectContext::new(
            AuthorityContext {
                actor: "owner".into(),
                surface: "test".into(),
                account_mode: "live".into(),
            },
            IdempotencyKey::new(key).unwrap(),
        )
    }

    fn storage() -> (tempfile::TempDir, LocalSqliteStorage) {
        let dir = tempdir().unwrap();
        let path = dir.path().join("state.db");
        (dir, LocalSqliteStorage::new(path.to_string_lossy()))
    }

    #[test]
    fn persisted_run_guard_binds_config_actor_digest_expiry_and_revocation() {
        let (_dir, storage) = storage();
        let (mut request, owner) = fixture();
        let config = serde_json::json!({
            "configId":"cfg-1", "strategyId":"strategy-1", "strategyVersionId":"v1",
            "strategySpecHash":"sha256:strat", "mode":"live", "accountRef":"acct-live",
            "capabilityGraphRevisionId":"cap-1", "capabilityGraphFingerprint":"sha256:cap"
        });
        request.binding.execution_config_digest = crate::spec::canonical_hash(&config).unwrap();
        let mandate =
            issue_local_live_mandate(&storage, &context("run-guard"), request, &owner, 150)
                .unwrap();
        let mut run = config.clone();
        run["localLiveAuthority"] = serde_json::json!({
            "schemaVersion":"tradeassembly.local_live_activation_authority.v1",
            "mandateId":mandate.mandate_id, "mandateDigest":mandate.digest, "actor":owner
        });
        assert_eq!(
            check_persisted_run_authority(&storage, &run, &config, &mandate.binding, 150).unwrap(),
            mandate
        );
        for field in [
            "configId",
            "strategyId",
            "strategyVersionId",
            "strategySpecHash",
            "mode",
            "accountRef",
            "capabilityGraphRevisionId",
            "capabilityGraphFingerprint",
        ] {
            let mut changed = run.clone();
            changed[field] = serde_json::json!("different");
            assert_eq!(
                check_persisted_run_authority(&storage, &changed, &config, &mandate.binding, 150),
                Err(RunAuthorityError::Invalid("live_mandate_stale_binding")),
                "{field}"
            );
        }
        let mut changed = run.clone();
        changed["localLiveAuthority"]["mandateDigest"] = serde_json::json!("different");
        assert_eq!(
            check_persisted_run_authority(&storage, &changed, &config, &mandate.binding, 150),
            Err(RunAuthorityError::Invalid("local_live_authority_invalid"))
        );
        changed = run.clone();
        changed["localLiveAuthority"]["actor"]["subject"] = serde_json::json!("different");
        assert_eq!(
            check_persisted_run_authority(&storage, &changed, &config, &mandate.binding, 150),
            Err(RunAuthorityError::Mandate(
                LocalLiveMandateError::ActorNotAllowed
            ))
        );
        let mut changed_config = config.clone();
        changed_config["riskLimits"] = serde_json::json!({"fixture":"changed"});
        assert_eq!(
            check_persisted_run_authority(&storage, &run, &changed_config, &mandate.binding, 150),
            Err(RunAuthorityError::Invalid("live_mandate_stale_binding"))
        );
        assert_eq!(
            check_persisted_run_authority(&storage, &run, &config, &mandate.binding, 200),
            Err(RunAuthorityError::Mandate(LocalLiveMandateError::Expired))
        );
        revoke_local_live_mandate(
            &storage,
            &context("run-revoke"),
            &mandate.mandate_id,
            &owner,
            160,
        )
        .unwrap();
        assert_eq!(
            check_persisted_run_authority(&storage, &run, &config, &mandate.binding, 160),
            Err(RunAuthorityError::Mandate(LocalLiveMandateError::Revoked))
        );
    }

    #[test]
    fn persists_replays_and_reopens() {
        let (dir, storage) = storage();
        let (request, owner) = fixture();
        let first =
            issue_local_live_mandate(&storage, &context("req-1"), request.clone(), &owner, 150)
                .unwrap();
        let replay =
            issue_local_live_mandate(&storage, &context("req-1"), request, &owner, 150).unwrap();
        assert_eq!(first, replay);
        drop(storage);
        let reopened = LocalSqliteStorage::new(dir.path().join("state.db").to_string_lossy());
        assert!(check_local_live_mandate(
            &reopened,
            &first.mandate_id,
            &first.binding,
            &owner,
            150
        )
        .is_ok());
    }

    #[test]
    fn rejects_conflicts_identity_and_expiry() {
        let (_dir, storage) = storage();
        let (request, owner) = fixture();
        issue_local_live_mandate(&storage, &context("req-1"), request.clone(), &owner, 150)
            .unwrap();
        let mut conflict = request.clone();
        conflict.expires_at_ms = 199;
        assert_eq!(
            issue_local_live_mandate(&storage, &context("req-1"), conflict, &owner, 150),
            Err(LocalLiveMandateError::Conflict)
        );
        let wrong = VerifiedLiveActor {
            actor_kind: "user".into(),
            issuer: "issuer".into(),
            subject: "wrong".into(),
        };
        assert_eq!(
            issue_local_live_mandate(&storage, &context("req-2"), request.clone(), &wrong, 150),
            Err(LocalLiveMandateError::NotOwner)
        );
        assert_eq!(
            issue_local_live_mandate(&storage, &context("req-3"), request.clone(), &owner, 50),
            Err(LocalLiveMandateError::NotYetValid)
        );
        let mut unknown_delegate = request;
        unknown_delegate.delegate = Some(VerifiedLiveActor {
            actor_kind: "service".into(),
            issuer: "issuer".into(),
            subject: "svc".into(),
        });
        assert!(matches!(
            issue_local_live_mandate(&storage, &context("req-4"), unknown_delegate, &owner, 150),
            Err(LocalLiveMandateError::Invalid(_))
        ));
    }

    #[test]
    fn checks_binding_delegate_and_revocation() {
        let (_dir, storage) = storage();
        let (request, owner) = fixture();
        let mandate =
            issue_local_live_mandate(&storage, &context("req-1"), request.clone(), &owner, 150)
                .unwrap();
        let delegate = request.delegate.unwrap();
        assert!(check_local_live_mandate(
            &storage,
            &mandate.mandate_id,
            &mandate.binding,
            &delegate,
            150
        )
        .is_ok());
        let mut wrong_binding = mandate.binding.clone();
        wrong_binding.account_ref = "other".into();
        assert_eq!(
            check_local_live_mandate(&storage, &mandate.mandate_id, &wrong_binding, &owner, 150),
            Err(LocalLiveMandateError::ActorNotAllowed)
        );
        revoke_local_live_mandate(
            &storage,
            &context("revoke-1"),
            &mandate.mandate_id,
            &owner,
            160,
        )
        .unwrap();
        assert_eq!(
            check_local_live_mandate(&storage, &mandate.mandate_id, &mandate.binding, &owner, 160),
            Err(LocalLiveMandateError::Revoked)
        );
    }

    #[test]
    fn named_agent_delegate_is_allowed_but_cannot_manage() {
        let (_dir, storage) = storage();
        let (mut request, owner) = fixture();
        let agent = VerifiedLiveActor {
            actor_kind: "agent".into(),
            issuer: "agent-issuer".into(),
            subject: "runner-1".into(),
        };
        request.delegate = Some(agent.clone());
        let mandate = issue_local_live_mandate(
            &storage,
            &context("agent-delegate"),
            request.clone(),
            &owner,
            150,
        )
        .unwrap();
        assert!(check_local_live_mandate(
            &storage,
            &mandate.mandate_id,
            &mandate.binding,
            &agent,
            150
        )
        .is_ok());
        assert_eq!(
            issue_local_live_mandate(
                &storage,
                &context("agent-mint"),
                request.clone(),
                &agent,
                150
            ),
            Err(LocalLiveMandateError::AgentNotAllowed)
        );
        assert_eq!(
            revoke_local_live_mandate(
                &storage,
                &context("agent-revoke"),
                &mandate.mandate_id,
                &agent,
                150
            ),
            Err(LocalLiveMandateError::AgentNotAllowed)
        );
        assert_eq!(
            check_local_live_mandate(&storage, &mandate.mandate_id, &mandate.binding, &agent, 200),
            Err(LocalLiveMandateError::Expired)
        );
    }

    #[test]
    fn fails_closed_on_missing_and_corrupt_records() {
        let (_dir, storage) = storage();
        let (request, owner) = fixture();
        assert_eq!(
            check_local_live_mandate(&storage, "missing", &request.binding, &owner, 150),
            Err(LocalLiveMandateError::Missing)
        );
        storage
            .put_json(
                GRANTS,
                "bad",
                serde_json::json!({"schema": 1}),
                &context("bad"),
            )
            .unwrap();
        assert_eq!(
            check_local_live_mandate(&storage, "bad", &request.binding, &owner, 150),
            Err(LocalLiveMandateError::Corrupt)
        );
    }

    #[test]
    fn every_binding_field_and_actor_component_is_enforced() {
        let (_dir, storage) = storage();
        let (request, owner) = fixture();
        let mandate =
            issue_local_live_mandate(&storage, &context("binding-check"), request, &owner, 150)
                .unwrap();
        let binding = serde_json::to_value(&mandate.binding).unwrap();
        for (field, original) in binding.as_object().unwrap() {
            let mut changed = binding.clone();
            changed[field] = if original.is_boolean() {
                Value::Bool(false)
            } else {
                Value::String("different-binding".into())
            };
            let changed = serde_json::from_value(changed).unwrap();
            assert_eq!(
                check_local_live_mandate(&storage, &mandate.mandate_id, &changed, &owner, 150,),
                Err(LocalLiveMandateError::ActorNotAllowed),
                "{field}"
            );
        }
        for mut actor in [owner.clone(), mandate.delegate.clone().unwrap()] {
            for component in ["actorKind", "issuer", "subject"] {
                let mut altered = serde_json::to_value(&actor).unwrap();
                altered[component] = Value::String("not-the-verified-actor".into());
                let altered = serde_json::from_value(altered).unwrap();
                assert_eq!(
                    check_local_live_mandate(
                        &storage,
                        &mandate.mandate_id,
                        &mandate.binding,
                        &altered,
                        150,
                    ),
                    Err(LocalLiveMandateError::ActorNotAllowed),
                    "{component}"
                );
            }
            // An agent repeating the owner's subject is still not the owner.
            actor.actor_kind = "agent".into();
            assert_eq!(
                check_local_live_mandate(
                    &storage,
                    &mandate.mandate_id,
                    &mandate.binding,
                    &actor,
                    150,
                ),
                Err(LocalLiveMandateError::ActorNotAllowed)
            );
        }
        assert_eq!(
            check_local_live_mandate(&storage, &mandate.mandate_id, &mandate.binding, &owner, 99,),
            Err(LocalLiveMandateError::NotYetValid)
        );
        assert!(check_local_live_mandate(
            &storage,
            &mandate.mandate_id,
            &mandate.binding,
            &owner,
            199,
        )
        .is_ok());
    }

    #[test]
    fn storage_failure_and_digest_tampering_never_return_a_grant() {
        let (dir, storage) = storage();
        let (request, owner) = fixture();
        let mandate =
            issue_local_live_mandate(&storage, &context("tamper"), request.clone(), &owner, 150)
                .unwrap();
        let mut altered = serde_json::to_value(&mandate).unwrap();
        altered["expiresAtMs"] = Value::from(999);
        storage
            .put_json(GRANTS, &mandate.mandate_id, altered, &context("corrupt"))
            .unwrap();
        assert_eq!(
            check_local_live_mandate(&storage, &mandate.mandate_id, &mandate.binding, &owner, 150,),
            Err(LocalLiveMandateError::Corrupt)
        );
        // A directory cannot be opened as a SQLite database. Exercise a real
        // adapter I/O failure rather than replacing authorization with a mock.
        let unavailable = LocalSqliteStorage::new(dir.path().to_string_lossy());
        assert!(matches!(
            issue_local_live_mandate(&unavailable, &context("unavailable"), request, &owner, 150,),
            Err(LocalLiveMandateError::Storage(_))
        ));
        assert!(matches!(
            check_local_live_mandate(
                &unavailable,
                &mandate.mandate_id,
                &mandate.binding,
                &owner,
                150,
            ),
            Err(LocalLiveMandateError::Storage(_))
        ));
        let error = LocalLiveMandateError::Storage("sensitive adapter detail".into());
        assert!(!error.to_string().contains("sensitive adapter detail"));
    }
}
