//! Durable V2 admission lifecycle, sharing the proposal cash ledger.
use super::*;
use tradeassembly_plugin_sdk::broker_v2::{OrderObservation, OrderStatus};
pub(crate) const STATE: &str = "execution_order_plan_submissions_v2";
#[derive(Clone)]
pub struct OrderPlanSource {
    service: TradeAssemblyService,
    id: String,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Phase {
    Admitting,
    Admitted,
    Sending,
    Unknown,
    Open,
    PartiallyFilled,
    Consumed,
    Rejected,
    Cancelled,
    Denied,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SubmissionRecord {
    pub schema_version: String,
    pub plan_id: String,
    pub owner_sha256: String,
    pub account_key: String,
    pub original_key: String,
    pub intent: Value,
    pub intent_sha256: String,
    pub phase: Phase,
    pub generation: u64,
    pub retained_cash: Decimal,
    pub filled_quantity: Decimal,
    pub provider_order_id: Option<String>,
    pub observation: Option<OrderObservation>,
    pub record_sha256: String,
}
fn record_hash(r: &SubmissionRecord) -> Result<String, String> {
    let mut r = r.clone();
    r.record_sha256.clear();
    hash(&r)
}
pub(crate) fn source(s: &TradeAssemblyService, id: &str) -> Result<OrderPlanSource, String> {
    read_owned(s, id)?;
    Ok(OrderPlanSource {
        service: s.clone(),
        id: id.into(),
    })
}
fn record(
    s: &TradeAssemblyService,
    p: &OrderPlan,
) -> Result<Option<(Value, SubmissionRecord)>, String> {
    let Some(raw) = s.runtime().storage.get_json(STATE, &p.plan_id)? else {
        return Ok(None);
    };
    let r: SubmissionRecord =
        serde_json::from_value(raw.clone()).map_err(|_| "broker_v2_state_invalid")?;
    if r.schema_version != "tradeassembly.broker_submission_state.v2"
        || r.plan_id != p.plan_id
        || r.owner_sha256 != p.owner_sha256
        || r.account_key != p.account_key
        || r.generation > 10000
        || r.record_sha256 != record_hash(&r)?
        || r.intent_sha256 != hash(&r.intent)?
        || r.intent["idempotencyKey"] != r.original_key
        || r.intent["planId"] != p.plan_id
        || !(r.retained_cash == Decimal::ZERO || r.retained_cash == p.purchase_cash)
    {
        return Err("broker_v2_state_invalid".into());
    }
    Ok(Some((raw, r)))
}
pub(crate) fn release_guard(
    s: &TradeAssemblyService,
    p: &OrderPlan,
) -> Result<Option<Value>, String> {
    match record(s, p)? {
        None => Ok(None),
        Some((raw, r)) if matches!(r.phase, Phase::Denied | Phase::Rejected | Phase::Cancelled) => {
            Ok(Some(raw))
        }
        Some(_) => Err("order_plan_submission_not_releasable".into()),
    }
}
pub(crate) fn reserve_guard(s: &TradeAssemblyService, p: &OrderPlan) -> Result<(), String> {
    if record(s, p)?.is_some() {
        Err("order_plan_already_submitted".into())
    } else {
        Ok(())
    }
}
impl OrderPlanSource {
    pub(crate) fn service(&self) -> &TradeAssemblyService {
        &self.service
    }
    pub(crate) fn historical(&self) -> Result<OrderPlan, String> {
        read_owned(&self.service, &self.id)
    }
    pub(crate) fn verified_signal(&self) -> Result<VerifiedSignal, String> {
        verify_signal(&self.service, &self.historical()?.command)
    }
    pub(crate) fn snapshot(&self) -> Result<OrderPlan, String> {
        let signal = self.verified_signal()?;
        self.snapshot_with_signal(&signal)
    }
    pub(crate) fn snapshot_with_signal(
        &self,
        signal: &VerifiedSignal,
    ) -> Result<OrderPlan, String> {
        let (p, f) = measured("snapshot.checked_current", || {
            checked_current_with_signal(&self.service, &self.id, signal)
        })?;
        if p.canonical_account.is_none() || p.account_binding_sha256.is_none() {
            return Err("broker_v2_canonical_plan_required".into());
        }
        let (_, mut ledger, remaining) = cash(&self.service, &f)?;
        ledger_policy(&mut ledger, &p.policy)?;
        if ledger.reservations.get(&p.plan_id) != Some(&p.purchase_cash) {
            return Err("broker_v2_reservation_mismatch".into());
        }
        risk(
            &p.policy,
            &f,
            &p.order,
            remaining.checked_add(p.purchase_cash)?,
        )?;
        Ok(p)
    }
    pub(crate) fn state(&self) -> Result<Option<SubmissionRecord>, String> {
        Ok(record(&self.service, &self.historical()?)?.map(|(_, r)| r))
    }
    pub(crate) fn begin(
        &self,
        p: &OrderPlan,
        intent: Value,
        ctx: &SideEffectContext,
    ) -> Result<(), String> {
        if self.historical()? != *p || p.canonical_account.is_none() {
            return Err("broker_v2_plan_changed".into());
        }
        if record(&self.service, p)?.is_some() {
            return Err("order_plan_already_submitted".into());
        }
        let storage = self.service.runtime().storage.clone();
        let old_cash = storage
            .get_json(CASH, &p.account_key)?
            .ok_or("broker_v2_reservation_required")?;
        let ledger = ledger_owned(&self.service, old_cash.clone())?;
        if ledger.reservations.get(&p.plan_id) != Some(&p.purchase_cash) {
            return Err("broker_v2_reservation_mismatch".into());
        }
        let key = ctx.idempotency_key.as_str();
        let digest = hash(&intent)?;
        let mut r = SubmissionRecord {
            schema_version: "tradeassembly.broker_submission_state.v2".into(),
            plan_id: p.plan_id.clone(),
            owner_sha256: p.owner_sha256.clone(),
            account_key: p.account_key.clone(),
            original_key: key.into(),
            intent,
            intent_sha256: digest,
            phase: Phase::Admitting,
            generation: 0,
            retained_cash: p.purchase_cash,
            filled_quantity: Decimal::ZERO,
            provider_order_id: None,
            observation: None,
            record_sha256: String::new(),
        };
        r.record_sha256 = record_hash(&r)?;
        let intent_key = format!("intent_{}", crate::broker_submission::v2::key_hash(key));
        if storage.put_json_batch(
            &[
                StorageWrite::new(STATE, &p.plan_id, value(&r)?, ctx.clone()),
                StorageWrite::new(
                    "broker_order_intents",
                    &intent_key,
                    r.intent.clone(),
                    ctx.clone(),
                ),
            ],
            &[
                StorageExpectation::new(STATE, &p.plan_id, None),
                StorageExpectation::new("broker_order_intents", &intent_key, None),
                StorageExpectation::new(PLANS, &p.plan_id, Some(value(p)?)),
                StorageExpectation::new(CASH, &p.account_key, Some(old_cash)),
                StorageExpectation::new(RELEASES, &p.plan_id, None),
            ],
        )? != ComparePutOutcome::Updated
        {
            return Err("broker_v2_admission_conflict".into());
        }
        Ok(())
    }
    fn transition(
        &self,
        allowed: &[Phase],
        phase: Phase,
        observation: Option<OrderObservation>,
        release_cash: bool,
        ctx: &SideEffectContext,
    ) -> Result<SubmissionRecord, String> {
        let p = self.historical()?;
        let storage = self.service.runtime().storage.clone();
        let (old, mut r) = record(&self.service, &p)?.ok_or("broker_v2_state_missing")?;
        if !allowed.contains(&r.phase) {
            return Err("broker_v2_transition_refused".into());
        }
        if r.original_key != ctx.idempotency_key.as_str() {
            return Err("broker_v2_original_key_mismatch".into());
        }
        let cash_old = storage
            .get_json(CASH, &p.account_key)?
            .ok_or("broker_v2_reservation_required")?;
        let mut ledger = ledger_owned(&self.service, cash_old.clone())?;
        if ledger.reservations.get(&p.plan_id) != Some(&r.retained_cash)
            || r.retained_cash != p.purchase_cash
        {
            return Err("broker_v2_reservation_mismatch".into());
        }
        if let Some(o) = observation {
            o.validate(self.service.runtime().clock.now_ms())?;
            if o.order != p.order
                || Some(o.canonical_account_sha256.as_str())
                    != p.canonical_account
                        .as_ref()
                        .map(|a| a.identity_sha256.as_str())
            {
                return Err("broker_v2_observation_binding_mismatch".into());
            }
            if r.observation.as_ref().is_some_and(|old| {
                o.observed_at_ms < old.observed_at_ms || o.known_at_ms < old.known_at_ms
            }) {
                return Err("broker_v2_observation_regressed".into());
            }
            if let Some(id) = &o.provider_order_id {
                if r.provider_order_id.as_ref().is_some_and(|old| old != id) {
                    return Err("broker_v2_provider_order_changed".into());
                }
                r.provider_order_id = Some(id.clone());
            }
            if let Observed::Available { value } = o.filled_quantity {
                if value.compare(r.filled_quantity)?.is_lt() {
                    return Err("broker_v2_fill_regressed".into());
                }
                r.filled_quantity = value;
            }
            if let Observed::Available { value } = o.cash_spent {
                if value.compare(p.purchase_cash)?.is_gt() {
                    return Err("broker_v2_debit_exceeds_reservation".into());
                }
            }
            r.observation = Some(o);
        }
        if release_cash {
            ledger.reservations.remove(&p.plan_id);
            r.retained_cash = Decimal::ZERO;
        }
        r.phase = phase;
        r.generation = r
            .generation
            .checked_add(1)
            .ok_or("broker_v2_generation_invalid")?;
        r.record_sha256 = record_hash(&r)?;
        if storage.put_json_batch(
            &[
                StorageWrite::new(STATE, &p.plan_id, value(&r)?, ctx.clone()),
                StorageWrite::new(CASH, &p.account_key, value(&ledger)?, ctx.clone()),
            ],
            &[
                StorageExpectation::new(STATE, &p.plan_id, Some(old)),
                StorageExpectation::new(CASH, &p.account_key, Some(cash_old)),
                StorageExpectation::new(RELEASES, &p.plan_id, None),
            ],
        )? != ComparePutOutcome::Updated
        {
            return Err("broker_v2_transition_conflict".into());
        }
        Ok(r)
    }
    pub(crate) fn admitted(&self, ctx: &SideEffectContext) -> Result<(), String> {
        self.transition(&[Phase::Admitting], Phase::Admitted, None, false, ctx)
            .map(|_| ())
    }
    pub(crate) fn before_send(&self, ctx: &SideEffectContext) -> Result<(), String> {
        self.transition(&[Phase::Admitted], Phase::Sending, None, false, ctx)
            .map(|_| ())
    }
    pub(crate) fn denied_before_send(&self, ctx: &SideEffectContext) -> Result<(), String> {
        let Some(state) = self.state()? else {
            return Ok(());
        };
        if state.original_key != ctx.idempotency_key.as_str()
            || !matches!(
                state.phase,
                Phase::Admitting | Phase::Admitted | Phase::Sending
            )
        {
            return Ok(());
        }
        self.transition(
            &[Phase::Admitting, Phase::Admitted, Phase::Sending],
            Phase::Denied,
            None,
            false,
            ctx,
        )
        .map(|_| ())
    }
    pub(crate) fn abort_unsent(&self, ctx: &SideEffectContext) -> Result<(), String> {
        self.transition(
            &[Phase::Admitting, Phase::Admitted],
            Phase::Denied,
            None,
            false,
            ctx,
        )
        .map(|_| ())
    }
    pub(crate) fn observe(
        &self,
        o: OrderObservation,
        ctx: &SideEffectContext,
    ) -> Result<SubmissionRecord, String> {
        let zero_filled = o.filled_quantity
            == Observed::Available {
                value: Decimal::ZERO,
            };
        let zero_cash = o.cash_spent
            == Observed::Available {
                value: Decimal::ZERO,
            };
        let release = zero_filled
            && zero_cash
            && matches!(
                o.status,
                OrderStatus::Rejected | OrderStatus::Cancelled | OrderStatus::Expired
            );
        let phase = match o.status {
            OrderStatus::Unknown => Phase::Unknown,
            OrderStatus::Open => Phase::Open,
            OrderStatus::PartiallyFilled => Phase::PartiallyFilled,
            OrderStatus::Filled if matches!(o.cash_spent, Observed::Available { .. }) => {
                Phase::Consumed
            }
            OrderStatus::Rejected if release => Phase::Rejected,
            OrderStatus::Cancelled | OrderStatus::Expired if release => Phase::Cancelled,
            OrderStatus::Cancelled | OrderStatus::Expired
                if matches!(o.cash_spent, Observed::Available { .. }) =>
            {
                Phase::Consumed
            }
            _ => Phase::Unknown,
        };
        self.transition(
            &[
                Phase::Sending,
                Phase::Unknown,
                Phase::Open,
                Phase::PartiallyFilled,
            ],
            phase,
            Some(o),
            release,
            ctx,
        )
    }
}

pub(crate) fn summary(r: SubmissionRecord) -> Value {
    json!({"schemaVersion":r.schema_version,"planId":r.plan_id,"originalKeySha256":crate::broker_submission::v2::key_hash(&r.original_key),"phase":r.phase,"generation":r.generation,"retainedCash":r.retained_cash,"filledQuantity":r.filled_quantity,"intentSha256":r.intent_sha256,"recordSha256":r.record_sha256,"providerOrderId":r.provider_order_id,"observation":r.observation,"executionAuthorityGranted":false})
}
