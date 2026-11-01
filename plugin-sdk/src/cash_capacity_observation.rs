// Copyright (c) 2026 OptionLab LLC. All rights reserved.

//! Conditional cash-capacity components, not debt absence or an admission policy.
//! Independent reads remain non-atomic. Neither validation nor a cash ceiling
//! grants product access, reserves funds, proves no debt, or authorizes an order.
use crate::{
    broker_lookup_v3::ExpectedPaperAccount,
    canonical::{canonical_sha256, MAX_EXACT_INTEGER},
    execution::{Decimal, InstrumentIdentity, QuantityUnit},
    execution_snapshot::{Currency, Observed, OpenOrder, Position},
    settlement_observation_v2::{NonAtomic, ProviderObservationStatus, RawResponseBytes},
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
pub const REQUEST_VERSION: &str = "tradeassembly.cash_capacity_request.v1";
pub const OBSERVATION_VERSION: &str = "tradeassembly.cash_capacity_observation.v1";
pub const REQUEST_SCHEMA: &str = "schema://tradeassembly/cash-capacity-request@1";
pub const RESPONSE_SCHEMA: &str = "schema://tradeassembly/cash-capacity-observation@1";
pub const OPERATION: &str = "account.cash_capacity.paper_v1";
pub const CAPABILITY: &str = "account.cash_capacity.paper@1";
pub const PURPOSE: &str = "execution";
pub const EFFECT: &str = "read";
pub const DEPENDENCY_ID: &str = "account.cash_capacity";
pub const MAX_REQUEST_BYTES: usize = 16_384;
pub const MAX_OBSERVATION_BYTES: usize = 524_288;
pub const MAX_EVIDENCE: usize = 64;
pub const MAX_ROWS: usize = 128;
macro_rules! exact_enum {
    ($name:ident{$($v:ident=>$w:literal),+$(,)?})=>{
        #[derive(Clone,Copy,Debug,PartialEq,Eq,PartialOrd,Ord)] pub enum $name{$($v),+}
        impl Serialize for $name{fn serialize<S:serde::Serializer>(&self,s:S)->Result<S::Ok,S::Error>{s.serialize_str(match self{$(Self::$v=>$w),+})}}
        impl<'de> Deserialize<'de> for $name{fn deserialize<D:serde::Deserializer<'de>>(d:D)->Result<Self,D::Error>{match String::deserialize(d)?.as_str(){$($w=>Ok(Self::$v),)+_=>Err(serde::de::Error::custom("cash_net_enum_invalid"))}}}
    }
}
exact_enum!(LongAction{BuyToOpen=>"buy_to_open",SellToClose=>"sell_to_close"});
exact_enum!(ReadKind{Account=>"account",Asset=>"asset",Positions=>"positions",OpenOrders=>"open_orders",Order=>"order",Fills=>"fills",Fees=>"fees",FeeClosure=>"fee_closure"});
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Request {
    pub schema_version: String,
    pub expected_account: ExpectedPaperAccount,
    pub canonical_account_sha256: String,
    pub host_account_ref: String,
    pub instrument: InstrumentIdentity,
    pub base_asset_id: String,
    pub quote_currency: Currency,
    pub position_action: LongAction,
}
impl Request {
    pub fn validate(&self) -> Result<(), String> {
        self.expected_account.validate()?;
        self.quote_currency.validate()?;
        validate_instrument(&self.instrument, &self.base_asset_id, &self.quote_currency)?;
        if self.schema_version != REQUEST_VERSION
            || !label(&self.host_account_ref, 256)
            || self.canonical_account_sha256 != self.expected_account.canonical_account_sha256()?
        {
            return Err("cash_capacity_request_invalid".into());
        }
        size(self, MAX_REQUEST_BYTES)
    }
    pub fn request_sha256(&self) -> Result<String, String> {
        self.validate()?;
        canonical_sha256(self).map_err(|_| "cash_capacity_encoding_invalid".into())
    }
    pub fn validate_binding(
        &self,
        operation: &str,
        capability: &str,
        purpose: &str,
    ) -> Result<(), String> {
        self.validate()?;
        if operation != OPERATION || capability != CAPABILITY || purpose != PURPOSE {
            return Err("cash_capacity_operation_mismatch".into());
        }
        Ok(())
    }
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ReadEvidence {
    pub id: String,
    pub kind: ReadKind,
    pub provider_record_id: Observed<String>,
    pub provider_order_id: Observed<String>,
    pub hash_basis: RawResponseBytes,
    pub response_sha256: String,
    pub started_at_ms: i64,
    pub completed_at_ms: i64,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Sample {
    pub started_at_ms: i64,
    pub completed_at_ms: i64,
    pub evidence_ids: Vec<String>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AccountRead {
    pub provider_account_id: String,
    pub host_account_ref: String,
    pub currency: Currency,
    pub cash_balance: Observed<Decimal>,
    pub non_marginable_buying_power: Observed<Decimal>,
    pub borrowing_debt: Observed<Decimal>,
    pub gross_exposure: Observed<Decimal>,
    pub product_enabled: Observed<bool>,
    pub trading_blocked: Observed<bool>,
    pub account_blocked: Observed<bool>,
    pub user_suspended: Observed<bool>,
    /// Product evidence only, never credential scope, mandate or host permission.
    pub product_actions: Observed<Vec<LongAction>>,
    pub sample: Sample,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AssetRead {
    pub instrument: InstrumentIdentity,
    pub base_asset_id: String,
    pub tradable: Observed<bool>,
    pub marginable: Observed<bool>,
    pub shortable: Observed<bool>,
    pub minimum_quantity: Observed<Decimal>,
    pub quantity_increment: Observed<Decimal>,
    pub price_increment: Observed<Decimal>,
    pub sample: Sample,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PositionsRead {
    pub complete: bool,
    pub positions: Vec<Position>,
    pub sample: Sample,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OpenOrdersRead {
    pub complete: bool,
    pub open_orders: Vec<OpenOrder>,
    pub sample: Sample,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Observation {
    pub schema_version: String,
    pub request_sha256: String,
    pub canonical_account_sha256: String,
    pub status: ProviderObservationStatus,
    pub coherence: NonAtomic,
    pub account: Observed<AccountRead>,
    pub asset: Observed<AssetRead>,
    pub positions: Observed<PositionsRead>,
    pub open_orders: Observed<OpenOrdersRead>,
    pub evidence: Vec<ReadEvidence>,
}
impl Observation {
    pub fn validate(&self, q: &Request, now: i64) -> Result<(), String> {
        q.validate()?;
        size(self, MAX_OBSERVATION_BYTES)?;
        if self.schema_version != OBSERVATION_VERSION
            || self.request_sha256 != q.request_sha256()?
            || self.canonical_account_sha256 != q.canonical_account_sha256
        {
            return Err("cash_capacity_response_binding_invalid".into());
        }
        let reads = validate_evidence(
            &self.evidence,
            now,
            &[
                ReadKind::Account,
                ReadKind::Asset,
                ReadKind::Positions,
                ReadKind::OpenOrders,
            ],
            None,
        )?;
        observed(&self.account, |a| {
            if a.provider_account_id != q.expected_account.provider_account_id
                || a.host_account_ref != q.host_account_ref
                || a.currency != q.quote_currency
            {
                return Err("cash_capacity_account_mismatch".into());
            }
            a.currency.validate()?;
            a.sample.validate(ReadKind::Account, &reads, now)?;
            // Negative balances remain observed negative facts; they are never
            // normalized to zero or accepted by reported_cash_ceiling().
            observed(&a.cash_balance, |_| Ok(()))?;
            observed(&a.non_marginable_buying_power, |_| Ok(()))?;
            observed(&a.borrowing_debt, nonnegative)?;
            observed(&a.gross_exposure, nonnegative)?;
            for flag in [
                &a.product_enabled,
                &a.trading_blocked,
                &a.account_blocked,
                &a.user_suspended,
            ] {
                observed(flag, |_| Ok(()))?;
            }
            observed(&a.product_actions, |actions| {
                if actions.len() > 2
                    || actions.iter().copied().collect::<BTreeSet<_>>().len() != actions.len()
                {
                    return Err("cash_capacity_product_actions_invalid".into());
                }
                if !actions.is_empty()
                    && (!is_bool(&a.product_enabled, true)
                        || !is_bool(&a.trading_blocked, false)
                        || !is_bool(&a.account_blocked, false)
                        || !is_bool(&a.user_suspended, false))
                {
                    return Err("cash_capacity_product_evidence_missing_or_denied".into());
                }
                Ok(())
            })
        })?;
        observed(&self.asset, |a| {
            if a.instrument != q.instrument || a.base_asset_id != q.base_asset_id {
                return Err("cash_capacity_asset_mismatch".into());
            }
            a.sample.validate(ReadKind::Asset, &reads, now)?;
            for flag in [&a.tradable, &a.marginable, &a.shortable] {
                observed(flag, |_| Ok(()))?;
            }
            for step in [
                &a.minimum_quantity,
                &a.quantity_increment,
                &a.price_increment,
            ] {
                observed(step, positive)?;
            }
            Ok(())
        })?;
        observed(&self.positions, |p| {
            p.sample.validate(ReadKind::Positions, &reads, now)?;
            if p.positions.len() > MAX_ROWS {
                return Err("cash_capacity_positions_bounds".into());
            }
            let mut ids = BTreeSet::new();
            for row in &p.positions {
                if !label(&row.instrument_id, 256)
                    || row.quantity.negative()
                    || !ids.insert((&row.instrument_id, format!("{:?}", row.side)))
                {
                    return Err("cash_capacity_position_invalid".into());
                }
                observed(&row.gross_value, nonnegative)?;
            }
            Ok(())
        })?;
        observed(&self.open_orders, |o| {
            o.sample.validate(ReadKind::OpenOrders, &reads, now)?;
            if o.open_orders.len() > MAX_ROWS {
                return Err("cash_capacity_orders_bounds".into());
            }
            let mut ids = BTreeSet::new();
            for row in &o.open_orders {
                if !label(&row.provider_order_id, 256)
                    || !label(&row.instrument_id, 256)
                    || row.remaining_quantity.negative()
                    || row.client_order_id.as_ref().is_some_and(|s| !label(s, 128))
                    || !ids.insert(&row.provider_order_id)
                {
                    return Err("cash_capacity_order_invalid".into());
                }
                observed(&row.worst_case_cash, nonnegative)?;
            }
            Ok(())
        })?;
        Ok(())
    }
    /// Arithmetic ceiling of two reported values, BEFORE host holds/risk and
    /// independent product/source/permission checks. It is not spendable cash.
    /// In particular, it neither reads nor changes the debt observation.
    pub fn reported_cash_ceiling(&self) -> Result<Decimal, String> {
        let a = self.account.required()?;
        let cash = *a.cash_balance.required()?;
        let buying = *a.non_marginable_buying_power.required()?;
        if cash.negative() || buying.negative() {
            return Err("cash_capacity_negative_capacity".into());
        }
        Ok(if cash.compare(buying)?.is_lt() {
            cash
        } else {
            buying
        })
    }
}
pub(crate) fn label(s: &str, max: usize) -> bool {
    !s.is_empty() && s.len() <= max && s.trim() == s && !s.chars().any(char::is_control)
}
pub(crate) fn digest(s: &str) -> bool {
    s.len() == 64
        && s.bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
pub(crate) fn nonnegative(v: &Decimal) -> Result<(), String> {
    if v.negative() {
        Err("cash_net_negative_quantity".into())
    } else {
        Ok(())
    }
}
pub(crate) fn positive(v: &Decimal) -> Result<(), String> {
    if !v.positive() {
        Err("cash_net_positive_quantity_required".into())
    } else {
        Ok(())
    }
}
pub(crate) fn is_bool(v: &Observed<bool>, expected: bool) -> bool {
    matches!(v,Observed::Available{value} if *value==expected)
}
pub(crate) fn observed<T>(
    v: &Observed<T>,
    check: impl FnOnce(&T) -> Result<(), String>,
) -> Result<(), String> {
    match v {
        Observed::Available { value } => check(value),
        Observed::Unavailable { reason } if label(reason, 256) => Ok(()),
        _ => Err("cash_net_unavailable_reason_invalid".into()),
    }
}
pub(crate) fn size<T: Serialize>(v: &T, max: usize) -> Result<(), String> {
    if serde_json::to_vec(v)
        .map_err(|_| "cash_net_encoding_invalid")?
        .len()
        > max
    {
        Err("cash_net_bounds_invalid".into())
    } else {
        Ok(())
    }
}
pub(crate) fn time(start: i64, end: i64, now: i64) -> Result<(), String> {
    if start <= 0 || end < start || now <= 0 || now > MAX_EXACT_INTEGER as i64 || end > now {
        Err("cash_net_sampling_invalid".into())
    } else {
        Ok(())
    }
}
pub(crate) fn validate_instrument(
    i: &InstrumentIdentity,
    base: &str,
    currency: &Currency,
) -> Result<(), String> {
    if !label(&i.instrument_id, 256)
        || !label(&i.provider_symbol, 128)
        || !label(base, 128)
        || i.asset_class != "crypto"
        || i.instrument_family != "crypto_spot"
        || i.quantity_unit != QuantityUnit::BaseAsset
        || i.multiplier != "1".parse::<Decimal>()?
        || i.quote_currency != currency.code
    {
        return Err("cash_net_spot_base_asset_profile_required".into());
    }
    Ok(())
}
pub(crate) type EvidenceIndex<'a> = BTreeMap<&'a str, &'a ReadEvidence>;
pub(crate) fn validate_evidence<'a>(
    rows: &'a [ReadEvidence],
    now: i64,
    allowed: &[ReadKind],
    order: Option<&str>,
) -> Result<EvidenceIndex<'a>, String> {
    time(1, 1, now)?;
    if rows.len() > MAX_EVIDENCE {
        return Err("cash_net_evidence_bounds".into());
    }
    let mut index = BTreeMap::new();
    for e in rows {
        time(e.started_at_ms, e.completed_at_ms, now)?;
        if !label(&e.id, 128)
            || !digest(&e.response_sha256)
            || !allowed.contains(&e.kind)
            || index.insert(e.id.as_str(), e).is_some()
        {
            return Err("cash_net_evidence_invalid".into());
        }
        observed(&e.provider_record_id, |s| {
            if label(s, 256) {
                Ok(())
            } else {
                Err("cash_net_record_id_invalid".into())
            }
        })?;
        observed(&e.provider_order_id, |s| {
            if label(s, 256) && order == Some(s.as_str()) {
                Ok(())
            } else {
                Err("cash_net_evidence_order_mismatch".into())
            }
        })?;
    }
    Ok(index)
}
impl Sample {
    pub(crate) fn validate(
        &self,
        kind: ReadKind,
        index: &EvidenceIndex<'_>,
        now: i64,
    ) -> Result<(), String> {
        time(self.started_at_ms, self.completed_at_ms, now)?;
        if self.evidence_ids.is_empty() || self.evidence_ids.len() > MAX_EVIDENCE {
            return Err("cash_net_evidence_reference_invalid".into());
        }
        let mut seen = BTreeSet::new();
        for id in &self.evidence_ids {
            let e = index
                .get(id.as_str())
                .ok_or("cash_net_evidence_unresolved")?;
            if !seen.insert(id)
                || e.kind != kind
                || e.started_at_ms < self.started_at_ms
                || e.completed_at_ms > self.completed_at_ms
            {
                return Err("cash_net_evidence_reference_invalid".into());
            }
        }
        Ok(())
    }
}
// No schema engine/dependency or runtime filesystem access: these are compiled
// immutable public documents. The host still authenticates all source evidence.
pub fn request_schema() -> Value {
    serde_json::from_str(include_str!(
        "../schemas/cash-capacity-request-v1.schema.json"
    ))
    .expect("static cash request schema")
}
pub fn response_schema() -> Value {
    serde_json::from_str(include_str!(
        "../schemas/cash-capacity-observation-v1.schema.json"
    ))
    .expect("static cash observation schema")
}
pub fn observation_schema() -> Value {
    response_schema()
}
pub fn schema() -> Value {
    response_schema()
}
pub(crate) fn decode_closed<T: serde::de::DeserializeOwned + Serialize>(
    bytes: &[u8],
    max: usize,
) -> Result<T, String> {
    if bytes.len() > max {
        return Err("cash_net_bounds_invalid".into());
    }
    let mut raw: Value = serde_json::from_slice(bytes).map_err(|_| "cash_net_json_invalid")?;
    crate::canonical::normalize_numbers(&mut raw).map_err(|_| "cash_net_number_invalid")?;
    let parsed: T = serde_json::from_value(raw.clone()).map_err(|_| "cash_net_json_invalid")?;
    fn shape(a: &Value, b: &Value) -> bool {
        match (a, b) {
            (Value::Object(a), Value::Object(b)) => {
                a.len() == b.len() && a.iter().all(|(k, a)| b.get(k).is_some_and(|b| shape(a, b)))
            }
            (Value::Array(a), Value::Array(b)) => {
                a.len() == b.len() && a.iter().zip(b).all(|(a, b)| shape(a, b))
            }
            (Value::String(_), Value::String(_))
            | (Value::Number(_), Value::Number(_))
            | (Value::Bool(_), Value::Bool(_))
            | (Value::Null, Value::Null) => true,
            _ => false,
        }
    }
    if !shape(
        &raw,
        &serde_json::to_value(&parsed).map_err(|_| "cash_net_encoding_invalid")?,
    ) {
        return Err("cash_net_wire_shape_invalid".into());
    }
    Ok(parsed)
}
pub fn decode_request_json(bytes: &[u8]) -> Result<Request, String> {
    let q: Request = decode_closed(bytes, MAX_REQUEST_BYTES)?;
    q.validate()?;
    Ok(q)
}
pub fn decode_observation_json(bytes: &[u8], q: &Request, now: i64) -> Result<Observation, String> {
    let o: Observation = decode_closed(bytes, MAX_OBSERVATION_BYTES)?;
    o.validate(q, now)?;
    Ok(o)
}
#[cfg(test)]
#[path = "cash_capacity_observation_tests.rs"]
mod tests;
