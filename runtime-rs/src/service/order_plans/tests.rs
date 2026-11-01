use super::*;
fn av<T>(value: T) -> Observed<T> {
    Observed::Available { value }
}
fn number(v: &str) -> Decimal {
    v.parse().unwrap()
}
pub(super) fn fixture() -> (tempfile::TempDir, TradeAssemblyService, Facts, Policy) {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("cash.db").to_string_lossy().into_owned();
    let service =
        TradeAssemblyService::from_runtime(&db, crate::adapters::local::test_runtime(&db))
            .for_authenticated_invocation("fixture", "cash-owner", None, None);
    service
        .bind_owned_object("strategy", "unit.strategy")
        .unwrap();
    let time = ObservationTime {
        observed_at_ms: service.runtime().clock.now_ms(),
        known_at_ms: service.runtime().clock.now_ms(),
        basis: TimeBasis::HostAcquired,
    };
    let currency = Currency {
        code: "USD".into(),
        decimals: 2,
    };
    let account = AccountSnapshot {
        schema_version: AccountVersion::V1,
        account_ref: "fixture.paper".into(),
        provider_account_id: "fixture.provider.paper".into(),
        mode: TradingMode::Paper,
        currency: currency.clone(),
        time: time.clone(),
        coverage: Coverage {
            account_complete: true,
            positions_complete: true,
            open_orders_complete: true,
        },
        cash_balance: av(number("100")),
        non_marginable_buying_power: av(number("100")),
        borrowing_debt: av(Decimal::ZERO),
        gross_exposure: av(Decimal::ZERO),
        instrument_permissions: vec![InstrumentPermission {
            asset_class: "equity".into(),
            instrument_family: "equity".into(),
            actions: av(vec![PositionAction::BuyToOpen]),
        }],
        positions: av(vec![]),
        open_orders: av(vec![]),
    };
    let metadata = InstrumentMetadata {
        schema_version: MetadataVersion::V1,
        instrument_id: "SPY".into(),
        provider_symbol: "SPY".into(),
        asset_class: "equity".into(),
        instrument_family: "equity".into(),
        time: time.clone(),
        quantity_unit: av(QuantityUnit::Shares),
        multiplier: av(number("1")),
        quote_currency: av(currency.clone()),
        minimum_quantity: av(number("0.001")),
        quantity_increment: av(number("0.001")),
        price_increment: av(number("0.01")),
        tradable: av(true),
        expires_at_ms: av(None),
        standard_deliverable: av(true),
    };
    let quote = Quote {
        schema_version: QuoteVersion::V1,
        instrument_id: "SPY".into(),
        currency,
        time,
        origin: QuoteOrigin::Provider,
        source_profile: "fixture".into(),
        indicative: av(false),
        bid: av(number("99")),
        ask: av(number("100")),
    };
    let facts = Facts {
        entry_id: "entry".into(),
        account,
        metadata,
        quote,
        receipts: BTreeMap::new(),
        account_key: "shared.fixture.paper".into(),
        canonical_account: None,
        account_binding_sha256: None,
        available: number("100"),
        valid_until: service.runtime().clock.now_ms() + 30000,
        capital_checkpoint_sha256: None,
        gross_floor: Decimal::ZERO,
    };
    let policy = Policy {
        schema_version: "tradeassembly.order_plan_policy.v1".into(),
        calculation_node: "calc".into(),
        semantics_version: "fixture.v1".into(),
        source_nodes: BTreeMap::from([
            ("account_data".into(), "account".into()),
            ("instrument_metadata".into(), "metadata".into()),
            ("pricing_quote".into(), "quote".into()),
        ]),
        parameters: json!({"quantity":"0.6"}),
        position_action: PositionAction::BuyToOpen,
        time_in_force: "day".into(),
        fee_cash_cap: number("1"),
        max_purchase_cash: number("100"),
        max_premium_gross: number("200"),
        max_quantity: number("10"),
        maximum_ask_multiple: number("1"),
        max_signal_age_ms: 600000,
        max_snapshot_age_ms: 30000,
        quote_profiles: vec!["fixture".into()],
    };
    (dir, service, facts, policy)
}
pub(super) fn context(id: &str) -> SideEffectContext {
    SideEffectContext::new(
        AuthorityContext::local_cli(),
        IdempotencyKey::new(id).unwrap(),
    )
}
pub(super) fn plan(s: &TradeAssemblyService, f: &Facts, p: &Policy, id: &str) -> OrderPlan {
    let order = BrokerOrderV2 {
        schema_version: OrderVersion::V2,
        instrument: InstrumentIdentity {
            instrument_id: "SPY".into(),
            provider_symbol: "SPY".into(),
            asset_class: "equity".into(),
            instrument_family: "equity".into(),
            quantity_unit: QuantityUnit::Shares,
            multiplier: number("1"),
            quote_currency: "USD".into(),
        },
        position_action: PositionAction::BuyToOpen,
        quantity: number("0.6"),
        pricing: PriceInstruction::Limit {
            limit_price: number("100"),
        },
        time_in_force: "day".into(),
        client_order_id: format!("fixture-{id}"),
    };
    let mut r = OrderPlan {
        management: None,
        plan_id: String::new(),
        entry_id: id.into(),
        command: PrepareCommand {
            scope: data_plans::PlanScope {
                strategy_id: "unit.strategy".into(),
                strategy_version_id: "fixture.v1".into(),
                strategy_spec_hash: "sha256:fixture".into(),
            },
            capability_graph_revision: "fixture.graph".into(),
            mode: "paper".into(),
            reserve_cash: true,
            signal_receipt_sha256: id.into(),
            account_collection_id: "fixture.account".into(),
            metadata_collection_id: "fixture.metadata".into(),
            quote_collection_id: "fixture.quote".into(),
        },
        policy: p.clone(),
        calculation: data_binding::PinnedOperation {
            plugin_instance_ref: "fixture".into(),
            plugin_ref: "fixture".into(),
            operation_id: "calc".into(),
            package_sha256: "a".repeat(64),
            manifest_sha256: "b".repeat(64),
            context_sha256: "c".repeat(64),
            account_ref: None,
        },
        source_receipts: BTreeMap::new(),
        account_key: f.account_key.clone(),
        canonical_account: f.canonical_account.clone(),
        account_binding_sha256: f.account_binding_sha256.clone(),
        account_currency: Some(f.account.currency.clone()),
        capital_checkpoint_sha256: f.capital_checkpoint_sha256.clone(),
        owner_sha256: owner(s).unwrap(),
        prepared_at_ms: s.runtime().clock.now_ms(),
        valid_until_ms: f.valid_until,
        available_cash: number("100"),
        purchase_cash: risk(p, f, &order, number("100")).unwrap(),
        order,
        calculation_response_sha256: "d".repeat(64),
    };
    r.plan_id = format!("orderplan_{}", hash(&r).unwrap());
    r
}
#[test]
fn competing_account_reservations_fence_loser_and_recomputed_cash_refuses_overspend() {
    let (_dir, s, f, p) = fixture();
    let second =
        TradeAssemblyService::from_runtime(s.db(), crate::adapters::local::test_runtime(s.db()))
            .for_authenticated_invocation("fixture", "cash-owner", None, None);
    let a = plan(&s, &f, &p, "a");
    let b = plan(&second, &f, &p, "b");
    assert_eq!(a.purchase_cash, number("61"));
    let (old_a, ledger_a, _) = cash(&s, &f).unwrap();
    let (old_b, ledger_b, _) = cash(&second, &f).unwrap();
    persist_prepared(&s, &a, "a", old_a, ledger_a, &context("a")).unwrap();
    assert_eq!(
        persist_prepared(&second, &b, "b", old_b, ledger_b, &context("b")).unwrap_err(),
        "order_plan_commit_conflict"
    );
    assert!(second
        .runtime()
        .storage
        .get_json(PLANS, &b.plan_id)
        .unwrap()
        .is_none());
    assert!(second
        .runtime()
        .storage
        .get_json(ENTRIES, "b")
        .unwrap()
        .is_none());
    let (_, ledger, available) = cash(&second, &f).unwrap();
    assert_eq!(ledger.reservations.len(), 1);
    assert_eq!(available, number("39"));
    assert_eq!(
        risk(&p, &f, &b.order, available).unwrap_err(),
        "order_plan_risk_limit_exceeded"
    );
}
#[test]
fn failed_cash_commit_rolls_back_plan_indexes_and_retry_is_atomic() {
    let (_dir, s, f, p) = fixture();
    let a = plan(&s, &f, &p, "a");
    let db = rusqlite::Connection::open(s.db()).unwrap();
    db.execute_batch("CREATE TRIGGER fail_cash BEFORE INSERT ON tradeassembly_kv WHEN NEW.namespace='execution_cash_reservations_v1' BEGIN SELECT RAISE(ABORT,'authored cash commit failure'); END;").unwrap();
    let (old, ledger, _) = cash(&s, &f).unwrap();
    assert_eq!(
        persist_prepared(&s, &a, "a", old, ledger, &context("a")).unwrap_err(),
        "order_plan_commit_failed"
    );
    for ns in [PLANS, KEYS, ENTRIES, COMMANDS, CASH] {
        assert!(
            s.runtime().storage.list_json(ns).unwrap().is_empty(),
            "partial commit: {ns}"
        );
    }
    db.execute_batch("DROP TRIGGER fail_cash;").unwrap();
    let (old, ledger, _) = cash(&s, &f).unwrap();
    persist_prepared(&s, &a, "a", old, ledger, &context("a")).unwrap();
    assert!(held(&s, &a).unwrap());
    assert_eq!(read_owned(&s, &a.plan_id).unwrap(), a);
}
#[test]
fn missing_price_and_fractional_contracts_never_become_an_executable_proposal() {
    let (_dir, s, mut f, p) = fixture();
    let a = plan(&s, &f, &p, "a");
    f.quote.ask = Observed::Unavailable {
        reason: "missing_fixture_quote".into(),
    };
    assert_eq!(
        risk(&p, &f, &a.order, number("100")).unwrap_err(),
        "execution_observation_unavailable"
    );
    f.quote.ask = av(number("100"));
    f.metadata.quantity_unit = av(QuantityUnit::Contracts);
    let mut order = a.order;
    order.instrument.quantity_unit = QuantityUnit::Contracts;
    assert_eq!(
        risk(&p, &f, &order, number("100")).unwrap_err(),
        "broker_order_v2_fractional_contract_unsupported"
    );
}

