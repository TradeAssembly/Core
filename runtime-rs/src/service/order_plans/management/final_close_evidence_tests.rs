// Copyright (c) 2026 OptionLab LLC. All rights reserved.
//! Internal future coherent-transition regression, not a public @2 transition,
//! provider-finality, C3/C5, sandbox or broker integration qualification.
//! Pending owned state is synthetic; both observations use real commit paths.
use super::*;
use tradeassembly_plugin_sdk::{broker_lookup_v3, broker_v2, close_settlement as wire};

fn n(s: &str) -> Decimal {
    s.parse().unwrap()
}
fn put(s: &TradeAssemblyService, ns: &str, key: &str, v: Value, ctx: &SideEffectContext) {
    s.runtime().storage.put_json(ns, key, v, ctx).unwrap();
}
fn reseal_plan(p: &mut OrderPlan) {
    p.plan_id.clear();
    p.plan_id = format!("orderplan_{}", hash(p).unwrap());
}

#[test]
fn coherent_commit_supersedes_disputed_v2_head_without_reapplying_quantity() {
    let (_dir, s, mut facts, policy) = super::super::tests::fixture();
    let ctx = super::super::tests::context("internal-close-transition");
    let expected = broker_lookup_v3::ExpectedPaperAccount {
        broker_namespace: "fixture.broker".into(),
        environment: broker_lookup_v3::PaperEnvironment::Paper,
        provider_account_id: facts.account.provider_account_id.clone(),
    };
    let account_sha = expected.canonical_account_sha256().unwrap();
    facts.canonical_account = Some(
        crate::broker_submission::account_identity::CanonicalAccountIdentity {
            schema_version: "tradeassembly.canonical_account.v1".into(),
            broker_namespace: expected.broker_namespace.clone(),
            environment: "paper".into(),
            provider_account_id: expected.provider_account_id.clone(),
            identity_sha256: account_sha.clone(),
        },
    );
    let entry = super::super::tests::plan(&s, &facts, &policy, "transition-entry");
    put(&s, PLANS, &entry.plan_id, value(&entry).unwrap(), &ctx);
    let position_id = "position_internal_transition";
    let activation = "fixture.transition.activation";
    s.bind_owned_object("execution_activation", activation)
        .unwrap();
    let mut close = entry.clone();
    close.order.position_action = PositionAction::SellToClose;
    close.order.client_order_id = "fixture.transition.close".into();
    close.purchase_cash = n("1");
    close.management = Some(CloseBinding {
        position_id: position_id.into(),
        original_entry_plan_id: entry.plan_id.clone(),
        execution_occurrence_id: "fixture.occurrence".into(),
        trigger_sha256: "a".repeat(64),
        exit_policy_sha256: "b".repeat(64),
    });
    reseal_plan(&mut close);
    put(&s, PLANS, &close.plan_id, value(&close).unwrap(), &ctx);
    let mut position: Position = serde_json::from_value(json!({
        "schemaVersion":"tradeassembly.managed_position.v2","positionId":position_id,
        "ownerSha256":owner(&s).unwrap(),"entryPlanId":entry.plan_id,"activationId":activation,
        "entrySettlementSha256":"","entryObservationSha256":"c".repeat(64),"entryCashReconciled":false,
        "policy":{"entryBasis":"provider_reported_average_fill","rules":{"schemaVersion":"tradeassembly.position_exit_rules.v1",
            "priceBasis":"bid","stopLossRatio":"0.1","trailingRatio":null,"trailingStart":null,"reversalSignal":null,
            "precedence":["stop_loss"],"maxQuoteAgeMs":30000},"feeCashCap":"1","maxLimitSlippageRatio":"0.01",
            "timeInForce":"day","maxReversalAgeMs":30000},
        "quantity":"0.6","closedQuantity":"0","basis":"100","entryTime":facts.account.time,
        "highWater":null,"lastQuoteKnownAtMs":0,"lastQuoteObservedAtMs":0,
        "lastReversalReceipt":null,"reversalWatermark":null,"lastQuoteReceipt":null,
        "reservations":BTreeMap::from([(close.plan_id.clone(),close.order.quantity)]),"generation":0,"stateSha256":""
    })).unwrap();
    seal(&mut position).unwrap();
    put(&s, POSITIONS, position_id, value(&position).unwrap(), &ctx);
    let ledger = Ledger {
        owner_sha256: owner(&s).unwrap(),
        reservations: BTreeMap::from([(close.plan_id.clone(), close.purchase_cash)]),
        gross_limit: None,
        capital_checkpoint: None,
    };
    put(&s, CASH, &close.account_key, value(&ledger).unwrap(), &ctx);
    let prepared = crate::ports::broker_submission::BrokerPreparedBinding {
        plugin_instance_ref: "fixture.broker.instance".into(),
        plugin_ref: "fixture.broker".into(),
        package_sha256: "a".repeat(64),
        manifest_fingerprint: "b".repeat(64),
        configuration_digest: hash(&json!({})).unwrap(),
        credential_ref: "fixture.unused".into(),
        credential_generation: 1,
    };
    let submit = broker_v2::SubmitRequest {
        schema_version: "tradeassembly.broker_submit.v2".into(),
        plan_id: close.plan_id.clone(),
        canonical_account_sha256: account_sha.clone(),
        order: close.order.clone(),
    };
    submit.validate().unwrap();
    let intent = json!({"planId":close.plan_id,"idempotencyKey":ctx.idempotency_key.as_str(),
        "accountRef":facts.account.account_ref,"preparedBinding":prepared,"originalRequest":{"input":submit}});
    let mut record = submission::SubmissionRecord {
        schema_version: "tradeassembly.broker_submission_state.v2".into(),
        plan_id: close.plan_id.clone(),
        owner_sha256: owner(&s).unwrap(),
        account_key: close.account_key.clone(),
        original_key: ctx.idempotency_key.as_str().into(),
        intent_sha256: hash(&intent).unwrap(),
        intent,
        phase: submission::Phase::Sending,
        generation: 0,
        retained_cash: close.purchase_cash,
        filled_quantity: Decimal::ZERO,
        provider_order_id: None,
        observation: None,
        settlement_receipt_sha256: None,
        record_sha256: String::new(),
    };
    record.record_sha256 = submission::record_hash(&record).unwrap();
    put(
        &s,
        submission::STATE,
        &close.plan_id,
        value(&record).unwrap(),
        &ctx,
    );
    let now = s.runtime().clock.trusted_now_ms().unwrap();
    let unavailable = json!({"status":"unavailable","reason":"synthetic_internal_observation"});
    let q = tradeassembly_plugin_sdk::settlement_observation_v2::Request {
        schema_version: tradeassembly_plugin_sdk::settlement_observation_v2::REQUEST_VERSION.into(),
        role: tradeassembly_plugin_sdk::settlement_observation_v2::Role::Close,
        lookup: broker_lookup_v3::LookupRequestV3 {
            schema_version: broker_lookup_v3::VERSION.into(),
            original_submit: submit,
            expected_account: expected,
        },
        host_account_ref: facts.account.account_ref.clone(),
    };
    let observation =
        |time: i64| -> tradeassembly_plugin_sdk::settlement_observation_v2::Observation {
            serde_json::from_value(json!({"schemaVersion":tradeassembly_plugin_sdk::settlement_observation_v2::OBSERVATION_VERSION,
            "requestSha256":q.request_sha256().unwrap(),"status":"provisional","coherence":"non_atomic",
            "order":{"schemaVersion":"tradeassembly.broker_order_observation.v2","canonicalAccountSha256":account_sha,
                "order":close.order,"providerOrderId":"fixture.close.order","status":"cancelled",
                "filledQuantity":{"status":"available","value":"0.2"},"cashSpent":unavailable,
                "observedAtMs":time,"knownAtMs":time+1},
            "averageFillPrice":{"status":"available","value":{"basis":"provider_reported","price":"100",
                "currency":{"code":"USD","decimals":2},"evidenceIds":["order"]}},
            "feeCash":unavailable,"feeFinality":unavailable,"account":unavailable,
            "evidence":[{"id":"order","kind":"order","providerRecordId":unavailable,"hashBasis":"raw_response_bytes",
                "responseSha256":hash(&time).unwrap(),"startedAtMs":time,"completedAtMs":time+2}]
        })).unwrap()
        };
    let first = observation(now - 1000);
    first.validate(&q, now).unwrap();
    let accepted = super::super::provisional::commit_authored_close_test_observation(
        &s,
        &close,
        first.clone(),
        "first-close",
        &ctx,
    )
    .unwrap();
    assert_eq!(accepted["status"], "provisional");
    let (_, applied) = load(&s, position_id).unwrap();
    assert_eq!(applied.closed_quantity, n("0.2"));
    assert_provisional_close_evidence(&s, &applied).unwrap();

    // This is an explicitly authored future coherent source, not a claim that
    // the selected public @2 provider can currently produce coherent V1 proof.
    let mut order = first.order;
    order.cash_spent = Observed::Available { value: n("0.25") };
    order.observed_at_ms = now - 500;
    order.known_at_ms = now - 499;
    let mut account = facts.account.clone();
    account.time.observed_at_ms = now - 400;
    account.time.known_at_ms = now - 399;
    account.cash_balance = Observed::Available { value: n("119.75") };
    account.non_marginable_buying_power = Observed::Available { value: n("119.75") };
    account.gross_exposure = Observed::Available { value: n("40") };
    account.positions=serde_json::from_value(json!({"status":"available","value":[{
        "instrumentId":"SPY","side":"long","quantity":"0.4","grossValue":{"status":"available","value":"40"}}]})).unwrap();
    let coherent = wire::Observation {
        schema_version: "tradeassembly.close_settlement_observation.v1".into(),
        canonical_account_sha256: account_sha.clone(),
        order,
        posting_id: "fixture.coherent.close".into(),
        posting_sequence: 1,
        gross_proceeds: n("20"),
        fee_cash: n("0.25"),
        fees_final: true,
        checkpoint: wire::AccountCheckpoint {
            ledger_epoch: "fixture.coherent.epoch".into(),
            sequence: 1,
            account,
        },
    };
    let request = wire::Request {
        schema_version: "tradeassembly.close_settlement_request.v1".into(),
        plan_id: close.plan_id.clone(),
        original_entry_plan_id: entry.plan_id.clone(),
        canonical_account_sha256: account_sha.clone(),
        client_order_id: close.order.client_order_id.clone(),
    };
    coherent
        .validate_for(&request, &close.order, now, 30000)
        .unwrap();
    let committed = commit_close(
        &s,
        &close,
        prepared,
        hash(&coherent).unwrap(),
        coherent,
        "coherent-close",
        &ctx,
    )
    .unwrap();
    let (_, final_position) = load(&s, position_id).unwrap();
    assert_eq!(
        final_position.closed_quantity,
        n("0.2"),
        "coherent posting must not reapply .2"
    );
    assert_eq!(
        final_position.provisional_closes, applied.provisional_closes,
        "duplicate protection survives"
    );
    assert_eq!(
        final_position
            .quantity
            .checked_subtract(final_position.closed_quantity)
            .unwrap(),
        n("0.4")
    );
    let cash_after = s
        .runtime()
        .storage
        .get_json(CASH, &close.account_key)
        .unwrap();
    let later = observation(now - 100);
    let disputed = super::super::provisional::commit_authored_close_test_observation(
        &s,
        &close,
        later,
        "later-unknown-debit",
        &ctx,
    )
    .unwrap();
    assert_eq!(disputed["status"], "blocked");
    assert_eq!(
        disputed["blockedReason"],
        "provider_observation_submission_facts_regressed"
    );
    let marker = final_position
        .provisional_closes
        .as_ref()
        .unwrap()
        .get(&close.plan_id)
        .unwrap();
    assert_eq!(
        super::super::provisional::verify_applied_close(
            &s,
            &close.plan_id,
            position_id,
            marker.quantity,
            &marker.receipt_sha256
        )
        .unwrap_err(),
        "provider_observation_reconciliation_blocked"
    );
    assert_provisional_close_evidence(&s, &final_position).unwrap();
    assert_eq!(
        s.runtime()
            .storage
            .get_json(CASH, &close.account_key)
            .unwrap(),
        cash_after
    );
    assert_eq!(load(&s, position_id).unwrap().1, final_position);
    assert_eq!(
        commit_close(
            &s,
            &close,
            read_close(&s, committed["receiptSha256"].as_str().unwrap())
                .unwrap()
                .source_binding,
            "d".repeat(64),
            read_close(&s, committed["receiptSha256"].as_str().unwrap())
                .unwrap()
                .observation,
            "duplicate-coherent",
            &ctx
        )
        .unwrap_err(),
        "close_settlement_already_committed"
    );

    let id = committed["receiptSha256"].as_str().unwrap();
    let original = s
        .runtime()
        .storage
        .get_json(CLOSE_RECEIPTS, id)
        .unwrap()
        .unwrap();
    let mut tampered = original.clone();
    tampered["positionId"] = json!("different-position");
    put(&s, CLOSE_RECEIPTS, id, tampered, &ctx);
    assert!(assert_provisional_close_evidence(&s, &final_position).is_err());
    put(&s, CLOSE_RECEIPTS, id, original, &ctx);
    let (_, original_record) = submission::record(&s, &close).unwrap().unwrap();
    for change_quantity in [false, true] {
        let mut foreign = read_close(&s, id).unwrap();
        if change_quantity {
            foreign.observation.order.filled_quantity = Observed::Available { value: n("0.3") };
        } else {
            foreign.position_id = "different-position".into();
        }
        foreign.receipt_sha256 = close_hash(&foreign).unwrap();
        put(
            &s,
            CLOSE_RECEIPTS,
            &foreign.receipt_sha256,
            value(&foreign).unwrap(),
            &ctx,
        );
        let mut changed = original_record.clone();
        changed.settlement_receipt_sha256 = Some(foreign.receipt_sha256);
        changed.record_sha256 = submission::record_hash(&changed).unwrap();
        put(
            &s,
            submission::STATE,
            &close.plan_id,
            value(&changed).unwrap(),
            &ctx,
        );
        assert_eq!(
            assert_provisional_close_evidence(&s, &final_position).unwrap_err(),
            "managed_final_close_evidence_invalid"
        );
    }
    put(
        &s,
        submission::STATE,
        &close.plan_id,
        value(&original_record).unwrap(),
        &ctx,
    );
    assert_provisional_close_evidence(&s, &final_position).unwrap();
}
