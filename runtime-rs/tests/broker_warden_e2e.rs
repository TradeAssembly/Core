//! Actual C5 transport proof. Does not yet prove agent/sandbox/broker dispatch.
use tradeassembly_runtime as runtime_crate;
#[path = "common/controlled_warden.rs"]
mod controlled_warden;

use serde_json::json;
use tradeassembly_runtime::control_plane::ControlPlaneCommandEnvelope;
use tradeassembly_runtime::finance_authority::{
    FinanceAuthorityPort, DECISION_NS, OUTCOME_NS, RECEIPT_NS,
};
use tradeassembly_runtime::ports::{AuthorityContext, IdempotencyKey};
use tradeassembly_runtime::service::TradeAssemblyService;

#[test]
#[ignore = "requires explicitly selected real F2_TEST_WARDEN_BINARY"]
fn real_c5_denies_then_allows_isolated_submission_and_records_outcome() {
    let root = tempfile::tempdir().unwrap();
    let root_path = root.path().canonicalize().unwrap();
    let mut warden = controlled_warden::ControlledWarden::start(&root_path);
    let service =
        TradeAssemblyService::test_local(root_path.join("test-runtime.db").to_string_lossy());
    let storage = service.runtime().storage.clone();
    let mut envelope = ControlPlaneCommandEnvelope {
        schema_version: "tradeassembly.control_plane.command.v1".into(),
        command_id: "controlled-denied".into(),
        correlation_id: "controlled-denied".into(),
        command_name: "order.submit.live".into(),
        command_group: "order".into(),
        source_interface: "broker_boundary".into(),
        target_object: Some("account://controlled/fixture".into()),
        side_effect_class: "live_order".into(),
        authority: AuthorityContext {
            actor: "controlled-fixture".into(),
            surface: "broker_boundary".into(),
            account_mode: "live".into(),
        },
        idempotency_key: IdempotencyKey::new("controlled-denied").unwrap(),
        idempotency_requirement: "required".into(),
        expected_sequence: None,
        evidence_refs: vec![],
        payload_hash: tradeassembly_runtime::spec::canonical_hash(&json!({"testOnly":true}))
            .unwrap(),
        payload_preview: json!({"testOnly":true}),
    };
    assert_eq!(
        warden
            .authority
            .prepare_broker_submission(storage.as_ref(), &envelope)
            .unwrap_err(),
        "denied:broker_submission_policy"
    );
    assert_eq!(
        storage
            .get_json(DECISION_NS, &envelope.command_id)
            .unwrap()
            .unwrap()["decision"]["decision"],
        "deny"
    );
    warden.allow_controlled_submission();
    envelope.command_id = "controlled-allowed".into();
    envelope.correlation_id = "controlled-allowed".into();
    envelope.idempotency_key = IdempotencyKey::new("controlled-allowed").unwrap();
    let admitted = warden
        .authority
        .prepare_broker_submission(storage.as_ref(), &envelope);
    assert!(
        admitted.is_ok(),
        "fixture authorization {admitted:?}; decision {:?}",
        storage
            .get_json(DECISION_NS, &envelope.command_id)
            .unwrap()
            .map(|value| value["decision"].clone())
    );
    let decision = storage
        .get_json(DECISION_NS, &envelope.command_id)
        .unwrap()
        .unwrap();
    assert_eq!(decision["decision"]["decision"], "allow");
    assert_eq!(
        decision["request"]["pep_id"],
        "pep-tradeassembly-broker-submission"
    );
    assert!(storage
        .get_json(RECEIPT_NS, &envelope.command_id)
        .unwrap()
        .is_some());
    warden
        .authority
        .complete_broker_submission(storage.as_ref(), &envelope, 202)
        .unwrap();
    assert!(storage
        .get_json(OUTCOME_NS, &envelope.command_id)
        .unwrap()
        .is_some());
    let mut altered = envelope.clone();
    altered.payload_hash.push_str("-changed");
    assert!(warden
        .authority
        .complete_broker_submission(storage.as_ref(), &altered, 202)
        .is_err());
}
