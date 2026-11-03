// Finite authored provider outcomes through true owned entry/close/C5 paths.
// Recreates the Core service over the same database; not whole-process restart.
fn close_outcome_reopen(
    s: &crate::service::TradeAssemblyService,
) -> crate::service::TradeAssemblyService {
    let owner = s.invocation_owner().unwrap();
    crate::service::TradeAssemblyService::from_runtime(s.db(), (*s.runtime()).clone())
        .for_authenticated_invocation(&owner.issuer, &owner.subject, None, None)
}
fn close_outcome_view(s: &crate::service::TradeAssemblyService, id: &str) -> Value {
    let r = s.handle_http(
        "GET",
        &format!("/product/strategies/positions/{id}"),
        json!({}),
    );
    assert_eq!(r.status, 200, "{}", r.body);
    r.body
}
fn close_outcome_records(s: &crate::service::TradeAssemblyService) -> Vec<Vec<(String, Value)>> {
    [
        "execution_cash_reservations_v1",
        "execution_order_plan_submissions_v2",
        "execution_managed_positions_v1",
        "execution_managed_position_journal_v1",
        "execution_close_settlement_receipts_v1",
        "execution_close_settlement_keys_v1",
        "execution_close_postings_v1",
    ]
    .into_iter()
    .map(|name| s.runtime().storage.list_json(name).unwrap())
    .collect()
}
fn close_outcome_assert_held(s: &crate::service::TradeAssemblyService, id: &str, close: &str) {
    let source = crate::service::order_plans::submission::source(s, close).unwrap();
    let plan = source.historical().unwrap();
    let state = source.state().unwrap().unwrap();
    assert_eq!(plan.purchase_cash.to_string(), "1");
    assert_eq!(state.retained_cash.to_string(), "1");
    assert!(state.settlement_receipt_sha256.is_none());
    let position = close_outcome_view(s, id);
    assert_eq!(position["position"]["closedQuantity"], "0");
    assert_eq!(position["position"]["reservations"][close], "0.4");
    let ledger = s
        .runtime()
        .storage
        .get_json("execution_cash_reservations_v1", &plan.account_key)
        .unwrap()
        .unwrap();
    assert_eq!(ledger["reservations"][close], "1");
    assert_eq!(ledger["capitalCheckpoint"]["sequence"], 2);
}
fn exercise_native_owned_close_outcome(
    s: &crate::service::TradeAssemblyService,
    scope: &data_plans::PlanScope,
    id: &str,
    entry: &str,
    decision: &Value,
    root: &std::path::Path,
    scenario: &str,
) {
    use crate::service::order_plans::submission::{self, Phase};
    let partial = scenario == "managed_plan_close_partial_cancel";
    let rejected = scenario == "managed_plan_close_rejected";
    let timeout = scenario == "managed_plan_close_timeout_filled";
    assert!(
        partial || rejected || timeout,
        "unsupported close fixture scenario"
    );
    let close = decision["planId"].as_str().unwrap();
    let initial = close_outcome_view(s, id);
    assert_eq!(initial["position"]["basis"], "100");
    assert_eq!(initial["position"]["quantity"], "0.4");
    assert_eq!(initial["position"]["entryPlanId"], entry);
    let entry_state = submission::source(s, entry)
        .unwrap()
        .state()
        .unwrap()
        .unwrap();
    let entry_receipt = entry_state.settlement_receipt_sha256.clone();
    let calls = || fs::read_to_string(root.join("synthetic-broker/calls.log")).unwrap();
    let submit_body = |key: &str| json!({"scope":scope,"submission":decision["submission"],"accountMode":"paper","idempotencyKey":key});
    let submitted = s.handle_http(
        "POST",
        "/product/strategies/order-plans/submit",
        submit_body("close-outcome-original-submit"),
    );
    let state = submission::source(s, close)
        .unwrap()
        .state()
        .unwrap()
        .unwrap();
    if partial {
        assert_eq!(submitted.status, 200, "{}", submitted.body);
        assert_eq!(state.phase, Phase::PartiallyFilled);
        assert_eq!(state.filled_quantity.to_string(), "0.1");
    }
    if rejected {
        assert!(matches!(submitted.status, 200 | 409), "{}", submitted.body);
        assert_eq!(state.phase, Phase::Rejected);
        assert_eq!(state.filled_quantity.to_string(), "0");
    }
    if timeout {
        assert!(submitted.status >= 400, "{}", submitted.body);
        assert!(
            matches!(state.phase, Phase::Sending | Phase::Unknown),
            "{state:?}"
        );
        assert!(root
            .join("synthetic-broker/close-accepted-request.json")
            .is_file());
    }
    assert_eq!(calls().lines().filter(|v| *v == "submit").count(), 2);
    close_outcome_assert_held(s, id, close);
    let original_key = state.original_key.clone();
    let original_acceptance = s
        .runtime()
        .storage
        .get_json("plugin_operation_receipts", &original_key)
        .unwrap();
    // New service construction exercises durable source/intent/receipt recovery.
    let restored = close_outcome_reopen(s);
    let before_retry = calls();
    let _same = restored.handle_http(
        "POST",
        "/product/strategies/order-plans/submit",
        submit_body("close-outcome-original-submit"),
    );
    let _new = restored.handle_http(
        "POST",
        "/product/strategies/order-plans/submit",
        submit_body("close-outcome-new-transport"),
    );
    assert_eq!(
        calls(),
        before_retry,
        "duplicate/new transport may not dispatch another close"
    );
    assert_eq!(
        submission::source(&restored, close)
            .unwrap()
            .state()
            .unwrap()
            .unwrap()
            .original_key,
        original_key
    );
    close_outcome_assert_held(&restored, id, close);
    let release=restored.handle_http("POST","/product/strategies/order-plans/release",json!({"scope":scope,"planId":close,"accountMode":"paper","idempotencyKey":"close-outcome-no-premature-release"}));
    assert_eq!(release.status, 409, "{}", release.body);
    close_outcome_assert_held(&restored, id, close);
    let lookup = |service: &crate::service::TradeAssemblyService, key: &str| {
        service.handle_http(
            "POST",
            "/product/strategies/order-plans/reconcile",
            json!({"scope":scope,"planId":close,"accountMode":"paper","idempotencyKey":key}),
        )
    };
    let mut recovery = Vec::new();
    if partial || timeout {
        let first = lookup(&restored, "close-outcome-first-lookup");
        assert_eq!(first.status, 200, "{}", first.body);
        recovery.push(first.body);
        let observed = submission::source(&restored, close)
            .unwrap()
            .state()
            .unwrap()
            .unwrap();
        assert_eq!(observed.original_key, original_key);
        if partial {
            assert_eq!(observed.phase, Phase::PartiallyFilled);
            assert_eq!(observed.filled_quantity.to_string(), "0.2");
        }
        if timeout {
            assert_eq!(observed.phase, Phase::Consumed);
            assert_eq!(observed.filled_quantity.to_string(), "0.4");
        }
        close_outcome_assert_held(&restored, id, close);
        let before_duplicate = calls();
        let duplicate = lookup(
            &close_outcome_reopen(&restored),
            "close-outcome-first-lookup",
        );
        assert_eq!(duplicate.status, 200, "{}", duplicate.body);
        assert_eq!(
            calls(),
            before_duplicate,
            "same lookup key must use its original receipt"
        );
    } else {
        let before = calls();
        let historical = lookup(&restored, "close-outcome-terminal-rejected-history");
        assert_eq!(historical.status, 200, "{}", historical.body);
        assert_eq!(
            calls(),
            before,
            "known terminal rejection requires no provider lookup"
        );
        recovery.push(historical.body);
    }
    let mut premature = Value::Null;
    if partial {
        let before = close_outcome_records(&restored);
        let nonfinal=restored.handle_http("POST","/product/strategies/order-plans/settle",json!({"scope":scope,"planId":close,"accountMode":"paper","idempotencyKey":"close-outcome-nonfinal-posting"}));
        assert_eq!(nonfinal.status, 409, "{}", nonfinal.body);
        assert_eq!(
            nonfinal.body["error"]["code"],
            "close_settlement_provider_unavailable"
        );
        assert_eq!(
            close_outcome_records(&restored),
            before,
            "nonterminal/nonfinal posting may not change cash, quantity, state or journal"
        );
        close_outcome_assert_held(&restored, id, close);
        premature = nonfinal.body;
        let terminal = lookup(
            &close_outcome_reopen(&restored),
            "close-outcome-terminal-lookup",
        );
        assert_eq!(terminal.status, 200, "{}", terminal.body);
        recovery.push(terminal.body);
        let state = submission::source(&restored, close)
            .unwrap()
            .state()
            .unwrap()
            .unwrap();
        assert_eq!(state.phase, Phase::Consumed);
        assert_eq!(state.filled_quantity.to_string(), "0.2");
        assert_eq!(
            state.observation.unwrap().status,
            tradeassembly_plugin_sdk::broker_v2::OrderStatus::Cancelled
        );
        close_outcome_assert_held(&restored, id, close);
    }
    if let Some(original) = original_acceptance {
        assert_eq!(
            restored
                .runtime()
                .storage
                .get_json("plugin_operation_receipts", &original_key)
                .unwrap(),
            Some(original),
            "later lookup must preserve immutable original acceptance/rejection"
        );
    } else {
        let repaired = restored
            .runtime()
            .storage
            .get_json("plugin_operation_receipts", &original_key)
            .unwrap()
            .expect("lost unknown original receipt is repaired by verified original lookup");
        assert_eq!(
            repaired["payload"]["order"]["clientOrderId"],
            submission::source(&restored, close)
                .unwrap()
                .historical()
                .unwrap()
                .order
                .client_order_id
        );
    }
    let reopened = close_outcome_reopen(&restored);
    let final_args = json!({"scope":scope,"planId":close,"accountMode":"paper","idempotencyKey":"close-outcome-final-posting"});
    let final_posting = reopened.handle_http(
        "POST",
        "/product/strategies/order-plans/settle",
        final_args.clone(),
    );
    assert_eq!(final_posting.status, 200, "{}", final_posting.body);
    let (expected_quantity, expected_gross, expected_fee, expected_cash, expected_remaining) =
        if partial {
            ("0.2", "18", "0.25", "77.75", "0.2")
        } else if rejected {
            ("0", "0", "0.1", "59.9", "0.4")
        } else {
            ("0.4", "36", "0.5", "95.5", "0")
        };
    assert_eq!(final_posting.body["grossProceeds"], expected_gross);
    assert_eq!(final_posting.body["feeCash"], expected_fee);
    assert_eq!(final_posting.body["releasedHold"], "1");
    assert_eq!(final_posting.body["checkpointSequence"], 3);
    let position = close_outcome_view(&reopened, id);
    assert_eq!(position["position"]["closedQuantity"], expected_quantity);
    assert_eq!(position["position"]["reservations"], json!({}));
    for key in [
        "entryPlanId",
        "entrySettlementSha256",
        "entryTime",
        "quantity",
        "basis",
        "policy",
    ] {
        assert_eq!(
            position["position"][key], initial["position"][key],
            "final close may not replace original entry evidence: {key}"
        );
    }
    let source = submission::source(&reopened, close).unwrap();
    let plan = source.historical().unwrap();
    let state = source.state().unwrap().unwrap();
    assert_eq!(state.original_key, original_key);
    assert_eq!(state.retained_cash.to_string(), "0");
    assert_eq!(state.filled_quantity.to_string(), expected_quantity);
    assert!(state.settlement_receipt_sha256.is_some());
    assert_eq!(
        state.phase,
        if rejected {
            Phase::Rejected
        } else {
            Phase::Consumed
        }
    );
    let ledger = reopened
        .runtime()
        .storage
        .get_json("execution_cash_reservations_v1", &plan.account_key)
        .unwrap()
        .unwrap();
    assert_eq!(ledger["reservations"], json!({}));
    assert_eq!(ledger["capitalCheckpoint"]["availableCash"], expected_cash);
    assert_eq!(ledger["capitalCheckpoint"]["sequence"], 3);
    assert_eq!(
        submission::source(&reopened, entry)
            .unwrap()
            .state()
            .unwrap()
            .unwrap()
            .settlement_receipt_sha256,
        entry_receipt
    );
    let final_records = close_outcome_records(&reopened);
    let final_calls = calls();
    let posting_reads =
        fs::read_to_string(root.join("synthetic-broker/close-settlement.log")).unwrap();
    let again = close_outcome_reopen(&reopened);
    let repeat = again.handle_http("POST", "/product/strategies/order-plans/settle", final_args);
    assert_eq!(repeat.status, 200, "{}", repeat.body);
    assert_eq!(
        repeat.body["receiptSha256"],
        final_posting.body["receiptSha256"]
    );
    let newkey=again.handle_http("POST","/product/strategies/order-plans/settle",json!({"scope":scope,"planId":close,"accountMode":"paper","idempotencyKey":"close-outcome-final-new-transport"}));
    assert_eq!(newkey.status, 200, "{}", newkey.body);
    assert_eq!(
        newkey.body["receiptSha256"],
        final_posting.body["receiptSha256"]
    );
    let _ = again.handle_http(
        "POST",
        "/product/strategies/order-plans/submit",
        submit_body("close-outcome-after-final-no-resubmit"),
    );
    assert_eq!(calls(), final_calls);
    assert_eq!(
        fs::read_to_string(root.join("synthetic-broker/close-settlement.log")).unwrap(),
        posting_reads
    );
    assert_eq!(
        close_outcome_records(&again),
        final_records,
        "duplicate settlement/submit must not reapply quantity, cash or journal"
    );
    assert_eq!(final_calls.lines().filter(|v| *v == "submit").count(), 2);
    assert_eq!(
        final_calls.lines().filter(|v| *v == "lookup").count(),
        if partial {
            3
        } else if rejected {
            1
        } else {
            2
        }
    );
    write_screen_interface_evidence(
        "managed_close_outcomes",
        &format!("{scenario}.json"),
        &json!({"scenario":scenario,"initialPosition":initial,"submission":submitted.body,"recovery":recovery,"nonfinalRefusal":premature,"finalPosting":final_posting.body,"positionAndJournal":position,"cashLedger":ledger,"remainingOwnedQuantity":expected_remaining,"originalEntryBasis":"100","originalEntryFeeCash":"0","closeFeesAreSeparateFinalDebits":true,"serviceRecreatedBeforeLookupAndPosting":true,"actualProcessRestartQualified":false,"realBrokerCalls":0,"syntheticSubmits":2,"qualification":"normal final entry/owned position/removed-membership generic reversal/close C5; authored native provider outcomes and postings, mock Warden/traced sandbox; terminal-only settlement, service reconstruction, no live/provider-cancel/whole-process/outcome-continuous-monitor qualification"}),
    );
}
#[test]
#[ignore = "explicit native owned close partial-to-cancel/final-fee/rejection/unknown-acceptance recovery cases"]
fn native_owned_close_outcomes_retain_hold_until_final_posting_and_resume() {
    crate::build_identity::install("1111111111111111111111111111111111111111").unwrap();
    for case in [
        "financial_screen_managed_plan_close_partial_cancel",
        "financial_screen_managed_plan_close_rejected",
        "financial_screen_managed_plan_close_timeout_filled",
    ] {
        native_execution_plan_fixture(Some(case));
    }
}
