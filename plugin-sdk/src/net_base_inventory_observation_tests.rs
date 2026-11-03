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
    decode_request_json(&serde_json::to_vec(&fixture("net-request.json")).unwrap()).unwrap()
}
fn check(v: &Value) -> Result<Observation, String> {
    decode_observation_json(&serde_json::to_vec(v).unwrap(), &request(), 1200)
}
#[test]
fn net_inventory_gross_minus_direct_final_base_fee_remains_distinct_from_cash_and_holdings() {
    let q = request();
    q.validate_binding(OPERATION, CAPABILITY, PURPOSE).unwrap();
    let o = check(&fixture("net-direct-complete-final.json")).unwrap();
    assert_eq!(
        *o.gross_base_filled.required().unwrap(),
        "0.1".parse::<Decimal>().unwrap()
    );
    assert_eq!(
        *o.net_base_credited.required().unwrap(),
        "0.09975".parse::<Decimal>().unwrap()
    );
    assert_eq!(
        o.aggregate_inventory.required().unwrap().quantity,
        "10".parse::<Decimal>().unwrap()
    );
    assert_eq!(
        o.reported_available_ceiling().unwrap(),
        "0.09975".parse::<Decimal>().unwrap()
    );
    assert!(
        o.order.cash_spent.required().is_err(),
        "net base proof is not final money"
    );
    for key in [
        "ownedQuantity",
        "cashReconciled",
        "releasedHold",
        "ready",
        "authority",
        "exclusiveControl",
    ] {
        assert!(serde_json::to_value(&o).unwrap().get(key).is_none());
    }
    let mut partial = fixture("net-direct-complete-final.json");
    partial["order"]["status"] = json!("cancelled");
    partial["order"]["filledQuantity"]["value"] = json!("0.05");
    partial["grossBaseFilled"]["value"] = json!("0.05");
    partial["fills"]["value"]["records"][0]["baseQuantity"] = json!("0.05");
    partial["fees"]["value"]["records"][0]["baseQuantity"]["value"] = json!("0.000125");
    partial["feeClosure"]["value"]["coveredGrossBase"] = json!("0.05");
    partial["netBaseCredited"]["value"] = json!("0.049875");
    assert_eq!(
        check(&partial)
            .unwrap()
            .reported_available_ceiling()
            .unwrap(),
        "0.049875".parse::<Decimal>().unwrap()
    );
}
#[test]
fn net_inventory_filtered_association_and_query_exhaustion_are_not_direct_finality() {
    let filtered = check(&fixture("net-filter-associated-not-ready.json")).unwrap();
    assert!(matches!(
        &filtered.fees.required().unwrap().records[0].attribution,
        FeeAttribution::ProviderOrderFilter { .. }
    ));
    assert!(filtered.net_base_credited.required().is_err());
    assert!(filtered.reported_available_ceiling().is_err());
    let mut v = fixture("net-direct-complete-final.json");
    v["fees"]["value"]["records"] = json!([]);
    v["fees"]["value"]["coverage"]["rowsRead"] = json!(0);
    v["feeClosure"] = json!({"status":"unavailable","reason":"no_future_fee_watermark"});
    v["netBaseCredited"] = json!({"status":"unavailable","reason":"not_proven"});
    assert!(check(&v).unwrap().net_base_credited.required().is_err());
    v["netBaseCredited"] = json!({"status":"available","value":"0.1"});
    assert!(check(&v).is_err(), "empty fees cannot imply zero");
    let closure = fixture("net-direct-complete-final.json")["feeClosure"].clone();
    v["feeClosure"] = closure;
    assert_eq!(
        *check(&v).unwrap().net_base_credited.required().unwrap(),
        "0.1".parse::<Decimal>().unwrap(),
        "explicit separate final closure permits genuinely zero fees"
    );
}
#[test]
fn net_inventory_unknown_and_filled_missing_components_preserve_unavailability() {
    for file in ["net-unknown.json", "net-filled-fees-unavailable.json"] {
        let o = check(&fixture(file)).unwrap();
        assert!(o.net_base_credited.required().is_err());
        assert!(o.reported_available_ceiling().is_err());
        assert!(o.order.cash_spent.required().is_err());
    }
    let mut v = fixture("net-direct-complete-final.json");
    v["fees"]["value"]["records"][0]["baseAssetId"] =
        json!({"status":"unavailable","reason":"asset_not_proven"});
    v["netBaseCredited"] = json!({"status":"unavailable","reason":"fee_asset_unknown"});
    assert!(check(&v).is_ok());
    v["netBaseCredited"] = json!({"status":"available","value":"0.09975"});
    assert!(check(&v).is_err());
}
#[test]
fn net_inventory_wrong_identity_link_revision_quantity_and_sampling_refuse() {
    for index in 0..9 {
        let mut v = fixture("net-direct-complete-final.json");
        match index {
            0 => v["order"]["order"]["clientOrderId"] = json!("different"),
            1 => {
                v["fees"]["value"]["records"][0]["attribution"] =
                    json!({"kind":"direct_order","providerOrderId":"different"})
            }
            2 => v["feeClosure"]["value"]["correctionsComplete"] = json!(false),
            3 => v["feeClosure"]["value"]["coveredGrossBase"] = json!("0.09"),
            4 => v["fees"]["value"]["records"][0]["baseQuantity"]["value"] = json!("-0.001"),
            5 => v["feeClosure"]["value"]["sample"]["evidenceIds"] = json!(["fees"]),
            6 => {
                v["evidence"][3]["providerOrderId"] =
                    json!({"status":"unavailable","reason":"no_direct_order_link"})
            }
            7 => v["fees"]["value"]["records"][0]["evidenceIds"] = json!(["missing"]),
            _ => v["fees"]["value"]["coverage"]["rowsRead"] = json!(2),
        }
        assert!(check(&v).is_err(), "mutation {index}");
    }
    let mut v = fixture("net-direct-complete-final.json");
    let record = v["fees"]["value"]["records"][0].clone();
    v["fees"]["value"]["records"]
        .as_array_mut()
        .unwrap()
        .push(record);
    v["fees"]["value"]["coverage"]["rowsRead"] = json!(2);
    assert!(check(&v).is_err());
}
#[test]
fn net_inventory_request_is_closed_original_limit_gtc_spot_and_bounded() {
    for index in 0..6 {
        let mut q = fixture("net-request.json");
        match index {
            0 => q["exclusiveControl"] = json!(true),
            1 => q["afterCreatedAt"] = json!(1000),
            2 => q["bounds"]["maxPagesPerType"] = json!(9),
            3 => {
                q["originalEntry"]["originalSubmit"]["order"]["positionAction"] =
                    json!("sell_to_close")
            }
            4 => q["originalEntry"]["originalSubmit"]["order"]["timeInForce"] = json!("day"),
            _ => q["originalEntry"]["expectedAccount"]["environment"] = json!("live"),
        }
        assert!(decode_request_json(&serde_json::to_vec(&q).unwrap()).is_err());
    }
    let mut q = fixture("net-request.json");
    q["originalEntry"]["originalSubmit"]["order"]["quantity"] = json!("0.123456789012");
    assert_eq!(
        decode_request_json(&serde_json::to_vec(&q).unwrap())
            .unwrap()
            .original_entry
            .original_submit
            .order
            .quantity
            .to_string(),
        "0.123456789012"
    );
    q["originalEntry"]["originalSubmit"]["order"]["quantity"] = json!("0.1234567890123");
    assert!(decode_request_json(&serde_json::to_vec(&q).unwrap()).is_err());
    assert!(request()
        .validate_binding(OPERATION, CAPABILITY, "settlement")
        .is_err());
}
#[test]
fn net_inventory_authored_schema_semantic_matrix_and_old_v2_wire_remain_separate() {
    for case in fixture("cases.json").as_array().unwrap() {
        let file = case["file"].as_str().unwrap();
        let v = fixture(file);
        let result = match case["kind"].as_str().unwrap() {
            "net_request" => decode_request_json(&serde_json::to_vec(&v).unwrap()).map(|_| ()),
            "net_observation" => check(&v).map(|_| ()),
            _ => continue,
        };
        assert_eq!(
            result.is_ok(),
            case["semanticValid"].as_bool().unwrap(),
            "{file}: {result:?}"
        );
    }
    assert_eq!(request_schema()["$id"], REQUEST_SCHEMA);
    assert_eq!(response_schema()["$id"], RESPONSE_SCHEMA);
    assert_eq!(schema(), observation_schema());
    assert_eq!(response_schema()["additionalProperties"], false);
    let old = crate::settlement_observation_v2::response_schema();
    assert_eq!(
        old["properties"]["schemaVersion"]["const"],
        "tradeassembly.settlement_observation.v2"
    );
    assert!(old["properties"].get("netBaseCredited").is_none());
    assert!(
        serde_json::from_value::<crate::settlement_observation_v2::Observation>(fixture(
            "net-direct-complete-final.json"
        ))
        .is_err()
    );
    assert!(decode_request_json(&vec![b' '; MAX_REQUEST_BYTES + 1]).is_err());
}
