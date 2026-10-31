//! Core-computed statements for Relay's explicit existing-owner source selection
//! and command preparation. No source record, enrollment or grant is created here.
use super::{
    data_runner::{DataRunnerCommand, DurableCoreDataRunner},
    reach_data::{ReachDataCommand, MAX_REACH_DATA_LIFETIME_MS, REACH_DATA_SCHEMA},
    reach_data_consumer::describe_source,
    TradeAssemblyService,
};
use crate::ports::{InstallationSignerPort, ReachNodeScope, RunnerEntropyPort};
use serde::{Deserialize, Serialize};
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SourceRegistration {
    pub schema: String,
    pub node: ReachNodeScope,
    pub request_id: String,
    pub nonce: String,
    pub issued_at_ms: i64,
    pub expected_revision: u64,
    pub source_binding_sha256: String,
    pub core_revision: String,
    pub owner_binding_sha256: String,
    pub signature: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Preparation {
    pub schema: String,
    pub command: ReachDataCommand,
    pub source_revision: u64,
    pub nonce: String,
    pub issued_at_ms: i64,
    pub signature: String,
}
fn unsigned<T: Serialize>(statement: &T) -> Result<String, String> {
    let mut value = serde_json::to_value(statement).map_err(|_| "reach_data_statement_invalid")?;
    value
        .as_object_mut()
        .ok_or("reach_data_statement_invalid")?
        .remove("signature");
    tradeassembly_plugin_sdk::canonical::normalize_numbers(&mut value)
        .map_err(|_| "reach_data_statement_invalid")?;
    serde_json_canonicalizer::to_string(&value).map_err(|_| "reach_data_statement_invalid".into())
}
fn hex(v: &str, n: usize) -> bool {
    v.len() == n
        && v.bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
fn label(v: &str) -> bool {
    !v.is_empty() && v.len() <= 200 && v.bytes().all(|b| b.is_ascii_graphic())
}
impl SourceRegistration {
    pub fn signing_payload(&self) -> Result<String, String> {
        self.node
            .validate()
            .map_err(|_| "reach_data_statement_invalid")?;
        if self.schema != "tradeassembly.relay.reach-data-source-selection/v1"
            || !label(&self.request_id)
            || !label(&self.nonce)
            || self.issued_at_ms <= 0
            || self.expected_revision > tradeassembly_plugin_sdk::canonical::MAX_EXACT_INTEGER
            || !hex(&self.source_binding_sha256, 64)
            || !hex(&self.core_revision, 40)
            || !hex(&self.owner_binding_sha256, 64)
            || self.signature.len() > 1024
        {
            return Err("reach_data_statement_invalid".into());
        }
        unsigned(self)
    }
}
impl Preparation {
    pub fn signing_payload(&self) -> Result<String, String> {
        self.command.validate()?;
        if self.schema != "tradeassembly.relay.reach-data-preparation/v1"
            || self.source_revision == 0
            || self.source_revision > tradeassembly_plugin_sdk::canonical::MAX_EXACT_INTEGER
            || !label(&self.nonce)
            || self.issued_at_ms <= 0
            || self.signature.len() > 1024
        {
            return Err("reach_data_statement_invalid".into());
        }
        unsigned(self)
    }
}
pub struct DataStatementProducer<'a> {
    service: &'a TradeAssemblyService,
    node: &'a ReachNodeScope,
    signer: &'a dyn InstallationSignerPort,
    entropy: &'a dyn RunnerEntropyPort,
    release: &'a str,
}
impl<'a> DataStatementProducer<'a> {
    pub fn new(
        service: &'a TradeAssemblyService,
        node: &'a ReachNodeScope,
        signer: &'a dyn InstallationSignerPort,
        entropy: &'a dyn RunnerEntropyPort,
        release: &'a str,
    ) -> Result<Self, String> {
        node.validate()
            .map_err(|_| "reach_data_signer_scope_invalid")?;
        if signer.installation_id() != node.node_id {
            return Err("reach_data_signer_scope_invalid".into());
        }
        Ok(Self {
            service,
            node,
            signer,
            entropy,
            release,
        })
    }
    pub fn source_selection(
        &self,
        request_id: String,
        expected_revision: u64,
    ) -> Result<SourceRegistration, String> {
        let actual = describe_source(self.service, self.node)?;
        let mut statement = SourceRegistration {
            schema: "tradeassembly.relay.reach-data-source-selection/v1".into(),
            node: self.node.clone(),
            request_id,
            nonce: self
                .entropy
                .opaque_id("reach-source-nonce")
                .map_err(|_| "reach_data_entropy_unavailable")?,
            issued_at_ms: self.service.runtime().clock.now_ms(),
            expected_revision,
            source_binding_sha256: actual.source_binding_sha256,
            core_revision: actual.core_revision,
            owner_binding_sha256: actual.owner_binding_sha256,
            signature: String::new(),
        };
        statement.signature = self
            .signer
            .sign(&statement.signing_payload()?)
            .map_err(|_| "reach_data_signing_unavailable")?;
        Ok(statement)
    }
    fn current_binding(
        &self,
        command: &DataRunnerCommand,
    ) -> Result<super::data_runner::DataMutationBinding, String> {
        DurableCoreDataRunner::new(self.service.clone(), self.release.into())?
            .mutation_binding(command)
    }
    fn sign_preparation(
        &self,
        envelope: ReachDataCommand,
        source_revision: u64,
        current: super::data_runner::DataMutationBinding,
    ) -> Result<Preparation, String> {
        let actual = describe_source(self.service, self.node)?;
        envelope.validate_bindings(
            self.node,
            &actual.source_binding_sha256,
            &current,
            self.service.runtime().clock.now_ms(),
        )?;
        let mut statement = Preparation {
            schema: "tradeassembly.relay.reach-data-preparation/v1".into(),
            command: envelope,
            source_revision,
            nonce: self
                .entropy
                .opaque_id("reach-preparation-nonce")
                .map_err(|_| "reach_data_entropy_unavailable")?,
            issued_at_ms: self.service.runtime().clock.now_ms(),
            signature: String::new(),
        };
        statement.signature = self
            .signer
            .sign(&statement.signing_payload()?)
            .map_err(|_| "reach_data_signing_unavailable")?;
        Ok(statement)
    }
    pub fn prepare_command(
        &self,
        command: DataRunnerCommand,
        source_revision: u64,
        ttl_ms: u64,
    ) -> Result<Preparation, String> {
        if ttl_ms == 0 || ttl_ms > MAX_REACH_DATA_LIFETIME_MS as u64 {
            return Err("reach_data_command_lifetime_invalid".into());
        }
        let actual = describe_source(self.service, self.node)?;
        let resource = self.current_binding(&command)?;
        let now = self.service.runtime().clock.now_ms();
        let envelope = ReachDataCommand {
            schema: REACH_DATA_SCHEMA.into(),
            node: self.node.clone(),
            request_id: command.command_id.clone(),
            idempotency_key: command.idempotency_key.as_str().into(),
            issued_at_ms: now,
            expires_at_ms: now
                .checked_add(ttl_ms as i64)
                .ok_or("reach_data_command_lifetime_invalid")?,
            source_binding_sha256: actual.source_binding_sha256,
            resource: resource.clone(),
            command,
        };
        self.sign_preparation(envelope, source_revision, resource)
    }
    pub fn retry_preparation(
        &self,
        envelope: ReachDataCommand,
        source_revision: u64,
    ) -> Result<Preparation, String> {
        let current = self.current_binding(&envelope.command)?;
        self.sign_preparation(envelope, source_revision, current)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn selection_is_closed_bounded_and_signature_exclusion_preserves_freshness_fields() {
        let mut s = SourceRegistration {
            schema: "tradeassembly.relay.reach-data-source-selection/v1".into(),
            node: ReachNodeScope {
                tenant_id: "tenant".into(),
                workspace_id: "workspace".into(),
                node_id: "node".into(),
                actor_ref: "actor".into(),
            },
            request_id: "request".into(),
            nonce: "nonce".into(),
            issued_at_ms: 1000,
            expected_revision: 0,
            source_binding_sha256: "a".repeat(64),
            core_revision: "b".repeat(40),
            owner_binding_sha256: "c".repeat(64),
            signature: String::new(),
        };
        let original = s.signing_payload().unwrap();
        s.signature = "fixture.signature".into();
        assert_eq!(s.signing_payload().unwrap(), original);
        s.nonce = "fresh.nonce".into();
        assert_ne!(s.signing_payload().unwrap(), original);
        s.expected_revision = tradeassembly_plugin_sdk::canonical::MAX_EXACT_INTEGER + 1;
        assert!(s.signing_payload().is_err());
        let mut raw = serde_json::to_value(&s).unwrap();
        raw["executionAuthorityGranted"] = serde_json::json!(true);
        assert!(serde_json::from_value::<SourceRegistration>(raw).is_err());
    }
}
