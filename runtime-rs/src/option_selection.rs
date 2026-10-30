//! Deterministic eligibility and ranking over host-acquired option snapshots.
//! This module cannot acquire data, estimate analytics or submit an order.
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use tradeassembly_strategy::spec::*;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum AnalyticsOrigin {
    ProviderSupplied,
    ModelEstimated {
        semantics_version: String,
        model_package_sha256: String,
        model_operation_id: String,
    },
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DeltaValue {
    pub value: f64,
    pub origin: AnalyticsOrigin,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OptionCandidate {
    pub contract_id: String,
    pub underlying: String,
    pub expiry: String,
    pub strike: f64,
    pub right: OptionRight,
    pub observed_at_ms: i64,
    pub bid: Option<f64>,
    pub ask: Option<f64>,
    pub delta: Option<DeltaValue>,
    pub volume: Option<f64>,
    pub open_interest: Option<f64>,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RejectedOption {
    pub contract_id: String,
    pub reasons: Vec<String>,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RankedOption {
    pub contract_id: String,
    pub ranking_values: Vec<f64>,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case", deny_unknown_fields)]
pub enum OptionOutcome {
    Selected { contract: Box<OptionCandidate> },
    NoEligibleContract,
    RequiredDataUnavailable,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OptionSelectionResult {
    pub outcome: OptionOutcome,
    pub stage_failed: bool,
    pub ranked: Vec<RankedOption>,
    pub rejected: Vec<RejectedOption>,
}
fn expiry_date(s: &str) -> Option<chrono::NaiveDate> {
    if s.len() != 10 {
        return None;
    }
    let date = chrono::NaiveDate::parse_from_str(s, "%Y-%m-%d").ok()?;
    (date.format("%Y-%m-%d").to_string() == s).then_some(date)
}
fn available(value: Option<f64>) -> Option<f64> {
    value.filter(|v| v.is_finite() && *v >= 0.0)
}
fn delta(candidate: &OptionCandidate, policy: &OptionAnalytics) -> Option<f64> {
    let d = candidate.delta.as_ref()?;
    if !d.value.is_finite() || !(-1.0..=1.0).contains(&d.value) {
        return None;
    }
    let matches = match (&d.origin, policy) {
        (AnalyticsOrigin::ProviderSupplied, OptionAnalytics::ProviderSupplied) => true,
        (
            AnalyticsOrigin::ModelEstimated {
                semantics_version,
                model_package_sha256,
                model_operation_id,
            },
            OptionAnalytics::ExplicitModel {
                semantics_version: expected,
                ..
            },
        ) => {
            semantics_version == expected
                && model_package_sha256.len() == 64
                && !model_operation_id.is_empty()
        }
        _ => false,
    };
    matches.then_some(d.value)
}
fn metric(
    candidate: &OptionCandidate,
    field: &OptionRankField,
    dte: f64,
    policy: &OptionAnalytics,
) -> Option<f64> {
    match field {
        OptionRankField::Dte => Some(dte),
        OptionRankField::Strike => Some(candidate.strike),
        OptionRankField::Delta => delta(candidate, policy),
        OptionRankField::Volume => available(candidate.volume),
        OptionRankField::OpenInterest => available(candidate.open_interest),
        OptionRankField::RelativeSpread => {
            let bid = available(candidate.bid)?;
            let ask = available(candidate.ask)?;
            let mid = bid / 2.0 + ask / 2.0;
            (ask >= bid && mid > 0.0).then_some((ask - bid) / mid)
        }
    }
}
pub fn select(
    policy: &OptionSelection,
    underlying: &str,
    as_of_ms: i64,
    candidates: &[OptionCandidate],
) -> Result<OptionSelectionResult, String> {
    policy.validate()?;
    if underlying.is_empty() || candidates.len() > policy.max_contracts as usize {
        return Err("option_chain_bounds_invalid".into());
    }
    let date = chrono::DateTime::from_timestamp_millis(as_of_ms)
        .ok_or("option_selection_time_invalid")?
        .date_naive();
    let mut ids = BTreeSet::new();
    let mut eligible = vec![];
    let mut rejected = vec![];
    let mut missing_required = false;
    for c in candidates {
        if c.contract_id.is_empty()
            || c.contract_id.len() > 256
            || !ids.insert(&c.contract_id)
            || c.underlying != underlying
            || !c.strike.is_finite()
            || c.strike <= 0.0
        {
            return Err("option_chain_identity_invalid".into());
        }
        let expiry = expiry_date(&c.expiry).ok_or("option_chain_expiry_invalid")?;
        let dte = (expiry - date).num_days() as f64;
        let mut reasons = vec![];
        if [c.bid, c.ask, c.volume, c.open_interest]
            .iter()
            .flatten()
            .any(|v| !v.is_finite() || *v < 0.0)
        {
            reasons.push("invalid_market_value".to_string());
        }
        if c.bid.zip(c.ask).is_some_and(|(bid, ask)| ask < bid) {
            reasons.push("crossed_quote".to_string());
        }

        if c.right != policy.filters.right {
            reasons.push("right".to_string());
        }
        if dte < 0.0 {
            reasons.push("expired".to_string());
        }
        if c.observed_at_ms <= 0
            || c.observed_at_ms > as_of_ms
            || as_of_ms.saturating_sub(c.observed_at_ms) > policy.max_age_ms as i64
        {
            reasons.push("snapshot_stale_or_future".to_string());
            missing_required = true;
        }
        if policy.filters.expiry.as_ref().is_some_and(|r| {
            r.earliest.as_ref().is_some_and(|v| &c.expiry < v)
                || r.latest.as_ref().is_some_and(|v| &c.expiry > v)
        }) {
            reasons.push("expiry".to_string());
        }
        for (name, field, range) in [
            ("dte", OptionRankField::Dte, &policy.filters.dte),
            ("strike", OptionRankField::Strike, &policy.filters.strike),
            ("delta", OptionRankField::Delta, &policy.filters.delta),
        ] {
            if let Some(r) = range {
                match metric(c, &field, dte, &policy.analytics) {
                    Some(v) if r.contains(v) => {}
                    Some(_) => reasons.push(name.into()),
                    None => {
                        reasons.push(format!("{name}_unavailable"));
                        missing_required = true;
                    }
                }
            }
        }
        for (name, field, bound, is_max) in [
            (
                "volume",
                OptionRankField::Volume,
                policy.filters.min_volume,
                false,
            ),
            (
                "open_interest",
                OptionRankField::OpenInterest,
                policy.filters.min_open_interest,
                false,
            ),
            (
                "relative_spread",
                OptionRankField::RelativeSpread,
                policy.filters.max_relative_spread,
                true,
            ),
        ] {
            if let Some(bound) = bound {
                match metric(c, &field, dte, &policy.analytics) {
                    Some(v) if (is_max && v <= bound) || (!is_max && v >= bound) => {}
                    Some(_) => reasons.push(name.into()),
                    None => {
                        reasons.push(format!("{name}_unavailable"));
                        missing_required = true;
                    }
                }
            }
        }
        let mut values = vec![];
        for rank in &policy.ranking {
            match metric(c, &rank.field, dte, &policy.analytics) {
                Some(value) => values.push(rank.target.map(|t| (value - t).abs()).unwrap_or(value)),
                None => {
                    reasons.push(format!("ranking_{:?}_unavailable", rank.field));
                    missing_required = true;
                }
            }
        }
        reasons.sort();
        reasons.dedup();
        if reasons.is_empty() {
            eligible.push((c.clone(), values));
        } else {
            rejected.push(RejectedOption {
                contract_id: c.contract_id.clone(),
                reasons,
            });
        }
    }
    eligible.sort_by(|(a, av), (b, bv)| {
        for (i, r) in policy.ranking.iter().enumerate() {
            let order = av[i].partial_cmp(&bv[i]).expect("validated finite metric");
            let order = if r.direction == RankDirection::Descending {
                order.reverse()
            } else {
                order
            };
            if !order.is_eq() {
                return order;
            }
        }
        a.contract_id.cmp(&b.contract_id)
    });
    rejected.sort_by(|a, b| a.contract_id.cmp(&b.contract_id));
    let fail_missing = missing_required && policy.missing_data == MissingOptionData::FailStage;
    let outcome = if fail_missing {
        OptionOutcome::RequiredDataUnavailable
    } else if let Some((c, _)) = eligible.first() {
        OptionOutcome::Selected {
            contract: Box::new(c.clone()),
        }
    } else {
        OptionOutcome::NoEligibleContract
    };
    let stage_failed =
        fail_missing || (eligible.is_empty() && policy.no_match == NoEligibleOption::FailStage);
    let ranked = eligible
        .into_iter()
        .map(|(c, ranking_values)| RankedOption {
            contract_id: c.contract_id,
            ranking_values,
        })
        .collect();
    Ok(OptionSelectionResult {
        outcome,
        stage_failed,
        ranked,
        rejected,
    })
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    pub fn policy() -> OptionSelection {
        serde_json::from_value(serde_json::json!({"version":"tradeassembly.option_selection.v1","signal_requirement_ref":"signal","trade_requirement_ref":"trade","underlying_selector_ref":"underlying","chain_capability_requirement_ref":"chain","filters":{"right":"call","dte":{"min":0,"max":60},"delta":{"min":0.3,"max":0.7},"min_open_interest":10,"max_relative_spread":0.2},"analytics":{"kind":"provider_supplied"},"ranking":[{"field":"delta","direction":"ascending","target":0.5}],"tiebreak":"contract_id_ascending","missing_data":"reject_candidate","no_match":"no_eligible_contract","max_age_ms":60000,"max_contracts":100,"max_pages":4})).unwrap()
    }
    pub fn candidate(id: &str) -> OptionCandidate {
        OptionCandidate {
            contract_id: id.into(),
            underlying: "SPY".into(),
            expiry: "2026-11-20".into(),
            strike: 500.0,
            right: OptionRight::Call,
            observed_at_ms: 1793318400000,
            bid: Some(4.0),
            ask: Some(4.2),
            delta: Some(DeltaValue {
                value: 0.5,
                origin: AnalyticsOrigin::ProviderSupplied,
            }),
            volume: Some(100.0),
            open_interest: Some(50.0),
        }
    }
    const NOW: i64 = 1793318400000;
    #[test]
    fn ranking_is_stable_across_provider_order_and_ties() {
        let a = candidate("A");
        let b = candidate("B");
        let p = policy();
        let forward = select(&p, "SPY", NOW, &[a.clone(), b.clone()]).unwrap();
        assert_eq!(forward, select(&p, "SPY", NOW, &[b, a.clone()]).unwrap());
        assert!(matches!(forward.outcome,OptionOutcome::Selected{contract} if *contract==a));
        assert!(!forward.stage_failed);
    }
    #[test]
    fn empty_and_ineligible_never_fall_back_to_first_contract() {
        let mut c = candidate("A");
        c.delta.as_mut().unwrap().value = 0.1;
        let mut p = policy();
        let result = select(&p, "SPY", NOW, &[c]).unwrap();
        assert_eq!(result.outcome, OptionOutcome::NoEligibleContract);
        assert_eq!(result.rejected[0].reasons, vec!["delta"]);
        p.no_match = NoEligibleOption::FailStage;
        assert!(select(&p, "SPY", NOW, &[]).unwrap().stage_failed);
    }
    #[test]
    fn missing_values_zero_values_and_estimates_are_distinct() {
        let mut p = policy();
        let mut c = candidate("A");
        c.delta = None;
        assert_eq!(
            select(&p, "SPY", NOW, &[c.clone()]).unwrap().outcome,
            OptionOutcome::NoEligibleContract
        );
        p.missing_data = MissingOptionData::FailStage;
        assert_eq!(
            select(&p, "SPY", NOW, &[c.clone()]).unwrap().outcome,
            OptionOutcome::RequiredDataUnavailable
        );
        p.filters.delta = Some(NumericRange {
            min: Some(0.0),
            max: Some(0.0),
        });
        c.delta = Some(DeltaValue {
            value: 0.0,
            origin: AnalyticsOrigin::ProviderSupplied,
        });
        assert!(matches!(
            select(&p, "SPY", NOW, &[c.clone()]).unwrap().outcome,
            OptionOutcome::Selected { .. }
        ));
        c.delta.as_mut().unwrap().origin = AnalyticsOrigin::ModelEstimated {
            semantics_version: "v1".into(),
            model_package_sha256: "a".repeat(64),
            model_operation_id: "estimate".into(),
        };
        assert_eq!(
            select(&p, "SPY", NOW, &[c]).unwrap().outcome,
            OptionOutcome::RequiredDataUnavailable
        );
    }
    #[test]
    fn stale_future_duplicate_and_wrong_underlying_fail_closed() {
        let p = policy();
        let mut c = candidate("A");
        c.observed_at_ms = NOW + 1;
        assert_eq!(
            select(&p, "SPY", NOW, &[c]).unwrap().outcome,
            OptionOutcome::NoEligibleContract
        );
        assert!(select(&p, "SPY", NOW, &[candidate("A"), candidate("A")]).is_err());
        assert!(select(&p, "QQQ", NOW, &[candidate("A")]).is_err());
    }
    #[test]
    fn expiry_calendar_and_spread_are_explicit() {
        let mut p = policy();
        let mut c = candidate("A");
        c.expiry = "2026-02-29".into();
        assert!(select(&p, "SPY", NOW, &[c.clone()]).is_err());
        c.expiry = "2026-10-30".into();
        p.filters.dte = Some(NumericRange {
            min: Some(0.0),
            max: Some(0.0),
        });
        assert!(matches!(
            select(&p, "SPY", NOW, &[c.clone()]).unwrap().outcome,
            OptionOutcome::Selected { .. }
        ));
        c.bid = Some(0.0);
        c.ask = Some(0.0);
        assert!(select(&p, "SPY", NOW, &[c]).unwrap().rejected[0]
            .reasons
            .contains(&"relative_spread_unavailable".into()));
    }
}
