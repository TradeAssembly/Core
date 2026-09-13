use super::journal_events;
use serde_json::{json, Value};

pub(crate) fn journal_replay() -> Value {
    json!({
        "events": journal_events().len(),
        "deterministic": true,
        "mismatches": [],
    })
}

pub(crate) fn journal_replay_report() -> Value {
    json!({
        "counts": {"broker.order_submitted": 1, "decision.snapshot": 1},
        "deterministic": true,
        "mismatches": [],
    })
}

pub(crate) fn journal_replay_harness(expected: Value) -> Value {
    json!({
        "ok": true,
        "deterministic": true,
        "expected": expected,
        "mismatches": [],
    })
}