#[test]
fn negative_or_unknown_debt_never_counts_as_cash_funding() {
    let (_dir, _s, mut f, _p) = fixture();
    for debt in ["-1", "1"] {
        f.account.borrowing_debt = av(number(debt));
        assert_eq!(
            funding_capacity(&f.account).unwrap_err(),
            "order_plan_borrowing_unsupported"
        );
    }
    f.account.borrowing_debt = Observed::Unavailable {
        reason: "not_reported".into(),
    };
    assert_eq!(
        funding_capacity(&f.account).unwrap_err(),
        "execution_observation_unavailable"
    );
}

struct FixedClock(i64);
impl VersionedPort for FixedClock {
    fn descriptors(&self) -> Vec<PortDescriptor> {
        vec![]
    }
}
impl ClockPort for FixedClock {
    fn now_ms(&self) -> i64 {
        self.0
    }
}
#[test]
fn expired_plan_release_is_atomic_idempotent_and_fences_late_reservation() {
    let (_dir, s, f, p) = fixture();
    let a = plan(&s, &f, &p, "release");
    let (old, ledger, _) = cash(&s, &f).unwrap();
    persist_prepared(&s, &a, "release", old, ledger, &context("prepare")).unwrap();
    let mut runtime = (*s.runtime()).clone();
    runtime.clock = std::sync::Arc::new(FixedClock(a.valid_until_ms + 1));
    let later = TradeAssemblyService::from_runtime(s.db(), runtime).for_authenticated_invocation(
        "fixture",
        "cash-owner",
        None,
        None,
    );
    let original = release(&later, &a.plan_id, &context("release")).unwrap();
    assert_eq!(original["releasedCash"], "61");
    assert_eq!(original["brokerDispatchAllowed"], false);
    assert_eq!(
        release(&later, &a.plan_id, &context("retry")).unwrap(),
        original
    );
    assert_eq!(
        current(&later, &a.plan_id).unwrap_err(),
        "order_plan_reservation_released"
    );
    assert_eq!(
        reserve(&later, &a.plan_id, &context("late-reserve")).unwrap_err(),
        "order_plan_reservation_released"
    );
    // A writer checked the original plan before cancellation, then read fresh
    // cash after cancellation. The terminal fence must defeat its final CAS.
    let (cash_after, mut ledger, available) = cash(&later, &f).unwrap();
    assert_eq!(available, number("100"));
    ledger
        .reservations
        .insert(a.plan_id.clone(), a.purchase_cash);
    assert_ne!(
        commit_reservation(&later, &a, cash_after, &ledger, &context("stale-writer")).unwrap(),
        ComparePutOutcome::Updated
    );
    assert!(!held(&later, &a).unwrap());
    assert_eq!(
        release(&later, &a.plan_id, &context("last")).unwrap(),
        original
    );
}
#[test]
fn release_commit_failure_rolls_back_cash_and_terminal_fence_together() {
    let (_dir, s, f, p) = fixture();
    let a = plan(&s, &f, &p, "release-fault");
    let (old, ledger, _) = cash(&s, &f).unwrap();
    persist_prepared(&s, &a, "release-fault", old, ledger, &context("prepare")).unwrap();
    let db = rusqlite::Connection::open(s.db()).unwrap();
    db.execute_batch("CREATE TRIGGER fail_release BEFORE INSERT ON tradeassembly_kv WHEN NEW.namespace='execution_cash_releases_v1' BEGIN SELECT RAISE(ABORT,'authored release commit failure'); END;").unwrap();
    assert_eq!(
        release(&s, &a.plan_id, &context("release-fault")).unwrap_err(),
        "order_plan_release_commit_failed"
    );
    assert!(held(&s, &a).unwrap());
    assert!(s.runtime().storage.list_json(RELEASES).unwrap().is_empty());
    db.execute_batch("DROP TRIGGER fail_release;").unwrap();
    assert_eq!(
        release(&s, &a.plan_id, &context("recover")).unwrap()["releasedCash"],
        "61"
    );
    assert!(!held(&s, &a).unwrap());
}

