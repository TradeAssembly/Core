// Copyright (c) 2026 OptionLab LLC. All rights reserved.

use super::*;
use serde_json::{json, Value};
fn fixture(name: &str) -> Value {
    serde_json::from_str(
        &std::fs::read_to_string(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("tests/fixtures/cash_net_v1")
                .join(name),
        )
        .unwrap(),
    )
    .unwrap()
}
fn request() -> Request {
    decode_request_json(&serde_json::to_vec(&fixture("cash-request.json")).unwrap()).unwrap()
}
fn check(v: &Value) -> Result<Observation, String> {
    decode_observation_json(&serde_json::to_vec(v).unwrap(), &request(), 1200)
}
#[test]
fn cash_capacity_known_values_do_not_establish_debt_absence_or_permission() {
    let q = request();
    q.validate_binding(OPERATION, CAPABILITY, PURPOSE).unwrap();
    let o = check(&fixture("cash-known-debt-unavailable.json")).unwrap();
    assert_eq!(
        o.reported_cash_ceiling().unwrap(),
        "80".parse::<Decimal>().unwrap()
    );
    assert!(o
        .account
        .required()
        .unwrap()
        .borrowing_debt
        .required()
        .is_err());
    assert!(
        serde_json::from_value::<crate::execution_snapshot::AccountSnapshot>(
            serde_json::to_value(&o).unwrap()
        )
        .is_err()
    );
    for key in [
        "allow",
        "ready",
        "accountComplete",
        "cashReconciled",
        "debtFree",
        "executionAuthorityGranted",
    ] {
        assert!(serde_json::to_value(&o).unwrap().get(key).is_none());
    }
    let mut wrong = fixture("cash-request.json");
    wrong["expectedAccount"]["providerAccountId"] = json!("changed");
    assert!(decode_request_json(&serde_json::to_vec(&wrong).unwrap()).is_err());
    assert!(q
        .validate_binding("account.cash_capacity.paper_v2", CAPABILITY, PURPOSE)
        .is_err());
    assert!(q
        .validate_binding(OPERATION, CAPABILITY, "recovery")
        .is_err());
}
#[test]
fn cash_capacity_unavailable_and_negative_values_never_default_to_zero() {
    let o = check(&fixture("cash-unavailable.json")).unwrap();
    assert!(o.reported_cash_ceiling().is_err());
    let mut negative = fixture("cash-known-debt-unavailable.json");
    negative["account"]["value"]["cashBalance"]["value"] = json!("-1");
    let negative = check(&negative).unwrap();
    assert_eq!(
        *negative
            .account
            .required()
            .unwrap()
            .cash_balance
            .required()
            .unwrap(),
        "-1".parse::<Decimal>().unwrap()
    );
    assert!(negative.reported_cash_ceiling().is_err());
    let mut unknown = fixture("cash-known-debt-unavailable.json");
    unknown["account"]["value"]["productEnabled"] =
        json!({"status":"unavailable","reason":"not_observed"});
    assert!(
        check(&unknown).is_err(),
        "positive actions need actual product evidence"
    );
    unknown["account"]["value"]["productActions"] =
        json!({"status":"unavailable","reason":"permission_not_proven"});
    assert!(check(&unknown)
        .unwrap()
        .account
        .required()
        .unwrap()
        .product_actions
        .required()
        .is_err());
}
#[test]
fn cash_capacity_closed_fields_enums_and_exact_decimal_bounds() {
    for field in [
        "policy",
        "allow",
        "borrowingDebtAssumedZero",
        "exclusiveControl",
    ] {
        let mut v = fixture("cash-known-debt-unavailable.json");
        v[field] = Value::Null;
        assert!(check(&v).is_err());
    }
    let mut too_precise = fixture("cash-known-debt-unavailable.json");
    too_precise["account"]["value"]["cashBalance"]["value"] = json!("1.0000000000001");
    assert!(check(&too_precise).is_err());
    let mut enum_alias = fixture("cash-known-debt-unavailable.json");
    enum_alias["status"] = json!({"provisional":null});
    assert!(check(&enum_alias).is_err());
    let mut short = fixture("cash-request.json");
    short["positionAction"] = json!("sell_to_open");
    assert!(decode_request_json(&serde_json::to_vec(&short).unwrap()).is_err());
    assert!(decode_request_json(&vec![b' '; MAX_REQUEST_BYTES + 1]).is_err());
    assert!(
        decode_observation_json(&vec![b' '; MAX_OBSERVATION_BYTES + 1], &request(), 1200).is_err()
    );
}
#[test]
fn cash_capacity_read_evidence_is_typed_bounded_and_causal() {
    for change in 0..5 {
        let mut v = fixture("cash-known-debt-unavailable.json");
        match change {
            0 => v["account"]["value"]["sample"]["evidenceIds"] = json!(["missing"]),
            1 => v["account"]["value"]["sample"]["evidenceIds"] = json!(["m"]),
            2 => v["evidence"][0]["completedAtMs"] = json!(1201),
            3 => {
                v["evidence"][0]["providerOrderId"] =
                    json!({"status":"available","value":"unrelated_order"})
            }
            _ => v["asset"]["value"]["instrument"]["providerSymbol"] = json!("wrong"),
        }
        assert!(check(&v).is_err());
    }
    let mut v = fixture("cash-known-debt-unavailable.json");
    let first = v["evidence"][0].clone();
    v["evidence"].as_array_mut().unwrap().push(first);
    assert!(check(&v).is_err());
}
#[test]
fn cash_capacity_independently_authored_fixture_matrix_and_static_schemas() {
    let cases = fixture("cases.json");
    for case in cases.as_array().unwrap() {
        let name = case["file"].as_str().unwrap();
        let v = fixture(name);
        let result = match case["kind"].as_str().unwrap() {
            "cash_request" => decode_request_json(&serde_json::to_vec(&v).unwrap()).map(|_| ()),
            "cash_observation" => check(&v).map(|_| ()),
            _ => continue,
        };
        assert_eq!(
            result.is_ok(),
            case["semanticValid"].as_bool().unwrap(),
            "{name}: {result:?}"
        );
    }
    assert_eq!(request_schema()["$id"], REQUEST_SCHEMA);
    assert_eq!(response_schema()["$id"], RESPONSE_SCHEMA);
    assert_eq!(response_schema()["additionalProperties"], false);
    assert_eq!(
        response_schema()["properties"]["coherence"]["const"],
        "non_atomic"
    );
    assert_eq!(schema(), observation_schema());
}
