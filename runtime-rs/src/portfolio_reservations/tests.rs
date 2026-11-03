use super::*;
use crate::adapters::local::sqlite::LocalSqliteStorage;
use crate::ports::{AuthorityContext, IdempotencyKey};
use std::sync::{Arc, Barrier};

fn inputs() -> (
    CapacityPolicy,
    AccountSnapshot,
    CapacityRequest,
    SideEffectContext,
) {
    let scope = Scope {
        owner_id: "fixture-owner".into(),
        account_ref: "account://controlled".into(),
    };
    (
        CapacityPolicy {
            authority_digest: "fixture-policy".into(),
            maximum_exposure_micros: 100,
            maximum_positions: 2,
        },
        AccountSnapshot {
            scope: scope.clone(),
            receipt_digest: "fixture-receipt".into(),
            source_at_ms: 1000,
            observed_at_ms: 1000,
            exposure_by_symbol_micros: BTreeMap::new(),
        },
        CapacityRequest {
            scope,
            request_id: "first".into(),
            intent_digest: "fixture-intent".into(),
            symbol: "AAA".into(),
            notional_micros: 60,
        },
        SideEffectContext::new(
            AuthorityContext::local_cli(),
            IdempotencyKey::new("capacity-fixture").unwrap(),
        ),
    )
}

#[test]
fn concurrent_connections_cannot_overbook_capacity() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("risk.db").display().to_string();
    // Initialize before racing separate connections; every reserve still opens its own transaction.
    LocalSqliteStorage::new(&path)
        .get_json(NS, "initialization")
        .unwrap();
    let barrier = Arc::new(Barrier::new(8));
    let threads: Vec<_> = (0..8)
        .map(|index| {
            let path = path.clone();
            let barrier = barrier.clone();
            std::thread::spawn(move || {
                let (policy, snapshot, mut request, context) = inputs();
                request.request_id = format!("competing-{index}");
                barrier.wait();
                reserve(
                    &LocalSqliteStorage::new(path),
                    &policy,
                    &snapshot,
                    &request,
                    &context,
                    1000,
                )
            })
        })
        .collect();
    let outcomes: Vec<_> = threads
        .into_iter()
        .map(|thread| thread.join().unwrap())
        .collect();
    assert_eq!(outcomes.iter().filter(|result| result.is_ok()).count(), 1);
    for error in outcomes.iter().filter_map(|result| result.as_ref().err()) {
        assert!(
            matches!(
                error.as_str(),
                "capacity_concurrent_update" | "portfolio_exposure_limit_exceeded"
            ),
            "{error}"
        );
    }
    let (_, snapshot, _, _) = inputs();
    let stored = LocalSqliteStorage::new(path)
        .get_json(NS, &digest(&snapshot.scope).unwrap())
        .unwrap()
        .unwrap();
    assert_eq!(stored["holds"].as_object().unwrap().len(), 1);
}

#[test]
fn duplicate_restart_and_ambiguous_outcome_never_release_capacity() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("risk.db").display().to_string();
    let (policy, mut snapshot, request, context) = inputs();
    let first = reserve(
        &LocalSqliteStorage::new(&path),
        &policy,
        &snapshot,
        &request,
        &context,
        1000,
    )
    .unwrap();
    // No completion recorded: model a process dying after reserve/broker dispatch.
    snapshot.source_at_ms = 100_000;
    snapshot.observed_at_ms = 100_000;
    let reopened = LocalSqliteStorage::new(&path);
    assert_eq!(
        reserve(&reopened, &policy, &snapshot, &request, &context, 100_000).unwrap(),
        first
    );
    let mut second = request.clone();
    second.request_id = "after-restart".into();
    assert_eq!(
        reserve(&reopened, &policy, &snapshot, &second, &context, 100_000),
        Err("portfolio_exposure_limit_exceeded".into())
    );
    second.request_id = request.request_id;
    second.intent_digest = "different-order".into();
    assert_eq!(
        reserve(&reopened, &policy, &snapshot, &second, &context, 100_000),
        Err("capacity_idempotency_conflict".into())
    );
}