fn lifecycle_fixture(
    id: &str,
) -> (
    tempfile::TempDir,
    TradeAssemblyService,
    Facts,
    OrderPlan,
    submission::OrderPlanSource,
) {
    let (dir, s, f, p) = fixture();
    let mut a = plan(&s, &f, &p, id);
    a.canonical_account = Some(
        crate::broker_submission::account_identity::CanonicalAccountIdentity {
            schema_version: "tradeassembly.canonical_account.v1".into(),
            broker_namespace: "fixture.broker".into(),
            environment: "paper".into(),
            provider_account_id: "fixture".into(),
            identity_sha256: "a".repeat(64),
        },
    );
    a.account_binding_sha256 = Some("b".repeat(64));
    a.plan_id.clear();
    a.plan_id = format!("orderplan_{}", hash(&a).unwrap());
    let (old, ledger, _) = cash(&s, &f).unwrap();
    persist_prepared(&s, &a, id, old, ledger, &context(id)).unwrap();
    let source = submission::source(&s, &a.plan_id).unwrap();
    (dir, s, f, a, source)
}
fn begin_lifecycle(p: &OrderPlan, source: &submission::OrderPlanSource, key: &str) {
    source
        .begin(
            p,
            json!({"planId":p.plan_id,"idempotencyKey":key}),
            &context(key),
        )
        .unwrap();
}
fn observed(
    p: &OrderPlan,
    status: tradeassembly_plugin_sdk::broker_v2::OrderStatus,
    filled: Observed<Decimal>,
    spent: Observed<Decimal>,
    now: i64,
) -> tradeassembly_plugin_sdk::broker_v2::OrderObservation {
    tradeassembly_plugin_sdk::broker_v2::OrderObservation {
        schema_version: "tradeassembly.broker_order_observation.v2".into(),
        canonical_account_sha256: p
            .canonical_account
            .as_ref()
            .unwrap()
            .identity_sha256
            .clone(),
        order: p.order.clone(),
        provider_order_id: Some("fixture.provider.order".into()),
        status,
        filled_quantity: filled,
        cash_spent: spent,
        observed_at_ms: now,
        known_at_ms: now,
    }
}
#[test]
fn unknown_acceptance_is_not_releasable_and_definitive_rejection_releases_once() {
    use tradeassembly_plugin_sdk::broker_v2::OrderStatus;
    let (_dir, s, f, p, source) = lifecycle_fixture("ambiguous");
    let ctx = context("submit");
    begin_lifecycle(&p, &source, "submit");
    source.admitted(&ctx).unwrap();
    source.before_send(&ctx).unwrap();
    assert!(
        source.abort_unsent(&ctx).is_err(),
        "a recovery cannot cancel once sending won the CAS"
    );
    let unknown = Observed::Unavailable {
        reason: "timeout_after_possible_acceptance".into(),
    };
    source
        .observe(
            observed(
                &p,
                OrderStatus::Unknown,
                unknown.clone(),
                unknown,
                s.runtime().clock.now_ms(),
            ),
            &ctx,
        )
        .unwrap();
    assert_eq!(
        release(&s, &p.plan_id, &context("release")).unwrap_err(),
        "order_plan_submission_not_releasable"
    );
    assert!(held(&s, &p).unwrap());
    assert!(source
        .begin(
            &p,
            json!({"planId":p.plan_id,"idempotencyKey":"new"}),
            &context("new")
        )
        .is_err());
    let result = source
        .observe(
            observed(
                &p,
                OrderStatus::Rejected,
                av(Decimal::ZERO),
                av(Decimal::ZERO),
                s.runtime().clock.now_ms(),
            ),
            &ctx,
        )
        .unwrap();
    assert_eq!(result.phase, submission::Phase::Rejected);
    assert!(!held(&s, &p).unwrap());
    assert_eq!(cash(&s, &f).unwrap().2, number("100"));
    assert_eq!(
        release(&s, &p.plan_id, &context("release-known")).unwrap()["releasedCash"],
        "0"
    );
}
#[test]
fn partial_and_filled_consume_cash_without_premature_budget_reuse() {
    use tradeassembly_plugin_sdk::broker_v2::OrderStatus;
    let (_dir, s, f, p, source) = lifecycle_fixture("fills");
    let ctx = context("fill-submit");
    begin_lifecycle(&p, &source, "fill-submit");
    source.admitted(&ctx).unwrap();
    source.before_send(&ctx).unwrap();
    source
        .observe(
            observed(
                &p,
                OrderStatus::PartiallyFilled,
                av(number("0.2")),
                av(number("20")),
                s.runtime().clock.now_ms(),
            ),
            &ctx,
        )
        .unwrap();
    let before = source.state().unwrap().unwrap();
    let mut wrong = observed(
        &p,
        OrderStatus::Filled,
        av(number("0.6")),
        av(number("60")),
        s.runtime().clock.now_ms(),
    );
    wrong.canonical_account_sha256 = "f".repeat(64);
    assert!(source.observe(wrong, &ctx).is_err());
    assert_eq!(source.state().unwrap().unwrap(), before);
    let result = source
        .observe(
            observed(
                &p,
                OrderStatus::Filled,
                av(number("0.6")),
                av(number("60")),
                s.runtime().clock.now_ms(),
            ),
            &ctx,
        )
        .unwrap();
    assert_eq!(result.phase, submission::Phase::Consumed);
    assert_eq!(result.retained_cash, number("61"));
    assert_eq!(cash(&s, &f).unwrap().2, number("39"));
    assert!(release(&s, &p.plan_id, &context("no-credit-for-spent-cash")).is_err());
}
#[test]
fn admission_and_release_races_are_fenced_and_unsent_abort_prevents_send() {
    let (_dir, s, _f, p, source) = lifecycle_fixture("cancel-wins");
    release(&s, &p.plan_id, &context("release-first")).unwrap();
    assert!(source
        .begin(
            &p,
            json!({"planId":p.plan_id,"idempotencyKey":"late"}),
            &context("late")
        )
        .is_err());
    assert!(source.state().unwrap().is_none());
    let (_dir, s, _f, p, source) = lifecycle_fixture("admit-wins");
    begin_lifecycle(&p, &source, "active");
    assert!(release(&s, &p.plan_id, &context("late-release")).is_err());
    source.abort_unsent(&context("active")).unwrap();
    assert!(source.admitted(&context("active")).is_err());
    assert!(source.before_send(&context("active")).is_err());
    assert_eq!(
        release(&s, &p.plan_id, &context("safe-unsent-release")).unwrap()["releasedCash"],
        "61"
    );
}
#[test]
fn intent_and_submission_state_commit_atomically_before_c5() {
    let (_dir, s, _f, p, source) = lifecycle_fixture("intent-fault");
    let db = rusqlite::Connection::open(s.db()).unwrap();
    db.execute_batch("CREATE TRIGGER fail_v2_intent BEFORE INSERT ON tradeassembly_kv WHEN NEW.namespace='broker_order_intents' BEGIN SELECT RAISE(ABORT,'authored intent commit failure'); END;").unwrap();
    assert!(source
        .begin(
            &p,
            json!({"planId":p.plan_id,"idempotencyKey":"fault"}),
            &context("fault")
        )
        .is_err());
    assert!(source.state().unwrap().is_none());
    assert!(held(&s, &p).unwrap());
    db.execute_batch("DROP TRIGGER fail_v2_intent;").unwrap();
    begin_lifecycle(&p, &source, "fault");
    assert_eq!(
        source.state().unwrap().unwrap().phase,
        submission::Phase::Admitting
    );
}

