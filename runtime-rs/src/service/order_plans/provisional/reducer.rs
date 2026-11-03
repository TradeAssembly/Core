//! Pure, conservative projection of non-atomic provider observations.
//! No storage, provider calls, cash calculation, reconciliation or hold release.
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tradeassembly_plugin_sdk::{
    broker_v2::OrderStatus,
    canonical::{canonical_sha256, MAX_EXACT_INTEGER},
    execution::Decimal,
    execution_snapshot::{Currency, Observed},
    settlement_observation_v2::{
        EvidenceKind, Observation, ProviderObservationStatus, Request, Role,
    },
};

const VERSION: &str = "tradeassembly.settlement_observation_projection.v1";
const MAX_BYTES: usize = 32_768;
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct MoneyFact {
    pub amount: Decimal,
    pub currency: Currency,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct ReadWatermark {
    pub kind: EvidenceKind,
    pub started_at_ms: i64,
    pub completed_at_ms: i64,
    pub contents_sha256: String,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum AccountAvailability {
    Unavailable,
    Partial,
    Complete,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct Projection {
    pub schema_version: String,
    pub request_sha256: String,
    pub role: Role,
    pub provider_order_id: Option<String>,
    pub filled_quantity: Option<Decimal>,
    /// Entry-order debit or closing fee debit; excludes closing proceeds.
    pub cash_debit: Option<Decimal>,
    pub average_fill_price: Option<MoneyFact>,
    pub fee_cash: Option<MoneyFact>,
    /// Provider-reported only, never host verification of final settlement.
    pub fee_finality: Option<bool>,
    pub order_status: OrderStatus,
    pub order_observed_at_ms: i64,
    pub order_known_at_ms: i64,
    pub order_facts_sha256: String,
    pub latest_evidence_completed_at_ms: Option<i64>,
    pub read_watermarks: Vec<ReadWatermark>,
    pub latest_account_availability: AccountAvailability,
    pub last_available_account_sha256: Option<String>,
    pub generation: u64,
    pub status: ProviderObservationStatus,
    pub observation_sha256: String,
    pub projection_sha256: String,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Disposition {
    Unchanged,
    Advanced,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Blocker {
    OrderPending,
    FilledQuantityUnavailable,
    CashDebitUnavailable,
    AverageFillPriceUnavailable,
    FeeCashUnavailable,
    FeeFinalityUnavailable,
    FeesNotFinal,
    ProviderFeeFinalityUnverified,
    AccountUnavailable,
    AccountCoverageIncomplete,
    CoherentPostingUnavailable,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct Outcome {
    pub disposition: Disposition,
    pub observation_sha256: String,
    pub projection: Projection,
    pub blockers: Vec<Blocker>,
}
fn hash<T: Serialize>(value: &T) -> Result<String, String> {
    canonical_sha256(value).map_err(|_| "settlement_projection_encoding_invalid".into())
}
fn digest(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
fn known<T>(value: &Observed<T>) -> Option<&T> {
    match value {
        Observed::Available { value } => Some(value),
        Observed::Unavailable { .. } => None,
    }
}
fn terminal(status: &OrderStatus) -> bool {
    matches!(
        status,
        OrderStatus::Filled | OrderStatus::Cancelled | OrderStatus::Expired | OrderStatus::Rejected
    )
}
fn status_for(status: &OrderStatus) -> ProviderObservationStatus {
    if terminal(status) {
        ProviderObservationStatus::Provisional
    } else {
        ProviderObservationStatus::Pending
    }
}
fn kind_index(kind: EvidenceKind) -> usize {
    match kind {
        EvidenceKind::Order => 0,
        EvidenceKind::Fees => 1,
        EvidenceKind::Account => 2,
        EvidenceKind::Positions => 3,
        EvidenceKind::OpenOrders => 4,
    }
}
fn projection_hash(projection: &Projection) -> Result<String, String> {
    let mut value = projection.clone();
    value.projection_sha256.clear();
    hash(&value)
}
fn order_facts(
    status: &OrderStatus,
    provider_id: &Option<String>,
    filled: &Option<Decimal>,
    cash: &Option<Decimal>,
) -> Result<String, String> {
    hash(&(status, provider_id, filled, cash))
}
fn incoming_order_facts(observation: &Observation) -> Result<String, String> {
    order_facts(
        &observation.order.status,
        &observation.order.provider_order_id,
        &known(&observation.order.filled_quantity).copied(),
        &known(&observation.order.cash_spent).copied(),
    )
}
fn field_facts(observation: &Observation, kind: EvidenceKind) -> Value {
    let account = known(&observation.account).map(|sample| &sample.snapshot);
    match kind {
        EvidenceKind::Order=>json!({"order":{
            "status":observation.order.status,"providerOrderId":observation.order.provider_order_id,
            "filledQuantity":known(&observation.order.filled_quantity),"cashSpent":known(&observation.order.cash_spent)},
            "averageFillPrice":known(&observation.average_fill_price).map(|v|(&v.price,&v.currency))}),
        EvidenceKind::Fees=>json!({"feeCash":known(&observation.fee_cash).map(|v|(&v.amount,&v.currency)),
            "feeFinality":known(&observation.fee_finality).map(|v|v.final_)}),
        EvidenceKind::Account=>json!(account.map(|a|json!({"currency":a.currency,"cash":known(&a.cash_balance),
            "buyingPower":known(&a.non_marginable_buying_power),"debt":known(&a.borrowing_debt),
            "exposure":known(&a.gross_exposure),"permissions":a.instrument_permissions,"complete":a.coverage.account_complete}))),
        EvidenceKind::Positions=>json!(account.map(|a|json!({"positions":known(&a.positions),"complete":a.coverage.positions_complete}))),
        EvidenceKind::OpenOrders=>json!(account.map(|a|json!({"orders":known(&a.open_orders),"complete":a.coverage.open_orders_complete}))),
    }
}
fn incoming_reads(observation: &Observation) -> Result<Vec<ReadWatermark>, String> {
    let mut output = Vec::new();
    for kind in [
        EvidenceKind::Order,
        EvidenceKind::Fees,
        EvidenceKind::Account,
        EvidenceKind::Positions,
        EvidenceKind::OpenOrders,
    ] {
        let reads = observation
            .evidence
            .iter()
            .filter(|item| item.kind == kind)
            .collect::<Vec<_>>();
        if reads.is_empty() {
            continue;
        }
        // Local reference IDs are not cross-observation provider identities.
        // Canonical row identities omit those IDs and do not depend on list order.
        let mut rows = reads
            .iter()
            .map(|item| {
                hash(&(
                    item.kind,
                    &item.provider_record_id,
                    item.hash_basis,
                    &item.response_sha256,
                    item.started_at_ms,
                    item.completed_at_ms,
                ))
            })
            .collect::<Result<Vec<_>, _>>()?;
        rows.sort();
        output.push(ReadWatermark {
            kind,
            started_at_ms: reads.iter().map(|r| r.started_at_ms).min().unwrap(),
            completed_at_ms: reads.iter().map(|r| r.completed_at_ms).max().unwrap(),
            contents_sha256: hash(&(rows, field_facts(observation, kind)))?,
        });
    }
    Ok(output)
}
fn validate_previous(previous: &Projection, request: &Request, now: i64) -> Result<(), String> {
    if previous.schema_version != VERSION
        || previous.request_sha256 != request.request_sha256()?
        || previous.role != request.role
        || !digest(&previous.projection_sha256)
        || !digest(&previous.observation_sha256)
        || !digest(&previous.order_facts_sha256)
        || previous.generation == 0
        || previous.generation > MAX_EXACT_INTEGER
        || previous.order_observed_at_ms <= 0
        || previous.order_known_at_ms < previous.order_observed_at_ms
        || previous.order_known_at_ms > now
        || previous.read_watermarks.len() > 5
        || previous.status != status_for(&previous.order_status)
        || previous
            .provider_order_id
            .as_ref()
            .is_some_and(|id| id.is_empty() || id.len() > 256 || id.chars().any(char::is_control))
        || previous
            .latest_evidence_completed_at_ms
            .is_some_and(|time| time <= 0 || time > now)
        || previous
            .last_available_account_sha256
            .as_ref()
            .is_some_and(|value| !digest(value))
        || previous.order_facts_sha256
            != order_facts(
                &previous.order_status,
                &previous.provider_order_id,
                &previous.filled_quantity,
                &previous.cash_debit,
            )?
    {
        return Err("settlement_projection_previous_invalid".into());
    }
    if let Some(filled) = previous.filled_quantity {
        if filled.negative()
            || filled
                .compare(request.lookup.original_submit.order.quantity)?
                .is_gt()
        {
            return Err("settlement_projection_previous_invalid".into());
        }
    }
    if previous.cash_debit.is_some_and(Decimal::negative) {
        return Err("settlement_projection_previous_invalid".into());
    }
    if previous.order_status != OrderStatus::Unknown {
        let filled = previous
            .filled_quantity
            .ok_or("settlement_projection_previous_invalid")?;
        if (previous.order_status == OrderStatus::Filled
            && filled != request.lookup.original_submit.order.quantity)
            || (matches!(
                previous.order_status,
                OrderStatus::Open | OrderStatus::Rejected
            ) && filled != Decimal::ZERO)
            || (previous.order_status == OrderStatus::PartiallyFilled
                && (!filled.positive() || filled == request.lookup.original_submit.order.quantity))
            || (previous.order_status != OrderStatus::Rejected
                && previous.provider_order_id.is_none())
        {
            return Err("settlement_projection_previous_invalid".into());
        }
    }
    if previous.average_fill_price.is_some()
        && previous
            .filled_quantity
            .is_some_and(|value| !value.positive())
    {
        return Err("settlement_projection_previous_invalid".into());
    }
    if previous.latest_account_availability != AccountAvailability::Unavailable
        && previous.last_available_account_sha256.is_none()
    {
        return Err("settlement_projection_previous_invalid".into());
    }
    for fact in [&previous.average_fill_price, &previous.fee_cash]
        .into_iter()
        .flatten()
    {
        fact.currency.validate()?;
        if fact.amount.negative() {
            return Err("settlement_projection_previous_invalid".into());
        }
    }
    if previous.average_fill_price.as_ref().is_some_and(|v| {
        !v.amount.positive()
            || v.currency.code
                != request
                    .lookup
                    .original_submit
                    .order
                    .instrument
                    .quote_currency
    }) {
        return Err("settlement_projection_previous_invalid".into());
    }
    let mut seen = std::collections::BTreeSet::new();
    for read in &previous.read_watermarks {
        if !seen.insert(kind_index(read.kind))
            || !digest(&read.contents_sha256)
            || read.started_at_ms <= 0
            || read.completed_at_ms < read.started_at_ms
            || read.completed_at_ms > now
        {
            return Err("settlement_projection_previous_invalid".into());
        }
    }
    if previous.latest_evidence_completed_at_ms
        != previous
            .read_watermarks
            .iter()
            .map(|r| r.completed_at_ms)
            .max()
        || previous.projection_sha256 != projection_hash(previous)?
        || serde_json::to_vec(previous)
            .map_err(|_| "settlement_projection_encoding_invalid")?
            .len()
            > MAX_BYTES
    {
        return Err("settlement_projection_previous_invalid".into());
    }
    Ok(())
}
fn keep_known<T>(previous: &Option<T>, next: &Option<T>) -> Result<(), String> {
    if previous.is_some() && next.is_none() {
        return Err("settlement_projection_known_fact_unavailable".into());
    }
    Ok(())
}
fn increasing(previous: Option<Decimal>, next: Option<Decimal>) -> Result<bool, String> {
    match (previous, next) {
        (Some(old), Some(new)) => Ok(new.compare(old)?.is_gt()),
        _ => Ok(false),
    }
}
fn blockers(projection: &Projection) -> Vec<Blocker> {
    let mut output = Vec::new();
    if projection.status == ProviderObservationStatus::Pending {
        output.push(Blocker::OrderPending);
    }
    if projection.filled_quantity.is_none() {
        output.push(Blocker::FilledQuantityUnavailable);
    }
    if projection.cash_debit.is_none() {
        output.push(Blocker::CashDebitUnavailable);
    }
    if projection.filled_quantity.is_some_and(Decimal::positive)
        && projection.average_fill_price.is_none()
    {
        output.push(Blocker::AverageFillPriceUnavailable);
    }
    if projection.fee_cash.is_none() {
        output.push(Blocker::FeeCashUnavailable);
    }
    output.push(match projection.fee_finality {
        None => Blocker::FeeFinalityUnavailable,
        Some(false) => Blocker::FeesNotFinal,
        Some(true) => Blocker::ProviderFeeFinalityUnverified,
    });
    match projection.latest_account_availability {
        AccountAvailability::Unavailable => output.push(Blocker::AccountUnavailable),
        AccountAvailability::Partial => output.push(Blocker::AccountCoverageIncomplete),
        AccountAvailability::Complete => {}
    }
    output.push(Blocker::CoherentPostingUnavailable);
    output
}

/// Pure reduction. On refusal the borrowed previous projection is untouched;
/// the host must persist the refused observation/block as a separate head.
/// SDK validation is performed first even for identical-observation recovery.
pub(crate) fn reduce(
    previous: Option<&Projection>,
    request: &Request,
    observation: &Observation,
    actual_now: i64,
) -> Result<Outcome, String> {
    observation.validate(request, actual_now)?;
    let observation_sha256 = hash(observation)?;
    let request_sha256 = request.request_sha256()?;
    if let Some(old) = previous {
        validate_previous(old, request, actual_now)?;
        if old.observation_sha256 == observation_sha256 {
            return Ok(Outcome {
                disposition: Disposition::Unchanged,
                observation_sha256,
                projection: old.clone(),
                blockers: blockers(old),
            });
        }
    }
    let filled = known(&observation.order.filled_quantity).copied();
    let cash = known(&observation.order.cash_spent).copied();
    let average = known(&observation.average_fill_price).map(|v| MoneyFact {
        amount: v.price,
        currency: v.currency.clone(),
    });
    let fees = known(&observation.fee_cash).map(|v| MoneyFact {
        amount: v.amount,
        currency: v.currency.clone(),
    });
    let finality = known(&observation.fee_finality).map(|v| v.final_);
    let order_facts_sha256 = incoming_order_facts(observation)?;
    let new_reads = incoming_reads(observation)?;
    if average.is_some() && filled.is_some_and(|value| !value.positive()) {
        return Err("settlement_projection_average_without_fill".into());
    }
    if let Some(old) = previous {
        if old.provider_order_id.is_some()
            && old.provider_order_id != observation.order.provider_order_id
        {
            return Err("settlement_projection_provider_order_changed".into());
        }
        keep_known(&old.filled_quantity, &filled)?;
        keep_known(&old.cash_debit, &cash)?;
        keep_known(&old.average_fill_price, &average)?;
        keep_known(&old.fee_cash, &fees)?;
        keep_known(&old.fee_finality, &finality)?;
        if let (Some(before), Some(after)) = (old.filled_quantity, filled) {
            if after.compare(before)?.is_lt() {
                return Err("settlement_projection_fill_regressed".into());
            }
        }
        if let (Some(before), Some(after)) = (old.cash_debit, cash) {
            if after.compare(before)?.is_lt() {
                return Err("settlement_projection_cash_regressed".into());
            }
        }
        if observation.order.observed_at_ms < old.order_observed_at_ms
            || observation.order.known_at_ms < old.order_known_at_ms
        {
            return Err("settlement_projection_order_clock_regressed".into());
        }
        if old.order_status != OrderStatus::Unknown
            && observation.order.status == OrderStatus::Unknown
        {
            return Err("settlement_projection_order_status_unavailable".into());
        }
        if terminal(&old.order_status) {
            if observation.order.status != old.order_status {
                return Err("settlement_projection_terminal_status_changed".into());
            }
            if filled != old.filled_quantity {
                return Err("settlement_projection_terminal_fill_correction".into());
            }
            if old.average_fill_price.is_some() && average != old.average_fill_price {
                return Err("settlement_projection_terminal_average_correction".into());
            }
        }
        if let (Some(before), Some(after)) = (&old.average_fill_price, &average) {
            if before.currency != after.currency {
                return Err("settlement_projection_average_currency_changed".into());
            }
            if before.amount != after.amount && !increasing(old.filled_quantity, filled)? {
                return Err("settlement_projection_average_changed_without_fill_progress".into());
            }
        }
        if let (Some(before), Some(after)) = (&old.fee_cash, &fees) {
            if before.currency != after.currency {
                return Err("settlement_projection_fee_currency_changed".into());
            }
            if old.fee_finality == Some(true) && before != after {
                return Err("settlement_projection_reported_final_fee_correction".into());
            }
        }
        if old.fee_finality == Some(true) && finality != Some(true) {
            return Err("settlement_projection_fee_finality_regressed".into());
        }
        if observation.order.observed_at_ms == old.order_observed_at_ms
            && observation.order.known_at_ms == old.order_known_at_ms
            && order_facts_sha256 != old.order_facts_sha256
        {
            return Err("settlement_projection_same_time_order_conflict".into());
        }
        let newest = new_reads.iter().map(|r| r.completed_at_ms).max();
        if let (Some(before), Some(after)) = (old.latest_evidence_completed_at_ms, newest) {
            if after < before {
                return Err("settlement_projection_evidence_clock_regressed".into());
            }
        }
        for read in &new_reads {
            if let Some(before) = old.read_watermarks.iter().find(|r| r.kind == read.kind) {
                if read.started_at_ms < before.started_at_ms
                    || read.completed_at_ms < before.completed_at_ms
                {
                    return Err("settlement_projection_evidence_clock_regressed".into());
                }
                if read.completed_at_ms == before.completed_at_ms
                    && read.contents_sha256 != before.contents_sha256
                {
                    return Err("settlement_projection_same_time_evidence_conflict".into());
                }
            }
        }
    }
    let mut reads = previous.map_or_else(Vec::new, |p| p.read_watermarks.clone());
    for read in new_reads {
        if let Some(old) = reads.iter_mut().find(|r| r.kind == read.kind) {
            *old = read;
        } else {
            reads.push(read);
        }
    }
    reads.sort_by_key(|r| kind_index(r.kind));
    let account = known(&observation.account);
    let availability = match account {
        None => AccountAvailability::Unavailable,
        Some(a)
            if a.snapshot.coverage.account_complete
                && a.snapshot.coverage.positions_complete
                && a.snapshot.coverage.open_orders_complete =>
        {
            AccountAvailability::Complete
        }
        Some(_) => AccountAvailability::Partial,
    };
    let account_hash = account
        .map(hash)
        .transpose()?
        .or_else(|| previous.and_then(|p| p.last_available_account_sha256.clone()));
    let mut projection = Projection {
        schema_version: VERSION.into(),
        request_sha256,
        role: request.role,
        provider_order_id: observation.order.provider_order_id.clone(),
        filled_quantity: filled,
        cash_debit: cash,
        average_fill_price: average,
        fee_cash: fees,
        fee_finality: finality,
        order_status: observation.order.status.clone(),
        order_observed_at_ms: observation.order.observed_at_ms,
        order_known_at_ms: observation.order.known_at_ms,
        order_facts_sha256,
        latest_evidence_completed_at_ms: reads.iter().map(|r| r.completed_at_ms).max(),
        read_watermarks: reads,
        latest_account_availability: availability,
        last_available_account_sha256: account_hash,
        generation: previous.map_or(1, |p| p.generation.saturating_add(1)),
        status: observation.status,
        observation_sha256: observation_sha256.clone(),
        projection_sha256: String::new(),
    };
    if projection.generation > MAX_EXACT_INTEGER {
        return Err("settlement_projection_generation_exhausted".into());
    }
    projection.projection_sha256 = projection_hash(&projection)?;
    validate_previous(&projection, request, actual_now)?;
    let blockers = blockers(&projection);
    Ok(Outcome {
        disposition: Disposition::Advanced,
        observation_sha256,
        projection,
        blockers,
    })
}

#[cfg(test)]
#[path = "settlement_observation_reducer_tests.rs"]
mod tests;