#[test]
fn observes_existing_exposure_and_counts_unique_position_slots() {
    let dir = tempfile::tempdir().unwrap();
    let storage = LocalSqliteStorage::new(dir.path().join("risk.db").display().to_string());
    let (mut policy, mut snapshot, mut request, context) = inputs();
    policy.maximum_positions = 1;
    snapshot.exposure_by_symbol_micros.insert("AAA".into(), 40);
    assert!(reserve(&storage, &policy, &snapshot, &request, &context, 1000).is_ok());
    request.request_id = "overflow".into();
    request.notional_micros = 1;
    assert_eq!(
        reserve(&storage, &policy, &snapshot, &request, &context, 1000),
        Err("portfolio_exposure_limit_exceeded".into())
    );
    let other = LocalSqliteStorage::new(dir.path().join("slots.db").display().to_string());
    request.symbol = "BBB".into();
    assert_eq!(
        reserve(&other, &policy, &snapshot, &request, &context, 1000),
        Err("portfolio_position_limit_exceeded".into())
    );
    assert!(other.list_json(NS).unwrap().is_empty());
}

#[test]
fn stale_future_wrong_account_and_invalid_observations_leave_no_hold() {
    let dir = tempfile::tempdir().unwrap();
    let storage = LocalSqliteStorage::new(dir.path().join("risk.db").display().to_string());
    let (policy, snapshot, request, context) = inputs();
    for timestamp in [-30_001, 1001] {
        let mut invalid = snapshot.clone();
        invalid.source_at_ms = timestamp;
        assert!(reserve(&storage, &policy, &invalid, &request, &context, 1000).is_err());
    }
    let mut invalid = snapshot.clone();
    invalid.scope.account_ref = "account://other".into();
    assert!(reserve(&storage, &policy, &invalid, &request, &context, 1000).is_err());
    invalid = snapshot;
    invalid.exposure_by_symbol_micros.insert("SHORT".into(), -1);
    assert!(reserve(&storage, &policy, &invalid, &request, &context, 1000).is_err());
    assert!(storage.list_json(NS).unwrap().is_empty());
}

#[test]
fn wide_integer_totals_cannot_wrap_and_corrupt_holds_fail_closed() {
    let dir = tempfile::tempdir().unwrap();
    let storage = LocalSqliteStorage::new(dir.path().join("risk.db").display().to_string());
    let (mut policy, mut snapshot, request, context) = inputs();
    policy.maximum_exposure_micros = i64::MAX;
    snapshot
        .exposure_by_symbol_micros
        .insert("BBB".into(), i64::MAX);
    assert_eq!(
        reserve(&storage, &policy, &snapshot, &request, &context, 1000),
        Err("portfolio_exposure_limit_exceeded".into())
    );
    snapshot.exposure_by_symbol_micros.clear();
    reserve(&storage, &policy, &snapshot, &request, &context, 1000).unwrap();
    let key = digest(&request.scope).unwrap();
    let mut corrupt = storage.get_json(NS, &key).unwrap().unwrap();
    corrupt["holds"]["first"]["request"]["notionalMicros"] = serde_json::json!(-1);
    storage.put_json(NS, &key, corrupt, &context).unwrap();
    assert_eq!(
        reserve(&storage, &policy, &snapshot, &request, &context, 1000),
        Err("capacity_ledger_corrupt".into())
    );
}

#[test]
fn holds_prevent_policy_rebinding_until_reconciled() {
    let dir = tempfile::tempdir().unwrap();
    let storage = LocalSqliteStorage::new(dir.path().join("risk.db").display().to_string());
    let (mut policy, snapshot, mut request, context) = inputs();
    reserve(&storage, &policy, &snapshot, &request, &context, 1000).unwrap();
    policy.maximum_exposure_micros = 1000;
    request.request_id = "new-policy".into();
    assert_eq!(
        reserve(&storage, &policy, &snapshot, &request, &context, 1000),
        Err("capacity_policy_change_requires_reconciliation".into())
    );
}
