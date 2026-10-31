//! Separate Reach-data wire binding. Validation is not transport authentication.
//! Existing Reach V1, enrolled-node authority and Core C3 remain unchanged.
use super::data_runner::{DataMutationBinding, DataRunnerCommand};
use crate::{data_pipeline::canonical_sha256, ports::ReachNodeScope};
use serde::{Deserialize, Serialize};

pub const REACH_DATA_SCHEMA: &str = "tradeassembly.relay.reach-data-command/v1";
pub const MAX_REACH_DATA_BYTES: usize = 294912;
pub const MAX_REACH_DATA_LIFETIME_MS: i64 = 300000;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ReachDataCommand {
    pub schema: String,
    pub node: ReachNodeScope,
    pub request_id: String,
    pub idempotency_key: String,
    pub issued_at_ms: i64,
    pub expires_at_ms: i64,
    pub source_binding_sha256: String,
    pub resource: DataMutationBinding,
    pub command: DataRunnerCommand,
}
fn digest(value: &str, n: usize) -> bool {
    value.len() == n
        && value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
impl ReachDataCommand {
    pub fn parse(bytes: &[u8]) -> Result<Self, String> {
        if bytes.len() > MAX_REACH_DATA_BYTES {
            return Err("reach_data_command_too_large".into());
        }
        let mut value: serde_json::Value =
            serde_json::from_slice(bytes).map_err(|_| "reach_data_command_invalid")?;
        tradeassembly_plugin_sdk::canonical::normalize_numbers(&mut value)
            .map_err(|_| "reach_data_number_invalid")?;
        let command: Self =
            serde_json::from_value(value).map_err(|_| "reach_data_command_invalid")?;
        command.validate()?;
        Ok(command)
    }
    pub fn validate(&self) -> Result<(), String> {
        self.node
            .validate()
            .map_err(|_| "reach_data_node_invalid")?;
        self.command.validate(&self.command.core_release)?;
        let encoded = serde_json::to_vec(self).map_err(|_| "reach_data_encoding_invalid")?;
        canonical_sha256(self).map_err(|_| "reach_data_number_invalid")?;
        let action = match &self.command.operation {
            super::data_runner::DataRunnerOperation::PrepareStrategy { .. } => {
                "strategy.data_plan.prepare"
            }
            super::data_runner::DataRunnerOperation::PrepareRecipe { .. } => {
                "strategy.data_recipe.prepare"
            }
            super::data_runner::DataRunnerOperation::EvaluateStrategy { .. } => {
                "strategy.data_run.step"
            }
            super::data_runner::DataRunnerOperation::EvaluateRecipe { .. } => {
                "strategy.data_recipe.step"
            }
            super::data_runner::DataRunnerOperation::EvaluateScreen { .. } => {
                "strategy.instrument_screen.create"
            }
            super::data_runner::DataRunnerOperation::PrepareSession { .. } => {
                "strategy.session_schedule.prepare"
            }
        };
        if self.schema != REACH_DATA_SCHEMA
            || encoded.len() > MAX_REACH_DATA_BYTES
            || self.request_id != self.command.command_id
            || self.idempotency_key != self.command.idempotency_key.as_str()
            || self.node.workspace_id != self.command.workspace_id
            || self.issued_at_ms <= 0
            || self.command.submitted_at_ms > self.issued_at_ms
            || self.expires_at_ms <= self.issued_at_ms
            || self
                .expires_at_ms
                .checked_sub(self.issued_at_ms)
                .is_none_or(|n| n > MAX_REACH_DATA_LIFETIME_MS)
            || !digest(&self.source_binding_sha256, 64)
            || self.resource.schema_version != "tradeassembly.core-data-mutation-binding.v1"
            || !digest(&self.resource.core_revision, 40)
            || !digest(&self.resource.command_sha256, 64)
            || self.resource.command_sha256 != self.command.canonical_hash()?
            || self.resource.mode != self.command.mode
            || self.resource.action != action
            || !digest(&self.resource.domain_binding_sha256, 64)
            || !digest(&self.resource.policy_sha256, 64)
            || self
                .resource
                .owner_binding_sha256
                .as_deref()
                .is_none_or(|v| !digest(v, 64))
            || self.resource.execution_authority_granted
        {
            return Err("reach_data_command_invalid".into());
        }
        Ok(())
    }
    pub fn command_sha256(&self) -> Result<String, String> {
        self.validate()?;
        canonical_sha256(self).map_err(|_| "reach_data_encoding_invalid".into())
    }
    /// Compare against facts independently resolved by the authenticated transport
    /// and the current Core service. This method neither authenticates a node nor
    /// creates a permission. The owning consumer must still do both before effects.
    pub fn validate_bindings(
        &self,
        enrolled_node: &ReachNodeScope,
        registered_source_sha256: &str,
        current_resource: &DataMutationBinding,
        now_ms: i64,
    ) -> Result<(), String> {
        self.validate()?;
        if &self.node != enrolled_node
            || self.source_binding_sha256 != registered_source_sha256
            || &self.resource != current_resource
            || now_ms <= 0
            || self.issued_at_ms > now_ms.saturating_add(5000)
            || now_ms >= self.expires_at_ms
        {
            return Err("reach_data_binding_unavailable".into());
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    fn fixture() -> ReachDataCommand {
        let command = DataRunnerCommand::parse(&serde_json::to_vec(&json!({"schemaVersion":"tradeassembly.core-data-runner.v1","coreRelease":"fixture.release","workspaceId":"workspace","commandId":"request","authorityRef":"provenance.only","idempotencyKey":"request","mode":"paper","submittedAtMs":1000,"operation":{"kind":"prepare_session","schedule":{"scope":{"strategyId":"strategy","strategyVersionId":"version","strategySpecHash":"spec"},"capabilityGraphRevision":"graph","mode":"paper","triggerRef":"trigger","sessionDate":"2026-10-31","calendarCollectionId":"calendar","executionCalendarCollectionId":"calendar","knowledgeCutoffMs":1000}}})).unwrap(), "fixture.release").unwrap();
        let resource = DataMutationBinding {
            schema_version: "tradeassembly.core-data-mutation-binding.v1".into(),
            core_revision: "a".repeat(40),
            owner_binding_sha256: Some("b".repeat(64)),
            command_sha256: command.canonical_hash().unwrap(),
            scope: super::super::data_plans::PlanScope {
                strategy_id: "strategy".into(),
                strategy_version_id: "version".into(),
                strategy_spec_hash: "spec".into(),
            },
            capability_graph_revision: "graph".into(),
            action: "strategy.session_schedule.prepare".into(),
            mode: command.mode.clone(),
            domain_binding_sha256: "c".repeat(64),
            policy_sha256: "d".repeat(64),
            execution_authority_granted: false,
        };
        ReachDataCommand {
            schema: REACH_DATA_SCHEMA.into(),
            node: ReachNodeScope {
                tenant_id: "tenant".into(),
                workspace_id: "workspace".into(),
                node_id: "node".into(),
                actor_ref: "actor".into(),
            },
            request_id: "request".into(),
            idempotency_key: "request".into(),
            issued_at_ms: 1000,
            expires_at_ms: 2000,
            source_binding_sha256: "e".repeat(64),
            resource,
            command,
        }
    }
    #[test]
    fn explicit_reach_data_binding_refuses_owner_source_mode_policy_expiry_and_legacy_wire() {
        let c = fixture();
        c.validate_bindings(&c.node, &c.source_binding_sha256, &c.resource, 1001)
            .unwrap();
        for change in 0..7 {
            let mut altered = c.clone();
            match change {
                0 => altered.node.actor_ref = "foreign".into(),
                1 => altered.source_binding_sha256 = "f".repeat(64),
                2 => altered.resource.owner_binding_sha256 = None,
                3 => altered.resource.policy_sha256 = "f".repeat(64),
                4 => altered.resource.execution_authority_granted = true,
                5 => altered.command.mode = super::super::data_runner::DataRunnerMode::Backtest,
                _ => altered.resource.scope.strategy_id = "foreign".into(),
            }
            assert!(altered
                .validate_bindings(&c.node, &c.source_binding_sha256, &c.resource, 1001)
                .is_err());
        }
        assert!(c
            .validate_bindings(&c.node, &c.source_binding_sha256, &c.resource, 2000)
            .is_err());
        assert!(c
            .validate_bindings(&c.node, &c.source_binding_sha256, &c.resource, -1)
            .is_err());
        let mut value = serde_json::to_value(&c).unwrap();
        value["unknown"] = json!(true);
        assert!(serde_json::from_value::<ReachDataCommand>(value).is_err());
        assert!(serde_json::from_value::<crate::ports::ReachCommand>(
            serde_json::to_value(&c).unwrap()
        )
        .is_err());
        let mut changed = c.clone();
        changed.expires_at_ms += 1;
        assert_ne!(
            c.command_sha256().unwrap(),
            changed.command_sha256().unwrap()
        );
        let mut numeric = serde_json::to_value(&c).unwrap();
        numeric["issuedAtMs"] = json!(1000.0);
        assert_eq!(
            ReachDataCommand::parse(&serde_json::to_vec(&numeric).unwrap())
                .unwrap()
                .command_sha256()
                .unwrap(),
            c.command_sha256().unwrap()
        );
        numeric["issuedAtMs"] = json!(9007199254740992u64);
        assert!(ReachDataCommand::parse(&serde_json::to_vec(&numeric).unwrap()).is_err());
        assert_eq!(
            ReachDataCommand::parse(&vec![b' '; MAX_REACH_DATA_BYTES + 1]).unwrap_err(),
            "reach_data_command_too_large"
        );
    }
}
