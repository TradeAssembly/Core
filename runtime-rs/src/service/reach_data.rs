//! Separate Reach-data wire binding. Validation is not transport authentication.
//! Existing Reach V1, enrolled-node authority and Core C3 remain unchanged.
use super::data_runner::{DataMutationBinding, DataRunnerCommand};
use crate::{data_pipeline::canonical_sha256, ports::ReachNodeScope};
use serde::{Deserialize, Serialize};

pub const REACH_DATA_SCHEMA: &str = "tradeassembly.relay.reach-data-command/v1";
pub const REACH_DATA_SCHEMA_V2: &str = "tradeassembly.relay.reach-data-command/v2";
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
            super::data_runner::DataRunnerOperation::ManagePosition { management } => {
                management.action()
            }
            super::data_runner::DataRunnerOperation::Financial { financial } => financial.action(),
            super::data_runner::DataRunnerOperation::DispatchSession { .. } => "scheduler.run",
            super::data_runner::DataRunnerOperation::PrepareSelectionPlan { .. } => {
                "strategy.selection_plan.prepare"
            }
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
        if let super::data_runner::DataRunnerOperation::ManagePosition { management } =
            &self.command.operation
        {
            if self.resource.scope != *management.scope() {
                return Err("reach_data_command_invalid".into());
            }
        }
        if let super::data_runner::DataRunnerOperation::Financial { financial } =
            &self.command.operation
        {
            if self.resource.scope != *financial.scope() {
                return Err("reach_data_command_invalid".into());
            }
        }
        if let super::data_runner::DataRunnerOperation::PrepareSelectionPlan { selection_plan } =
            &self.command.operation
        {
            if self.resource.scope != selection_plan.scope
                || self.resource.capability_graph_revision
                    != selection_plan.capability_graph_revision
            {
                return Err("reach_data_command_invalid".into());
            }
        }
        if self.schema != self.command.reach_schema()
            || [
                &self.command.core_release,
                &self.command.workspace_id,
                &self.command.command_id,
                &self.command.authority_ref,
                &self.resource.capability_graph_revision,
                &self.resource.scope.strategy_id,
                &self.resource.scope.strategy_version_id,
                &self.resource.scope.strategy_spec_hash,
            ]
            .iter()
            .any(|s| s.is_empty() || s.len() > 200 || !s.bytes().all(|b| b.is_ascii_graphic()))
            || self.command.idempotency_key.as_str().len() > 200
            || !self
                .command
                .idempotency_key
                .as_str()
                .bytes()
                .all(|b| b.is_ascii_graphic())
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
            || self.resource.schema_version != self.command.binding_schema()
            || !digest(&self.resource.core_revision, 40)
            || !digest(&self.resource.command_sha256, 64)
            || self.resource.command_sha256 != self.command.canonical_hash()?
            || self.resource.mode != self.command.mode
            || self.resource.action != action
            || self.resource.resource_kind.as_deref() != self.command.resource_kind()
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
        self.validate_reconciliation_bindings(
            enrolled_node,
            registered_source_sha256,
            current_resource,
            now_ms,
        )?;
        if now_ms >= self.expires_at_ms {
            return Err("reach_data_binding_unavailable".into());
        }
        Ok(())
    }
    /// Current scope check for reporting an already-final ORIGINAL Core receipt.
    /// Expiry is tolerated only by that read/reconciliation path; this method
    /// must never admit another computation or substitute a synthetic clock.
    pub(crate) fn validate_reconciliation_bindings(
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
            resource_kind: None,
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
        let mut oversized_label = c.clone();
        oversized_label.command.authority_ref = "a".repeat(201);
        assert!(oversized_label.validate().is_err());
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
    #[test]
    fn selected_creation_versions_action_kind_and_preparation_are_coupled() {
        use super::super::{
            data_runner::{DataRunnerOperation, DATA_RUNNER_SCHEMA_V2},
            reach_data_proofs::Preparation,
            selection_plans::SelectionPlanCommand,
        };
        let mut c = fixture();
        c.command.operation = DataRunnerOperation::PrepareSelectionPlan {
            selection_plan: SelectionPlanCommand {
                scope: c.resource.scope.clone(),
                capability_graph_revision: c.resource.capability_graph_revision.clone(),
                mode: "paper".into(),
                subject_instrument_id: "A".into(),
                selection_receipts: std::collections::BTreeMap::new(),
                source_captures: std::collections::BTreeMap::new(),
            },
        };
        c.command.schema_version = DATA_RUNNER_SCHEMA_V2.into();
        c.resource.schema_version = c.command.binding_schema().into();
        c.resource.command_sha256 = c.command.canonical_hash().unwrap();
        c.resource.action = "strategy.selection_plan.prepare".into();
        c.resource.resource_kind = Some("strategy_selection_plan".into());
        c.schema = REACH_DATA_SCHEMA_V2.into();
        c.validate().unwrap();
        let prep = Preparation {
            schema: "tradeassembly.relay.reach-data-preparation/v2".into(),
            command: c.clone(),
            source_revision: 1,
            nonce: "fixture.nonce".into(),
            issued_at_ms: 1000,
            signature: String::new(),
        };
        assert!(prep.signing_payload().is_ok());
        if let Some(root) = std::env::var_os("SELECTED_WIRE_EVIDENCE_DIR") {
            let root = std::path::PathBuf::from(root);
            std::fs::create_dir_all(&root).unwrap();
            for (name, value) in [
                ("reach-command-v2.json", serde_json::to_value(&c).unwrap()),
                (
                    "core-command-v2.json",
                    serde_json::to_value(&c.command).unwrap(),
                ),
                ("preparation-v2.json", serde_json::to_value(&prep).unwrap()),
            ] {
                use std::io::Write;
                let mut file = std::fs::OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .open(root.join(name))
                    .unwrap();
                file.write_all(&serde_json::to_vec_pretty(&value).unwrap())
                    .unwrap();
            }
        }
        let mut old = prep;
        old.schema = "tradeassembly.relay.reach-data-preparation/v1".into();
        assert!(old.signing_payload().is_err());
        for i in 0..7 {
            let mut bad = c.clone();
            match i {
                0 => bad.schema = REACH_DATA_SCHEMA.into(),
                1 => {
                    bad.command.schema_version =
                        super::super::data_runner::DATA_RUNNER_SCHEMA.into()
                }
                2 => {
                    bad.resource.schema_version =
                        "tradeassembly.core-data-mutation-binding.v1".into()
                }
                3 => bad.resource.resource_kind = None,
                4 => bad.resource.action = "strategy.data_plan.prepare".into(),
                5 => bad.resource.scope.strategy_id = "foreign".into(),
                _ => {
                    if let DataRunnerOperation::PrepareSelectionPlan { selection_plan } =
                        &mut bad.command.operation
                    {
                        selection_plan.mode = "live".into();
                    }
                }
            }
            assert!(bad.validate().is_err(), "mismatch {i}");
        }
        let mut old = fixture();
        old.command.schema_version = DATA_RUNNER_SCHEMA_V2.into();
        assert!(old.validate().is_err());
        let mut v = serde_json::to_value(c).unwrap();
        v["command"]["operation"]["tool"] = json!("arbitrary");
        assert!(ReachDataCommand::parse(&serde_json::to_vec(&v).unwrap()).is_err());
    }
    #[test]
    fn scheduler_dispatch_couples_all_versions_and_actual_scheduler_resource() {
        use super::super::{
            data_runner::{DataRunnerOperation, DATA_RUNNER_SCHEMA_V3},
            reach_data_proofs::Preparation,
            session_dispatch::RemoteRequest,
        };
        let mut c = fixture();
        c.command.operation = DataRunnerOperation::DispatchSession {
            dispatch: RemoteRequest::Inspect {
                job_id: "sessionjob_fixture".into(),
            },
        };
        c.command.schema_version = DATA_RUNNER_SCHEMA_V3.into();
        c.resource.schema_version = c.command.binding_schema().into();
        c.resource.command_sha256 = c.command.canonical_hash().unwrap();
        c.resource.action = "scheduler.run".into();
        c.resource.resource_kind = Some("scheduler".into());
        c.schema = c.command.reach_schema().into();
        c.validate().unwrap();
        let prep = Preparation {
            schema: c.command.preparation_schema().into(),
            command: c.clone(),
            source_revision: 1,
            nonce: "fixture.nonce".into(),
            issued_at_ms: 1000,
            signature: String::new(),
        };
        prep.signing_payload().unwrap();
        for n in 0..5 {
            let mut bad = c.clone();
            match n {
                0 => bad.schema = REACH_DATA_SCHEMA_V2.into(),
                1 => {
                    bad.resource.schema_version =
                        "tradeassembly.core-data-mutation-binding.v2".into()
                }
                2 => bad.resource.resource_kind = Some("strategy_selection_plan".into()),
                3 => bad.resource.action = "strategy.selection_plan.prepare".into(),
                _ => {
                    bad.command.schema_version =
                        super::super::data_runner::DATA_RUNNER_SCHEMA.into()
                }
            };
            assert!(bad.validate().is_err());
        }
        let mut old = prep;
        old.schema = "tradeassembly.relay.reach-data-preparation/v2".into();
        assert!(old.signing_payload().is_err());
    }
    #[test]
    fn financial_reach_couples_scope_action_resource_and_all_versions() {
        use super::super::data_runner::{DataRunnerOperation, DATA_RUNNER_SCHEMA_V4};
        let mut c = fixture();
        let scope = c.resource.scope.clone();
        c.command.operation = DataRunnerOperation::Financial {
            financial: super::super::order_plans::remote::FinancialOperation::Release {
                scope,
                plan_id: "owned-plan".into(),
            },
        };
        c.command.schema_version = DATA_RUNNER_SCHEMA_V4.into();
        c.schema = c.command.reach_schema().into();
        c.resource.schema_version = c.command.binding_schema().into();
        c.resource.command_sha256 = c.command.canonical_hash().unwrap();
        c.resource.action = "strategy.order_plan.release".into();
        c.resource.resource_kind = Some("strategy_order_plan".into());
        c.validate().unwrap();
        for n in 0..7 {
            let mut bad = c.clone();
            match n {
                0 => bad.schema = "tradeassembly.relay.reach-data-command/v3".into(),
                1 => bad.command.schema_version = "tradeassembly.core-data-runner.v3".into(),
                2 => {
                    bad.resource.schema_version =
                        "tradeassembly.core-data-mutation-binding.v3".into()
                }
                3 => bad.resource.resource_kind = Some("scheduler".into()),
                4 => bad.resource.action = "strategy.order_plan.submit".into(),
                5 => bad.resource.scope.strategy_id = "foreign".into(),
                _ => bad.command.mode = super::super::data_runner::DataRunnerMode::Backtest,
            };
            assert!(bad.validate().is_err(), "variant {n}");
        }
    }
    #[test]
    fn settlement_reach_v5_couples_action_resource_scope_and_versions() {
        let mut c = fixture();
        c.command.operation = super::super::data_runner::DataRunnerOperation::Financial {
            financial: super::super::order_plans::remote::FinancialOperation::Settle {
                scope: c.resource.scope.clone(),
                plan_id: "owned-plan".into(),
            },
        };
        c.command.schema_version = super::super::data_runner::DATA_RUNNER_SCHEMA_V5.into();
        c.schema = c.command.reach_schema().into();
        c.resource.schema_version = c.command.binding_schema().into();
        c.resource.command_sha256 = c.command.canonical_hash().unwrap();
        c.resource.action = "strategy.order_plan.settle".into();
        c.resource.resource_kind = Some("strategy_order_plan".into());
        c.validate().unwrap();
        for n in 0..6 {
            let mut bad = c.clone();
            match n {
                0 => bad.schema = "tradeassembly.relay.reach-data-command/v4".into(),
                1 => {
                    bad.resource.schema_version =
                        "tradeassembly.core-data-mutation-binding.v4".into()
                }
                2 => bad.resource.action = "strategy.order_plan.submit".into(),
                3 => bad.resource.resource_kind = Some("broker_account".into()),
                4 => bad.resource.scope.strategy_id = "foreign".into(),
                _ => bad.resource.execution_authority_granted = true,
            };
            assert!(bad.validate().is_err());
        }
    }
    #[test]
    fn management_v6_couples_source_preparation_resource_scope_action_and_receipt_versions() {
        use super::super::{data_runner::*, order_plans::management::remote::ManagementOperation};
        let mut c = fixture();
        c.command.operation = DataRunnerOperation::ManagePosition {
            management: ManagementOperation::Inspect {
                scope: c.resource.scope.clone(),
                position_id: "position".into(),
            },
        };
        c.command.schema_version = DATA_RUNNER_SCHEMA_V6.into();
        c.schema = c.command.reach_schema().into();
        c.resource.schema_version = c.command.binding_schema().into();
        c.resource.command_sha256 = c.command.canonical_hash().unwrap();
        c.resource.action = "strategy.position.inspect".into();
        c.resource.resource_kind = Some("strategy_managed_position".into());
        c.validate().unwrap();
        let statement = super::super::reach_data_proofs::Preparation {
            schema: c.command.preparation_schema().into(),
            command: c.clone(),
            source_revision: 1,
            nonce: "fixture.nonce".into(),
            issued_at_ms: 1000,
            signature: String::new(),
        };
        statement.signing_payload().unwrap();
        for index in 0..6 {
            let mut bad = c.clone();
            match index {
                0 => bad.schema = "tradeassembly.relay.reach-data-command/v5".into(),
                1 => bad.command.schema_version = DATA_RUNNER_SCHEMA_V5.into(),
                2 => {
                    bad.resource.schema_version =
                        "tradeassembly.core-data-mutation-binding.v5".into()
                }
                3 => bad.resource.action = "strategy.order_plan.submit".into(),
                4 => bad.resource.resource_kind = Some("strategy_order_plan".into()),
                _ => bad.resource.scope.strategy_id = "foreign".into(),
            }
            assert!(bad.validate().is_err());
        }
        let mut bad = statement;
        bad.schema = "tradeassembly.relay.reach-data-preparation/v5".into();
        assert!(bad.signing_payload().is_err());
    }
}