#[test]
fn older_broker_observation_cannot_release_a_newer_live_hold() {
    use tradeassembly_plugin_sdk::broker_v2::OrderStatus;
    let (_d, s, _f, p, source) = lifecycle_fixture("stale-observation");
    let ctx = context("submit");
    begin_lifecycle(&p, &source, "submit");
    source.admitted(&ctx).unwrap();
    source.before_send(&ctx).unwrap();
    let now = s.runtime().clock.now_ms();
    source
        .observe(
            observed(
                &p,
                OrderStatus::Open,
                av(Decimal::ZERO),
                av(Decimal::ZERO),
                now,
            ),
            &ctx,
        )
        .unwrap();
    let before = source.state().unwrap().unwrap();
    assert_eq!(
        source
            .observe(
                observed(
                    &p,
                    OrderStatus::Rejected,
                    av(Decimal::ZERO),
                    av(Decimal::ZERO),
                    now - 1
                ),
                &ctx
            )
            .unwrap_err(),
        "broker_v2_observation_regressed"
    );
    assert_eq!(source.state().unwrap().unwrap(), before);
    assert!(held(&s, &p).unwrap());
}

#[test]
fn recreated_service_retains_unknown_send_and_recovers_original_hold() {
    use tradeassembly_plugin_sdk::broker_v2::OrderStatus;
    let (_dir, s, _f, p, source) = lifecycle_fixture("restart");
    let ctx = context("submit");
    begin_lifecycle(&p, &source, "submit");
    source.admitted(&ctx).unwrap();
    source.before_send(&ctx).unwrap();
    let before = source.state().unwrap().unwrap();
    let db = s.db().to_string();
    drop(source);
    drop(s);
    let restored =
        TradeAssemblyService::from_runtime(&db, crate::adapters::local::test_runtime(&db))
            .for_authenticated_invocation("fixture", "cash-owner", None, None);
    let source = submission::source(&restored, &p.plan_id).unwrap();
    assert_eq!(source.state().unwrap().unwrap(), before);
    assert!(held(&restored, &p).unwrap());
    assert!(release(&restored, &p.plan_id, &context("restart-release")).is_err());
    assert!(source
        .begin(
            &p,
            json!({"planId":p.plan_id,"idempotencyKey":"retry"}),
            &context("retry")
        )
        .is_err());
    source
        .observe(
            observed(
                &p,
                OrderStatus::Rejected,
                av(Decimal::ZERO),
                av(Decimal::ZERO),
                restored.runtime().clock.now_ms(),
            ),
            &ctx,
        )
        .unwrap();
    assert!(!held(&restored, &p).unwrap());
    assert_eq!(source.state().unwrap().unwrap().original_key, "submit");
}

