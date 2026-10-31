use super::*;
fn av<T>(value: T) -> Observed<T> {
    Observed::Available { value }
}
fn number(v: &str) -> Decimal {
    v.parse().unwrap()
}
fn fixture() -> (tempfile::TempDir, TradeAssemblyService, Facts, Policy) {
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
        available: number("100"),
        valid_until: service.runtime().clock.now_ms() + 30000,
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
fn context(id: &str) -> SideEffectContext {
    SideEffectContext::new(
        AuthorityContext::local_cli(),
        IdempotencyKey::new(id).unwrap(),
    )
}
fn plan(s: &TradeAssemblyService, f: &Facts, p: &Policy, id: &str) -> OrderPlan {
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
