//! Session-scoped metrics over explicitly completed, ordered bars.
//! The caller supplies the declared VWAP price basis; this module never substitutes one.

use std::collections::VecDeque;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MetricsError {
    InvalidWindow,
    InvalidBar,
    NonMonotonicBar,
    ArithmeticOverflow,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CompletedBar {
    pub close_time_ms: i64,
    /// Calendar-resolved start of this regular session, not a guessed UTC date.
    pub session_start_ms: i64,
    pub vwap_basis_micros: i64,
    pub volume_micros: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SessionMetrics {
    pub vwap_micros: Option<i64>,
    /// Previous completed bars only; current volume is deliberately excluded.
    pub prior_volume_sum_micros: i128,
    pub prior_bar_count: usize,
    pub volume_window_ready: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionAccumulator {
    window: usize,
    session: Option<i64>,
    last_time: Option<i64>,
    weighted_price: i128,
    volume: i128,
    prior_volumes: VecDeque<i64>,
}

impl SessionAccumulator {
    pub fn new(window: usize) -> Result<Self, MetricsError> {
        if window == 0 || window > 10_000 {
            return Err(MetricsError::InvalidWindow);
        }
        Ok(Self {
            window,
            session: None,
            last_time: None,
            weighted_price: 0,
            volume: 0,
            prior_volumes: VecDeque::new(),
        })
    }

    /// Failure is atomic: invalid or duplicate bars do not modify accumulated state.
    pub fn observe(&mut self, bar: CompletedBar) -> Result<SessionMetrics, MetricsError> {
        if bar.vwap_basis_micros <= 0
            || bar.volume_micros < 0
            || bar.session_start_ms >= bar.close_time_ms
        {
            return Err(MetricsError::InvalidBar);
        }
        if self.last_time.is_some_and(|last| bar.close_time_ms <= last)
            || self
                .session
                .is_some_and(|session| bar.session_start_ms < session)
        {
            return Err(MetricsError::NonMonotonicBar);
        }
        let new_session = self.session != Some(bar.session_start_ms);
        let old_weighted = if new_session { 0 } else { self.weighted_price };
        let old_volume = if new_session { 0 } else { self.volume };
        let weighted = i128::from(bar.vwap_basis_micros)
            .checked_mul(i128::from(bar.volume_micros))
            .and_then(|term| old_weighted.checked_add(term))
            .ok_or(MetricsError::ArithmeticOverflow)?;
        let volume = old_volume
            .checked_add(i128::from(bar.volume_micros))
            .ok_or(MetricsError::ArithmeticOverflow)?;
        let vwap = if volume == 0 {
            None
        } else {
            Some(i64::try_from(weighted / volume).map_err(|_| MetricsError::ArithmeticOverflow)?)
        };
        let count = if new_session {
            0
        } else {
            self.prior_volumes.len()
        };
        let prior_sum = if new_session {
            0
        } else {
            self.prior_volumes
                .iter()
                .map(|value| i128::from(*value))
                .sum()
        };
        if new_session {
            self.prior_volumes.clear();
        }
        self.session = Some(bar.session_start_ms);
        self.last_time = Some(bar.close_time_ms);
        self.weighted_price = weighted;
        self.volume = volume;
        self.prior_volumes.push_back(bar.volume_micros);
        if self.prior_volumes.len() > self.window {
            self.prior_volumes.pop_front();
        }
        Ok(SessionMetrics {
            vwap_micros: vwap,
            prior_volume_sum_micros: prior_sum,
            prior_bar_count: count,
            volume_window_ready: count == self.window,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn bar(time: i64, price: i64, volume: i64) -> CompletedBar {
        CompletedBar {
            close_time_ms: time,
            session_start_ms: 0,
            vwap_basis_micros: price,
            volume_micros: volume,
        }
    }
    #[test]
    fn weighted_vwap_and_prior_only_window() {
        let mut state = SessionAccumulator::new(2).unwrap();
        assert!(!state.observe(bar(1, 100, 10)).unwrap().volume_window_ready);
        assert_eq!(
            state.observe(bar(2, 200, 30)).unwrap().vwap_micros,
            Some(175)
        );
        let metrics = state.observe(bar(3, 100, 1000)).unwrap();
        assert!(metrics.volume_window_ready);
        assert_eq!(metrics.prior_volume_sum_micros, 40);
        assert_eq!(metrics.prior_bar_count, 2);
        assert_eq!(
            state
                .observe(bar(4, 100, 0))
                .unwrap()
                .prior_volume_sum_micros,
            1030
        );
    }
    #[test]
    fn session_reset_and_zero_volume_are_explicit() {
        let mut state = SessionAccumulator::new(1).unwrap();
        assert_eq!(state.observe(bar(1, 100, 0)).unwrap().vwap_micros, None);
        state.observe(bar(2, 100, 10)).unwrap();
        let metrics = state
            .observe(CompletedBar {
                session_start_ms: 10,
                ..bar(11, 200, 10)
            })
            .unwrap();
        assert_eq!(metrics.vwap_micros, Some(200));
        assert!(!metrics.volume_window_ready);
        assert_eq!(metrics.prior_bar_count, 0);
    }
    #[test]
    fn duplicate_invalid_and_overflow_leave_state_unchanged() {
        let mut state = SessionAccumulator::new(2).unwrap();
        state.observe(bar(1, 100, 10)).unwrap();
        let before = state.clone();
        assert_eq!(
            state.observe(bar(1, 100, 10)),
            Err(MetricsError::NonMonotonicBar)
        );
        assert_eq!(
            state.observe(bar(2, 100, -1)),
            Err(MetricsError::InvalidBar)
        );
        assert_eq!(state, before);
        state.weighted_price = i128::MAX;
        let before = state.clone();
        assert_eq!(
            state.observe(bar(2, 100, 10)),
            Err(MetricsError::ArithmeticOverflow)
        );
        assert_eq!(state, before);
    }
}
