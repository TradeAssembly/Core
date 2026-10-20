//! Owner-configured signal rules. No broker calls or implicit trading defaults.
use super::session_metrics::SessionMetrics;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct VwapRules {
    pub entry_discount_bps: u32,
    pub minimum_volume_ratio_bps: u32,
    pub exit_proximity_bps: u32,
    pub maximum_holding_ms: i64,
    pub maximum_loss_bps: u32,
    pub entry_cooldown_ms: i64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PositionFacts {
    pub average_entry_price_micros: i64,
    pub first_fill_time_ms: i64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Signal {
    Hold,
    Enter,
    Exit {
        vwap: bool,
        holding_time: bool,
        loss: bool,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RuleError {
    InvalidConfiguration,
    InvalidFacts,
}

impl VwapRules {
    pub fn validate(&self) -> Result<(), RuleError> {
        if self.entry_discount_bps >= 10_000
            || self.exit_proximity_bps >= 10_000
            || self.maximum_loss_bps >= 10_000
            || self.minimum_volume_ratio_bps == 0
            || self.maximum_holding_ms <= 0
            || self.entry_cooldown_ms < 0
        {
            return Err(RuleError::InvalidConfiguration);
        }
        Ok(())
    }

    pub fn evaluate(
        &self,
        now_ms: i64,
        price_micros: i64,
        volume_micros: i64,
        metrics: SessionMetrics,
        position: Option<PositionFacts>,
        last_entry_ms: Option<i64>,
    ) -> Result<Signal, RuleError> {
        self.validate()?;
        if price_micros <= 0
            || volume_micros < 0
            || metrics.prior_volume_sum_micros < 0
            || metrics.prior_bar_count > 10_000
            || metrics.vwap_micros.is_some_and(|v| v <= 0)
            || last_entry_ms.is_some_and(|t| t > now_ms)
        {
            return Err(RuleError::InvalidFacts);
        }
        let price = i128::from(price_micros);
        if let Some(position) = position {
            if position.average_entry_price_micros <= 0 || position.first_fill_time_ms > now_ms {
                return Err(RuleError::InvalidFacts);
            }
            let entry = i128::from(position.average_entry_price_micros);
            let vwap = metrics.vwap_micros.is_some_and(|v| {
                (price - i128::from(v)).abs() * 10_000
                    <= i128::from(v) * i128::from(self.exit_proximity_bps)
            });
            let holding_time = i128::from(now_ms) - i128::from(position.first_fill_time_ms)
                > i128::from(self.maximum_holding_ms);
            let loss = (entry - price) * 10_000 > entry * i128::from(self.maximum_loss_bps);
            return Ok(if vwap || holding_time || loss {
                Signal::Exit {
                    vwap,
                    holding_time,
                    loss,
                }
            } else {
                Signal::Hold
            });
        }
        let Some(vwap) = metrics.vwap_micros else {
            return Ok(Signal::Hold);
        };
        if !metrics.volume_window_ready
            || metrics.prior_bar_count == 0
            || metrics.prior_volume_sum_micros == 0
            || last_entry_ms.is_some_and(|t| {
                i128::from(now_ms) - i128::from(t) < i128::from(self.entry_cooldown_ms)
            })
        {
            return Ok(Signal::Hold);
        }
        // Compare ratios without division or rounding at the decision boundary.
        let discounted =
            price * 10_000 <= i128::from(vwap) * (10_000 - i128::from(self.entry_discount_bps));
        let required = metrics
            .prior_volume_sum_micros
            .checked_mul(i128::from(self.minimum_volume_ratio_bps))
            .ok_or(RuleError::InvalidFacts)?;
        let sufficient_volume =
            i128::from(volume_micros) * metrics.prior_bar_count as i128 * 10_000 >= required;
        Ok(if discounted && sufficient_volume {
            Signal::Enter
        } else {
            Signal::Hold
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn rules() -> VwapRules {
        VwapRules {
            entry_discount_bps: 50,
            minimum_volume_ratio_bps: 12_000,
            exit_proximity_bps: 10,
            maximum_holding_ms: 900_000,
            maximum_loss_bps: 50,
            entry_cooldown_ms: 600_000,
        }
    }
    fn metrics() -> SessionMetrics {
        SessionMetrics {
            vwap_micros: Some(100_000_000),
            prior_volume_sum_micros: 2000,
            prior_bar_count: 20,
            volume_window_ready: true,
        }
    }
    #[test]
    fn entry_thresholds_and_cooldown_are_exact() {
        let r = rules();
        assert_eq!(
            r.evaluate(600_000, 99_500_000, 120, metrics(), None, Some(0)),
            Ok(Signal::Enter)
        );
        for (time, price, volume) in [
            (599_999, 99_500_000, 120),
            (600_000, 99_500_001, 120),
            (600_000, 99_500_000, 119),
        ] {
            assert_eq!(
                r.evaluate(time, price, volume, metrics(), None, Some(0)),
                Ok(Signal::Hold)
            );
        }
    }
    #[test]
    fn exits_are_independent_and_use_strict_time_and_loss_thresholds() {
        let r = rules();
        let p = Some(PositionFacts {
            average_entry_price_micros: 100_000_000,
            first_fill_time_ms: 0,
        });
        let mut m = metrics();
        m.vwap_micros = None;
        assert_eq!(
            r.evaluate(900_000, 99_500_000, 0, m, p, None),
            Ok(Signal::Hold)
        );
        assert_eq!(
            r.evaluate(900_001, 99_500_000, 0, m, p, None),
            Ok(Signal::Exit {
                vwap: false,
                holding_time: true,
                loss: false
            })
        );
        assert_eq!(
            r.evaluate(1, 99_499_999, 0, m, p, None),
            Ok(Signal::Exit {
                vwap: false,
                holding_time: false,
                loss: true
            })
        );
        assert_eq!(
            r.evaluate(1, 99_900_000, 0, metrics(), p, None),
            Ok(Signal::Exit {
                vwap: true,
                holding_time: false,
                loss: false
            })
        );
    }
    #[test]
    fn warmup_and_missing_baseline_do_not_generate_entries() {
        for m in [
            SessionMetrics {
                volume_window_ready: false,
                ..metrics()
            },
            SessionMetrics {
                prior_volume_sum_micros: 0,
                ..metrics()
            },
        ] {
            assert_eq!(
                rules().evaluate(0, 99_000_000, 120, m, None, None),
                Ok(Signal::Hold)
            );
        }
        assert_eq!(
            rules().evaluate(0, 99_000_000, 120, metrics(), None, Some(1)),
            Err(RuleError::InvalidFacts)
        );
    }
}
