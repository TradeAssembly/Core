// Copyright (c) 2026 OptionLab LLC. All rights reserved.

use crate::backtest_contracts::canonical_hash;
use crate::domain::RobustnessRunState;
use crate::ports::AuthorityContext;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

pub const ROBUSTNESS_MANIFEST_SCHEMA: &str = "tradeassembly.robustness-run-manifest.v1";
pub const ROBUSTNESS_RESULT_SCHEMA: &str = "tradeassembly.robustness-result.v1";
pub const ROBUSTNESS_RUN_SCHEMA: &str = "tradeassembly.robustness-run.v1";
pub const ROBUSTNESS_ATTEMPT_SCHEMA: &str = "tradeassembly.robustness-attempt.v1";
pub const ROBUSTNESS_PUBLIC_EVIDENCE_SCHEMA: &str = "tradeassembly.robustness-public-evidence.v1";

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RobustnessStudyKind {
    MonteCarlo,
    ParameterSensitivity,
    SlippageSensitivity,
    FeeSensitivity,
    ExecutionModelSensitivity,
    WalkForward,
    Regime,
    Stress,
    CapacityLiquidity,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RobustnessSourceBinding {
    pub source_run_id: String,
    pub source_manifest_hash: String,
    pub source_result_hash: String,
    pub source_report_hash: String,
    pub source_dataset_id: String,
    pub source_dataset_hash: String,
}

/// Stable, public-facing description of what a robustness result measures.
///
/// This is deliberately built from the typed manifest/study inputs and source
/// bindings. It does not infer claims from engine output or natural language.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RobustnessPublicEvidence {
    pub schema: String,
    pub study_kind: RobustnessStudyKind,
    pub measures: String,
    pub limitations: Vec<String>,
    pub manifest_hash: String,
    pub result_hash: String,
    pub source: RobustnessSourceBinding,
    #[serde(default)]
    pub supporting_sources: Vec<RobustnessSourceBinding>,
}

