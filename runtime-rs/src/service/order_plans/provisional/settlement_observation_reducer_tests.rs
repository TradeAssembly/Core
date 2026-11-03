use super::*;
use tradeassembly_plugin_sdk::settlement_observation_v2::{
    ProviderFeeFinality, ProviderReported, ProviderReportedFeeCash,
};
fn unavailable() -> Value {
    json!({"status":"unavailable","reason":"not_evidenced"})
}
fn request(role: Role) -> Request {
    let value = if role == Role::Entry {
        include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../plugin-sdk/tests/fixtures/settlement_observation_v2/request-entry.json"
        ))
    } else {
        include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../plugin-sdk/tests/fixtures/settlement_observation_v2/request-close.json"
        ))
    };
    serde_json::from_str(value).unwrap()
}
fn observation(
    request: &Request,
    status: OrderStatus,
    filled: Option<&str>,
    cash: Option<&str>,
    average: Option<&str>,
    time: i64,
) -> Observation {
    let known_value = |value: Option<&str>| {
        value.map_or_else(unavailable, |v| json!({"status":"available","value":v}))
    };
    serde_json::from_value(json!({"schemaVersion":"tradeassembly.settlement_observation.v2","requestSha256":request.request_sha256().unwrap(),
        "status":status_for(&status),"coherence":"non_atomic",
        "order":{"schemaVersion":"tradeassembly.broker_order_observation.v2","order":request.lookup.original_submit.order,
            "canonicalAccountSha256":request.lookup.original_submit.canonical_account_sha256,
            "providerOrderId":if status==OrderStatus::Unknown{None}else{Some("provider.order")},"status":status,
            "filledQuantity":known_value(filled),"cashSpent":known_value(cash),"observedAtMs":time,"knownAtMs":time+1},
        "averageFillPrice":average.map_or_else(unavailable,|value|json!({"status":"available","value":{
            "basis":"provider_reported","price":value,"currency":{"code":"USD","decimals":2},"evidenceIds":["order"]}})),
        "feeCash":unavailable(),"feeFinality":unavailable(),"account":unavailable(),
        "evidence":[{"id":"order","kind":"order","providerRecordId":unavailable(),"hashBasis":"raw_response_bytes",
            "responseSha256":"b".repeat(64),"startedAtMs":time,"completedAtMs":time+2}]})).unwrap()
}
fn partial(
    request: &Request,
    fill: &str,
    cash: Option<&str>,
    average: Option<&str>,
    time: i64,
) -> Observation {
    observation(
        request,
        OrderStatus::PartiallyFilled,
        Some(fill),
        cash,
        average,
        time,
    )
}
fn projected(request: &Request, observation: &Observation) -> Projection {
    reduce(None, request, observation, 10_000)
        .unwrap()
        .projection
}
fn refusal(previous: &Projection, request: &Request, observation: &Observation, code: &str) {
    let before = serde_json::to_vec(previous).unwrap();
    assert_eq!(
        reduce(Some(previous), request, observation, 10_000).unwrap_err(),
        code
    );
    assert_eq!(serde_json::to_vec(previous).unwrap(), before);
}
#[test]
fn exact_repeat_is_idempotent_after_serialized_restart_and_still_revalidates_sdk() {
    let r = request(Role::Entry);
    let o = partial(&r, "0.03", None, Some("10"), 1000);
    let first = reduce(None, &r, &o, 10_000).unwrap();
    assert_eq!(first.disposition, Disposition::Advanced);
    assert_eq!(first.projection.generation, 1);
    let restored: Projection =
        serde_json::from_slice(&serde_json::to_vec(&first.projection).unwrap()).unwrap();
    let again = reduce(Some(&restored), &r, &o, 10_000).unwrap();
    assert_eq!(again.disposition, Disposition::Unchanged);
    assert_eq!(again.projection, first.projection);
    assert_eq!(again.observation_sha256, first.observation_sha256);
    assert!(reduce(Some(&restored), &r, &o, 1000).is_err()); // future known time, even for an identical hash
    let close = request(Role::Close);
    let other = partial(&close, "0.03", None, Some("10"), 2000);
    assert!(reduce(Some(&restored), &close, &other, 10_000).is_err());
}
#[test]
fn partial_progress_and_cancelled_partial_are_accepted_without_cash_inference() {
    let r = request(Role::Close);
    let first = partial(&r, "0.03", None, Some("10"), 1000);
    let p = projected(&r, &first);
    let next = partial(&r, "0.06", None, Some("11"), 2000);
    let second = reduce(Some(&p), &r, &next, 10_000).unwrap();
    assert_eq!(second.projection.generation, 2);
    assert_eq!(
        second.projection.filled_quantity,
        Some("0.06".parse().unwrap())
    );
    assert!(second.projection.cash_debit.is_none());
    assert!(second.blockers.contains(&Blocker::CashDebitUnavailable));
    let cancelled = observation(
        &r,
        OrderStatus::Cancelled,
        Some("0.09"),
        None,
        Some("10.5"),
        3000,
    );
    let final_read = reduce(Some(&second.projection), &r, &cancelled, 10_000).unwrap();
    assert_eq!(
        final_read.projection.status,
        ProviderObservationStatus::Provisional
    );
    assert_eq!(final_read.projection.order_status, OrderStatus::Cancelled);
    assert!(final_read
        .blockers
        .contains(&Blocker::CoherentPostingUnavailable));
    assert!(final_read.blockers.contains(&Blocker::FeeCashUnavailable));
    assert!(final_read
        .blockers
        .contains(&Blocker::FeeFinalityUnavailable));
}
#[test]
fn known_fill_cash_identity_and_average_cannot_regress_or_disappear() {
    let r = request(Role::Entry);
    let p = projected(&r, &partial(&r, "0.06", Some("0.70"), Some("11"), 1000));
    refusal(
        &p,
        &r,
        &partial(&r, "0.03", Some("0.70"), Some("11"), 2000),
        "settlement_projection_fill_regressed",
    );
    refusal(
        &p,
        &r,
        &partial(&r, "0.06", Some("0.35"), Some("11"), 2000),
        "settlement_projection_cash_regressed",
    );
    refusal(
        &p,
        &r,
        &partial(&r, "0.06", None, Some("11"), 2000),
        "settlement_projection_known_fact_unavailable",
    );
    refusal(
        &p,
        &r,
        &partial(&r, "0.06", Some("0.70"), None, 2000),
        "settlement_projection_known_fact_unavailable",
    );
    let mut switch = partial(&r, "0.06", Some("0.70"), Some("11"), 2000);
    switch.order.provider_order_id = Some("different.provider.order".into());
    refusal(
        &p,
        &r,
        &switch,
        "settlement_projection_provider_order_changed",
    );
    let mut missing = observation(&r, OrderStatus::Unknown, None, None, None, 2000);
    missing.order.provider_order_id = p.provider_order_id.clone();
    refusal(
        &p,
        &r,
        &missing,
        "settlement_projection_known_fact_unavailable",
    );
}
#[test]
fn partial_average_changes_need_fill_progress_and_terminal_corrections_refuse() {
    let r = request(Role::Entry);
    let p = projected(&r, &partial(&r, "0.03", None, Some("10"), 1000));
    refusal(
        &p,
        &r,
        &partial(&r, "0.03", None, Some("11"), 2000),
        "settlement_projection_average_changed_without_fill_progress",
    );
    let mut currency = partial(&r, "0.06", None, Some("11"), 2000);
    if let Observed::Available { value } = &mut currency.average_fill_price {
        value.currency.decimals = 3;
    }
    refusal(
        &p,
        &r,
        &currency,
        "settlement_projection_average_currency_changed",
    );
    let cancelled = observation(
        &r,
        OrderStatus::Cancelled,
        Some("0.03"),
        None,
        Some("10"),
        2000,
    );
    let terminal = reduce(Some(&p), &r, &cancelled, 10_000).unwrap().projection;
    refusal(
        &terminal,
        &r,
        &observation(
            &r,
            OrderStatus::Cancelled,
            Some("0.06"),
            None,
            Some("10"),
            3000,
        ),
        "settlement_projection_terminal_fill_correction",
    );
    refusal(
        &terminal,
        &r,
        &observation(
            &r,
            OrderStatus::Cancelled,
            Some("0.03"),
            None,
            Some("11"),
            3000,
        ),
        "settlement_projection_terminal_average_correction",
    );
    refusal(
        &terminal,
        &r,
        &partial(&r, "0.03", None, Some("10"), 3000),
        "settlement_projection_terminal_status_changed",
    );
}
#[test]
fn same_time_order_and_evidence_conflicts_and_backwards_clocks_refuse() {
    let r = request(Role::Entry);
    let p = projected(&r, &partial(&r, "0.03", None, Some("10"), 1000));
    let mut same = partial(&r, "0.06", None, Some("10"), 2000);
    same.order.observed_at_ms = 1000;
    same.order.known_at_ms = 1001;
    refusal(
        &p,
        &r,
        &same,
        "settlement_projection_same_time_order_conflict",
    );
    let mut conflicting = partial(&r, "0.03", None, Some("10"), 1000);
    conflicting.evidence[0].response_sha256 = "c".repeat(64);
    refusal(
        &p,
        &r,
        &conflicting,
        "settlement_projection_same_time_evidence_conflict",
    );
    let mut backwards = partial(&r, "0.06", None, Some("10"), 2000);
    backwards.order.observed_at_ms = 999;
    refusal(
        &p,
        &r,
        &backwards,
        "settlement_projection_order_clock_regressed",
    );
    backwards = partial(&r, "0.06", None, Some("10"), 2000);
    backwards.evidence[0].started_at_ms = 999;
    refusal(
        &p,
        &r,
        &backwards,
        "settlement_projection_evidence_clock_regressed",
    );
    backwards = partial(&r, "0.06", None, Some("10"), 2000);
    backwards.evidence[0].started_at_ms = 998;
    backwards.evidence[0].completed_at_ms = 999;
    refusal(
        &p,
        &r,
        &backwards,
        "settlement_projection_evidence_clock_regressed",
    );
}
#[test]
fn rejected_or_filled_terminal_reads_still_never_reconcile_cash() {
    for role in [Role::Entry, Role::Close] {
        let r = request(role);
        let quantity = r.lookup.original_submit.order.quantity.to_string();
        let o = observation(&r, OrderStatus::Filled, Some(&quantity), None, None, 1000);
        let outcome = reduce(None, &r, &o, 10_000).unwrap();
        assert_eq!(
            outcome.projection.status,
            ProviderObservationStatus::Provisional
        );
        for blocker in [
            Blocker::CashDebitUnavailable,
            Blocker::AverageFillPriceUnavailable,
            Blocker::FeeCashUnavailable,
            Blocker::FeeFinalityUnavailable,
            Blocker::AccountUnavailable,
            Blocker::CoherentPostingUnavailable,
        ] {
            assert!(outcome.blockers.contains(&blocker));
        }
        let rejected = observation(&r, OrderStatus::Rejected, Some("0"), None, None, 2000);
        let outcome = reduce(None, &r, &rejected, 10_000).unwrap();
        assert!(outcome.projection.cash_debit.is_none());
        assert!(outcome.projection.fee_cash.is_none());
        assert!(outcome
            .blockers
            .contains(&Blocker::CoherentPostingUnavailable));
    }
}
#[test]
fn provider_reported_fee_finality_is_retained_but_is_not_host_finality() {
    let r = request(Role::Entry);
    let mut o = partial(&r, "0.03", None, Some("10"), 1000);
    let mut fees = o.evidence[0].clone();
    fees.id = "fees".into();
    fees.kind = EvidenceKind::Fees;
    o.evidence.push(fees);
    o.fee_cash = Observed::Available {
        value: ProviderReportedFeeCash {
            amount: "0.01".parse().unwrap(),
            currency: Currency {
                code: "USD".into(),
                decimals: 2,
            },
            evidence_ids: vec!["fees".into()],
        },
    };
    o.fee_finality = Observed::Available {
        value: ProviderFeeFinality {
            basis: ProviderReported::ProviderReported,
            final_: true,
            evidence_ids: vec!["fees".into()],
        },
    };
    let out = reduce(None, &r, &o, 10_000).unwrap();
    assert!(out
        .blockers
        .contains(&Blocker::ProviderFeeFinalityUnverified));
    assert!(out.blockers.contains(&Blocker::CashDebitUnavailable));
    assert!(out.blockers.contains(&Blocker::CoherentPostingUnavailable));
    let mut lost = o.clone();
    lost.order.observed_at_ms = 2000;
    lost.order.known_at_ms = 2001;
    for e in &mut lost.evidence {
        e.started_at_ms = 2000;
        e.completed_at_ms = 2002;
    }
    lost.fee_finality = Observed::Unavailable {
        reason: "not_evidenced".into(),
    };
    refusal(
        &out.projection,
        &r,
        &lost,
        "settlement_projection_known_fact_unavailable",
    );
}
#[test]
fn refused_observation_leaves_prior_generation_and_fresh_progress_can_recover() {
    let r = request(Role::Entry);
    let p = projected(&r, &partial(&r, "0.03", None, Some("10"), 1000));
    refusal(
        &p,
        &r,
        &partial(&r, "0.02", None, Some("10"), 2000),
        "settlement_projection_fill_regressed",
    );
    let valid = partial(&r, "0.06", None, Some("11"), 3000);
    let outcome = reduce(Some(&p), &r, &valid, 10_000).unwrap();
    assert_eq!(outcome.projection.generation, p.generation + 1);
    let mut corrupted = outcome.projection.clone();
    corrupted.generation += 1;
    assert!(reduce(Some(&corrupted), &r, &valid, 10_000).is_err());
    let mut role = outcome.projection.clone();
    role.role = Role::Close;
    role.projection_sha256 = projection_hash(&role).unwrap();
    assert!(reduce(Some(&role), &r, &valid, 10_000).is_err());
}

#[test]
fn later_independent_fee_read_does_not_need_a_fabricated_new_order_clock() {
    let r = request(Role::Entry);
    let original = partial(&r, "0.03", None, Some("10"), 1000);
    let previous = projected(&r, &original);
    let mut later = original;
    let mut fees = later.evidence[0].clone();
    fees.id = "fees".into();
    fees.kind = EvidenceKind::Fees;
    fees.started_at_ms = 2000;
    fees.completed_at_ms = 2002;
    later.evidence.push(fees);
    later.fee_cash = Observed::Available {
        value: ProviderReportedFeeCash {
            amount: "0.01".parse().unwrap(),
            currency: Currency {
                code: "USD".into(),
                decimals: 2,
            },
            evidence_ids: vec!["fees".into()],
        },
    };
    let result = reduce(Some(&previous), &r, &later, 10_000).unwrap();
    assert_eq!(
        result.projection.order_known_at_ms,
        previous.order_known_at_ms
    );
    assert_eq!(
        result.projection.latest_evidence_completed_at_ms,
        Some(2002)
    );
    assert!(result.blockers.contains(&Blocker::FeeFinalityUnavailable));
    assert!(result
        .blockers
        .contains(&Blocker::CoherentPostingUnavailable));
}
