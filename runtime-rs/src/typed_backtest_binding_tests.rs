use super::*;
use serde_json::{json, Value};

fn legacy_bytes() -> String {
    format!(
        r#"{{"schemaVersion":"tradeassembly.typed-backtest-binding.v1","planRevisionId":"dataplan_original","planRevisionSha256":"{}","sourceCaptures":{{"source":{{"captureId":"capture_original","receiptSha256":"{}"}}}},"eventStartMs":1000,"eventEndMs":153000,"asOfMs":154000,"intervalMs":1000,"maxFramesPerStep":64,"maximumFrames":152,"quantityMicros":1000000,"initialPosition":"flat","knowledgePolicy":"retrospective_capture_cutoff","fillTimestampBasis":"event_start"}}"#,
        "a".repeat(64),
        "b".repeat(64)
    )
}
fn v2() -> Value {
    let mut value: Value = serde_json::from_str(&legacy_bytes()).unwrap();
    value["schemaVersion"] = json!("tradeassembly.typed-backtest-binding.v2");
    value["warmupObservations"] = json!(150);
    value["calendarEvidence"] = json!({"kind":"provider_bar_sessions","calendar":"synthetic","timezone":"UTC","purpose":"research_simulation_only","acknowledgeResearchAssumption":false});
    value
}
#[test]
fn typed_warmup_binding_v1_bytes_and_explicit_field_refusal_are_preserved() {
    let legacy: TypedBacktestBinding = serde_json::from_str(&legacy_bytes()).unwrap();
    assert!(legacy.validate());
    assert_eq!(legacy.no_trade_warmup_observations(), 0);
    assert_eq!(serde_json::to_string(&legacy).unwrap(), legacy_bytes());
    for value in [json!(0), json!(150), Value::Null] {
        let mut raw: Value = serde_json::from_str(&legacy_bytes()).unwrap();
        raw["warmupObservations"] = value;
        assert!(serde_json::from_value::<TypedBacktestBinding>(raw).is_err());
    }
}
#[test]
fn typed_warmup_binding_v2_requires_bounded_explicit_count_and_binds_hash() {
    let raw = v2();
    let binding: TypedBacktestBinding = serde_json::from_value(raw.clone()).unwrap();
    assert!(binding.validate());
    assert_eq!(binding.no_trade_warmup_observations(), 150);
    assert_eq!(serde_json::to_value(&binding).unwrap(), raw);
    let original = canonical_hash(&binding, "fixture_hash").unwrap();
    let mut changed = binding.clone();
    changed.warmup_observations = Some(149);
    assert_ne!(canonical_hash(&changed, "fixture_hash").unwrap(), original);
    for count in [152, 256, u64::MAX] {
        let mut bad = raw.clone();
        bad["warmupObservations"] = json!(count);
        assert!(!serde_json::from_value::<TypedBacktestBinding>(bad)
            .unwrap()
            .validate());
    }
    for value in [Value::Null, json!(-1), json!(1.5), json!("150")] {
        let mut bad = raw.clone();
        bad["warmupObservations"] = value;
        assert!(serde_json::from_value::<TypedBacktestBinding>(bad).is_err());
    }
    for field in ["calendarEvidence", "warmupObservations"] {
        let mut missing = raw.clone();
        missing.as_object_mut().unwrap().remove(field);
        assert!(
            serde_json::from_value::<TypedBacktestBinding>(missing).is_err(),
            "{field}"
        );
    }
    let mut null_calendar = raw.clone();
    null_calendar["calendarEvidence"] = Value::Null;
    assert!(serde_json::from_value::<TypedBacktestBinding>(null_calendar).is_err());
    for value in [Value::Null, raw["calendarEvidence"].clone()] {
        let mut legacy: Value = serde_json::from_str(&legacy_bytes()).unwrap();
        legacy["calendarEvidence"] = value;
        assert!(serde_json::from_value::<TypedBacktestBinding>(legacy).is_err());
    }
    let mut missing = raw.clone();
    missing
        .as_object_mut()
        .unwrap()
        .remove("warmupObservations");
    assert!(serde_json::from_value::<TypedBacktestBinding>(missing).is_err());
    let mut unknown = raw;
    unknown["callerSignals"] = json!([true]);
    assert!(serde_json::from_value::<TypedBacktestBinding>(unknown).is_err());
}

#[test]
fn typed_calendar_evidence_is_closed_research_only_and_part_of_binding_hash() {
    let raw = v2();
    let binding: TypedBacktestBinding = serde_json::from_value(raw.clone()).unwrap();
    let original = canonical_hash(&binding, "fixture_hash").unwrap();
    let mut changed = binding.clone();
    changed.calendar_evidence.as_mut().unwrap().calendar = "different".into();
    assert_ne!(canonical_hash(&changed, "fixture_hash").unwrap(), original);
    for (field, value) in [
        ("kind", json!("infer")),
        ("purpose", json!("paper_trading")),
        ("tradingEligibilityGranted", json!(true)),
    ] {
        let mut wrong = raw.clone();
        wrong["calendarEvidence"][field] = value;
        assert!(serde_json::from_value::<TypedBacktestBinding>(wrong).is_err());
    }
    let mut unacknowledged = raw;
    unacknowledged["calendarEvidence"]["kind"] = json!("declared_dataset_calendar");
    assert!(
        !serde_json::from_value::<TypedBacktestBinding>(unacknowledged)
            .unwrap()
            .validate()
    );
}