// These tests exercise the real atomic capital boundary with authored provider
// observations. They do not qualify provider acquisition, C3/C5 or OS sandboxing.
fn settlement_binding() -> crate::ports::broker_submission::BrokerPreparedBinding {
    crate::ports::broker_submission::BrokerPreparedBinding {
        plugin_instance_ref: "fixture".into(),
        plugin_ref: "fixture".into(),
        package_sha256: "a".repeat(64),
        manifest_fingerprint: "b".repeat(64),
        configuration_digest: "c".repeat(64),
        credential_ref: "fixture-only".into(),
        credential_generation: 1,
    }
}
fn settlement_fixture(
    id: &str,
) -> (
    tempfile::TempDir,
    TradeAssemblyService,
    Facts,
    OrderPlan,
    submission::OrderPlanSource,
) {
    let (dir, s, mut f, p, source) = lifecycle_fixture(id);
    f.account.provider_account_id = "fixture".into();
    source.begin(&p,json!({"planId":p.plan_id,"accountRef":f.account.account_ref,"idempotencyKey":"settlement-submit"}),&context("settlement-submit")).unwrap();
    source.admitted(&context("settlement-submit")).unwrap();
    source.before_send(&context("settlement-submit")).unwrap();
    (dir, s, f, p, source)
}
fn settlement_observation(
    s: &TradeAssemblyService,
    f: &Facts,
    p: &OrderPlan,
    partial: bool,
) -> tradeassembly_plugin_sdk::cash_settlement::Observation {
    use tradeassembly_plugin_sdk::{
        broker_v2::OrderStatus,
        cash_settlement::{AccountCheckpoint, Observation},
    };
    let now = s.runtime().clock.now_ms();
    let mut account = f.account.clone();
    account.time.observed_at_ms = now;
    account.time.known_at_ms = now;
    let (quantity, gross, fee, cash, status) = if partial {
        ("0.2", "20", "0.25", "79.75", OrderStatus::Cancelled)
    } else {
        ("0.6", "60", "0", "40", OrderStatus::Filled)
    };
    account.cash_balance = av(number(cash));
    account.non_marginable_buying_power = av(number(cash));
    account.gross_exposure = av(number(gross));
    account.positions = av(vec![
        tradeassembly_plugin_sdk::execution_snapshot::Position {
            instrument_id: "SPY".into(),
            side: HoldingSide::Long,
            quantity: number(quantity),
            gross_value: av(number(gross)),
        },
    ]);
    Observation {
        schema_version: "tradeassembly.cash_settlement_observation.v1".into(),
        canonical_account_sha256: "a".repeat(64),
        order: observed(
            p,
            status,
            av(number(quantity)),
            av(number(gross).checked_add(number(fee)).unwrap()),
            now,
        ),
        posting_id: "provider.final.posting".into(),
        posting_sequence: 2,
        gross_cash: number(gross),
        fee_cash: number(fee),
        fees_final: true,
        checkpoint: AccountCheckpoint {
            ledger_epoch: "provider.epoch.1".into(),
            sequence: 2,
            account,
        },
    }
}
fn commit_settlement(
    s: &TradeAssemblyService,
    p: &OrderPlan,
    o: tradeassembly_plugin_sdk::cash_settlement::Observation,
    key: &str,
) -> Result<Value, String> {
    settlement::commit(
        s,
        p,
        settlement_binding(),
        "d".repeat(64),
        o,
        key,
        &context(key),
    )
}
fn rebased_facts(s: &TradeAssemblyService, f: &Facts) -> Facts {
    let mut f = f.clone();
    let (available, gross, digest) =
        settlement::funding_floor(s, &f.account_key, number("100")).unwrap();
    f.available = available;
    f.gross_floor = gross;
    f.capital_checkpoint_sha256 = digest;
    f
}
#[test]
fn settlement_final_posting_rebases_stale_cash_and_survives_service_restart() {
    let (_dir, s, f, p, source) = settlement_fixture("settled");
    let o = settlement_observation(&s, &f, &p, false);
    source
        .observe(o.order.clone(), &context("settlement-submit"))
        .unwrap();
    assert_eq!(source.state().unwrap().unwrap().retained_cash, number("61"));
    let result = commit_settlement(&s, &p, o.clone(), "settle").unwrap();
    assert_eq!(result["releasedHold"], "61");
    let db = s.db().to_string();
    drop(source);
    drop(s);
    let restored =
        TradeAssemblyService::from_runtime(&db, crate::adapters::local::test_runtime(&db))
            .for_authenticated_invocation("fixture", "cash-owner", None, None);
    let source = submission::source(&restored, &p.plan_id).unwrap();
    let state = source.state().unwrap().unwrap();
    assert_eq!(state.retained_cash, Decimal::ZERO);
    assert!(!held(&restored, &p).unwrap());
    assert_eq!(
        cash(&restored, &f).unwrap_err(),
        "order_plan_capital_checkpoint_changed"
    );
    let mut updated = rebased_facts(&restored, &f);
    assert_eq!(updated.available, number("40"));
    assert_eq!(updated.gross_floor, number("60"));
    assert_eq!(cash(&restored, &updated).unwrap().2, number("40"));
    let mut next = p.order.clone();
    next.quantity = number("0.3");
    let mut policy = p.policy.clone();
    policy.max_premium_gross = number("85");
    assert_eq!(
        risk(&policy, &updated, &next, number("40")).unwrap_err(),
        "order_plan_risk_limit_exceeded"
    );
    policy.max_premium_gross = number("200");
    let a = plan(&restored, &updated, &policy, "reuse-a");
    let b = plan(&restored, &updated, &policy, "reuse-b");
    let mut a = a;
    let mut b = b;
    // Recompute actual affordable orders and immutable identities, never inflate balance.
    for p in [&mut a, &mut b] {
        p.order.quantity = number("0.3");
        p.purchase_cash = risk(&p.policy, &updated, &p.order, number("40")).unwrap();
        p.plan_id.clear();
        p.plan_id = format!("orderplan_{}", hash(p).unwrap());
    }
    let (old_a, ledger_a, _) = cash(&restored, &updated).unwrap();
    let (old_b, ledger_b, _) = cash(&restored, &updated).unwrap();
    persist_prepared(
        &restored,
        &a,
        "reuse-a",
        old_a,
        ledger_a,
        &context("reuse-a"),
    )
    .unwrap();
    assert!(persist_prepared(
        &restored,
        &b,
        "reuse-b",
        old_b,
        ledger_b,
        &context("reuse-b")
    )
    .is_err());
    assert_eq!(cash(&restored, &updated).unwrap().2, number("9"));
    let replay = commit_settlement(&restored, &p, o, "settle-new-observation").unwrap();
    assert_eq!(replay["releasedHold"], "0");
    assert_eq!(
        source.state().unwrap().unwrap().settlement_receipt_sha256,
        state.settlement_receipt_sha256
    );
    updated = rebased_facts(&restored, &updated);
    assert_eq!(cash(&restored, &updated).unwrap().2, number("9"));
}
#[test]
fn settlement_partial_cancel_and_final_fees_release_only_after_complete_evidence() {
    let (_dir, s, f, p, source) = settlement_fixture("settled-partial");
    let o = settlement_observation(&s, &f, &p, true);
    let before = source.state().unwrap().unwrap();
    let cash_before = s.runtime().storage.get_json(CASH, &p.account_key).unwrap();
    for mutation in [
        "unknown",
        "fees",
        "incomplete",
        "unposted",
        "stale",
        "currency",
    ] {
        let mut bad = o.clone();
        match mutation {
            "unknown" => {
                bad.order.status = tradeassembly_plugin_sdk::broker_v2::OrderStatus::Unknown
            }
            "fees" => bad.fees_final = false,
            "incomplete" => bad.checkpoint.account.coverage.open_orders_complete = false,
            "unposted" => bad.posting_sequence = 3,
            "stale" => {
                bad.checkpoint.account.time.observed_at_ms -= 30000;
                bad.checkpoint.account.time.known_at_ms -= 30000;
            }
            "currency" => bad.checkpoint.account.currency.code = "EUR".into(),
            _ => unreachable!(),
        }
        assert!(
            commit_settlement(&s, &p, bad, mutation).is_err(),
            "{mutation}"
        );
        assert_eq!(source.state().unwrap().unwrap(), before);
        assert_eq!(
            s.runtime().storage.get_json(CASH, &p.account_key).unwrap(),
            cash_before
        );
    }
    commit_settlement(&s, &p, o.clone(), "settle-partial").unwrap();
    assert_eq!(rebased_facts(&s, &f).available, number("79.75"));
    assert_eq!(
        source.state().unwrap().unwrap().phase,
        submission::Phase::Consumed
    );
    let settled = source.state().unwrap().unwrap();
    for mutation in [
        "epoch",
        "sequence",
        "same-sequence-cash",
        "changed-final-fee",
    ] {
        let mut bad = o.clone();
        match mutation {
            "epoch" => bad.checkpoint.ledger_epoch = "different.epoch".into(),
            "sequence" => {
                bad.posting_sequence = 1;
                bad.checkpoint.sequence = 1;
            }
            "same-sequence-cash" => bad.checkpoint.account.cash_balance = av(number("100")),
            "changed-final-fee" => {
                bad.fee_cash = number("0.5");
                bad.order.cash_spent = av(number("20.5"));
                bad.checkpoint.sequence = 3;
            }
            _ => unreachable!(),
        };
        assert!(
            commit_settlement(&s, &p, bad, mutation).is_err(),
            "{mutation}"
        );
        assert_eq!(source.state().unwrap().unwrap(), settled);
    }
}
#[test]
fn settlement_commit_failure_keeps_hold_and_receipts_atomic() {
    let (_dir, s, f, p, source) = settlement_fixture("settlement-atomic");
    let o = settlement_observation(&s, &f, &p, false);
    let before = source.state().unwrap().unwrap();
    let ledger = s.runtime().storage.get_json(CASH, &p.account_key).unwrap();
    let db = rusqlite::Connection::open(s.db()).unwrap();
    db.execute_batch("CREATE TRIGGER fail_settlement BEFORE INSERT ON tradeassembly_kv WHEN NEW.namespace='execution_order_plan_submissions_v2' BEGIN SELECT RAISE(ABORT,'authored settlement failure'); END;").unwrap();
    assert!(commit_settlement(&s, &p, o.clone(), "atomic-settle").is_err());
    assert_eq!(source.state().unwrap().unwrap(), before);
    assert_eq!(
        s.runtime().storage.get_json(CASH, &p.account_key).unwrap(),
        ledger
    );
    for ns in [
        "execution_cash_settlement_receipts_v1",
        "execution_cash_settlement_keys_v1",
    ] {
        assert!(s.runtime().storage.list_json(ns).unwrap().is_empty());
    }
    db.execute_batch("DROP TRIGGER fail_settlement").unwrap();
    commit_settlement(&s, &p, o, "atomic-settle").unwrap();
    assert_eq!(
        source.state().unwrap().unwrap().retained_cash,
        Decimal::ZERO
    );
}
#[test]
fn settlement_keeps_other_unknown_acceptance_hold_and_legacy_wire_bytes() {
    let (_dir, s, f, p, _source) = settlement_fixture("settlement-other");
    let mut other = plan(&s, &f, &p.policy, "other-unknown");
    other.order.quantity = number("0.2");
    other.purchase_cash = number("21");
    other.canonical_account = p.canonical_account.clone();
    other.account_binding_sha256 = p.account_binding_sha256.clone();
    other.plan_id.clear();
    other.plan_id = format!("orderplan_{}", hash(&other).unwrap());
    let (old, ledger, _) = cash(&s, &f).unwrap();
    persist_prepared(
        &s,
        &other,
        "other-unknown",
        old,
        ledger,
        &context("other-unknown"),
    )
    .unwrap();
    let source = submission::source(&s, &other.plan_id).unwrap();
    begin_lifecycle(&other, &source, "other-submit");
    source.admitted(&context("other-submit")).unwrap();
    source.before_send(&context("other-submit")).unwrap();
    let before = source.state().unwrap().unwrap();
    commit_settlement(
        &s,
        &p,
        settlement_observation(&s, &f, &p, true),
        "settlement-one-only",
    )
    .unwrap();
    let rebased = rebased_facts(&s, &f);
    assert_eq!(cash(&s, &rebased).unwrap().2, number("58.75"));
    assert_eq!(source.state().unwrap().unwrap(), before);
    let mut legacy = value(&p).unwrap();
    legacy.as_object_mut().unwrap().remove("accountCurrency");
    legacy
        .as_object_mut()
        .unwrap()
        .remove("capitalCheckpointSha256");
    let restored: OrderPlan = serde_json::from_value(legacy.clone()).unwrap();
    assert_eq!(value(&restored).unwrap(), legacy);
    let legacy_ledger =
        json!({"ownerSha256":owner(&s).unwrap(),"reservations":{},"grossLimit":"200"});
    let ledger: Ledger = serde_json::from_value(legacy_ledger.clone()).unwrap();
    assert_eq!(value(&ledger).unwrap(), legacy_ledger);
}
#[test]
fn settlement_generation_bound_refuses_before_any_capital_commit() {
    let (_dir, s, f, p, source) = settlement_fixture("settlement-generation");
    let mut record = source.state().unwrap().unwrap();
    record.generation = 10000;
    record.record_sha256 = submission::record_hash(&record).unwrap();
    s.runtime()
        .storage
        .put_json(
            submission::STATE,
            &p.plan_id,
            value(&record).unwrap(),
            &context("authored-generation-bound"),
        )
        .unwrap();
    let ledger = s.runtime().storage.get_json(CASH, &p.account_key).unwrap();
    assert_eq!(
        commit_settlement(
            &s,
            &p,
            settlement_observation(&s, &f, &p, false),
            "generation-exhausted"
        )
        .unwrap_err(),
        "cash_settlement_generation_invalid"
    );
    assert_eq!(source.state().unwrap().unwrap(), record);
    assert_eq!(
        s.runtime().storage.get_json(CASH, &p.account_key).unwrap(),
        ledger
    );
}

