// Copyright (c) 2026 OptionLab LLC. All rights reserved.

//! Durable conservative capacity accounting, not broker authorization.
//!
//! Callers must authenticate the owner/account and verify snapshot provenance
//! before calling this kernel. No MCP route accepts these inputs directly.
//! A reservation survives timeout, restart and lost responses. This first
//! contract deliberately has no release operation: terminal reconciliation must
//! prove that exposure has moved into verified position accounting before release.

use crate::ports::{
    ComparePutOutcome, SideEffectContext, StorageExpectation, StoragePort, StorageWrite,
};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

const NS: &str = "portfolio_capacity_v1";
const MAX_HOLDS: usize = 1024;
const MAX_SNAPSHOT_AGE_MS: i64 = 30_000;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Scope {
    pub owner_id: String,
    pub account_ref: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CapacityPolicy {
    /// Digest of the owner-authorized policy, verified by the admission caller.
    pub authority_digest: String,
    pub maximum_exposure_micros: i64,
    pub maximum_positions: usize,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AccountSnapshot {
    pub scope: Scope,
    pub receipt_digest: String,
    pub source_at_ms: i64,
    pub observed_at_ms: i64,
    /// Gross absolute exposure, including broker-open orders. Never net shorts
    /// against longs. Pending local reservations are additionally counted.
    pub exposure_by_symbol_micros: BTreeMap<String, i64>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CapacityRequest {
    pub scope: Scope,
    pub request_id: String,
    pub intent_digest: String,
    pub symbol: String,
    pub notional_micros: i64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CapacityHold {
    pub request: CapacityRequest,
    pub policy: CapacityPolicy,
    pub snapshot_digest: String,
    pub reserved_at_ms: i64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Ledger {
    schema_version: u32,
    scope: Scope,
    holds: BTreeMap<String, CapacityHold>,
}

fn valid_text(value: &str, max: usize) -> bool {
    !value.is_empty()
        && value.len() <= max
        && value.trim() == value
        && !value.chars().any(char::is_control)
}

fn fresh(timestamp: i64, now: i64) -> bool {
    now.checked_sub(timestamp)
        .is_some_and(|age| (0..=MAX_SNAPSHOT_AGE_MS).contains(&age))
}

fn digest<T: Serialize>(value: &T) -> Result<String, String> {
    crate::spec::canonical_hash(
        &serde_json::to_value(value).map_err(|_| "capacity_encoding_failed")?,
    )
}

/// Atomically check total observed + held exposure and reserve capacity.
/// Contention is returned to the caller for a bounded retry, never ignored.
/// Success does not grant permission to dispatch or prove the order filled.
pub fn reserve(
    storage: &dyn StoragePort,
    policy: &CapacityPolicy,
    snapshot: &AccountSnapshot,
    request: &CapacityRequest,
    context: &SideEffectContext,
    now_ms: i64,
) -> Result<CapacityHold, String> {
    if now_ms < 0
        || request.scope != snapshot.scope
        || !valid_text(&request.scope.owner_id, 256)
        || !valid_text(&request.scope.account_ref, 256)
        || !valid_text(&request.request_id, 256)
        || !valid_text(&request.intent_digest, 256)
        || !valid_text(&request.symbol, 32)
        || request.notional_micros <= 0
        || !valid_text(&policy.authority_digest, 256)
        || policy.maximum_exposure_micros <= 0
        || policy.maximum_positions == 0
        || policy.maximum_positions > MAX_HOLDS
    {
        return Err("capacity_request_invalid".into());
    }
    if !valid_text(&snapshot.receipt_digest, 256)
        || snapshot.source_at_ms < 0
        || snapshot.observed_at_ms < 0
        || !fresh(snapshot.source_at_ms, now_ms)
        || !fresh(snapshot.observed_at_ms, now_ms)
        || snapshot.source_at_ms > snapshot.observed_at_ms
        || snapshot.exposure_by_symbol_micros.len() > MAX_HOLDS
        || snapshot
            .exposure_by_symbol_micros
            .iter()
            .any(|(symbol, exposure)| !valid_text(symbol, 32) || *exposure <= 0)
    {
        return Err("capacity_snapshot_invalid_or_stale".into());
    }
    let key = digest(&request.scope)?;
    let previous = storage.get_json(NS, &key)?;
    let mut ledger = match previous.clone() {
        Some(value) => {
            serde_json::from_value::<Ledger>(value).map_err(|_| "capacity_ledger_corrupt")?
        }
        None => Ledger {
            schema_version: 1,
            scope: request.scope.clone(),
            holds: BTreeMap::new(),
        },
    };
    if ledger.schema_version != 1 || ledger.scope != request.scope || ledger.holds.len() > MAX_HOLDS
    {
        return Err("capacity_ledger_corrupt".into());
    }
    let mut total = snapshot
        .exposure_by_symbol_micros
        .values()
        .map(|v| i128::from(*v))
        .sum::<i128>();
    let mut symbols: BTreeSet<&str> = snapshot
        .exposure_by_symbol_micros
        .keys()
        .map(String::as_str)
        .collect();
    for (id, hold) in &ledger.holds {
        if id != &hold.request.request_id
            || hold.request.scope != request.scope
            || hold.request.notional_micros <= 0
            || !valid_text(&hold.request.symbol, 32)
            || !valid_text(&hold.request.intent_digest, 256)
            || !valid_text(&hold.snapshot_digest, 256)
            || hold.reserved_at_ms > now_ms
        {
            return Err("capacity_ledger_corrupt".into());
        }
        if hold.policy != *policy {
            return Err("capacity_policy_change_requires_reconciliation".into());
        }
        total += i128::from(hold.request.notional_micros);
        symbols.insert(&hold.request.symbol);
    }
    if let Some(existing) = ledger.holds.get(&request.request_id) {
        return if existing.request == *request {
            Ok(existing.clone())
        } else {
            Err("capacity_idempotency_conflict".into())
        };
    }
    if ledger.holds.len() == MAX_HOLDS {
        return Err("capacity_ledger_full".into());
    }
    total += i128::from(request.notional_micros);
    symbols.insert(&request.symbol);
    if total > i128::from(policy.maximum_exposure_micros) {
        return Err("portfolio_exposure_limit_exceeded".into());
    }
    if symbols.len() > policy.maximum_positions {
        return Err("portfolio_position_limit_exceeded".into());
    }
    let hold = CapacityHold {
        request: request.clone(),
        policy: policy.clone(),
        snapshot_digest: digest(snapshot)?,
        reserved_at_ms: now_ms,
    };
    ledger
        .holds
        .insert(request.request_id.clone(), hold.clone());
    let replacement = serde_json::to_value(ledger).map_err(|_| "capacity_encoding_failed")?;
    match storage.put_json_batch(
        &[StorageWrite::new(
            NS,
            &key,
            replacement.clone(),
            context.clone(),
        )],
        &[StorageExpectation::new(NS, &key, previous)],
    )? {
        ComparePutOutcome::Conflict => Err("capacity_concurrent_update".into()),
        ComparePutOutcome::Updated => {
            // A false write acknowledgement must never grant capacity.
            let stored = storage
                .get_json(NS, &key)?
                .ok_or("capacity_write_unverified")?;
            let stored: Ledger =
                serde_json::from_value(stored).map_err(|_| "capacity_write_unverified")?;
            if stored.scope != request.scope || stored.holds.get(&request.request_id) != Some(&hold)
            {
                return Err("capacity_write_unverified".into());
            }
            Ok(hold)
        }
    }
}

#[cfg(test)]
mod tests;
