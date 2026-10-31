//! Authenticated-node data channel. Enrollment and human mutation authorization
//! remain owned by the existing Relay Reach backend; Core retains its own C3.
use super::{
    data_runner::{DataRunnerReceipt, DataRunnerState, DurableCoreDataRunner},
    reach_data::ReachDataCommand,
    TradeAssemblyService,
};
use crate::{
    data_pipeline::canonical_sha256,
    ports::{InstallationSignerPort, ReachNodeScope, RunnerEntropyPort},
};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::sync::Arc;

pub const NODE_CALL_SCHEMA: &str = "tradeassembly.relay.reach-data-node-request/v1";
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DataNodeOperation {
    Claim,
    Accept,
    Record,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DataNodeCall {
    pub schema: String,
    pub node: ReachNodeScope,
    pub request_id: String,
    pub nonce: String,
    pub issued_at_ms: i64,
    pub source_binding_sha256: String,
    pub operation: DataNodeOperation,
    pub command_sha256: Option<String>,
    pub receipt_sha256: Option<String>,
    pub expected_revision: Option<u64>,
    pub signature: String,
}
fn hex(v: &str) -> bool {
    v.len() == 64
        && v.bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
fn label(v: &str) -> bool {
    !v.is_empty() && v.len() <= 256 && v.bytes().all(|b| b.is_ascii_graphic())
}
impl DataNodeCall {
    pub fn signing_payload(&self) -> Result<String, String> {
        self.node
            .validate()
            .map_err(|_| "reach_data_node_invalid")?;
        if self.schema != NODE_CALL_SCHEMA
            || !label(&self.request_id)
            || !label(&self.nonce)
            || self.issued_at_ms <= 0
            || !hex(&self.source_binding_sha256)
            || self.signature.len() > 1024
            || self.command_sha256.as_deref().is_some_and(|v| !hex(v))
            || self.receipt_sha256.as_deref().is_some_and(|v| !hex(v))
            || match self.operation {
                DataNodeOperation::Claim => {
                    self.command_sha256.is_some()
                        || self.receipt_sha256.is_some()
                        || self.expected_revision.is_some()
                }
                DataNodeOperation::Accept => {
                    self.command_sha256.is_none()
                        || self.receipt_sha256.is_some()
                        || self.expected_revision.is_none_or(|v| v == 0)
                }
                DataNodeOperation::Record => {
                    self.command_sha256.is_none()
                        || self.receipt_sha256.is_none()
                        || self.expected_revision.is_none_or(|v| v == 0)
                }
            }
        {
            return Err("reach_data_node_request_invalid".into());
        }
        let mut value = serde_json::to_value(self).map_err(|_| "reach_data_encoding_invalid")?;
        value.as_object_mut().unwrap().remove("signature");
        tradeassembly_plugin_sdk::canonical::normalize_numbers(&mut value)
            .map_err(|_| "reach_data_number_invalid")?;
        serde_json_canonicalizer::to_string(&value)
            .map_err(|_| "reach_data_encoding_invalid".into())
    }
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DataDeliveryState {
    Delivered,
    Accepted,
    Uncertain,
    Completed,
    Failed,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DataDelivery {
    pub command: ReachDataCommand,
    pub command_sha256: String,
    pub state: DataDeliveryState,
    pub revision: u64,
    pub receipt: Option<DataRunnerReceipt>,
}
impl DataDelivery {
    pub fn validate(&self) -> Result<(), String> {
        self.command.validate()?;
        if self.revision == 0
            || self.revision > tradeassembly_plugin_sdk::canonical::MAX_EXACT_INTEGER
            || self.command_sha256 != self.command.command_sha256()?
            || serde_json::to_vec(self)
                .map_err(|_| "reach_data_encoding_invalid")?
                .len()
                > 655360
        {
            return Err("reach_data_delivery_invalid".into());
        }
        if let Some(r) = &self.receipt {
            if r.command_id != self.command.command.command_id
                || r.workspace_id != self.command.node.workspace_id
                || r.command_sha256 != self.command.resource.command_sha256
                || r.core_revision.as_deref() != Some(self.command.resource.core_revision.as_str())
                || r.execution_authority_granted
            {
                return Err("reach_data_delivery_receipt_invalid".into());
            }
        }
        let expected = match self.receipt.as_ref().map(|r| &r.state) {
            Some(DataRunnerState::Completed) => Some(DataDeliveryState::Completed),
            Some(DataRunnerState::Failed) => Some(DataDeliveryState::Failed),
            Some(DataRunnerState::Uncertain) => Some(DataDeliveryState::Uncertain),
            Some(DataRunnerState::Accepted) => Some(DataDeliveryState::Accepted),
            None => None,
        };
        if expected.as_ref().is_some_and(|s| s != &self.state)
            || matches!(
                self.state,
                DataDeliveryState::Completed
                    | DataDeliveryState::Failed
                    | DataDeliveryState::Uncertain
            ) && self.receipt.is_none()
        {
            return Err("reach_data_delivery_state_invalid".into());
        }
        Ok(())
    }
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DataNodeRecord {
    pub request: DataNodeCall,
    pub receipt: DataRunnerReceipt,
}
/// Implementations must reuse current authenticated human/active-enrollment
/// authority, verify node signatures/nonces and reserve original IDs atomically.
/// Read-source grants alone must never implement this interface successfully.
pub trait ReachDataTransport: Send + Sync {
    fn claim(&self, request: &DataNodeCall) -> Result<Option<DataDelivery>, String>;
    fn accept(&self, request: &DataNodeCall) -> Result<DataDelivery, String>;
    fn record(&self, request: &DataNodeRecord) -> Result<DataDelivery, String>;
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(
    tag = "status",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
pub enum ReachDataPoll {
    Idle,
    Recorded {
        request_id: String,
        state: DataDeliveryState,
        receipt_sha256: String,
    },
    Blocked {
        request_id: String,
        code: String,
    },
}

/// Source facts come from the actual configured Core and bound human, never an
/// operation argument. Hashes include opaque credential references, not values.
pub fn source_binding(
    service: &TradeAssemblyService,
    node: &ReachNodeScope,
) -> Result<String, String> {
    node.validate().map_err(|_| "reach_data_node_invalid")?;
    let (issuer, session_ref) = service
        .responsible_human_identity()
        .ok_or("reach_data_human_session_required")?;
    let now = service.runtime().clock.now_ms();
    let verified = service
        .runtime()
        .identity
        .resolve(session_ref, now)
        .map_err(|_| "reach_data_current_identity_unavailable")?;
    if verified.issuer != issuer || verified.expires_at_ms <= now {
        return Err("reach_data_current_identity_unavailable".into());
    }
    let owner = service
        .invocation_owner()
        .ok_or("reach_data_human_session_required")?;
    let config = service
        .oauth_config
        .as_ref()
        .ok_or("reach_data_configured_source_required")?;
    let revision = crate::build_identity::current().ok_or("reach_data_build_identity_required")?;
    let database = if config.profile == crate::runtime_config::RuntimeProfile::Local {
        std::fs::canonicalize(&service.db)
            .map_err(|_| "reach_data_database_identity_unavailable")?
            .to_string_lossy()
            .into_owned()
    } else {
        service.db.clone()
    };
    canonical_sha256(&json!({"schemaVersion":"tradeassembly.reach-data-source.v1","node":node,"coreRevision":revision.revision(),"owner":owner,"profile":config.redacted_configuration(),"database":database,
        "privateReferenceBindings":{"artifactRoot":config.artifact_root,"organization":config.oidc_organization_id,"identityClientSecretRef":config.oidc_client_secret_ref,"identitySessionKeyPath":config.oidc_session_key_path,"studioCoreTokenRef":config.studio_core_token_ref,"studioBaseUrl":config.studio_base_url,"wardenTokenRef":config.warden_token_ref,"postgresUrlRef":config.postgres_url_ref,"natsUrlRef":config.nats_url_ref,"objectStoreEndpoint":config.object_store_endpoint,"objectStoreCredentialRef":config.object_store_credential_ref,"pluginSandboxCommand":config.plugin_sandbox_command,"pluginSandboxWindowsProxyPortRange":config.plugin_sandbox_windows_proxy_port_range,"pluginSandboxAllowLocalEgress":config.plugin_sandbox_allow_local_egress},
        "launchDirectory":std::env::current_dir().map_err(|_|"reach_data_launch_context_unavailable")?}))
        .map_err(|_|"reach_data_source_identity_invalid".into())
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DataSourceDescription {
    pub schema_version: String,
    pub node: ReachNodeScope,
    pub core_revision: String,
    pub source_binding_sha256: String,
    pub owner_binding_sha256: String,
}
pub fn describe_source(
    service: &TradeAssemblyService,
    node: &ReachNodeScope,
) -> Result<DataSourceDescription, String> {
    let source = source_binding(service, node)?;
    let owner = service
        .invocation_owner()
        .ok_or("reach_data_human_session_required")?;
    Ok(DataSourceDescription {
        schema_version: "tradeassembly.reach-data-source.v1".into(),
        node: node.clone(),
        core_revision: crate::build_identity::current()
            .ok_or("reach_data_build_identity_required")?
            .revision()
            .into(),
        source_binding_sha256: source,
        owner_binding_sha256: canonical_sha256(&owner)
            .map_err(|_| "reach_data_source_identity_invalid")?,
    })
}

pub struct ReachDataConsumer {
    service: TradeAssemblyService,
    runner: DurableCoreDataRunner,
    node: ReachNodeScope,
    source: String,
    signer: Arc<dyn InstallationSignerPort>,
    transport: Arc<dyn ReachDataTransport>,
    entropy: Arc<dyn RunnerEntropyPort>,
}
impl ReachDataConsumer {
    pub fn new(
        service: TradeAssemblyService,
        release: String,
        node: ReachNodeScope,
        signer: Arc<dyn InstallationSignerPort>,
        transport: Arc<dyn ReachDataTransport>,
        entropy: Arc<dyn RunnerEntropyPort>,
    ) -> Result<Self, String> {
        let source = source_binding(&service, &node)?;
        if signer.installation_id() != node.node_id {
            return Err("reach_data_signer_scope_invalid".into());
        }
        let runner = DurableCoreDataRunner::new(service.clone(), release)?;
        Ok(Self {
            service,
            runner,
            node,
            source,
            signer,
            transport,
            entropy,
        })
    }
    pub fn source_binding_sha256(&self) -> &str {
        &self.source
    }
    fn signed(
        &self,
        operation: DataNodeOperation,
        request_id: String,
        command_sha256: Option<String>,
        receipt_sha256: Option<String>,
        expected_revision: Option<u64>,
    ) -> Result<DataNodeCall, String> {
        if source_binding(&self.service, &self.node)? != self.source {
            return Err("reach_data_source_changed".into());
        }
        let mut r = DataNodeCall {
            schema: NODE_CALL_SCHEMA.into(),
            node: self.node.clone(),
            request_id,
            nonce: self
                .entropy
                .opaque_id("reach-data-nonce")
                .map_err(|_| "reach_data_entropy_unavailable")?,
            issued_at_ms: self.service.runtime().clock.now_ms(),
            source_binding_sha256: self.source.clone(),
            operation,
            command_sha256,
            receipt_sha256,
            expected_revision,
            signature: String::new(),
        };
        r.signature = self
            .signer
            .sign(&r.signing_payload()?)
            .map_err(|_| "reach_data_signing_unavailable")?;
        Ok(r)
    }
    pub fn poll_once(&self) -> Result<ReachDataPoll, String> {
        let claim = self.signed(
            DataNodeOperation::Claim,
            self.entropy
                .opaque_id("reach-data-claim")
                .map_err(|_| "reach_data_entropy_unavailable")?,
            None,
            None,
            None,
        )?;
        let Some(record) = self.transport.claim(&claim)? else {
            return Ok(ReachDataPoll::Idle);
        };
        record.validate()?;
        if !matches!(
            record.state,
            DataDeliveryState::Delivered
                | DataDeliveryState::Accepted
                | DataDeliveryState::Uncertain
        ) {
            return Err("reach_data_claim_state_invalid".into());
        }
        let c = &record.command;
        if c.node != self.node || c.source_binding_sha256 != self.source {
            return Err("reach_data_source_scope_unavailable".into());
        }
        let current = self.runner.mutation_binding(&c.command)?;
        // An expired accepted command may only reconcile an already-final local
        // receipt; it never starts or resumes another effect under that expiry.
        let now = self.service.runtime().clock.now_ms();
        let prior = self
            .runner
            .receipt(&c.node.workspace_id, &c.command.command_id)?;
        let final_prior = prior.as_ref().is_some_and(|r| {
            matches!(
                r.state,
                DataRunnerState::Completed | DataRunnerState::Failed
            )
        });
        if now >= c.expires_at_ms && !final_prior {
            return Ok(ReachDataPoll::Blocked {
                request_id: c.request_id.clone(),
                code: "reach_data_expired_reconciliation_required".into(),
            });
        }
        if final_prior {
            c.validate_reconciliation_bindings(&self.node, &self.source, &current, now)?;
        } else {
            c.validate_bindings(&self.node, &self.source, &current, now)?;
        }
        let accept = self.signed(
            DataNodeOperation::Accept,
            c.request_id.clone(),
            Some(record.command_sha256.clone()),
            None,
            Some(record.revision),
        )?;
        let accepted = self.transport.accept(&accept)?;
        accepted.validate()?;
        if accepted.command_sha256 != record.command_sha256
            || accepted.revision < record.revision
            || !matches!(
                accepted.state,
                DataDeliveryState::Accepted | DataDeliveryState::Uncertain
            )
        {
            return Err("reach_data_acceptance_invalid".into());
        }
        let receipt = if final_prior {
            prior.unwrap()
        } else {
            c.validate_bindings(
                &self.node,
                &self.source,
                &self.runner.mutation_binding(&c.command)?,
                self.service.runtime().clock.now_ms(),
            )?;
            self.runner.dispatch(&c.command)?
        };
        // Revalidate live source/resource separately from immutable outcome.
        // On refusal, keep the original local receipt, never invent no effect.
        if source_binding(&self.service, &self.node)? != self.source
            || self.runner.mutation_binding(&c.command)? != current
        {
            return Ok(ReachDataPoll::Blocked {
                request_id: c.request_id.clone(),
                code: "reach_data_post_dispatch_binding_changed".into(),
            });
        }
        let request = self.signed(
            DataNodeOperation::Record,
            c.request_id.clone(),
            Some(record.command_sha256.clone()),
            Some(receipt.receipt_sha256.clone()),
            Some(accepted.revision),
        )?;
        let ack = self.transport.record(&DataNodeRecord {
            request,
            receipt: receipt.clone(),
        })?;
        ack.validate()?;
        if ack.command_sha256 != record.command_sha256
            || ack.revision < accepted.revision
            || ack.receipt.as_ref() != Some(&receipt)
        {
            return Err("reach_data_record_ack_invalid".into());
        }
        Ok(ReachDataPoll::Recorded {
            request_id: c.request_id.clone(),
            state: ack.state,
            receipt_sha256: receipt.receipt_sha256,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
    use ed25519_dalek::{Signature, Verifier as _};
    #[test]
    fn signed_data_request_has_separate_domain_and_binds_source_nonce_and_outcome() {
        let node = ReachNodeScope {
            tenant_id: "fixture.tenant".into(),
            workspace_id: "fixture.workspace".into(),
            node_id: "fixture.node".into(),
            actor_ref: "fixture.actor".into(),
        };
        let signer = crate::adapters::connected_runner::Ed25519InstallationSigner::from_seed(
            node.node_id.clone(),
            [7; 32],
        )
        .unwrap();
        let mut call = DataNodeCall {
            schema: NODE_CALL_SCHEMA.into(),
            node: node.clone(),
            request_id: "fixture.request".into(),
            nonce: "fixture.nonce".into(),
            issued_at_ms: 1000,
            source_binding_sha256: "a".repeat(64),
            operation: DataNodeOperation::Claim,
            command_sha256: None,
            receipt_sha256: None,
            expected_revision: None,
            signature: String::new(),
        };
        let payload = call.signing_payload().unwrap();
        call.signature = signer.sign(&payload).unwrap();
        let signature =
            Signature::from_slice(&URL_SAFE_NO_PAD.decode(&call.signature).unwrap()).unwrap();
        let key = ed25519_dalek::SigningKey::from_bytes(&[7; 32]).verifying_key();
        let message = |text: &str| {
            [
                crate::ports::INSTALLATION_AUTHENTICATION_DOMAIN.as_bytes(),
                text.as_bytes(),
            ]
            .concat()
        };
        key.verify(&message(&payload), &signature).unwrap();
        let old = crate::ports::ReachNodeRequest {
            node,
            request_id: call.request_id.clone(),
            nonce: call.nonce.clone(),
            issued_at_ms: call.issued_at_ms,
            signature: String::new(),
        }
        .signing_payload("claim", None)
        .unwrap();
        assert_ne!(old, payload);
        assert!(key.verify(&message(&old), &signature).is_err());
        let mut changed = call.clone();
        changed.nonce.push_str(".different");
        assert!(key
            .verify(&message(&changed.signing_payload().unwrap()), &signature)
            .is_err());
        changed = call.clone();
        changed.source_binding_sha256 = "b".repeat(64);
        assert!(key
            .verify(&message(&changed.signing_payload().unwrap()), &signature)
            .is_err());
        changed = call.clone();
        changed.receipt_sha256 = Some("c".repeat(64));
        assert!(changed.signing_payload().is_err());
        changed.operation = DataNodeOperation::Record;
        changed.command_sha256 = Some("d".repeat(64));
        changed.expected_revision = Some(1);
        assert!(changed.signing_payload().is_ok());
        let mut value = serde_json::to_value(&call).unwrap();
        value["callerAuthority"] = json!(true);
        assert!(serde_json::from_value::<DataNodeCall>(value).is_err());
    }
    #[test]
    fn existing_human_and_configured_source_are_required_before_node_consumption() {
        let root = tempfile::tempdir().unwrap();
        let db = root
            .path()
            .join("service.db")
            .to_string_lossy()
            .into_owned();
        let service =
            TradeAssemblyService::from_runtime(&db, crate::adapters::local::test_runtime(&db));
        let node = ReachNodeScope {
            tenant_id: "tenant".into(),
            workspace_id: "workspace".into(),
            node_id: "node".into(),
            actor_ref: "caller.assertion.only".into(),
        };
        assert_eq!(
            source_binding(&service, &node).unwrap_err(),
            "reach_data_human_session_required"
        );
        let service = service.for_authenticated_invocation("fixture", "alice", None, None);
        assert_eq!(
            source_binding(&service, &node).unwrap_err(),
            "reach_data_current_identity_unavailable"
        );
    }
}
