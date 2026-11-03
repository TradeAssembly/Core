//! Non-atomic provider reads, never a coherent posting or permission to release
//! reservations. Original coherent cash/close V1 contracts remain separate.
use crate::{
    broker_lookup_v3::LookupRequestV3,
    broker_v2::{OrderObservation, OrderStatus},
    canonical::{canonical_sha256, MAX_EXACT_INTEGER},
    execution::{Decimal, PositionAction},
    execution_snapshot::{
        self as account, AccountSnapshot, Currency, Observed, TimeBasis, TradingMode,
    },
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};

pub const REQUEST_VERSION: &str = "tradeassembly.settlement_observation_request.v2";
pub const OBSERVATION_VERSION: &str = "tradeassembly.settlement_observation.v2";
pub const REQUEST_SCHEMA: &str = "schema://tradeassembly/settlement-observation-request@2";
pub const RESPONSE_SCHEMA: &str = "schema://tradeassembly/settlement-observation@2";
pub const CASH_OPERATION: &str = "broker.cash_settlement.paper_v2";
pub const CASH_CAPABILITY: &str = "broker.cash_settlement.paper@2";
pub const CLOSE_OPERATION: &str = "broker.close_settlement.paper_v2";
pub const CLOSE_CAPABILITY: &str = "broker.close_settlement.paper@2";
pub const MAX_REQUEST_BYTES: usize = 32_768;
pub const MAX_OBSERVATION_BYTES: usize = 1_048_576;
pub const MAX_EVIDENCE: usize = 64;