impl RobustnessSourceBinding {
    pub fn validate(&self) -> bool {
        [
            &self.source_run_id,
            &self.source_manifest_hash,
            &self.source_result_hash,
            &self.source_report_hash,
            &self.source_dataset_id,
            &self.source_dataset_hash,
        ]
        .iter()
        .all(|value| !value.trim().is_empty())
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RobustnessBudget {
    pub maximum_samples: u32,
    pub maximum_grid_points: u32,
    pub maximum_windows: u32,
    pub maximum_scenarios: u32,
    pub maximum_output_bytes: u64,
    pub maximum_attempts: u32,
}

impl RobustnessBudget {
    pub fn validate(&self) -> bool {
        self.maximum_samples > 0
            && self.maximum_grid_points > 0
            && self.maximum_windows > 0
            && self.maximum_scenarios > 0
            && self.maximum_output_bytes > 0
            && self.maximum_attempts > 0
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RobustnessRunManifestContent {
    pub schema: String,
    pub run_id: String,
    pub request_hash: String,
    pub idempotency_key: String,
    pub study_kind: RobustnessStudyKind,
    pub engine_version: String,
    pub deterministic_seed: u64,
    pub source: RobustnessSourceBinding,
    #[serde(default)]
    pub supporting_sources: Vec<RobustnessSourceBinding>,
    pub assumptions: serde_json::Value,
    pub budget: RobustnessBudget,
    pub authority: AuthorityContext,
    pub client: String,
    pub purpose: String,
    pub first_attempt_id: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RobustnessRunManifest {
    pub manifest_hash: String,
    pub manifest_ref: String,
    pub byte_length: u64,
    pub created_at_ms: i64,
    pub content: RobustnessRunManifestContent,
}

impl RobustnessRunManifest {
    pub fn build(
        content: RobustnessRunManifestContent,
        created_at_ms: i64,
    ) -> Result<Self, String> {
        if content.schema != ROBUSTNESS_MANIFEST_SCHEMA
            || !content.source.validate()
            || !valid_supporting_sources(&content.source, &content.supporting_sources)
            || !content.budget.validate()
            || content.run_id.trim().is_empty()
            || content.request_hash.trim().is_empty()
            || content.idempotency_key.trim().is_empty()
            || content.engine_version.trim().is_empty()
            || content.client.trim().is_empty()
            || content.purpose.trim().is_empty()
            || content.first_attempt_id.trim().is_empty()
            || !content.assumptions.is_object()
        {
            return Err("robustness_manifest_invalid".to_string());
        }
        let bytes = serde_json_canonicalizer::to_vec(&content)
            .map_err(|_| "robustness_manifest_serialization_failed".to_string())?;
        let digest = format!("{:x}", Sha256::digest(&bytes));
        Ok(Self {
            manifest_hash: format!("sha256:{digest}"),
            manifest_ref: format!("tradeassembly://robustness/{}/manifest", content.run_id),
            byte_length: bytes.len() as u64,
            created_at_ms,
            content,
        })
    }

    pub fn verify(&self) -> Result<(), String> {
        let rebuilt = Self::build(self.content.clone(), self.created_at_ms)
            .map_err(|_| "robustness_manifest_integrity_failed".to_string())?;
        if rebuilt.manifest_hash != self.manifest_hash
            || rebuilt.manifest_ref != self.manifest_ref
            || rebuilt.byte_length != self.byte_length
        {
            return Err("robustness_manifest_integrity_failed".to_string());
        }
        Ok(())
    }
}

fn valid_supporting_sources(
    primary: &RobustnessSourceBinding,
    supporting: &[RobustnessSourceBinding],
) -> bool {
    let mut run_ids = std::collections::BTreeSet::new();
    run_ids.insert(primary.source_run_id.as_str());
    supporting
        .iter()
        .all(|source| source.validate() && run_ids.insert(source.source_run_id.as_str()))
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RobustnessAttemptState {
    Queued,
    Running,
    Completed,
    Failed,
    Canceled,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RobustnessAttempt {
    pub schema: String,
    pub attempt_id: String,
    pub run_id: String,
    pub ordinal: u32,
    pub state: RobustnessAttemptState,
    pub lease_owner: Option<String>,
    pub lease_until_ms: Option<i64>,
    pub fencing_token: i64,
    pub heartbeat_at_ms: Option<i64>,
    pub created_at_ms: i64,
    pub updated_at_ms: i64,
    pub failure_code: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RobustnessLifecycleEvent {
    pub event_id: String,
    pub run_id: String,
    pub sequence: i64,
    pub event_type: String,
    pub from_state: RobustnessRunState,
    pub to_state: RobustnessRunState,
    pub attempt_id: String,
    pub occurred_at_ms: i64,
    pub previous_event_hash: Option<String>,
    pub event_hash: String,
    pub detail: String,
}

impl RobustnessLifecycleEvent {
    pub fn verify_hash(&self) -> Result<(), String> {
        let hash = canonical_hash(
            &serde_json::json!({
                "runId": self.run_id, "sequence": self.sequence, "from": self.from_state,
                "to": self.to_state, "attemptId": self.attempt_id, "previous": self.previous_event_hash,
                "detail": self.detail
            }),
            "robustness_event_hash_failed",
        )?;
        if self.event_hash != hash {
            return Err("robustness_lifecycle_event_integrity_failed".to_string());
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RobustnessResultContent {
    pub schema: String,
    pub run_id: String,
    pub manifest_hash: String,
    pub source: RobustnessSourceBinding,
    pub engine_version: String,
    pub output: serde_json::Value,
    pub diagnostics: Vec<serde_json::Value>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RobustnessResult {
    pub result_hash: String,
    pub result_ref: String,
    pub byte_length: u64,
    pub created_at_ms: i64,
    pub content: RobustnessResultContent,
}

impl RobustnessResult {
    pub fn build(content: RobustnessResultContent, created_at_ms: i64) -> Result<Self, String> {
        if content.schema != ROBUSTNESS_RESULT_SCHEMA
            || content.run_id.trim().is_empty()
            || content.manifest_hash.trim().is_empty()
            || content.engine_version.trim().is_empty()
            || !content.source.validate()
            || !content.output.is_object()
        {
            return Err("robustness_result_invalid".to_string());
        }
        let bytes = serde_json_canonicalizer::to_vec(&content)
            .map_err(|_| "robustness_result_serialization_failed".to_string())?;
        let digest = format!("{:x}", Sha256::digest(&bytes));
        Ok(Self {
            result_hash: format!("sha256:{digest}"),
            result_ref: format!("tradeassembly://robustness/{}/result", content.run_id),
            byte_length: bytes.len() as u64,
            created_at_ms,
            content,
        })
    }
    pub fn verify(&self) -> Result<(), String> {
        let rebuilt = Self::build(self.content.clone(), self.created_at_ms)
            .map_err(|_| "robustness_result_integrity_failed".to_string())?;
        if rebuilt != *self {
            return Err("robustness_result_integrity_failed".to_string());
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RobustnessRunRecord {
    pub schema: String,
    pub run_id: String,
    pub manifest_hash: String,
    pub request_hash: String,
    pub idempotency_key: String,
    pub state: RobustnessRunState,
    pub sequence: i64,
    pub current_attempt_id: String,
    pub fencing_token: i64,
    pub result_hash: Option<String>,
    pub result: Option<RobustnessResult>,
    pub failure_code: Option<String>,
    pub events: Vec<RobustnessLifecycleEvent>,
    pub created_at_ms: i64,
    pub updated_at_ms: i64,
}
