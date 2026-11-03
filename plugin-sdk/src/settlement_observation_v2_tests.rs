use super::*;
fn unavailable() -> Value {
    json!({"status":"unavailable","reason":"not_evidenced"})
}
fn request(role: Role) -> Request {
    serde_json::from_value(json!({"schemaVersion":REQUEST_VERSION,"role":role,
        "hostAccountRef":"account://fixture/fixture-account",
        "lookup":{"schemaVersion":"tradeassembly.broker_lookup.v3",
            "expectedAccount":{"brokerNamespace":"fixture.broker","environment":"paper","providerAccountId":"fixture-account"},
            "originalSubmit":{"schemaVersion":"tradeassembly.broker_submit.v2","planId":format!("orderplan_{}","a".repeat(64)),
                "canonicalAccountSha256":"794362c12a3edb69c64643c583b7fe562f0db4fbf1a3028809fd56f8fdd5d7ad",
                "order":{"schemaVersion":"tradeassembly.broker_order.v2","instrument":{"instrumentId":"opaque.host","providerSymbol":"FIX",
                    "assetClass":"equity","instrumentFamily":"equity","quantityUnit":"shares","multiplier":"1","quoteCurrency":"USD"},
                    "positionAction":if role==Role::Entry{"buy_to_open"}else{"sell_to_close"},"quantity":"0.123456789012",
                    "pricing":{"kind":"limit","limitPrice":"12.000000000001"},"timeInForce":"day","clientOrderId":"original.client"}}}
    })).unwrap()
}
fn pending(request: &Request) -> Observation {
    serde_json::from_value(json!({"schemaVersion":OBSERVATION_VERSION,"requestSha256":request.request_sha256().unwrap(),
        "status":"pending","coherence":"non_atomic",
        "order":{"schemaVersion":"tradeassembly.broker_order_observation.v2",
            "canonicalAccountSha256":request.lookup.original_submit.canonical_account_sha256,
            "order":request.lookup.original_submit.order,"providerOrderId":null,"status":"unknown",
            "filledQuantity":unavailable(),"cashSpent":unavailable(),"observedAtMs":1000,"knownAtMs":1001},
        "averageFillPrice":unavailable(),"feeCash":unavailable(),"feeFinality":unavailable(),"account":unavailable(),"evidence":[]
    })).unwrap()
}
fn evidence(id: &str, kind: EvidenceKind, start: i64, end: i64) -> ProviderSourceEvidence {
    serde_json::from_value(json!({"id":id,"kind":kind,"providerRecordId":unavailable(),
        "hashBasis":"raw_response_bytes","responseSha256":"b".repeat(64),"startedAtMs":start,"completedAtMs":end})).unwrap()
}
fn account_value(request: &Request) -> NonAtomicAccountSample {
    serde_json::from_value(json!({"snapshot":{"schemaVersion":"tradeassembly.execution_account_snapshot.v1",
        "accountRef":request.host_account_ref,"providerAccountId":request.lookup.expected_account.provider_account_id,"mode":"paper",
        "currency":{"code":"USD","decimals":2},"time":{"observedAtMs":1500,"knownAtMs":1501,"basis":"host_acquired"},
        "coverage":{"accountComplete":true,"positionsComplete":false,"openOrdersComplete":false},
        "cashBalance":{"status":"available","value":"999.125"},"nonMarginableBuyingPower":unavailable(),
        "borrowingDebt":unavailable(),"grossExposure":unavailable(),"instrumentPermissions":[],"positions":unavailable(),"openOrders":unavailable()},
        "components":{"account":{"status":"available","value":{"startedAtMs":1100,"completedAtMs":1300,"evidenceIds":["acct"]}},
            "positions":unavailable(),"openOrders":unavailable()}})).unwrap()
}
fn decode(value: &Value, request: &Request) -> Result<Observation, String> {
    decode_observation_json(&serde_json::to_vec(value).unwrap(), request, 2000)
}
#[test]
fn non_atomic_v2_bindings_are_two_exact_paper_roles_and_preserve_lookup_decimals() {
    for role in [Role::Entry, Role::Close] {
        let r = request(role);
        r.validate_binding(role.operation_id(), role.capability())
            .unwrap();
        let other = if role == Role::Entry {
            Role::Close
        } else {
            Role::Entry
        };
        assert!(r
            .validate_binding(other.operation_id(), other.capability())
            .is_err());
        assert!(r
            .validate_binding(role.operation_id(), other.capability())
            .is_err());
        let bytes = serde_json::to_vec(&r).unwrap();
        assert_eq!(decode_request_json(&bytes).unwrap(), r);
        assert_eq!(
            r.lookup.original_submit.order.quantity.to_string(),
            "0.123456789012"
        );
        for action in [PositionAction::SellToOpen, PositionAction::BuyToClose] {
            let mut wrong = r.clone();
            wrong.lookup.original_submit.order.position_action = action;
            assert_eq!(
                wrong.validate().unwrap_err(),
                "settlement_observation_v2_role_mismatch"
            );
        }
        let mut wrong = r.clone();
        wrong.role = other;
        assert!(wrong.validate().is_err());
    }
}
#[test]
fn unknown_order_fee_and_account_stay_unavailable_without_evidence_or_zero_defaults() {
    let r = request(Role::Entry);
    let o = pending(&r);
    o.validate(&r, 2000).unwrap();
    assert!(o.order.filled_quantity.required().is_err());
    assert!(o.order.cash_spent.required().is_err());
    assert!(o.fee_cash.required().is_err());
    assert!(o.fee_finality.required().is_err());
    assert!(o.account.required().is_err());
    assert_eq!(decode(&serde_json::to_value(&o).unwrap(), &r).unwrap(), o);
    let mut drifted = r.clone();
    drifted.host_account_ref = "account://foreign/fixture-account".into();
    assert!(o.validate(&drifted, 2000).is_err());
    let mut wrong = o.clone();
    wrong.order.order.instrument.instrument_id = "same-symbol.other-host".into();
    assert!(wrong.validate(&r, 2000).is_err());
}
#[test]
fn terminal_fills_are_provisional_and_reported_average_never_populates_cash_or_fees() {
    let r = request(Role::Close);
    let mut o = pending(&r);
    o.order.status = OrderStatus::Filled;
    o.order.provider_order_id = Some("provider-order".into());
    o.order.filled_quantity = Observed::Available {
        value: r.lookup.original_submit.order.quantity,
    };
    o.status = ProviderObservationStatus::Provisional;
    o.evidence = vec![evidence("order", EvidenceKind::Order, 1000, 1100)];
    o.average_fill_price = Observed::Available {
        value: ProviderReportedAveragePrice {
            basis: ProviderReported::ProviderReported,
            price: "12.000000000001".parse().unwrap(),
            currency: Currency {
                code: "USD".into(),
                decimals: 2,
            },
            evidence_ids: vec!["order".into()],
        },
    };
    o.validate(&r, 2000).unwrap();
    assert!(o.order.cash_spent.required().is_err());
    assert!(o.fee_cash.required().is_err());
    assert!(o.fee_finality.required().is_err());
    let mut wrong = o.clone();
    wrong.status = ProviderObservationStatus::Pending;
    assert!(wrong.validate(&r, 2000).is_err());
    wrong = o.clone();
    wrong.evidence.clear();
    assert!(wrong.validate(&r, 2000).is_err());
    wrong = o.clone();
    if let Observed::Available { value } = &mut wrong.average_fill_price {
        value.price = Decimal::ZERO;
    }
    assert!(wrong.validate(&r, 2000).is_err());
    for price in [json!(12.5), json!("0.0000000000001")] {
        let mut invalid = serde_json::to_value(&o).unwrap();
        invalid["averageFillPrice"]["value"]["price"] = price;
        assert!(decode(&invalid, &r).is_err());
    }
    let mut value = serde_json::to_value(&o).unwrap();
    value["status"] = json!("reconciled");
    assert!(decode(&value, &r).is_err());
}
#[test]
fn non_atomic_component_intervals_and_missing_positions_are_explicit_and_bounded() {
    let r = request(Role::Entry);
    let mut o = pending(&r);
    o.account = Observed::Available {
        value: account_value(&r),
    };
    o.evidence = vec![evidence("acct", EvidenceKind::Account, 1100, 1300)];
    o.validate(&r, 2000).unwrap();
    let a = o.account.required().unwrap();
    assert!(a.snapshot.borrowing_debt.required().is_err());
    assert!(a.snapshot.positions.required().is_err());
    let original = serde_json::to_value(&o).unwrap();
    for (pointer, value) in [
        (
            "/account/value/snapshot/accountRef",
            json!("account://other/fixture-account"),
        ),
        (
            "/account/value/snapshot/providerAccountId",
            json!("other-account"),
        ),
        ("/account/value/snapshot/mode", json!("live")),
        (
            "/account/value/snapshot/time/basis",
            json!("provider_observed"),
        ),
        ("/account/value/snapshot/time/observedAtMs", json!(1200)),
        (
            "/account/value/snapshot/coverage/positionsComplete",
            json!(true),
        ),
        (
            "/account/value/snapshot/positions",
            json!({"status":"available","value":[]}),
        ),
        (
            "/account/value/components/account/value/completedAtMs",
            json!(1250),
        ),
        (
            "/account/value/components/account/value/evidenceIds",
            json!(["missing"]),
        ),
        ("/evidence/0/completedAtMs", json!(2001)),
    ] {
        let mut invalid = original.clone();
        *invalid.pointer_mut(pointer).unwrap() = value;
        assert!(decode(&invalid, &r).is_err(), "{pointer}");
    }
    let mut full = account_value(&r);
    full.snapshot.positions = Observed::Available { value: vec![] };
    full.snapshot.coverage.positions_complete = true;
    full.components.positions = Observed::Available {
        value: ComponentSample {
            started_at_ms: 1301,
            completed_at_ms: 1400,
            evidence_ids: vec!["positions".into()],
        },
    };
    o.account = Observed::Available { value: full };
    o.evidence
        .push(evidence("positions", EvidenceKind::Positions, 1301, 1400));
    o.validate(&r, 2000).unwrap(); // independent times are not forced equal or called atomic
}
#[test]
fn fee_claims_require_typed_evidence_and_never_imply_settlement() {
    let r = request(Role::Entry);
    let mut o = pending(&r);
    o.fee_cash = Observed::Available {
        value: ProviderReportedFeeCash {
            amount: Decimal::ZERO,
            currency: Currency {
                code: "USD".into(),
                decimals: 2,
            },
            evidence_ids: vec!["fees".into()],
        },
    };
    assert!(o.validate(&r, 2000).is_err()); // no automatic zero-fee assumption
    o.evidence
        .push(evidence("fees", EvidenceKind::Fees, 1000, 1200));
    o.validate(&r, 2000).unwrap();
    o.fee_finality = Observed::Available {
        value: ProviderFeeFinality {
            basis: ProviderReported::ProviderReported,
            final_: true,
            evidence_ids: vec!["fees".into()],
        },
    };
    o.validate(&r, 2000).unwrap();
    assert_eq!(o.status, ProviderObservationStatus::Pending);
    assert!(o.order.cash_spent.required().is_err());
    assert!(o.account.required().is_err());
    let mut wrong = o.clone();
    if let Observed::Available { value } = &mut wrong.fee_cash {
        value.amount = "-1".parse().unwrap();
    }
    assert!(wrong.validate(&r, 2000).is_err());
    wrong = o.clone();
    wrong.evidence[0].kind = EvidenceKind::Order;
    assert!(wrong.validate(&r, 2000).is_err());
}
#[test]
fn non_atomic_v2_rejects_invented_revisions_extra_fields_and_legacy_coherent_decoding() {
    let r = request(Role::Entry);
    let o = pending(&r);
    let original = serde_json::to_value(&o).unwrap();
    for (field, value) in [
        ("checkpoint", json!({"ledgerEpoch":"invented","sequence":1})),
        ("postingSequence", json!(1)),
        ("postingId", json!("invented")),
        ("hostReconciled", json!(true)),
    ] {
        let mut invalid = original.clone();
        invalid.as_object_mut().unwrap().insert(field.into(), value);
        assert!(decode(&invalid, &r).is_err());
    }
    for value in [json!("atomic"), json!({"non_atomic":null}), Value::Null] {
        let mut invalid = original.clone();
        invalid["coherence"] = value;
        assert!(decode(&invalid, &r).is_err());
    }
    assert!(
        serde_json::from_value::<crate::cash_settlement::Observation>(original.clone()).is_err()
    );
    assert!(serde_json::from_value::<crate::close_settlement::Observation>(original).is_err());
    let legacy = json!({"schemaVersion":"tradeassembly.cash_settlement_request.v1","planId":r.lookup.original_submit.plan_id,
        "canonicalAccountSha256":r.lookup.original_submit.canonical_account_sha256,"clientOrderId":r.lookup.original_submit.order.client_order_id});
    let old: crate::cash_settlement::Request = serde_json::from_value(legacy.clone()).unwrap();
    assert_eq!(serde_json::to_value(old).unwrap(), legacy);
    assert!(serde_json::from_value::<Request>(legacy).is_err());
}
#[test]
fn evidence_identity_times_reasons_and_total_wire_bytes_fail_closed() {
    let r = request(Role::Entry);
    let mut o = pending(&r);
    o.evidence = vec![
        evidence("same", EvidenceKind::Order, 1000, 1100),
        evidence("same", EvidenceKind::Fees, 1101, 1200),
    ];
    assert!(o.validate(&r, 2000).is_err());
    o.evidence = vec![evidence("one", EvidenceKind::Order, 1100, 1000)];
    assert!(o.validate(&r, 2000).is_err());
    o.evidence = vec![evidence("one", EvidenceKind::Order, 1000, 2001)];
    assert!(o.validate(&r, 2000).is_err());
    o.evidence = (0..=MAX_EVIDENCE)
        .map(|i| evidence(&i.to_string(), EvidenceKind::Fees, 1000, 1100))
        .collect();
    assert!(o.validate(&r, 2000).is_err());
    o = pending(&r);
    o.fee_cash = Observed::Unavailable {
        reason: "x".repeat(257),
    };
    assert!(o.validate(&r, 2000).is_err());
    assert!(pending(&r)
        .validate(&r, MAX_EXACT_INTEGER as i64 + 1)
        .is_err());
    let mut bytes = serde_json::to_vec(&r).unwrap();
    bytes.resize(MAX_REQUEST_BYTES + 1, b' ');
    assert_eq!(
        decode_request_json(&bytes).unwrap_err(),
        "settlement_observation_v2_request_too_large"
    );
    let mut bytes = serde_json::to_vec(&pending(&r)).unwrap();
    bytes.resize(MAX_OBSERVATION_BYTES + 1, b' ');
    assert_eq!(
        decode_observation_json(&bytes, &r, 2000).unwrap_err(),
        "settlement_observation_v2_response_too_large"
    );
}
#[test]
fn exported_schema_values_are_closed_and_do_not_redefine_old_coherent_v1() {
    let request = request_schema();
    let response = response_schema();
    assert_eq!(request["additionalProperties"], false);
    assert_eq!(response["additionalProperties"], false);
    assert_eq!(
        request["properties"]["lookup"],
        crate::broker_lookup_v3::request_schema()
    );
    assert_eq!(response["properties"]["coherence"]["const"], "non_atomic");
    assert_eq!(
        response["properties"]["status"]["enum"],
        json!(["pending", "provisional"])
    );
    assert!(response["properties"].get("checkpoint").is_none());
    assert!(response["properties"].get("postingSequence").is_none());
    assert_eq!(
        crate::cash_settlement::request_schema()["properties"]["schemaVersion"]["const"],
        "tradeassembly.cash_settlement_request.v1"
    );
    assert_eq!(
        crate::close_settlement::request_schema()["properties"]["schemaVersion"]["const"],
        "tradeassembly.close_settlement_request.v1"
    );
}

#[test]
fn shared_json_fixtures_match_sdk_semantics_for_schema_validator_adoption() {
    // Root's runtime jsonschema gate uses the same files and schemaValid axis.
    // This SDK test exercises semanticValid, not an unexecuted schema engine.
    let cases: Vec<Value> = serde_json::from_str(include_str!(
        "../tests/fixtures/settlement_observation_v2/cases.json"
    ))
    .unwrap();
    let directory = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/settlement_observation_v2");
    for case in cases {
        let bytes = std::fs::read(directory.join(case["file"].as_str().unwrap())).unwrap();
        let valid = if case["schema"] == "request" {
            decode_request_json(&bytes).is_ok()
        } else {
            let original =
                std::fs::read(directory.join(case["requestFile"].as_str().unwrap())).unwrap();
            let request = decode_request_json(&original).unwrap();
            decode_observation_json(&bytes, &request, case["nowMs"].as_i64().unwrap()).is_ok()
        };
        assert_eq!(
            valid,
            case["semanticValid"].as_bool().unwrap(),
            "{}",
            case["file"]
        );
    }
}