// New enums accept their exact JSON string form only, never object-form aliases.
macro_rules! string_enum {
    ($name:ident { $($variant:ident => $wire:literal),+ $(,)? }) => {
        #[derive(Clone, Copy, Debug, PartialEq, Eq)]
        pub enum $name { $($variant),+ }
        impl Serialize for $name {
            fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
                serializer.serialize_str(match self { $(Self::$variant => $wire),+ })
            }
        }
        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
                match String::deserialize(deserializer)?.as_str() {
                    $($wire => Ok(Self::$variant)),+,
                    _ => Err(serde::de::Error::custom("settlement_observation_v2_enum_invalid")),
                }
            }
        }
    };
}
string_enum!(Role { Entry => "entry", Close => "close" });
string_enum!(ProviderObservationStatus { Pending => "pending", Provisional => "provisional" });
string_enum!(NonAtomic { NonAtomic => "non_atomic" });
string_enum!(ProviderReported { ProviderReported => "provider_reported" });
string_enum!(RawResponseBytes { RawResponseBytes => "raw_response_bytes" });
string_enum!(EvidenceKind {
    Order => "order", Fees => "fees", Account => "account",
    Positions => "positions", OpenOrders => "open_orders",
});
impl Role {
    pub fn operation_id(self) -> &'static str {
        match self {
            Self::Entry => CASH_OPERATION,
            Self::Close => CLOSE_OPERATION,
        }
    }
    pub fn capability(self) -> &'static str {
        match self {
            Self::Entry => CASH_CAPABILITY,
            Self::Close => CLOSE_CAPABILITY,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SettlementObservationRequestV2 {
    pub schema_version: String,
    pub role: Role,
    pub lookup: LookupRequestV3,
    /// Supplied original host alias, not inferred from a provider account ID.
    pub host_account_ref: String,
}
pub type Request = SettlementObservationRequestV2;
impl SettlementObservationRequestV2 {
    pub fn validate(&self) -> Result<(), String> {
        self.lookup.validate()?;
        if self.schema_version != REQUEST_VERSION || !label(&self.host_account_ref, 256) {
            return Err("settlement_observation_v2_request_invalid".into());
        }
        if !matches!(
            (
                &self.role,
                &self.lookup.original_submit.order.position_action
            ),
            (Role::Entry, PositionAction::BuyToOpen) | (Role::Close, PositionAction::SellToClose)
        ) {
            return Err("settlement_observation_v2_role_mismatch".into());
        }
        Ok(())
    }
    pub fn validate_binding(&self, operation: &str, capability: &str) -> Result<(), String> {
        self.validate()?;
        if operation != self.role.operation_id() || capability != self.role.capability() {
            return Err("settlement_observation_v2_operation_mismatch".into());
        }
        Ok(())
    }
    pub fn request_sha256(&self) -> Result<String, String> {
        self.validate()?;
        canonical_sha256(self).map_err(|_| "settlement_observation_v2_request_invalid".into())
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProviderReportedAveragePrice {
    pub basis: ProviderReported,
    /// Provider-reported unit price, never gross cash or a cash debit.
    pub price: Decimal,
    pub currency: Currency,
    pub evidence_ids: Vec<String>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProviderReportedFeeCash {
    pub amount: Decimal,
    pub currency: Currency,
    pub evidence_ids: Vec<String>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProviderFeeFinality {
    pub basis: ProviderReported,
    #[serde(rename = "final")]
    pub final_: bool,
    pub evidence_ids: Vec<String>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ComponentSample {
    pub started_at_ms: i64,
    pub completed_at_ms: i64,
    pub evidence_ids: Vec<String>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ComponentSamples {
    pub account: Observed<ComponentSample>,
    pub positions: Observed<ComponentSample>,
    pub open_orders: Observed<ComponentSample>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct NonAtomicAccountSample {
    pub snapshot: AccountSnapshot,
    pub components: ComponentSamples,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProviderSourceEvidence {
    pub id: String,
    pub kind: EvidenceKind,
    pub provider_record_id: Observed<String>,
    pub hash_basis: RawResponseBytes,
    /// Digest of an actual raw read response, not a ledger revision/sequence.
    pub response_sha256: String,
    pub started_at_ms: i64,
    pub completed_at_ms: i64,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SettlementObservationV2 {
    pub schema_version: String,
    pub request_sha256: String,
    pub status: ProviderObservationStatus,
    pub coherence: NonAtomic,
    pub order: OrderObservation,
    pub average_fill_price: Observed<ProviderReportedAveragePrice>,
    pub fee_cash: Observed<ProviderReportedFeeCash>,
    pub fee_finality: Observed<ProviderFeeFinality>,
    pub account: Observed<NonAtomicAccountSample>,
    pub evidence: Vec<ProviderSourceEvidence>,
}
pub type Observation = SettlementObservationV2;

fn label(s: &str, max: usize) -> bool {
    !s.is_empty() && s.len() <= max && s.trim() == s && !s.chars().any(char::is_control)
}
fn digest(s: &str) -> bool {
    s.len() == 64
        && s.bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
fn range(start: i64, end: i64, now: i64) -> Result<(), String> {
    if start <= 0 || end < start || end > now || now > MAX_EXACT_INTEGER as i64 {
        return Err("settlement_observation_v2_sampling_invalid".into());
    }
    Ok(())
}
fn observed<T>(
    value: &Observed<T>,
    validate: impl FnOnce(&T) -> Result<(), String>,
) -> Result<(), String> {
    match value {
        Observed::Available { value } => validate(value),
        Observed::Unavailable { reason } if label(reason, 256) => Ok(()),
        Observed::Unavailable { .. } => {
            Err("settlement_observation_v2_unavailable_reason_invalid".into())
        }
    }
}
fn available<T>(value: &Observed<T>) -> bool {
    matches!(value, Observed::Available { .. })
}
type Evidence<'a> = BTreeMap<&'a str, &'a ProviderSourceEvidence>;
fn references(ids: &[String], kind: EvidenceKind, evidence: &Evidence<'_>) -> Result<(), String> {
    if ids.is_empty() || ids.len() > MAX_EVIDENCE {
        return Err("settlement_observation_v2_evidence_reference_invalid".into());
    }
    let mut seen = BTreeSet::new();
    for id in ids {
        if !label(id, 128)
            || !seen.insert(id)
            || evidence.get(id.as_str()).is_none_or(|e| e.kind != kind)
        {
            return Err("settlement_observation_v2_evidence_reference_invalid".into());
        }
    }
    Ok(())
}
fn component(
    value: &Observed<ComponentSample>,
    kind: EvidenceKind,
    evidence: &Evidence<'_>,
    aggregate: i64,
    now: i64,
) -> Result<(), String> {
    observed(value, |sample| {
        range(sample.started_at_ms, sample.completed_at_ms, now)?;
        references(&sample.evidence_ids, kind, evidence)?;
        if sample.completed_at_ms > aggregate
            || sample.evidence_ids.iter().any(|id| {
                let item = evidence[id.as_str()];
                item.started_at_ms < sample.started_at_ms
                    || item.completed_at_ms > sample.completed_at_ms
            })
        {
            return Err("settlement_observation_v2_component_sampling_invalid".into());
        }
        Ok(())
    })
}
fn account_sample(
    sample: &NonAtomicAccountSample,
    request: &Request,
    evidence: &Evidence<'_>,
    now: i64,
) -> Result<(), String> {
    let a = &sample.snapshot;
    a.validate()?;
    if a.account_ref != request.host_account_ref
        || a.provider_account_id != request.lookup.expected_account.provider_account_id
        || a.mode != TradingMode::Paper
        || a.time.basis != TimeBasis::HostAcquired
        || a.time.known_at_ms > now
    {
        return Err("settlement_observation_v2_account_binding_invalid".into());
    }
    component(
        &sample.components.account,
        EvidenceKind::Account,
        evidence,
        a.time.observed_at_ms,
        now,
    )?;
    component(
        &sample.components.positions,
        EvidenceKind::Positions,
        evidence,
        a.time.observed_at_ms,
        now,
    )?;
    component(
        &sample.components.open_orders,
        EvidenceKind::OpenOrders,
        evidence,
        a.time.observed_at_ms,
        now,
    )?;
    let account_observed = available(&sample.components.account);
    let positions_observed = available(&sample.components.positions);
    let orders_observed = available(&sample.components.open_orders);
    if !(account_observed || positions_observed || orders_observed)
        || (!account_observed
            && (a.coverage.account_complete
                || !a.instrument_permissions.is_empty()
                || available(&a.cash_balance)
                || available(&a.non_marginable_buying_power)
                || available(&a.borrowing_debt)
                || available(&a.gross_exposure)))
        || (!positions_observed && (a.coverage.positions_complete || available(&a.positions)))
        || (!orders_observed && (a.coverage.open_orders_complete || available(&a.open_orders)))
        || (a.coverage.positions_complete && !available(&a.positions))
        || (a.coverage.open_orders_complete && !available(&a.open_orders))
    {
        return Err("settlement_observation_v2_account_coverage_invalid".into());
    }
    for value in [
        &a.cash_balance,
        &a.non_marginable_buying_power,
        &a.borrowing_debt,
        &a.gross_exposure,
    ] {
        observed(value, |_| Ok(()))?;
    }
    for permission in &a.instrument_permissions {
        observed(&permission.actions, |actions| {
            if actions.len() > 4
                || actions
                    .iter()
                    .enumerate()
                    .any(|(i, action)| actions[..i].contains(action))
            {
                return Err("settlement_observation_v2_permission_invalid".into());
            }
            Ok(())
        })?;
    }
    observed(&a.positions, |positions| {
        for position in positions {
            observed(&position.gross_value, |_| Ok(()))?;
        }
        Ok(())
    })?;
    observed(&a.open_orders, |orders| {
        for order in orders {
            observed(&order.worst_case_cash, |_| Ok(()))?;
        }
        Ok(())
    })?;
    Ok(())
}
impl SettlementObservationV2 {
    /// Checks bounded structure, causal read times and original identity only.
    /// Success never means reconciled, final/coherent, or safe to release holds.
    pub fn validate(&self, request: &Request, now: i64) -> Result<(), String> {
        request.validate()?;
        if now <= 0 || now > MAX_EXACT_INTEGER as i64 {
            return Err("settlement_observation_v2_time_invalid".into());
        }
        if self.schema_version != OBSERVATION_VERSION
            || self.request_sha256 != request.request_sha256()?
        {
            return Err("settlement_observation_v2_request_mismatch".into());
        }
        observed(&self.order.filled_quantity, |value| {
            if value.negative()
                || value
                    .compare(request.lookup.original_submit.order.quantity)?
                    .is_gt()
            {
                return Err("settlement_observation_v2_fill_invalid".into());
            }
            Ok(())
        })?;
        observed(&self.order.cash_spent, |_| Ok(()))?;
        request.lookup.validate_observation(&self.order, now)?;
        let expected = match self.order.status {
            OrderStatus::Open | OrderStatus::PartiallyFilled | OrderStatus::Unknown => {
                ProviderObservationStatus::Pending
            }
            OrderStatus::Filled
            | OrderStatus::Rejected
            | OrderStatus::Cancelled
            | OrderStatus::Expired => ProviderObservationStatus::Provisional,
        };
        if self.status != expected || self.evidence.len() > MAX_EVIDENCE {
            return Err("settlement_observation_v2_status_or_budget_invalid".into());
        }
        let mut evidence = BTreeMap::new();
        for item in &self.evidence {
            if !label(&item.id, 128)
                || !digest(&item.response_sha256)
                || evidence.insert(item.id.as_str(), item).is_some()
            {
                return Err("settlement_observation_v2_evidence_invalid".into());
            }
            range(item.started_at_ms, item.completed_at_ms, now)?;
            observed(&item.provider_record_id, |id| {
                if !label(id, 256) {
                    return Err("settlement_observation_v2_evidence_invalid".into());
                }
                if (item.kind == EvidenceKind::Account
                    && id != &request.lookup.expected_account.provider_account_id)
                    || (item.kind == EvidenceKind::Order
                        && self
                            .order
                            .provider_order_id
                            .as_ref()
                            .is_some_and(|original| original != id))
                {
                    return Err("settlement_observation_v2_evidence_binding_invalid".into());
                }
                Ok(())
            })?;
        }
        let order_known = self.order.status != OrderStatus::Unknown
            || self.order.provider_order_id.is_some()
            || available(&self.order.filled_quantity)
            || available(&self.order.cash_spent);
        if order_known
            && !self
                .evidence
                .iter()
                .any(|item| item.kind == EvidenceKind::Order)
        {
            return Err("settlement_observation_v2_order_evidence_missing".into());
        }
        observed(&self.average_fill_price, |value| {
            value.currency.validate()?;
            if !value.price.positive()
                || value.currency.code
                    != request
                        .lookup
                        .original_submit
                        .order
                        .instrument
                        .quote_currency
            {
                return Err("settlement_observation_v2_average_price_invalid".into());
            }
            references(&value.evidence_ids, EvidenceKind::Order, &evidence)
        })?;
        observed(&self.fee_cash, |value| {
            value.currency.validate()?;
            if value.amount.negative() {
                return Err("settlement_observation_v2_fee_invalid".into());
            }
            references(&value.evidence_ids, EvidenceKind::Fees, &evidence)
        })?;
        observed(&self.fee_finality, |value| {
            references(&value.evidence_ids, EvidenceKind::Fees, &evidence)
        })?;
        observed(&self.account, |value| {
            account_sample(value, request, &evidence, now)
        })?;
        if serde_json::to_vec(self)
            .map_err(|_| "settlement_observation_v2_encoding_invalid")?
            .len()
            > MAX_OBSERVATION_BYTES
        {
            return Err("settlement_observation_v2_response_too_large".into());
        }
        Ok(())
    }
}
pub fn decode_request_json(bytes: &[u8]) -> Result<Request, String> {
    if bytes.len() > MAX_REQUEST_BYTES {
        return Err("settlement_observation_v2_request_too_large".into());
    }
    let value: Request =
        serde_json::from_slice(bytes).map_err(|_| "settlement_observation_v2_request_invalid")?;
    value.validate()?;
    Ok(value)
}
pub fn decode_observation_json(
    bytes: &[u8],
    request: &Request,
    now: i64,
) -> Result<Observation, String> {
    if bytes.len() > MAX_OBSERVATION_BYTES {
        return Err("settlement_observation_v2_response_too_large".into());
    }
    let value: Observation =
        serde_json::from_slice(bytes).map_err(|_| "settlement_observation_v2_response_invalid")?;
    value.validate(request, now)?;
    Ok(value)
}

fn object(properties: Value) -> Value {
    let required = properties
        .as_object()
        .unwrap()
        .keys()
        .cloned()
        .collect::<Vec<_>>();
    json!({"type":"object","additionalProperties":false,"properties":properties,"required":required})
}
fn text(max: usize) -> Value {
    json!({"type":"string","minLength":1,"maxLength":max})
}
fn sha() -> Value {
    json!({"type":"string","pattern":"^[0-9a-f]{64}$"})
}
fn stamp() -> Value {
    json!({"type":"integer","minimum":1,"maximum":MAX_EXACT_INTEGER})
}
fn observed_schema(value: Value) -> Value {
    json!({"oneOf":[object(json!({"status":{"const":"available"},"value":value})),
        object(json!({"status":{"const":"unavailable"},"reason":text(256)}))]})
}
fn refs_schema() -> Value {
    json!({"type":"array","minItems":1,"maxItems":MAX_EVIDENCE,"uniqueItems":true,"items":text(128)})
}
fn currency_schema() -> Value {
    object(
        json!({"code":{"type":"string","pattern":"^[A-Z0-9]{1,16}$"},"decimals":{"type":"integer","minimum":0,"maximum":12}}),
    )
}
fn sample_schema() -> Value {
    object(json!({"startedAtMs":stamp(),"completedAtMs":stamp(),"evidenceIds":refs_schema()}))
}
pub fn request_schema() -> Value {
    let mut value = object(json!({"schemaVersion":{"const":REQUEST_VERSION},
        "role":{"enum":["entry","close"]},"lookup":crate::broker_lookup_v3::request_schema(),
        "hostAccountRef":text(256)}));
    value["allOf"] = json!([
        {"if":{"properties":{"role":{"const":"entry"}}},"then":{"properties":{"lookup":{"properties":{"originalSubmit":{"properties":{"order":{"properties":{"positionAction":{"const":"buy_to_open"}}}}}}}}}},
        {"if":{"properties":{"role":{"const":"close"}}},"then":{"properties":{"lookup":{"properties":{"originalSubmit":{"properties":{"order":{"properties":{"positionAction":{"const":"sell_to_close"}}}}}}}}}}
    ]);
    value
}
pub fn response_schema() -> Value {
    let mut snapshot = account::account_schema();
    snapshot["properties"]["mode"] = json!({"const":"paper"});
    snapshot["properties"]["time"]["properties"]["basis"] = json!({"const":"host_acquired"});
    let evidence = object(
        json!({"id":text(128),"kind":{"enum":["order","fees","account","positions","open_orders"]},
        "providerRecordId":observed_schema(text(256)),"hashBasis":{"const":"raw_response_bytes"},
        "responseSha256":sha(),"startedAtMs":stamp(),"completedAtMs":stamp()}),
    );
    let mut value = object(
        json!({"schemaVersion":{"const":OBSERVATION_VERSION},"requestSha256":sha(),
        "status":{"enum":["pending","provisional"]},"coherence":{"const":"non_atomic"},
        "order":crate::broker_v2::observation_schema(),
        "averageFillPrice":observed_schema(object(json!({"basis":{"const":"provider_reported"},
            "price":account::decimal_schema(),"currency":currency_schema(),"evidenceIds":refs_schema()}))),
        "feeCash":observed_schema(object(json!({"amount":account::decimal_schema(),"currency":currency_schema(),"evidenceIds":refs_schema()}))),
        "feeFinality":observed_schema(object(json!({"basis":{"const":"provider_reported"},"final":{"type":"boolean"},"evidenceIds":refs_schema()}))),
        "account":observed_schema(object(json!({"snapshot":snapshot,"components":object(json!({
            "account":observed_schema(sample_schema()),"positions":observed_schema(sample_schema()),"openOrders":observed_schema(sample_schema())}))}))),
        "evidence":{"type":"array","maxItems":MAX_EVIDENCE,"items":evidence}}),
    );
    value["allOf"] = json!([
        {"if":{"properties":{"status":{"const":"pending"}}},"then":{"properties":{"order":{"properties":{"status":{"enum":["unknown","open","partially_filled"]}}}}}},
        {"if":{"properties":{"status":{"const":"provisional"}}},"then":{"properties":{"order":{"properties":{"status":{"enum":["filled","rejected","cancelled","expired"]}}}}}}
    ]);
    value
}

#[cfg(test)]
#[path = "settlement_observation_v2_tests.rs"]
mod tests;
