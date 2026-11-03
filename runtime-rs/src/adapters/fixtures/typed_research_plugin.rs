//! Authored 1-second stream/counter fixture. No real market or indicator formula.
//! Each acquisition returns up to eight exact requested completed intervals. State is
//! entirely supplied/committed by Core; this executable has no durable storage.
use serde_json::{json, Value};
use std::{
    collections::BTreeMap,
    io::BufRead,
    time::{SystemTime, UNIX_EPOCH},
};
use tradeassembly_plugin_sdk::{data_contract::*, data_evaluation::*};
fn hash(v: &Value) -> String {
    tradeassembly_plugin_sdk::canonical::canonical_sha256(v).unwrap()
}
fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64
}
fn main() {
    let mut line = String::new();
    std::io::stdin().lock().read_line(&mut line).unwrap();
    let request: Value = serde_json::from_str(&line).unwrap();
    assert!(matches!(
        request["context"]["mode"].as_str(),
        Some("paper" | "backtest")
    ));
    assert!(request["ephemeralCredentialGrant"].is_null());
    let payload = &request["payload"]["payload"];
    let operation = request["metadata"]["operationId"].as_str().unwrap();
    let (result, schema) = if operation == "research_acquire" {
        let feed = payload["feed"].as_str().unwrap();
        assert!(matches!(
            feed,
            "synthetic_typed_research_v1" | "synthetic_typed_crypto_spot_v1"
        ));
        let start = payload["start"].as_i64().unwrap();
        let end = payload["end"].as_i64().unwrap();
        let origin = payload["origin"].as_i64().unwrap();
        let cutoff = payload["cutoff"].as_i64().unwrap();
        assert_eq!(payload["interval"], 1000);
        assert!(origin > 0 && origin <= start && end > start);
        assert_eq!((start - origin) % 1000, 0);
        assert!((1000..=8000).contains(&(end - start)));
        assert_eq!((end - start) % 1000, 0);
        assert!(end <= cutoff && cutoff <= now());
        let instrument = payload["instrument"].as_str().unwrap();
        assert!(matches!(
            (instrument, feed),
            ("SPY" | "B", "synthetic_typed_research_v1")
                | ("BTC/USD", "synthetic_typed_crypto_spot_v1")
        ));
        // Actual host acquisition time stays separate from historical event time.
        let values = (0..((end - start) / 1000))
            .map(|index| 100 + (start - origin) / 1000 + index)
            .collect::<Vec<_>>();
        let snapshot = json!({"instrument":instrument,"start":start,"origin":origin,"interval":1000,
            "observedAtMs":now(),"values":values,"feed":feed});
        (
            json!({"snapshot":snapshot,"digest":hash(&snapshot)}),
            "schema://fixture-typed-research/acquisition@1",
        )
    } else {
        let (evaluation, _) = decode_operation_payload(payload.clone()).unwrap();
        assert_eq!(evaluation.instruments.len(), 1);
        let mut state = BTreeMap::new();
        let points = if operation == "research_replay" {
            assert!(evaluation.inputs.is_empty() && evaluation.prior_state.is_empty());
            let snapshot = &evaluation.parameters["snapshot"];
            assert_eq!(hash(snapshot), evaluation.parameters["digest"]);
            let feed = snapshot["feed"].as_str().unwrap();
            assert!(matches!(
                (snapshot["instrument"].as_str().unwrap(), feed),
                ("SPY" | "B", "synthetic_typed_research_v1")
                    | ("BTC/USD", "synthetic_typed_crypto_spot_v1")
            ));
            assert_eq!(snapshot["interval"], 1000);
            let start = snapshot["start"].as_i64().unwrap();
            let values = snapshot["values"].as_array().unwrap();
            assert!(!values.is_empty() && values.len() <= 8);
            let captured_end = start + values.len() as i64 * 1000;
            let from = evaluation.window.event_start_ms;
            let to = evaluation.window.event_end_ms;
            assert!(start <= from && from < to && to <= captured_end);
            assert_eq!((from - start) % 1000, 0);
            assert_eq!((to - start) % 1000, 0);
            assert_eq!(evaluation.instruments[0], snapshot["instrument"]);
            let observed = snapshot["observedAtMs"].as_i64().unwrap();
            assert!(captured_end <= observed && observed <= evaluation.window.as_of_ms);
            // Replay the exact intersection, never the whole batch repeatedly.
            (from..to)
                .step_by(1000)
                .map(|event_start| {
                    let end = event_start + 1000;
                    let mut point = DataPoint {
                        instrument_id: evaluation.instruments[0].clone(),
                        source_event_id: format!("{feed}:{}:{end}", evaluation.instruments[0]),
                        result: PointValue::Ready(
                            values[((event_start - start) / 1000) as usize].clone(),
                        ),
                        provenance: BatchProvenance {
                            content_sha256: String::new(),
                            upstream_content_sha256: vec![],
                            event_start_ms: event_start,
                            event_end_ms: end,
                            known_at_ms: observed,
                            arrived_at_ms: observed,
                            completed: true,
                            source_revision: "synthetic_typed_research_v1".into(),
                        },
                    };
                    point.provenance.content_sha256 = point_sha256(&point).unwrap();
                    point
                })
                .collect()
        } else {
            assert_eq!(operation, "research_counter");
            assert_eq!(evaluation.inputs["source"].points.len(), 1);
            let mut point = evaluation.inputs["source"].points[0].clone();
            assert!(matches!(point.result, PointValue::Ready(_)));
            let prior = evaluation
                .prior_state
                .get(&point.instrument_id)
                .map(|v| v["seen"].as_u64().unwrap())
                .unwrap_or(0);
            let seen = prior + 1;
            // This deliberately simple publisher-owned stateful numeric rule is
            // a host continuity oracle, not MA/TSI or financial advice.
            point.result = if seen < 2 {
                PointValue::WarmingUp
            } else {
                PointValue::Ready(json!(if (4..=5).contains(&seen) { -100 } else { 100 }))
            };
            point.source_event_id = format!("synthetic_counter:{seen}:{}", point.source_event_id);
            point.provenance.upstream_content_sha256 =
                vec![point.provenance.content_sha256.clone()];
            point.provenance.content_sha256 = point_sha256(&point).unwrap();
            state.insert(point.instrument_id.clone(), json!({"seen":seen}));
            vec![point]
        };
        let output = DataEvaluationResponse {
            schema_version: EvaluationVersion::V1,
            state,
            outputs: BTreeMap::from([(
                "value".into(),
                DataBatch {
                    schema: SchemaIdentity {
                        schema_ref: "schema://test/series@1".into(),
                        sha256: hash(&json!({"type":"array","items":{"type":"number"}})),
                    },
                    shape: DataShape::Series,
                    points,
                },
            )]),
        };
        (
            serde_json::to_value(output).unwrap(),
            EVALUATION_RESPONSE_SCHEMA,
        )
    };
    println!(
        "{}",
        json!({"schemaVersion":"1","requestId":request["requestId"],"status":"succeeded",
        "responseSchema":schema,"payload":result,"providerOutcome":{"code":"synthetic_observed"},"reconciliation":"not_required"})
    );
}
