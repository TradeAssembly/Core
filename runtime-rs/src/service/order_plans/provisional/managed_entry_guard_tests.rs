// Copyright (c) 2026 OptionLab LLC. All rights reserved.

//! Service-storage regression only: direct private commit authors synthetic
//! observations, not a provider/C3/C5/process qualification or broker permission.
use super::*;
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};

#[derive(Default)]
struct NoProvider(AtomicUsize);
impl VersionedPort for NoProvider {
    fn descriptors(&self) -> Vec<PortDescriptor> {
        Vec::new()
    }
}
impl PluginOperationPort for NoProvider {
    fn invoke(
        &self,
        _: &PluginOperationRequest,
        _: &SideEffectContext,
    ) -> Result<PluginOperationResponse, String> {
        self.0.fetch_add(1, Ordering::SeqCst);
        Err("unexpected_provider_call_in_blocked_entry_guard".into())
    }
}
type StoredRow = (String, String, String, String, String, String);
type StoredState = (Vec<StoredRow>, i64);
fn stored_state(s: &TradeAssemblyService) -> StoredState {
    let db =
        rusqlite::Connection::open_with_flags(s.db(), rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
            .unwrap();
    let mut statement = db.prepare("SELECT namespace,item_key,value_json,idempotency_key,authority_actor,CAST(updated_at_ms AS TEXT) FROM tradeassembly_kv ORDER BY namespace,item_key").unwrap();
    let rows = statement
        .query_map([], |row| {
            Ok((
                row.get(0)?,
                row.get(1)?,
                row.get(2)?,
                row.get(3)?,
                row.get(4)?,
                row.get(5)?,
            ))
        })
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    // LocalJournal uses runtime_events; it is created lazily by the operations
    // adapter, so an untouched fixture can legitimately have no event table yet.
    let has_events: bool = db.query_row("SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='runtime_events')", [], |row|row.get(0)).unwrap();
    let journal_count = if has_events {
        db.query_row("SELECT COUNT(*) FROM runtime_events", [], |row| row.get(0))
            .unwrap()
    } else {
        0
    };
    (rows, journal_count)
}
fn filled_observation(q: &wire::Request, time: i64, average: &str) -> wire::Observation {
    let unavailable = json!({"status":"unavailable","reason":"authored_unit_observation_only"});
    let o: wire::Observation = serde_json::from_value(json!({
        "schemaVersion":wire::OBSERVATION_VERSION,"requestSha256":q.request_sha256().unwrap(),
        "status":"provisional","coherence":"non_atomic",
        "order":{"schemaVersion":"tradeassembly.broker_order_observation.v2",
            "canonicalAccountSha256":q.lookup.original_submit.canonical_account_sha256,
            "order":q.lookup.original_submit.order,"providerOrderId":"fixture.entry.order","status":"filled",
            "filledQuantity":{"status":"available","value":q.lookup.original_submit.order.quantity},
            "cashSpent":unavailable,"observedAtMs":time,"knownAtMs":time+1},
        "averageFillPrice":{"status":"available","value":{"basis":"provider_reported","price":average,
            "currency":{"code":"USD","decimals":2},"evidenceIds":["order"]}},
        "feeCash":unavailable,"feeFinality":unavailable,"account":unavailable,
        "evidence":[{"id":"order","kind":"order","providerRecordId":unavailable,"hashBasis":"raw_response_bytes",
            "responseSha256":hash(&(time,average)).unwrap(),"startedAtMs":time,"completedAtMs":time+2}]
    })).unwrap();
    o.validate(q, time + 2).unwrap();
    o
}

#[test]
fn blocked_reported_entry_refuses_static_price_advance_and_c5_snapshot_without_effects() {
    let (_dir, service, mut facts, policy) = super::super::tests::fixture();
    let provider = Arc::new(NoProvider::default());
    let mut runtime = (*service.runtime()).clone();
    runtime.plugin_operations = provider.clone();
    let s = service.with_test_runtime(runtime);
    let ctx = super::super::tests::context("blocked-entry-guard");
    let expected = broker_lookup_v3::ExpectedPaperAccount {
        broker_namespace: "fixture.broker".into(),
        environment: broker_lookup_v3::PaperEnvironment::Paper,
        provider_account_id: facts.account.provider_account_id.clone(),
    };
    facts.canonical_account = Some(
        crate::broker_submission::account_identity::CanonicalAccountIdentity {
            schema_version: "tradeassembly.canonical_account.v1".into(),
            broker_namespace: expected.broker_namespace.clone(),
            environment: "paper".into(),
            provider_account_id: expected.provider_account_id.clone(),
            identity_sha256: expected.canonical_account_sha256().unwrap(),
        },
    );
    facts.account_binding_sha256 = Some("e".repeat(64));
    let entry = super::super::tests::plan(&s, &facts, &policy, "reported-entry");
    let (old, ledger, _) = cash(&s, &facts).unwrap();
    persist_prepared(&s, &entry, "blocked-entry-prepare", old, ledger, &ctx).unwrap();
    let binding = BrokerPreparedBinding {
        plugin_instance_ref: "fixture.broker.instance".into(),
        plugin_ref: "fixture.broker".into(),
        package_sha256: "a".repeat(64),
        manifest_fingerprint: "b".repeat(64),
        configuration_digest: hash(&json!({})).unwrap(),
        credential_ref: "fixture-unused-no-credential".into(),
        credential_generation: 1,
    };
    let submit = broker_v2::SubmitRequest {
        schema_version: "tradeassembly.broker_submit.v2".into(),
        plan_id: entry.plan_id.clone(),
        canonical_account_sha256: expected.canonical_account_sha256().unwrap(),
        order: entry.order.clone(),
    };
    submit.validate().unwrap();
    let activation = "fixture.reported.activation";
    s.bind_owned_object("execution_activation", activation)
        .unwrap();
    let original = PluginOperationRequest {
        correlation_id: "fixture.original.submit".into(),
        plugin_instance_ref: binding.plugin_instance_ref.clone(),
        plugin_ref: binding.plugin_ref.clone(),
        manifest_fingerprint: binding.manifest_fingerprint.clone(),
        operation_id: crate::broker_submission::v2::SUBMIT_OPERATION.into(),
        capability: crate::broker_submission::v2::SUBMIT_CAPABILITY.into(),
        capability_graph_revision_id: entry.command.capability_graph_revision.clone(),
        capability_graph_fingerprint: "c".repeat(64),
        strategy_id: entry.command.scope.strategy_id.clone(),
        strategy_version_id: entry.command.scope.strategy_version_id.clone(),
        strategy_spec_hash: entry.command.scope.strategy_spec_hash.clone(),
        activation_id: activation.into(),
        attempt_id: "fixture.attempt".into(),
        evaluation_tick_id: "fixture.tick".into(),
        mode: "paper".into(),
        purpose: "execution".into(),
        account_ref: Some(facts.account.account_ref.clone()),
        timeout_ms: 15000,
        fencing_token: Some(1),
        input: value(&submit).unwrap(),
        evidence_refs: vec![],
    };
    let intent = json!({"planId":entry.plan_id,"idempotencyKey":ctx.idempotency_key.as_str(),
        "accountRef":facts.account.account_ref,"preparedBinding":binding,"originalRequest":original});
    let mut record = submission::SubmissionRecord {
        schema_version: "tradeassembly.broker_submission_state.v2".into(),
        plan_id: entry.plan_id.clone(),
        owner_sha256: owner(&s).unwrap(),
        account_key: entry.account_key.clone(),
        original_key: ctx.idempotency_key.as_str().into(),
        intent_sha256: hash(&intent).unwrap(),
        intent,
        phase: submission::Phase::Sending,
        generation: 0,
        retained_cash: entry.purchase_cash,
        filled_quantity: Decimal::ZERO,
        provider_order_id: None,
        observation: None,
        settlement_receipt_sha256: None,
        record_sha256: String::new(),
    };
    record.record_sha256 = submission::record_hash(&record).unwrap();
    s.runtime()
        .storage
        .put_json(
            submission::STATE,
            &entry.plan_id,
            value(&record).unwrap(),
            &ctx,
        )
        .unwrap();
    assert_eq!(submission::record(&s, &entry).unwrap().unwrap().1, record);
    let request = request(&entry, &record).unwrap();
    assert_eq!(request.lookup.expected_account, expected);
    let time = s.runtime().clock.trusted_now_ms().unwrap() - 1000;
    let first = filled_observation(&request, time, "100");
    let accepted = commit(
        &s,
        &entry,
        &record,
        Acquired {
            binding: binding.clone(),
            request: request.clone(),
            response_hash: hash(&first).unwrap(),
            observation: first,
        },
        "accepted-entry",
        &ctx,
    )
    .unwrap();
    assert_eq!(accepted["status"], "provisional");
    assert_eq!(accepted["cashReconciled"], false);
    let (quantity, basis, pin, entry_time) = entry_basis(&s, &entry, None).unwrap();
    assert_eq!(quantity, entry.order.quantity);
    assert_eq!(basis, "100".parse::<Decimal>().unwrap());
    // Even a valid terminal reported gross fill cannot become crypto net
    // ownership. Preserve the original accepted observation and all state.
    let before_net_refusal = stored_state(&s);
    let mut crypto = entry.clone();
    crypto.order.instrument.asset_class = "crypto".into();
    crypto.order.instrument.instrument_family = "crypto_spot".into();
    crypto.order.instrument.instrument_id = "BTC/USD".into();
    assert_eq!(
        entry_basis(&s, &crypto, None).unwrap_err(),
        "managed_order_attributed_net_inventory_required"
    );
    assert_eq!(
        super::super::management::require_reported_inventory_family("crypto_spot").unwrap_err(),
        "managed_order_attributed_net_inventory_required"
    );
    assert_eq!(stored_state(&s), before_net_refusal);
    assert_eq!(provider.0.load(Ordering::SeqCst), 0);
    let position_id = format!(
        "position_{}",
        hash(&(owner(&s).unwrap(), &entry.plan_id)).unwrap()
    );
    let mut position = json!({"schemaVersion":"tradeassembly.managed_position.v2","positionId":position_id,
        "ownerSha256":owner(&s).unwrap(),"entryPlanId":entry.plan_id,"activationId":activation,
        "entrySettlementSha256":"","entryObservationSha256":pin,"entryCashReconciled":false,
        "policy":{"entryBasis":"provider_reported_average_fill","rules":{"schemaVersion":"tradeassembly.position_exit_rules.v1",
            "priceBasis":"bid","stopLossRatio":"0.1","trailingRatio":null,"trailingStart":null,"reversalSignal":null,
            "precedence":["stop_loss"],"maxQuoteAgeMs":30000},"feeCashCap":"1","maxLimitSlippageRatio":"0.01",
            "timeInForce":"day","maxReversalAgeMs":30000},
        "quantity":quantity,"closedQuantity":"0","basis":basis,"entryTime":entry_time,
        "highWater":"120","lastQuoteKnownAtMs":0,"lastQuoteObservedAtMs":0,
        "lastReversalReceipt":null,"reversalWatermark":null,"lastQuoteReceipt":null,"reservations":{},
        "generation":0,"stateSha256":""});
    position["stateSha256"] = json!(hash(&position).unwrap());
    s.runtime()
        .storage
        .put_json(
            "execution_managed_positions_v1",
            &position_id,
            position.clone(),
            &ctx,
        )
        .unwrap();
    assert_eq!(
        management::inspect(&s, &position_id).unwrap()["position"],
        position,
        "the original owned V2 position must pass real load/seal/owner checks"
    );

    // A genuine reducer refusal, not a manually set blocked flag. The earlier
    // positive terminal fill remains pinned, but its current head is quarantined.
    let correction = filled_observation(&request, time + 100, "101");
    let blocked = commit(
        &s,
        &entry,
        &record,
        Acquired {
            binding,
            request,
            response_hash: hash(&correction).unwrap(),
            observation: correction,
        },
        "corrected-entry",
        &ctx,
    )
    .unwrap();
    assert_eq!(blocked["status"], "blocked");
    assert_eq!(
        blocked["blockedReason"],
        "settlement_projection_terminal_average_correction"
    );
    assert_eq!(
        entry_basis(&s, &entry, Some(&pin)).unwrap_err(),
        "provider_observation_reconciliation_blocked"
    );
    assert_eq!(s.runtime().storage.list_json(RECEIPTS).unwrap().len(), 2);
    let cash_before = s
        .runtime()
        .storage
        .get_json(CASH, &entry.account_key)
        .unwrap()
        .unwrap();
    assert_eq!(
        ledger_owned(&s, cash_before.clone())
            .unwrap()
            .reservations
            .get(&entry.plan_id),
        Some(&entry.purchase_cash)
    );

    // V7 historical inspection remains available for a disputed observed entry,
    // with no current run, policy, provider permission or new close authority.
    use management::provider_remote::{self, ProviderManagementOperation};
    let inspect = ProviderManagementOperation::Inspect {
        scope: entry.command.scope.clone(),
        position_id: position_id.clone(),
    };
    provider_remote::binding(&s, &inspect).unwrap();
    let response = provider_remote::inspect_response(
        &s,
        &json!({"scope":entry.command.scope,"positionId":position_id}),
    );
    assert_eq!(response.status, 200, "{}", response.body);
    let observe = ProviderManagementOperation::ObserveSettlement {
        scope: entry.command.scope.clone(),
        plan_id: entry.plan_id.clone(),
    };
    let observation_outcome =
        provider_remote::project(&s, &observe, &"d".repeat(64), accepted.clone()).unwrap();
    assert!(
        observation_outcome.position.is_none(),
        "later mutable position cannot become this original observation outcome"
    );
    assert!(observation_outcome.phase.is_none());
    assert!(observation_outcome.retained_cash.is_none());
    assert!(observation_outcome.filled_quantity.is_none());
    assert_eq!(
        observation_outcome
            .observation
            .as_ref()
            .unwrap()
            .receipt_sha256,
        pin
    );
    assert_eq!(
        observation_outcome
            .observation
            .as_ref()
            .unwrap()
            .head_receipt_sha256,
        pin
    );
    assert!(
        observation_outcome
            .observation
            .as_ref()
            .unwrap()
            .head_blocked_reason
            .is_none(),
        "later disputed head is not substituted for the original outcome"
    );
    let command_hash = "e".repeat(64);
    let projected = provider_remote::project(&s, &inspect, &command_hash, response.body).unwrap();
    assert_eq!(
        projected.position.as_ref().unwrap().closed_quantity,
        Decimal::ZERO
    );
    assert_eq!(
        projected
            .observation
            .as_ref()
            .unwrap()
            .head_blocked_reason
            .as_deref(),
        Some("settlement_projection_terminal_average_correction")
    );
    let mut later = position.clone();
    later["generation"] = json!(9);
    later["closedQuantity"] = later["quantity"].clone();
    later["stateSha256"] = json!("");
    later["stateSha256"] = json!(hash(&later).unwrap());
    s.runtime()
        .storage
        .put_json("execution_managed_positions_v1", &position_id, later, &ctx)
        .unwrap();
    assert_eq!(
        provider_remote::stored(&s, &inspect, &command_hash).unwrap(),
        Some(projected.clone()),
        "sealed original position image survives later state changes"
    );
    let foreign = s.for_authenticated_invocation("fixture", "other", None, None);
    assert!(provider_remote::stored(&foreign, &inspect, &command_hash).is_err());
    s.runtime()
        .storage
        .put_json(
            "execution_managed_positions_v1",
            &position_id,
            position.clone(),
            &ctx,
        )
        .unwrap();

    // No management run/control, occurrence, calendar, quote or metadata rows
    // exist: both real paths must refuse the entry head before touching those.
    for trailing in [false, true] {
        position["policy"]["rules"]["stopLossRatio"] =
            if trailing { Value::Null } else { json!("0.1") };
        position["policy"]["rules"]["trailingRatio"] =
            if trailing { json!("0.2") } else { Value::Null };
        position["policy"]["rules"]["trailingStart"] = if trailing {
            json!({"kind":"immediate"})
        } else {
            Value::Null
        };
        position["policy"]["rules"]["precedence"] = if trailing {
            json!(["trailing"])
        } else {
            json!(["stop_loss"])
        };
        position["stateSha256"] = json!("");
        position["stateSha256"] = json!(hash(&position).unwrap());
        s.runtime()
            .storage
            .put_json(
                "execution_managed_positions_v1",
                &position_id,
                position.clone(),
                &ctx,
            )
            .unwrap();
        management::inspect(&s, &position_id).unwrap();
        let mut close = entry.clone();
        close.management = Some(management::CloseBinding {
            execution_occurrence_id: "static.missing.occurrence".into(),
            position_id: position_id.clone(),
            trigger_sha256: "f".repeat(64),
            exit_policy_sha256: hash(&position["policy"]).unwrap(),
            original_entry_plan_id: entry.plan_id.clone(),
        });
        close.order.position_action = PositionAction::SellToClose;
        close.policy.position_action = PositionAction::SellToClose;
        close.purchase_cash = "1".parse().unwrap();
        close.plan_id.clear();
        close.plan_id = format!("orderplan_{}", hash(&close).unwrap());
        s.runtime()
            .storage
            .put_json(PLANS, &close.plan_id, value(&close).unwrap(), &ctx)
            .unwrap();
        let mut coherent = position.clone();
        coherent["schemaVersion"] = json!("tradeassembly.managed_position.v1");
        coherent["entryObservationSha256"] = Value::Null;
        coherent["entryCashReconciled"] = Value::Null;
        coherent["entrySettlementSha256"] = json!("a".repeat(64));
        coherent["policy"]["entryBasis"] = json!("provider_final_gross_excluding_fees");
        // Serialize omitted V1 optional fields exactly as the normal model does.
        coherent
            .as_object_mut()
            .unwrap()
            .remove("entryObservationSha256");
        coherent
            .as_object_mut()
            .unwrap()
            .remove("entryCashReconciled");
        coherent["stateSha256"] = json!("");
        coherent["stateSha256"] = json!(hash(&coherent).unwrap());
        s.runtime()
            .storage
            .put_json(
                "execution_managed_positions_v1",
                &position_id,
                coherent.clone(),
                &ctx,
            )
            .unwrap();
        let mut coherent_close = close.clone();
        coherent_close
            .management
            .as_mut()
            .unwrap()
            .exit_policy_sha256 = hash(&coherent["policy"]).unwrap();
        coherent_close.plan_id.clear();
        coherent_close.plan_id = format!("orderplan_{}", hash(&coherent_close).unwrap());
        s.runtime()
            .storage
            .put_json(
                PLANS,
                &coherent_close.plan_id,
                value(&coherent_close).unwrap(),
                &ctx,
            )
            .unwrap();
        for op in [
            ProviderManagementOperation::ObserveSettlement {
                scope: entry.command.scope.clone(),
                plan_id: coherent_close.plan_id.clone(),
            },
            ProviderManagementOperation::ReconcileClose {
                scope: entry.command.scope.clone(),
                plan_id: coherent_close.plan_id.clone(),
            },
            ProviderManagementOperation::SettleClose {
                scope: entry.command.scope.clone(),
                plan_id: coherent_close.plan_id.clone(),
            },
        ] {
            assert_eq!(
                provider_remote::binding(&s, &op).unwrap_err(),
                "provider_management_observed_entry_required",
                "unsupported position family must refuse before source/read acquisition"
            );
        }
        s.runtime()
            .storage
            .put_json(
                "execution_managed_positions_v1",
                &position_id,
                position.clone(),
                &ctx,
            )
            .unwrap();
        let before = stored_state(&s);
        assert_eq!(
            management::advance(
                &s,
                management::AdvanceCommand {
                    execution_occurrence_id: "static.missing.occurrence".into(),
                    position_id: position_id.clone(),
                    account_collection_id: "missing.account".into(),
                    metadata_collection_id: "missing.metadata".into(),
                    quote_collection_id: "missing.quote".into(),
                    reversal_signal_receipt_sha256: None,
                },
                &ctx
            )
            .unwrap_err(),
            "provider_observation_reconciliation_blocked"
        );
        assert_eq!(
            stored_state(&s),
            before,
            "blocked Advance must not mutate storage"
        );
        assert_eq!(
            management::snapshot(&s, &close).unwrap_err(),
            "provider_observation_reconciliation_blocked"
        );
        assert_eq!(
            stored_state(&s),
            before,
            "blocked C5 snapshot must not mutate storage"
        );
        assert_eq!(
            provider.0.load(Ordering::SeqCst),
            0,
            "no provider computation or acquisition is permitted"
        );
    }
    assert_eq!(
        s.runtime()
            .storage
            .get_json(CASH, &entry.account_key)
            .unwrap(),
        Some(cash_before)
    );
    assert_eq!(submission::record(&s, &entry).unwrap().unwrap().1, record);
}
