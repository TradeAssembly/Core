//! Portable option eligibility/ordering contract. No provider credentials or
//! brokerage authority; analytics origins never imply permission to estimate.
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub enum OptionSelectionVersion {
    #[serde(rename = "tradeassembly.option_selection.v1")]
    V1,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct NumericRange {
    pub min: Option<f64>,
    pub max: Option<f64>,
}
impl NumericRange {
    fn valid(&self, minimum: f64, maximum: f64) -> bool {
        (self.min.is_some() || self.max.is_some())
            && [self.min, self.max]
                .iter()
                .flatten()
                .all(|v| v.is_finite() && *v >= minimum && *v <= maximum)
            && self.min.zip(self.max).is_none_or(|(a, b)| a <= b)
    }
    pub fn contains(&self, value: f64) -> bool {
        value.is_finite()
            && self.min.is_none_or(|v| value >= v)
            && self.max.is_none_or(|v| value <= v)
    }
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ExpiryRange {
    /// Inclusive ISO-8601 UTC calendar dates. DTE is UTC date difference.
    pub earliest: Option<String>,
    pub latest: Option<String>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum OptionRight {
    Call,
    Put,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct OptionFilters {
    pub right: OptionRight,
    pub expiry: Option<ExpiryRange>,
    pub dte: Option<NumericRange>,
    pub strike: Option<NumericRange>,
    /// Signed delta, not absolute delta; puts normally have negative delta.
    pub delta: Option<NumericRange>,
    pub min_volume: Option<f64>,
    pub min_open_interest: Option<f64>,
    /// (ask-bid)/((ask+bid)/2); zero mid is unavailable, never zero spread.
    pub max_relative_spread: Option<f64>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum MissingOptionData {
    RejectCandidate,
    FailStage,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum NoEligibleOption {
    NoEligibleContract,
    FailStage,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum OptionTiebreak {
    ContractIdAscending,
}
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum OptionRankField {
    Dte,
    Strike,
    Delta,
    Volume,
    OpenInterest,
    RelativeSpread,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum RankDirection {
    Ascending,
    Descending,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct OptionRanking {
    pub field: OptionRankField,
    pub direction: RankDirection,
    /// When set, compare absolute distance from this target.
    pub target: Option<f64>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum OptionAnalytics {
    ProviderSupplied,
    ExplicitModel {
        capability_requirement_ref: String,
        semantics_version: String,
    },
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct OptionSelection {
    pub version: OptionSelectionVersion,
    pub signal_requirement_ref: String,
    pub trade_requirement_ref: String,
    pub underlying_selector_ref: String,
    pub chain_capability_requirement_ref: String,
    pub filters: OptionFilters,
    pub analytics: OptionAnalytics,
    pub ranking: Vec<OptionRanking>,
    pub tiebreak: OptionTiebreak,
    pub missing_data: MissingOptionData,
    pub no_match: NoEligibleOption,
    pub max_age_ms: u64,
    pub max_contracts: u32,
    pub max_pages: u32,
}
fn date_valid(value: &str) -> bool {
    let b = value.as_bytes();
    if b.len() != 10
        || b[4] != b'-'
        || b[7] != b'-'
        || b.iter()
            .enumerate()
            .any(|(i, v)| i != 4 && i != 7 && !v.is_ascii_digit())
    {
        return false;
    }
    let y = value[..4].parse::<u32>().unwrap_or(0);
    let m = value[5..7].parse::<u32>().unwrap_or(0);
    let d = value[8..].parse::<u32>().unwrap_or(0);
    let days = match m {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 => {
            if y.is_multiple_of(400) || (y.is_multiple_of(4) && !y.is_multiple_of(100)) {
                29
            } else {
                28
            }
        }
        _ => 0,
    };
    y > 0 && d > 0 && d <= days
}
impl OptionSelection {
    pub fn validate(&self) -> Result<(), &'static str> {
        let refs = [
            &self.signal_requirement_ref,
            &self.trade_requirement_ref,
            &self.underlying_selector_ref,
            &self.chain_capability_requirement_ref,
        ];
        if refs.iter().any(|s| s.is_empty() || s.len() > 128)
            || self.max_age_ms == 0
            || self.max_age_ms > 86_400_000
            || self.max_contracts == 0
            || self.max_contracts > 4096
            || self.max_pages == 0
            || self.max_pages > 16
            || self.ranking.is_empty()
            || self.ranking.len() > 6
        {
            return Err("option_selection_bounds_invalid");
        }
        let f = &self.filters;
        for (r, min, max) in [
            (&f.dte, 0.0, 36500.0),
            (&f.strike, 0.0, 1e12),
            (&f.delta, -1.0, 1.0),
        ] {
            if r.as_ref().is_some_and(|r| !r.valid(min, max)) {
                return Err("option_selection_range_invalid");
            }
        }
        if f.dte
            .as_ref()
            .is_some_and(|r| [r.min, r.max].iter().flatten().any(|v| v.fract() != 0.0))
        {
            return Err("option_selection_dte_invalid");
        }
        if [f.min_volume, f.min_open_interest, f.max_relative_spread]
            .iter()
            .flatten()
            .any(|v| !v.is_finite() || *v < 0.0 || *v > 1e15)
        {
            return Err("option_selection_liquidity_invalid");
        }
        if let Some(r) = &f.expiry {
            if (r.earliest.is_none() && r.latest.is_none())
                || [r.earliest.as_ref(), r.latest.as_ref()]
                    .iter()
                    .flatten()
                    .any(|v| !date_valid(v))
                || r.earliest
                    .as_ref()
                    .zip(r.latest.as_ref())
                    .is_some_and(|(a, b)| a > b)
            {
                return Err("option_selection_expiry_invalid");
            }
        }
        let mut fields = BTreeSet::new();
        if self.ranking.iter().any(|r| {
            !fields.insert(&r.field) || r.target.is_some_and(|v| !v.is_finite() || v.abs() > 1e15)
        }) {
            return Err("option_selection_ranking_invalid");
        }
        if let OptionAnalytics::ExplicitModel {
            capability_requirement_ref,
            semantics_version,
        } = &self.analytics
        {
            if capability_requirement_ref.is_empty()
                || capability_requirement_ref.len() > 128
                || semantics_version.is_empty()
                || semantics_version.len() > 128
            {
                return Err("option_selection_model_identity_required");
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn value() -> serde_json::Value {
        serde_json::json!({"version":"tradeassembly.option_selection.v1","signal_requirement_ref":"signal","trade_requirement_ref":"trade","underlying_selector_ref":"underlying","chain_capability_requirement_ref":"chain","filters":{"right":"call","expiry":{"earliest":"2026-10-30","latest":"2026-12-31"},"dte":{"min":0,"max":60},"delta":{"min":0.3,"max":0.7},"min_open_interest":10,"max_relative_spread":0.2},"analytics":{"kind":"provider_supplied"},"ranking":[{"field":"delta","direction":"ascending","target":0.5}],"tiebreak":"contract_id_ascending","missing_data":"reject_candidate","no_match":"no_eligible_contract","max_age_ms":60000,"max_contracts":100,"max_pages":4})
    }
    fn policy(value: serde_json::Value) -> OptionSelection {
        serde_json::from_value(value).unwrap()
    }
    #[test]
    fn explicit_selection_schema_is_closed_and_versioned() {
        assert!(policy(value()).validate().is_ok());
        let mut v = value();
        v["version"] = serde_json::json!("tradeassembly.option_selection.v2");
        assert!(serde_json::from_value::<OptionSelection>(v).is_err());
        let mut v = value();
        v["filters"]["unknown_filter"] = serde_json::json!(1);
        assert!(serde_json::from_value::<OptionSelection>(v).is_err());
        let mut v = value();
        v.as_object_mut().unwrap().remove("trade_requirement_ref");
        assert!(serde_json::from_value::<OptionSelection>(v).is_err());
        let mut v = value();
        v["analytics"] =
            serde_json::json!({"kind":"explicit_model","capability_requirement_ref":"model"});
        assert!(serde_json::from_value::<OptionSelection>(v).is_err());
    }
    #[test]
    fn selection_rejects_ambiguous_ranges_dates_ranks_and_unbounded_queries() {
        for (pointer, v) in [
            ("/filters/delta", serde_json::json!({"min":0.7,"max":0.3})),
            ("/filters/expiry/earliest", serde_json::json!("2026-02-29")),
            ("/filters/dte/min", serde_json::json!(0.5)),
            ("/ranking", serde_json::json!([])),
            ("/max_pages", serde_json::json!(17)),
            ("/max_contracts", serde_json::json!(4097)),
            ("/max_age_ms", serde_json::json!(0)),
        ] {
            let mut spec = value();
            *spec.pointer_mut(pointer).unwrap() = v;
            assert!(policy(spec).validate().is_err(), "{pointer}");
        }
        let mut p = policy(value());
        p.ranking.push(p.ranking[0].clone());
        assert_eq!(p.validate(), Err("option_selection_ranking_invalid"));
        assert!(date_valid("2024-02-29"));
        assert!(!date_valid("2026-02-29"));
    }
}
