//! Pure long-position exit rules. Inputs are evidence, never broker authority.
use serde::{Deserialize, Serialize};
use tradeassembly_plugin_sdk::{
    execution::{Decimal, Rounding},
    execution_snapshot::Quote,
};

pub(super) const VERSION: &str = "tradeassembly.position_exit_rules.v1";

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum PriceBasis {
    Bid,
    Ask,
    Mid,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub(super) enum TrailingActivation {
    Immediate {},
    GainRatio { ratio: Decimal },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum Trigger {
    StopLoss,
    Trailing,
    Reversal,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct ExitRules {
    pub schema_version: String,
    pub price_basis: PriceBasis,
    /// Fraction of fee-exclusive entry basis lost, e.g. "0.1" means 10%.
    pub stop_loss_ratio: Option<Decimal>,
    /// Fraction below the observed high water, e.g. "0.2" means 20%.
    pub trailing_ratio: Option<Decimal>,
    #[serde(rename = "trailingStart")]
    pub trailing_activation: Option<TrailingActivation>,
    /// Exact published exit signal identifier. The caller must bind the ready
    /// boolean to this identifier, the managed instrument, scope and cutoff.
    pub reversal_signal: Option<String>,
    /// Every enabled trigger exactly once, highest priority first.
    pub precedence: Vec<Trigger>,
    pub max_quote_age_ms: i64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Outcome {
    pub trigger: Option<Trigger>,
    pub next_high_water: Decimal,
    pub trailing_active: bool,
}

fn one() -> Result<Decimal, String> {
    "1".parse()
}

fn loss_ratio(ratio: Decimal) -> Result<(), String> {
    if !ratio.positive() || !ratio.compare(one()?)?.is_lt() {
        return Err("position_exit_ratio_invalid".into());
    }
    Ok(())
}

impl ExitRules {
    pub fn validate(&self) -> Result<(), String> {
        if self.schema_version != VERSION || !(1..=30_000).contains(&self.max_quote_age_ms) {
            return Err("position_exit_rules_invalid".into());
        }
        for ratio in [self.stop_loss_ratio, self.trailing_ratio]
            .into_iter()
            .flatten()
        {
            loss_ratio(ratio)?;
        }
        match (self.trailing_ratio, self.trailing_activation) {
            (Some(_), Some(TrailingActivation::GainRatio { ratio })) => {
                if !ratio.positive() {
                    return Err("position_exit_activation_invalid".into());
                }
                one()?.checked_add(ratio)?;
            }
            (Some(_), Some(TrailingActivation::Immediate {})) | (None, None) => {}
            _ => return Err("position_exit_activation_invalid".into()),
        }
        if self.reversal_signal.as_ref().is_some_and(|id| {
            id.is_empty() || id.len() > 256 || id.trim() != id || id.chars().any(char::is_control)
        }) {
            return Err("position_exit_reversal_identifier_invalid".into());
        }
        let enabled = [
            (Trigger::StopLoss, self.stop_loss_ratio.is_some()),
            (Trigger::Trailing, self.trailing_ratio.is_some()),
            (Trigger::Reversal, self.reversal_signal.is_some()),
        ];
        if self.precedence.is_empty()
            || self.precedence.len() > 3
            || enabled.iter().any(|(trigger, enabled)| {
                self.precedence.iter().filter(|t| *t == trigger).count() != usize::from(*enabled)
            })
        {
            return Err("position_exit_precedence_invalid".into());
        }
        Ok(())
    }

    /// Select a current observed price. Mid must be exactly representable; no
    /// truncation, substituted side, or inference from unavailable values.
    pub fn price(&self, quote: &Quote, now_ms: i64) -> Result<Decimal, String> {
        self.validate()?;
        quote.validate()?;
        if quote.time.known_at_ms > now_ms
            || now_ms
                .checked_sub(quote.time.observed_at_ms)
                .is_none_or(|age| age < 0 || age >= self.max_quote_age_ms)
        {
            return Err("position_exit_quote_not_current".into());
        }
        let price = match self.price_basis {
            PriceBasis::Bid => *quote.bid.required()?,
            PriceBasis::Ask => *quote.ask.required()?,
            PriceBasis::Mid => quote
                .bid
                .required()?
                .checked_add(*quote.ask.required()?)?
                .multiply("0.5".parse()?)?
                .round(12, Rounding::Exact)?,
        };
        if !price.positive() {
            return Err("position_exit_price_invalid".into());
        }
        Ok(price)
    }

    /// Inclusive long-position thresholds. High water begins at the supplied
    /// fee-exclusive per-unit entry basis and advances only to observed prices.
    /// The caller persists this result atomically with its monitor transition.
    pub fn evaluate(
        &self,
        basis: Decimal,
        persisted_high_water: Option<Decimal>,
        current_price: Decimal,
        ready_reversal: Option<bool>,
    ) -> Result<Outcome, String> {
        self.validate()?;
        if !basis.positive() || !current_price.positive() {
            return Err("position_exit_price_invalid".into());
        }
        // Resolve required evidence before considering any trigger: missing
        // reversal evidence cannot silently become false or lose precedence.
        let reversal = if self.reversal_signal.is_some() {
            ready_reversal.ok_or("position_exit_reversal_unavailable")?
        } else {
            false
        };
        let high_water = persisted_high_water.unwrap_or(basis);
        if !high_water.positive() || high_water.compare(basis)?.is_lt() {
            return Err("position_exit_high_water_invalid".into());
        }
        let next_high_water = if current_price.compare(high_water)?.is_gt() {
            current_price
        } else {
            high_water
        };
        let stop_loss = self
            .stop_loss_ratio
            .map(|ratio| below_loss(current_price, basis, ratio))
            .transpose()?
            .unwrap_or(false);
        let trailing_active = match self.trailing_activation {
            None => false,
            Some(TrailingActivation::Immediate {}) => true,
            Some(TrailingActivation::GainRatio { ratio }) => {
                // A Decimal price lies on a grid of at most 12 decimal places.
                // Comparing to the ceiling of a rational activation threshold
                // is exact on that grid, even if the product has >12 places.
                let threshold = basis
                    .multiply(one()?.checked_add(ratio)?)?
                    .round(12, Rounding::Ceil)?;
                !next_high_water.compare(threshold)?.is_lt()
            }
        };
        let trailing = if trailing_active {
            below_loss(
                current_price,
                next_high_water,
                self.trailing_ratio
                    .ok_or("position_exit_activation_invalid")?,
            )?
        } else {
            false
        };
        let trigger = self
            .precedence
            .iter()
            .copied()
            .find(|trigger| match trigger {
                Trigger::StopLoss => stop_loss,
                Trigger::Trailing => trailing,
                Trigger::Reversal => reversal,
            });
        Ok(Outcome {
            trigger,
            next_high_water,
            trailing_active,
        })
    }
}

fn below_loss(price: Decimal, reference: Decimal, ratio: Decimal) -> Result<bool, String> {
    // price <= reference * (1 - ratio). Directed rounding is only used for
    // this comparison: no rounded threshold is reported as a fill or quote.
    // For a <=12-place price, <=floor12(product) is exactly <=product.
    let threshold = reference
        .multiply(one()?.checked_subtract(ratio)?)?
        .round(12, Rounding::Floor)?;
    Ok(!price.compare(threshold)?.is_gt())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn d(value: &str) -> Decimal {
        value.parse().unwrap()
    }
    fn rules() -> ExitRules {
        ExitRules {
            schema_version: VERSION.into(),
            price_basis: PriceBasis::Bid,
            stop_loss_ratio: Some(d("0.1")),
            trailing_ratio: Some(d("0.2")),
            trailing_activation: Some(TrailingActivation::Immediate {}),
            reversal_signal: Some("published.exit.reversal".into()),
            precedence: vec![Trigger::StopLoss, Trigger::Trailing, Trigger::Reversal],
            max_quote_age_ms: 30_000,
        }
    }
    #[test]
    fn ten_percent_stop_and_twenty_percent_trail_have_inclusive_boundaries() {
        let r = rules();
        assert_eq!(
            r.evaluate(d("100"), None, d("90"), Some(false))
                .unwrap()
                .trigger,
            Some(Trigger::StopLoss)
        );
        assert_eq!(
            r.evaluate(d("100"), None, d("90.000000000001"), Some(false))
                .unwrap()
                .trigger,
            None
        );
        let raised = r.evaluate(d("100"), None, d("150"), Some(false)).unwrap();
        assert_eq!(raised.next_high_water, d("150"));
        assert_eq!(
            r.evaluate(
                d("100"),
                Some(raised.next_high_water),
                d("120"),
                Some(false)
            )
            .unwrap()
            .trigger,
            Some(Trigger::Trailing)
        );
        assert_eq!(
            r.evaluate(d("100"), Some(d("150")), d("120.000000000001"), Some(false))
                .unwrap()
                .trigger,
            None
        );
    }
    #[test]
    fn gain_activation_is_explicit_and_retained_by_high_water() {
        let mut r = rules();
        r.trailing_activation = Some(TrailingActivation::GainRatio { ratio: d("0.5") });
        let before = r
            .evaluate(d("100"), None, d("149.999999999999"), Some(false))
            .unwrap();
        assert!(!before.trailing_active);
        let activated = r.evaluate(d("100"), None, d("150"), Some(false)).unwrap();
        assert!(activated.trailing_active);
        let after = r
            .evaluate(
                d("100"),
                Some(activated.next_high_water),
                d("120"),
                Some(false),
            )
            .unwrap();
        assert!(after.trailing_active);
        assert_eq!(after.trigger, Some(Trigger::Trailing));
    }
    #[test]
    fn precedence_selects_only_the_first_matching_enabled_rule() {
        let mut r = rules();
        assert_eq!(
            r.evaluate(d("100"), Some(d("150")), d("90"), Some(true))
                .unwrap()
                .trigger,
            Some(Trigger::StopLoss)
        );
        r.precedence = vec![Trigger::Trailing, Trigger::Reversal, Trigger::StopLoss];
        assert_eq!(
            r.evaluate(d("100"), Some(d("150")), d("90"), Some(true))
                .unwrap()
                .trigger,
            Some(Trigger::Trailing)
        );
        r.precedence = vec![Trigger::Reversal, Trigger::StopLoss, Trigger::Trailing];
        assert_eq!(
            r.evaluate(d("100"), Some(d("150")), d("90"), Some(true))
                .unwrap()
                .trigger,
            Some(Trigger::Reversal)
        );
        assert_eq!(
            r.evaluate(d("100"), None, d("1"), None).unwrap_err(),
            "position_exit_reversal_unavailable"
        );
    }
    #[test]
    fn rational_thresholds_do_not_round_an_exit_into_existence() {
        let mut r = rules();
        r.stop_loss_ratio = Some(d("0.000000000001"));
        // Exact threshold is 0.999999999999999999999999, between two prices.
        assert_eq!(
            r.evaluate(d("1.000000000001"), None, d("1"), Some(false))
                .unwrap()
                .trigger,
            None
        );
        assert_eq!(
            r.evaluate(d("1.000000000001"), None, d("0.999999999999"), Some(false))
                .unwrap()
                .trigger,
            Some(Trigger::StopLoss)
        );
        r.trailing_activation = Some(TrailingActivation::GainRatio {
            ratio: d("0.000000000001"),
        });
        assert!(
            !r.evaluate(d("1.000000000001"), None, d("1.000000000002"), Some(false))
                .unwrap()
                .trailing_active
        );
        assert!(
            r.evaluate(d("1.000000000001"), None, d("1.000000000003"), Some(false))
                .unwrap()
                .trailing_active
        );
    }
    #[test]
    fn invalid_ratios_activation_and_precedence_fail_closed() {
        for ratio in ["-0.1", "0", "1", "1.1"] {
            let mut r = rules();
            r.stop_loss_ratio = Some(d(ratio));
            assert!(r.validate().is_err());
            let mut r = rules();
            r.trailing_ratio = Some(d(ratio));
            assert!(r.validate().is_err());
        }
        let mut r = rules();
        r.trailing_activation = None;
        assert!(r.validate().is_err());
        r.trailing_activation = Some(TrailingActivation::GainRatio { ratio: d("0") });
        assert!(r.validate().is_err());
        let mut r = rules();
        r.precedence = vec![Trigger::StopLoss, Trigger::StopLoss, Trigger::Reversal];
        assert!(r.validate().is_err());
        let mut r = rules();
        r.max_quote_age_ms = 30_001;
        assert!(r.validate().is_err());
        assert!(rules()
            .evaluate(d("100"), Some(d("99")), d("100"), Some(false))
            .is_err());
    }
    #[test]
    fn closed_versioned_configuration_has_no_implicit_numeric_defaults() {
        let value = serde_json::to_value(rules()).unwrap();
        assert_eq!(
            serde_json::from_value::<ExitRules>(value.clone()).unwrap(),
            rules()
        );
        let mut extra = value.clone();
        extra["unknown"] = serde_json::json!(true);
        assert!(serde_json::from_value::<ExitRules>(extra).is_err());
        let mut missing = value;
        missing.as_object_mut().unwrap().remove("priceBasis");
        assert!(serde_json::from_value::<ExitRules>(missing).is_err());
        let mut r = rules();
        r.schema_version = "future".into();
        assert!(r.validate().is_err());
    }

    #[test]
    fn quote_basis_freshness_and_midpoint_are_explicit() {
        let mut quote: Quote = serde_json::from_value(serde_json::json!({
            "schemaVersion":"tradeassembly.execution_quote.v1",
            "instrumentId":"SPY", "currency":{"code":"USD","decimals":2},
            "time":{"observedAtMs":100,"knownAtMs":100,"basis":"provider_observed"},
            "origin":"provider", "sourceProfile":"fixture",
            "indicative":{"status":"available","value":false},
            "bid":{"status":"available","value":"100"},
            "ask":{"status":"available","value":"102"}
        }))
        .unwrap();
        let mut r = rules();
        assert_eq!(r.price(&quote, 100).unwrap(), d("100"));
        r.price_basis = PriceBasis::Ask;
        assert_eq!(r.price(&quote, 100).unwrap(), d("102"));
        r.price_basis = PriceBasis::Mid;
        assert_eq!(r.price(&quote, 100).unwrap(), d("101"));
        assert!(r.price(&quote, 30_099).is_ok());
        assert_eq!(
            r.price(&quote, 30_100).unwrap_err(),
            "position_exit_quote_not_current"
        );
        assert!(r.price(&quote, 99).is_err());
        use tradeassembly_plugin_sdk::execution_snapshot::Observed;
        quote.bid = Observed::Available { value: d("1") };
        quote.ask = Observed::Available {
            value: d("1.000000000001"),
        };
        assert_eq!(
            r.price(&quote, 100).unwrap_err(),
            "execution_decimal_precision_loss"
        );
        quote.bid = Observed::Unavailable {
            reason: "unobserved".into(),
        };
        assert!(r.price(&quote, 100).is_err());
    }
}
