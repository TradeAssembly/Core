//! Candidate membership shared by static selectors and read-only screeners.
//! This value neither owns managed positions nor grants execution authority.
use crate::canonical::{canonical_sha256, MAX_EXACT_INTEGER};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

pub const MAX_SELECTED_INSTRUMENTS: usize = 256;
pub const MAX_INSTRUMENT_SET_BYTES: usize = 262_144;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum InstrumentSetVersion {
    #[serde(rename = "tradeassembly.instrument_set.v1")]
    V1,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum SelectionSource {
    Static {
        definition_sha256: String,
    },
    Expression {
        receipt_sha256: String,
        semantics_version: String,
    },
    Screener {
        receipt_sha256: String,
        semantics_version: String,
    },
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SelectionStatus {
    CompleteNonempty,
    CompleteEmpty,
    Partial,
    Failed,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SelectedInstrument {
    pub instrument_id: String,
    pub asset_class: String,
    pub instrument_family: String,
    /// One-based, contiguous order. Its semantics and tie rules are pinned below.
    pub rank: u32,
    /// Context instruments need not be the traded member. No subscriptions or
    /// positions are implicitly removed when candidate membership changes.
    pub context_instruments: Vec<String>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RejectionSummary {
    pub reason_code: String,
    pub count: u64,
    /// Hash of bounded, independently retrievable rejection evidence. The set
    /// does not embed a potentially unbounded provider universe or raw pages.
    pub evidence_sha256: String,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct InstrumentSet {
    pub schema_version: InstrumentSetVersion,
    pub source: SelectionSource,
    pub status: SelectionStatus,
    /// Identifies the exact investable scope/query, never an implied global set.
    pub universe_definition_sha256: String,
    /// Actual candidate membership inspected, including rejected candidates.
    pub candidate_membership_sha256: String,
    pub ranking_policy_sha256: String,
    pub candidate_count: u64,
    pub evaluated_at_ms: i64,
    /// Only information known by this cutoff may inform selection. Acquisition
    /// witnessed now does not establish a historical publication timestamp.
    pub knowledge_cutoff_ms: i64,
    pub known_at_ms: i64,
    pub expires_at_ms: i64,
    /// Original data/metadata receipts retain field-effective dates (e.g. OI),
    /// source timestamps, pagination and coverage. No date is inferred here.
    pub data_receipt_sha256: Vec<String>,
    pub members: Vec<SelectedInstrument>,
    pub rejection_summary: Vec<RejectionSummary>,
    pub issues: Vec<String>,
}
fn digest(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
}
fn label(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 256
        && value.trim() == value
        && !value.chars().any(char::is_control)
}
impl InstrumentSet {
    pub fn validate(&self) -> Result<(), &'static str> {
        let bytes = serde_json::to_vec(self).map_err(|_| "instrument_set_encoding_invalid")?;
        if bytes.len() > MAX_INSTRUMENT_SET_BYTES
            || self.members.len() > MAX_SELECTED_INSTRUMENTS
            || self.data_receipt_sha256.len() > 64
            || self.rejection_summary.len() > 128
            || self.issues.len() > 64
            || self.candidate_count > MAX_EXACT_INTEGER
        {
            return Err("instrument_set_bounds_invalid");
        }
        if [
            &self.universe_definition_sha256,
            &self.candidate_membership_sha256,
            &self.ranking_policy_sha256,
        ]
        .into_iter()
        .any(|v| !digest(v))
            || self.data_receipt_sha256.iter().any(|v| !digest(v))
            || self
                .data_receipt_sha256
                .iter()
                .collect::<BTreeSet<_>>()
                .len()
                != self.data_receipt_sha256.len()
        {
            return Err("instrument_set_evidence_invalid");
        }
        match &self.source {
            SelectionSource::Static { definition_sha256 } if digest(definition_sha256) => {}
            SelectionSource::Expression {
                receipt_sha256,
                semantics_version,
            }
            | SelectionSource::Screener {
                receipt_sha256,
                semantics_version,
            } if digest(receipt_sha256)
                && label(semantics_version)
                && !self.data_receipt_sha256.is_empty() => {}
            _ => return Err("instrument_set_source_invalid"),
        }
        if self.knowledge_cutoff_ms <= 0
            || self.evaluated_at_ms < self.knowledge_cutoff_ms
            || self.known_at_ms < self.evaluated_at_ms
            || self.expires_at_ms <= self.known_at_ms
            || [
                self.knowledge_cutoff_ms,
                self.evaluated_at_ms,
                self.known_at_ms,
                self.expires_at_ms,
            ]
            .into_iter()
            .any(|v| v.unsigned_abs() > MAX_EXACT_INTEGER)
        {
            return Err("instrument_set_time_invalid");
        }
        let mut ids = BTreeSet::new();
        for (i, member) in self.members.iter().enumerate() {
            if member.rank as usize != i + 1
                || !label(&member.instrument_id)
                || !label(&member.asset_class)
                || !label(&member.instrument_family)
                || !ids.insert(&member.instrument_id)
                || member.context_instruments.len() > 32
                || member.context_instruments.iter().any(|id| !label(id))
                || member
                    .context_instruments
                    .iter()
                    .collect::<BTreeSet<_>>()
                    .len()
                    != member.context_instruments.len()
            {
                return Err("instrument_set_members_invalid");
            }
        }
        if self.members.len() as u64 > self.candidate_count
            || self.issues.iter().any(|issue| !label(issue))
            || self.issues.iter().collect::<BTreeSet<_>>().len() != self.issues.len()
        {
            return Err("instrument_set_members_invalid");
        }
        let mut reasons = BTreeSet::new();
        for reason in &self.rejection_summary {
            if !label(&reason.reason_code)
                || !reasons.insert(&reason.reason_code)
                || reason.count == 0
                || reason.count > self.candidate_count
                || !digest(&reason.evidence_sha256)
            {
                return Err("instrument_set_rejections_invalid");
            }
        }
        match self.status {
            SelectionStatus::CompleteNonempty
                if !self.members.is_empty() && self.issues.is_empty() => {}
            SelectionStatus::CompleteEmpty if self.members.is_empty() && self.issues.is_empty() => {
            }
            SelectionStatus::Partial if !self.issues.is_empty() => {}
            SelectionStatus::Failed if self.members.is_empty() && !self.issues.is_empty() => {}
            _ => return Err("instrument_set_status_invalid"),
        }
        Ok(())
    }
    pub fn identity_sha256(&self) -> Result<String, &'static str> {
        self.validate()?;
        canonical_sha256(self).map_err(|_| "instrument_set_encoding_invalid")
    }
    /// Conservative admission for complete-scope consumers. A partial set is
    /// never equivalent to an empty result or a complete global top-N result.
    /// The caller still verifies receipt ownership, bindings and data evidence.
    pub fn require_complete_scope(
        &self,
        expected_universe: &str,
        now_ms: i64,
    ) -> Result<(), &'static str> {
        self.validate()?;
        if self.universe_definition_sha256 != expected_universe {
            return Err("instrument_set_universe_mismatch");
        }
        if now_ms < self.known_at_ms || now_ms >= self.expires_at_ms {
            return Err("instrument_set_stale");
        }
        if !matches!(
            self.status,
            SelectionStatus::CompleteNonempty | SelectionStatus::CompleteEmpty
        ) {
            return Err("instrument_set_incomplete");
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn set() -> InstrumentSet {
        InstrumentSet {
            schema_version: InstrumentSetVersion::V1,
            source: SelectionSource::Static {
                definition_sha256: "a".repeat(64),
            },
            status: SelectionStatus::CompleteNonempty,
            universe_definition_sha256: "b".repeat(64),
            candidate_membership_sha256: "c".repeat(64),
            ranking_policy_sha256: "d".repeat(64),
            candidate_count: 2,
            evaluated_at_ms: 100,
            knowledge_cutoff_ms: 99,
            known_at_ms: 101,
            expires_at_ms: 200,
            data_receipt_sha256: vec![],
            members: vec![SelectedInstrument {
                instrument_id: "contract-A".into(),
                asset_class: "options".into(),
                instrument_family: "equity_option".into(),
                rank: 1,
                context_instruments: vec!["SPY".into()],
            }],
            rejection_summary: vec![],
            issues: vec![],
        }
    }
    #[test]
    fn complete_empty_is_distinct_from_partial_and_failed() {
        let mut s = set();
        s.require_complete_scope(&"b".repeat(64), 101).unwrap();
        s.members.clear();
        s.status = SelectionStatus::CompleteEmpty;
        s.require_complete_scope(&"b".repeat(64), 199).unwrap();
        for status in [SelectionStatus::Partial, SelectionStatus::Failed] {
            s.status = status;
            s.issues = vec!["provider_coverage_incomplete".into()];
            s.validate().unwrap();
            assert_eq!(
                s.require_complete_scope(&"b".repeat(64), 150),
                Err("instrument_set_incomplete")
            );
        }
        s.members = set().members;
        assert_eq!(s.validate(), Err("instrument_set_status_invalid"));
    }
    #[test]
    fn freshness_scope_and_causal_times_are_not_interchangeable() {
        let s = set();
        assert_eq!(
            s.require_complete_scope(&"a".repeat(64), 150),
            Err("instrument_set_universe_mismatch")
        );
        for now in [100, 200, 201] {
            assert_eq!(
                s.require_complete_scope(&"b".repeat(64), now),
                Err("instrument_set_stale")
            );
        }
        let mut invalid = s.clone();
        invalid.knowledge_cutoff_ms = 102;
        assert_eq!(invalid.validate(), Err("instrument_set_time_invalid"));
        invalid = s.clone();
        invalid.known_at_ms = 98;
        assert_eq!(invalid.validate(), Err("instrument_set_time_invalid"));
        invalid = s;
        invalid.expires_at_ms = MAX_EXACT_INTEGER as i64 + 1;
        assert_eq!(invalid.validate(), Err("instrument_set_time_invalid"));
    }
    #[test]
    fn rank_duplicates_context_and_counts_reject() {
        let mut s = set();
        s.members.push(s.members[0].clone());
        s.members[1].rank = 2;
        assert_eq!(s.validate(), Err("instrument_set_members_invalid"));
        s = set();
        s.members[0].rank = 2;
        assert_eq!(s.validate(), Err("instrument_set_members_invalid"));
        s = set();
        s.members[0].context_instruments.push("SPY".into());
        assert_eq!(s.validate(), Err("instrument_set_members_invalid"));
        s = set();
        s.candidate_count = 0;
        assert_eq!(s.validate(), Err("instrument_set_members_invalid"));
    }
    #[test]
    fn plugin_sources_require_semantics_and_receipt_evidence() {
        let mut s = set();
        s.source = SelectionSource::Screener {
            receipt_sha256: "e".repeat(64),
            semantics_version: "rank.oi.v1".into(),
        };
        assert_eq!(s.validate(), Err("instrument_set_source_invalid"));
        s.data_receipt_sha256.push("f".repeat(64));
        s.validate().unwrap();
        let original = s.identity_sha256().unwrap();
        s.knowledge_cutoff_ms -= 1;
        assert_ne!(original, s.identity_sha256().unwrap());
        s.data_receipt_sha256.push("f".repeat(64));
        assert_eq!(s.validate(), Err("instrument_set_evidence_invalid"));
    }
    #[test]
    fn closed_wire_and_bounded_rejections_preserve_missing_data() {
        let s = set();
        let mut value = serde_json::to_value(&s).unwrap();
        value["executionAuthorityGranted"] = serde_json::json!(true);
        assert!(serde_json::from_value::<InstrumentSet>(value).is_err());
        let mut s = s;
        s.rejection_summary.push(RejectionSummary {
            reason_code: "missing_open_interest_date".into(),
            count: 1,
            evidence_sha256: "e".repeat(64),
        });
        s.validate().unwrap();
        s.rejection_summary[0].count = 3;
        assert_eq!(s.validate(), Err("instrument_set_rejections_invalid"));
        s = set();
        s.issues = vec!["missing_open_interest_date".into()];
        assert_eq!(s.validate(), Err("instrument_set_status_invalid"));
    }
}
