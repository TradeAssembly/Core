use serde_json::json;
use tradeassembly_runtime::ports::AuthorityContext;
use tradeassembly_runtime::robustness_contracts::{
    RobustnessBudget, RobustnessResult, RobustnessResultContent, RobustnessRunManifest,
    RobustnessRunManifestContent, RobustnessSourceBinding, RobustnessStudyKind,
    ROBUSTNESS_MANIFEST_SCHEMA, ROBUSTNESS_RESULT_SCHEMA,
};
use tradeassembly_runtime::robustness_engine::{
    MonteCarloInput, MonteCarloMode, StudyInput, ROBUSTNESS_ENGINE_VERSION,
};

// The aggregate slice owns the public lib.rs wiring. Re-export the runtime modules so this
// isolated integration test compiles the new adapter without changing that shared file.
mod backtest_contracts {
    pub use tradeassembly_runtime::backtest_contracts::*;
}
mod backtest_report {
    pub use tradeassembly_runtime::backtest_report::*;
}
mod domain {
    pub use tradeassembly_runtime::domain::*;
}
mod historical_data {
    pub use tradeassembly_runtime::historical_data::*;
}
mod robustness_contracts {
    pub use tradeassembly_runtime::robustness_contracts::*;
}
mod robustness_engine {
    pub use tradeassembly_runtime::robustness_engine::*;
}

#[path = "../src/robustness_projection.rs"]
#[allow(dead_code)]
mod projection;

fn source() -> RobustnessSourceBinding {
    RobustnessSourceBinding {
        source_run_id: "backtest-1".to_string(),
        source_manifest_hash: "sha256:manifest".to_string(),
        source_result_hash: "sha256:result".to_string(),
        source_report_hash: "sha256:report".to_string(),
        source_dataset_id: "dataset-1".to_string(),
        source_dataset_hash: "sha256:dataset".to_string(),
    }
}

fn assumptions() -> serde_json::Value {
    serde_json::to_value(StudyInput::MonteCarlo(MonteCarloInput {
        path_count: 2,
        confidence_level: 0.9,
        ruin_equity_ratio: 0.5,
        mode: MonteCarloMode::TradeBootstrap,
    }))
    .expect("typed assumptions")
}

fn manifest() -> RobustnessRunManifest {
    RobustnessRunManifest::build(
        RobustnessRunManifestContent {
            schema: ROBUSTNESS_MANIFEST_SCHEMA.to_string(),
            run_id: "robustness-1".to_string(),
            request_hash: "sha256:request".to_string(),
            idempotency_key: "request-1".to_string(),
            study_kind: RobustnessStudyKind::MonteCarlo,
            engine_version: ROBUSTNESS_ENGINE_VERSION.to_string(),
            deterministic_seed: 7,
            source: source(),
            supporting_sources: Vec::new(),
            assumptions: assumptions(),
            budget: RobustnessBudget {
                maximum_samples: 100,
                maximum_grid_points: 10,
                maximum_windows: 10,
                maximum_scenarios: 10,
                maximum_output_bytes: 100_000,
                maximum_attempts: 1,
            },
            authority: AuthorityContext::local_cli(),
            client: "test".to_string(),
            purpose: "research".to_string(),
            first_attempt_id: "robustness-1:attempt:1".to_string(),
        },
        1,
    )
    .expect("manifest")
}

#[test]
fn typed_study_assumptions_are_stable_and_match_the_declared_kind() {
    let first = projection::parse_study(&manifest()).expect("study");
    let second = projection::parse_study(&manifest()).expect("study");
    assert_eq!(first, second);
}

#[test]
fn study_kind_mismatch_fails_closed() {
    let mut value = manifest();
    value.content.study_kind = RobustnessStudyKind::Stress;
    value.manifest_hash = RobustnessRunManifest::build(value.content.clone(), value.created_at_ms)
        .expect("rebuilt manifest")
        .manifest_hash;
    assert_eq!(
        projection::parse_study(&value).expect_err("mismatch"),
        "robustness_study_kind_mismatch"
    );
}

#[test]
fn tampered_manifest_is_rejected_before_projection() {
    let mut value = manifest();
    value.content.assumptions = json!({"study": "stress", "input": {"scenarios": []}});
    assert_eq!(
        value.verify().expect_err("tampered"),
        "robustness_manifest_integrity_failed"
    );
}

#[test]
fn source_binding_requires_all_exact_fields() {
    let mut value = source();
    value.source_dataset_hash.clear();
    assert!(!value.validate());
}

#[test]
fn declared_budget_maps_deterministically_without_expansion() {
    let first = projection::map_budget(&manifest()).expect("budget");
    let second = projection::map_budget(&manifest()).expect("budget");
    assert_eq!(first, second);
    assert_eq!(first.max_samples, 100);
}

#[test]
fn public_evidence_is_typed_and_preserves_all_source_hashes() {
    let manifest = manifest();
    let result = RobustnessResult::build(
        RobustnessResultContent {
            schema: ROBUSTNESS_RESULT_SCHEMA.to_string(),
            run_id: manifest.content.run_id.clone(),
            manifest_hash: manifest.manifest_hash.clone(),
            source: manifest.content.source.clone(),
            engine_version: ROBUSTNESS_ENGINE_VERSION.to_string(),
            output: json!({"summary": {"paths": 2}}),
            diagnostics: Vec::new(),
        },
        2,
    )
    .expect("result");
    let evidence = projection::public_evidence(&manifest, &result).expect("evidence");
    assert_eq!(evidence.study_kind, RobustnessStudyKind::MonteCarlo);
    assert!(evidence.measures.contains("simulated outcomes"));
    assert!(evidence
        .limitations
        .iter()
        .any(|value| value.contains("not overfit validation")));
    assert!(evidence
        .limitations
        .iter()
        .any(|value| value.contains("No unavailable holdout")));
    assert_eq!(evidence.manifest_hash, manifest.manifest_hash);
    assert_eq!(evidence.result_hash, result.result_hash);
    assert_eq!(evidence.source.source_run_id, "backtest-1");
    assert_eq!(evidence.source.source_manifest_hash, "sha256:manifest");
    assert_eq!(evidence.source.source_result_hash, "sha256:result");
    assert_eq!(evidence.source.source_report_hash, "sha256:report");
    assert_eq!(evidence.source.source_dataset_hash, "sha256:dataset");
}
