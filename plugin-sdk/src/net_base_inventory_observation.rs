// Copyright (c) 2026 OptionLab LLC. All rights reserved.

//! Original-entry component evidence. Net credit is not account-wide holdings,
//! an owned strategy lot, coherent cash settlement, or permission to close.
//! An exhausted order-filtered fee query is NOT a final fee closure.
use crate::cash_capacity_observation::{self as common, ReadEvidence, ReadKind, Sample};
use crate::{
    broker_lookup_v3::LookupRequestV3,
    broker_v2::{OrderObservation, OrderStatus},
    canonical::canonical_sha256,
    execution::{Decimal, PositionAction, PriceInstruction},
    execution_snapshot::{Currency, Observed},
    settlement_observation_v2::{NonAtomic, ProviderObservationStatus},
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeSet;
pub const REQUEST_VERSION: &str = "tradeassembly.net_base_inventory_request.v1";
pub const OBSERVATION_VERSION: &str = "tradeassembly.net_base_inventory_observation.v1";
pub const REQUEST_SCHEMA: &str = "schema://tradeassembly/net-base-inventory-request@1";
pub const RESPONSE_SCHEMA: &str = "schema://tradeassembly/net-base-inventory-observation@1";
pub const OPERATION: &str = "broker.net_base_inventory.paper_v1";
pub const CAPABILITY: &str = "broker.net_base_inventory.paper@1";
pub const PURPOSE: &str = "recovery";
pub const EFFECT: &str = "read";
pub const DEPENDENCY_ID: &str = "broker.net_base_inventory";
pub const MAX_REQUEST_BYTES: usize = 32_768;
pub const MAX_OBSERVATION_BYTES: usize = 524_288;
pub const MAX_ACTIVITY_ROWS: usize = 128;
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct QueryBounds {
    pub max_pages_per_type: u8,
    pub page_size: u16,
    pub max_rows_per_type: u16,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Request {
    pub schema_version: String,
    pub original_entry: LookupRequestV3,
    pub host_account_ref: String,
    /// Host receipt pin only: never provider-derived ownership authority.
    pub original_order_receipt_sha256: String,
    pub base_asset_id: String,
    pub quote_currency: Currency,
    pub bounds: QueryBounds,
}
impl Request {
    pub fn validate(&self) -> Result<(), String> {
        self.original_entry.validate()?;
        self.quote_currency.validate()?;
        let order = &self.original_entry.original_submit.order;
        common::validate_instrument(&order.instrument, &self.base_asset_id, &self.quote_currency)?;
        if self.schema_version != REQUEST_VERSION
            || !common::label(&self.host_account_ref, 256)
            || !common::digest(&self.original_order_receipt_sha256)
            || order.position_action != PositionAction::BuyToOpen
            || !matches!(order.pricing, PriceInstruction::Limit { .. })
            || order.time_in_force != "gtc"
            || !(1..=8).contains(&self.bounds.max_pages_per_type)
            || !(1..=128).contains(&self.bounds.page_size)
            || !(1..=128).contains(&self.bounds.max_rows_per_type)
        {
            return Err("net_base_inventory_request_invalid".into());
        }
        common::size(self, MAX_REQUEST_BYTES)
    }
    pub fn request_sha256(&self) -> Result<String, String> {
        self.validate()?;
        canonical_sha256(self).map_err(|_| "net_base_inventory_encoding_invalid".into())
    }
    pub fn validate_binding(
        &self,
        operation: &str,
        capability: &str,
        purpose: &str,
    ) -> Result<(), String> {
        self.validate()?;
        if operation != OPERATION || capability != CAPABILITY || purpose != PURPOSE {
            return Err("net_base_inventory_operation_mismatch".into());
        }
        Ok(())
    }
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct QueryCoverage {
    pub provider_order_id: String,
    pub pages_read: u8,
    pub rows_read: u16,
    pub query_exhausted: bool,
    pub sample: Sample,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct FillRecord {
    pub provider_fill_id: String,
    pub provider_order_id: String,
    pub base_asset_id: String,
    pub base_quantity: Decimal,
    pub transaction_at_ms: i64,
    pub evidence_ids: Vec<String>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct FillRead {
    pub records: Vec<FillRecord>,
    pub coverage: QueryCoverage,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "snake_case",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum FeeAttribution {
    DirectOrder { provider_order_id: String },
    DirectFill { provider_fill_id: String },
    ProviderOrderFilter { provider_order_id: String },
    Unavailable { reason: String },
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct FeeRecord {
    pub provider_fee_id: String,
    pub base_asset_id: Observed<String>,
    pub base_quantity: Observed<Decimal>,
    pub effective_at_ms: Observed<i64>,
    pub attribution: FeeAttribution,
    pub evidence_ids: Vec<String>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct FeeRead {
    pub records: Vec<FeeRecord>,
    pub coverage: QueryCoverage,
}
/// A separate authoritative order-specific closure claim backed by its own read.
/// This is not derivable from pages, dates, symbol matching, or current balances.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct FeeClosure {
    pub provider_order_id: String,
    pub covered_gross_base: Decimal,
    pub all_order_fees_final: bool,
    pub corrections_complete: bool,
    pub final_through_ms: i64,
    pub sample: Sample,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AggregateScope {
    AccountAggregate,
}
impl<'de> Deserialize<'de> for AggregateScope {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        if String::deserialize(d)? == "account_aggregate" {
            Ok(Self::AccountAggregate)
        } else {
            Err(serde::de::Error::custom(
                "net_base_inventory_aggregate_scope_invalid",
            ))
        }
    }
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AggregateInventory {
    pub scope: AggregateScope,
    pub instrument_id: String,
    pub base_asset_id: String,
    pub quantity: Decimal,
    pub quantity_available: Observed<Decimal>,
    pub sample: Sample,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Observation {
    pub schema_version: String,
    pub request_sha256: String,
    pub status: ProviderObservationStatus,
    pub coherence: NonAtomic,
    pub order: OrderObservation,
    pub order_sample: Sample,
    pub gross_base_filled: Observed<Decimal>,
    pub fills: Observed<FillRead>,
    pub fees: Observed<FeeRead>,
    pub fee_closure: Observed<FeeClosure>,
    pub net_base_credited: Observed<Decimal>,
    pub aggregate_inventory: Observed<AggregateInventory>,
    pub evidence: Vec<ReadEvidence>,
}
impl QueryCoverage {
    fn validate(
        &self,
        q: &Request,
        order: &str,
        count: usize,
        kind: ReadKind,
        reads: &common::EvidenceIndex<'_>,
        now: i64,
    ) -> Result<(), String> {
        if self.provider_order_id != order
            || self.pages_read == 0
            || self.pages_read > q.bounds.max_pages_per_type
            || usize::from(self.rows_read) != count
            || self.rows_read > q.bounds.max_rows_per_type
            || count > MAX_ACTIVITY_ROWS
            || self.sample.evidence_ids.len() != usize::from(self.pages_read)
            || usize::from(self.rows_read)
                > usize::from(self.pages_read) * usize::from(q.bounds.page_size)
        {
            return Err("net_base_inventory_query_coverage_invalid".into());
        }
        self.sample.validate(kind, reads, now)
    }
}
fn references(
    ids: &[String],
    sample: &Sample,
    kind: ReadKind,
    reads: &common::EvidenceIndex<'_>,
    now: i64,
) -> Result<(), String> {
    let subset = Sample {
        started_at_ms: sample.started_at_ms,
        completed_at_ms: sample.completed_at_ms,
        evidence_ids: ids.to_vec(),
    };
    subset.validate(kind, reads, now)?;
    if ids.iter().any(|id| !sample.evidence_ids.contains(id)) {
        return Err("net_base_inventory_row_evidence_invalid".into());
    }
    Ok(())
}
impl Observation {
    pub fn validate(&self, q: &Request, now: i64) -> Result<(), String> {
        q.validate()?;
        common::size(self, MAX_OBSERVATION_BYTES)?;
        q.original_entry.validate_observation(&self.order, now)?;
        if self.schema_version != OBSERVATION_VERSION
            || self.request_sha256 != q.request_sha256()?
        {
            return Err("net_base_inventory_binding_invalid".into());
        }
        let terminal = matches!(
            self.order.status,
            OrderStatus::Filled
                | OrderStatus::Cancelled
                | OrderStatus::Expired
                | OrderStatus::Rejected
        );
        if (self.status == ProviderObservationStatus::Provisional) != terminal {
            return Err("net_base_inventory_status_invalid".into());
        }
        let order_id = self.order.provider_order_id.as_deref();
        let reads = common::validate_evidence(
            &self.evidence,
            now,
            &[
                ReadKind::Order,
                ReadKind::Fills,
                ReadKind::Fees,
                ReadKind::FeeClosure,
                ReadKind::Positions,
            ],
            order_id,
        )?;
        self.order_sample.validate(ReadKind::Order, &reads, now)?;
        if self.order.known_at_ms > self.order_sample.completed_at_ms
            || self
                .order_sample
                .evidence_ids
                .iter()
                .any(|id| reads[id.as_str()].completed_at_ms > self.order.known_at_ms)
        {
            return Err("net_base_inventory_order_sampling_invalid".into());
        }
        common::observed(&self.gross_base_filled, |gross| {
            common::nonnegative(gross)?;
            if self.order.filled_quantity != (Observed::Available { value: *gross }) {
                return Err("net_base_inventory_gross_fill_mismatch".into());
            }
            Ok(())
        })?;
        if matches!(&self.order.filled_quantity, Observed::Unavailable { .. })
            && matches!(&self.gross_base_filled, Observed::Available { .. })
        {
            return Err("net_base_inventory_gross_fill_mismatch".into());
        }
        let mut fill_ids = BTreeSet::new();
        let mut fill_sum = Decimal::ZERO;
        common::observed(&self.fills, |fill| {
            let order_id = order_id.ok_or("net_base_inventory_order_identity_unavailable")?;
            fill.coverage.validate(
                q,
                order_id,
                fill.records.len(),
                ReadKind::Fills,
                &reads,
                now,
            )?;
            for row in &fill.records {
                if !common::label(&row.provider_fill_id, 256)
                    || !fill_ids.insert(row.provider_fill_id.clone())
                    || row.provider_order_id != order_id
                    || row.base_asset_id != q.base_asset_id
                    || row.transaction_at_ms <= 0
                    || row.transaction_at_ms > self.order.known_at_ms
                {
                    return Err("net_base_inventory_fill_invalid".into());
                }
                common::positive(&row.base_quantity)?;
                references(
                    &row.evidence_ids,
                    &fill.coverage.sample,
                    ReadKind::Fills,
                    &reads,
                    now,
                )?;
                fill_sum = fill_sum.checked_add(row.base_quantity)?;
            }
            if let Observed::Available { value: gross } = &self.gross_base_filled {
                if fill_sum.compare(*gross)?.is_gt() {
                    return Err("net_base_inventory_overfilled_activities".into());
                }
            }
            Ok(())
        })?;
        let mut fee_ids = BTreeSet::new();
        let mut fee_sum = Decimal::ZERO;
        let mut direct_complete = true;
        common::observed(&self.fees, |fee| {
            let order_id = order_id.ok_or("net_base_inventory_order_identity_unavailable")?;
            fee.coverage
                .validate(q, order_id, fee.records.len(), ReadKind::Fees, &reads, now)?;
            for row in &fee.records {
                if !common::label(&row.provider_fee_id, 256)
                    || !fee_ids.insert(row.provider_fee_id.clone())
                {
                    return Err("net_base_inventory_fee_id_invalid".into());
                }
                references(
                    &row.evidence_ids,
                    &fee.coverage.sample,
                    ReadKind::Fees,
                    &reads,
                    now,
                )?;
                common::observed(&row.base_asset_id, |asset| {
                    if asset == &q.base_asset_id {
                        Ok(())
                    } else {
                        Err("net_base_inventory_fee_asset_mismatch".into())
                    }
                })?;
                common::observed(&row.base_quantity, |quantity| {
                    common::nonnegative(quantity)?;
                    fee_sum = fee_sum.checked_add(*quantity)?;
                    Ok(())
                })?;
                common::observed(&row.effective_at_ms, |t| {
                    common::time(*t, *t, fee.coverage.sample.completed_at_ms)
                })?;
                match &row.attribution {
                    FeeAttribution::DirectOrder { provider_order_id }
                        if provider_order_id == order_id => {}
                    FeeAttribution::DirectFill { provider_fill_id }
                        if fill_ids.contains(provider_fill_id.as_str()) => {}
                    FeeAttribution::ProviderOrderFilter { provider_order_id }
                        if provider_order_id == order_id =>
                    {
                        direct_complete = false;
                    }
                    FeeAttribution::Unavailable { reason } if common::label(reason, 256) => {
                        direct_complete = false;
                    }
                    _ => return Err("net_base_inventory_fee_attribution_invalid".into()),
                }
                if matches!(&row.base_asset_id, Observed::Unavailable { .. })
                    || matches!(&row.base_quantity, Observed::Unavailable { .. })
                {
                    direct_complete = false;
                }
            }
            Ok(())
        })?;
        common::observed(&self.fee_closure, |closure| {
            closure.sample.validate(ReadKind::FeeClosure, &reads, now)?;
            if Some(closure.provider_order_id.as_str()) != order_id
                || closure.covered_gross_base.negative()
            {
                return Err("net_base_inventory_fee_closure_identity_invalid".into());
            }
            common::time(
                closure.final_through_ms,
                closure.final_through_ms,
                closure.sample.completed_at_ms,
            )?;
            for id in &closure.sample.evidence_ids {
                if !matches!(&reads[id.as_str()].provider_order_id,Observed::Available{value} if Some(value.as_str())==order_id)
                {
                    return Err("net_base_inventory_fee_closure_link_required".into());
                }
            }
            Ok(())
        })?;
        common::observed(&self.net_base_credited, |net| {
            common::nonnegative(net)?;
            let gross = *self.gross_base_filled.required()?;
            let fills = self.fills.required()?;
            let fees = self.fees.required()?;
            let closure = self.fee_closure.required()?;
            if !terminal
                || !fills.coverage.query_exhausted
                || fill_sum != gross
                || !fees.coverage.query_exhausted
                || !direct_complete
                || !closure.all_order_fees_final
                || !closure.corrections_complete
                || closure.covered_gross_base != gross
                || closure.final_through_ms < self.order.observed_at_ms
                || closure.sample.completed_at_ms < fees.coverage.sample.completed_at_ms
                || closure.sample.completed_at_ms < self.order.known_at_ms
                || gross.checked_subtract(fee_sum)? != *net
            {
                return Err("net_base_inventory_complete_final_direct_evidence_required".into());
            }
            Ok(())
        })?;
        common::observed(&self.aggregate_inventory, |aggregate| {
            if aggregate.instrument_id
                != q.original_entry
                    .original_submit
                    .order
                    .instrument
                    .instrument_id
                || aggregate.base_asset_id != q.base_asset_id
            {
                return Err("net_base_inventory_aggregate_binding_invalid".into());
            }
            common::nonnegative(&aggregate.quantity)?;
            aggregate
                .sample
                .validate(ReadKind::Positions, &reads, now)?;
            common::observed(&aggregate.quantity_available, |available| {
                common::nonnegative(available)?;
                if available.compare(aggregate.quantity)?.is_gt() {
                    return Err("net_base_inventory_aggregate_available_invalid".into());
                }
                Ok(())
            })
        })?;
        Ok(())
    }
    /// Upper bound only, not an owned quantity. The host must separately prove
    /// surviving lot attribution and subtract its retained close reservations.
    pub fn reported_available_ceiling(&self) -> Result<Decimal, String> {
        let net = *self.net_base_credited.required()?;
        let available = *self
            .aggregate_inventory
            .required()?
            .quantity_available
            .required()?;
        common::nonnegative(&net)?;
        common::nonnegative(&available)?;
        Ok(if net.compare(available)?.is_lt() {
            net
        } else {
            available
        })
    }
}
pub fn decode_request_json(bytes: &[u8]) -> Result<Request, String> {
    let q: Request = common::decode_closed(bytes, MAX_REQUEST_BYTES)?;
    q.validate()?;
    Ok(q)
}
pub fn decode_observation_json(bytes: &[u8], q: &Request, now: i64) -> Result<Observation, String> {
    let o: Observation = common::decode_closed(bytes, MAX_OBSERVATION_BYTES)?;
    o.validate(q, now)?;
    Ok(o)
}
pub fn request_schema() -> Value {
    serde_json::from_str(include_str!(
        "../schemas/net-base-inventory-request-v1.schema.json"
    ))
    .expect("static net inventory request schema")
}
pub fn response_schema() -> Value {
    serde_json::from_str(include_str!(
        "../schemas/net-base-inventory-observation-v1.schema.json"
    ))
    .expect("static net inventory observation schema")
}
pub fn observation_schema() -> Value {
    response_schema()
}
pub fn schema() -> Value {
    response_schema()
}
#[cfg(test)]
#[path = "net_base_inventory_observation_tests.rs"]
mod tests;
