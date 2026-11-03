//! Whitelisted non-atomic evidence projection; raw account identity stays local.
use super::*;
use tradeassembly_plugin_sdk::{
    broker_v2::OrderObservation,
    execution_snapshot::{Coverage, Currency},
};
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AccountCoverageProjection {
    pub coherence: wire::NonAtomic,
    pub snapshot_sha256: String,
    pub time: ObservationTime,
    pub coverage: Coverage,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ReadEvidenceProjection {
    pub kind: wire::EvidenceKind,
    pub hash_basis: wire::RawResponseBytes,
    pub response_sha256: String,
    pub started_at_ms: i64,
    pub completed_at_ms: i64,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ReportedObservation {
    pub schema_version: String,
    pub request_sha256: String,
    pub status: wire::ProviderObservationStatus,
    pub coherence: wire::NonAtomic,
    pub order: OrderObservation,
    pub average_fill_price: Observed<wire::ProviderReportedAveragePrice>,
    pub fee_cash: Observed<wire::ProviderReportedFeeCash>,
    pub fee_finality: Observed<wire::ProviderFeeFinality>,
    pub account_coverage: Observed<AccountCoverageProjection>,
    pub evidence: Vec<ReadEvidenceProjection>,
}
impl ReportedObservation {
    pub(in crate::service::order_plans) fn from_observation(
        o: &wire::Observation,
    ) -> Result<Self, String> {
        let account_coverage = match &o.account {
            Observed::Unavailable { reason } => Observed::Unavailable {
                reason: reason.clone(),
            },
            Observed::Available { value } => Observed::Available {
                value: AccountCoverageProjection {
                    coherence: wire::NonAtomic::NonAtomic,
                    snapshot_sha256: hash(&value.snapshot)?,
                    time: value.snapshot.time.clone(),
                    coverage: value.snapshot.coverage.clone(),
                },
            },
        };
        Ok(Self {
            schema_version: "tradeassembly.provider-reported-observation.v7".into(),
            request_sha256: o.request_sha256.clone(),
            status: o.status,
            coherence: o.coherence,
            order: o.order.clone(),
            average_fill_price: o.average_fill_price.clone(),
            fee_cash: o.fee_cash.clone(),
            fee_finality: o.fee_finality.clone(),
            account_coverage,
            evidence: o
                .evidence
                .iter()
                .map(|e| ReadEvidenceProjection {
                    kind: e.kind,
                    hash_basis: e.hash_basis,
                    response_sha256: e.response_sha256.clone(),
                    started_at_ms: e.started_at_ms,
                    completed_at_ms: e.completed_at_ms,
                })
                .collect(),
        })
    }
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ReportedMoney {
    pub amount: Decimal,
    pub currency: Currency,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AcceptedObservationProjection {
    pub generation: u64,
    pub projection_sha256: String,
    pub observation_sha256: String,
    pub filled_quantity: Option<Decimal>,
    pub cash_debit: Option<Decimal>,
    pub average_fill_price: Option<ReportedMoney>,
    pub fee_cash: Option<ReportedMoney>,
    pub fee_finality: Option<bool>,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ObservationProjection {
    pub receipt_sha256: String,
    pub plan_id: String,
    pub role: wire::Role,
    pub original_intent_sha256: String,
    pub source_binding_sha256: String,
    pub provider_response_sha256: String,
    pub observation: ReportedObservation,
    pub accepted_projection: Option<AcceptedObservationProjection>,
    pub blockers: Vec<String>,
    pub blocked_reason: Option<String>,
    /// The immutable head captured by this read, not a currency guarantee.
    pub head_receipt_sha256: String,
    pub head_blocked_reason: Option<String>,
    pub observed_at_ms: i64,
    pub cash_reconciled: bool,
    pub released_hold: Decimal,
}