#[test]
fn prepared_health_hint_refuses_original_plan_expiry_and_real_release_fence() {
    let (_dir, s, f, policy) = fixture();
    let p = plan(&s, &f, &policy, "prepared-health-expiry");
    let (old, ledger, _) = cash(&s, &f).unwrap();
    persist_prepared(
        &s,
        &p,
        "prepared-health-expiry",
        old,
        ledger,
        &context("prepare-health-hint"),
    )
    .unwrap();
    let command = crate::service::execution::typed_cycle::Command {
        scope: p.command.scope.clone(),
        activation_id: "hint-activation".into(),
        occurrence_id: "hint-occurrence".into(),
        plan_revision_id: "hint-plan".into(),
        signal_receipt_sha256: p.command.signal_receipt_sha256.clone(),
    };
    let mut state = json!({"schemaVersion":"tradeassembly.typed_execution_cycle.v1","ownerSha256":owner(&s).unwrap(),"command":command,
        "configSha256":"hint-config","phase":5,"status":"running","collections":{},"planId":p.plan_id,"submission":null,"error":null,"stateSha256":""});
    state["stateSha256"] = json!(hash(&state).unwrap());
    s.runtime()
        .storage
        .put_json(
            "typed_execution_cycles_v1",
            &hash(&command).unwrap(),
            state,
            &context("persist-health-hint"),
        )
        .unwrap();
    let mut runtime = (*s.runtime()).clone();
    runtime.clock = std::sync::Arc::new(FixedClock(p.valid_until_ms));
    let later = TradeAssemblyService::from_runtime(s.db(), runtime).for_authenticated_invocation(
        "fixture",
        "cash-owner",
        None,
        None,
    );
    assert!(
        crate::service::execution::typed_cycle::prepared_plan_for_health(
            &later,
            &command,
            "hint-config"
        )
        .unwrap()
        .is_none(),
        "original exclusive plan expiry must not create a hint"
    );
    release(&s, &p.plan_id, &context("release-health-hint")).unwrap();
    assert!(
        crate::service::execution::typed_cycle::prepared_plan_for_health(
            &s,
            &command,
            "hint-config"
        )
        .unwrap()
        .is_none(),
        "an actual durable unsent-plan release must refuse before source work"
    );
    assert!(s
        .runtime()
        .storage
        .list_json("plugin_operation_requests")
        .unwrap()
        .is_empty());
    assert!(s
        .runtime()
        .storage
        .list_json(submission::STATE)
        .unwrap()
        .is_empty());
}
