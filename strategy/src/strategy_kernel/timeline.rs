//! Deterministic bar ordering and next-bar identity, independent of provider row order.
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BarTime {
    pub symbol: String,
    /// Provider bar timestamp is the start, not the completion time.
    pub start_ms: i64,
    pub session_start_ms: i64,
    pub session_end_ms: i64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TimelineError {
    InvalidInterval,
    InvalidBar,
    DuplicateBar,
    UnknownIndex,
}

#[derive(Debug)]
pub struct Timeline {
    interval_ms: i64,
    bars: Vec<BarTime>,
    indices: BTreeMap<(i64, String), usize>,
    ordered_indices: Vec<usize>,
}

impl Timeline {
    pub fn new(bars: Vec<BarTime>, interval_ms: i64) -> Result<Self, TimelineError> {
        if interval_ms <= 0 {
            return Err(TimelineError::InvalidInterval);
        }
        let mut indices = BTreeMap::new();
        let mut sessions = BTreeMap::new();
        for (index, bar) in bars.iter().enumerate() {
            let end = bar
                .start_ms
                .checked_add(interval_ms)
                .ok_or(TimelineError::InvalidBar)?;
            if bar.symbol.trim().is_empty()
                || bar.start_ms < bar.session_start_ms
                || end > bar.session_end_ms
                || bar.session_start_ms >= bar.session_end_ms
                || (i128::from(bar.start_ms) - i128::from(bar.session_start_ms))
                    % i128::from(interval_ms)
                    != 0
            {
                return Err(TimelineError::InvalidBar);
            }
            let session_key = (bar.symbol.clone(), bar.session_start_ms);
            if sessions
                .insert(session_key, bar.session_end_ms)
                .is_some_and(|end| end != bar.session_end_ms)
            {
                return Err(TimelineError::InvalidBar);
            }
            if indices
                .insert((bar.start_ms, bar.symbol.clone()), index)
                .is_some()
            {
                return Err(TimelineError::DuplicateBar);
            }
        }
        let ordered_indices = indices.values().copied().collect();
        Ok(Self {
            interval_ms,
            bars,
            indices,
            ordered_indices,
        })
    }

    /// Source indices, ordered by event time then symbol. Caller processes whole batches.
    pub fn batches(&self) -> Vec<Vec<usize>> {
        let times: BTreeSet<_> = self.bars.iter().map(|bar| bar.start_ms).collect();
        let mut result = Vec::with_capacity(times.len());
        let mut previous = None;
        for &index in &self.ordered_indices {
            let timestamp = self.bars[index].start_ms;
            if previous != Some(timestamp) {
                result.push(Vec::new());
                previous = Some(timestamp);
            }
            result.last_mut().expect("batch created").push(index);
        }
        result
    }

    pub fn next_bar(&self, source_index: usize) -> Result<Option<usize>, TimelineError> {
        let bar = self
            .bars
            .get(source_index)
            .ok_or(TimelineError::UnknownIndex)?;
        let next_start = bar
            .start_ms
            .checked_add(self.interval_ms)
            .ok_or(TimelineError::InvalidBar)?;
        Ok(self
            .indices
            .get(&(next_start, bar.symbol.clone()))
            .copied()
            .filter(|&index| {
                let next = &self.bars[index];
                next.session_start_ms == bar.session_start_ms
                    && next.session_end_ms == bar.session_end_ms
            }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn bar(symbol: &str, start: i64) -> BarTime {
        BarTime {
            symbol: symbol.into(),
            start_ms: start,
            session_start_ms: 0,
            session_end_ms: 180_000,
        }
    }
    #[test]
    fn interleaved_symbols_use_time_then_symbol_not_source_index() {
        let t = Timeline::new(
            vec![bar("B", 60_000), bar("A", 0), bar("B", 0), bar("A", 60_000)],
            60_000,
        )
        .unwrap();
        assert_eq!(t.batches(), vec![vec![1, 2], vec![3, 0]]);
        assert_eq!(t.next_bar(1), Ok(Some(3)));
        assert_eq!(t.next_bar(2), Ok(Some(0)));
    }
    #[test]
    fn missing_next_minute_and_session_boundaries_never_jump() {
        let t = Timeline::new(
            vec![
                bar("A", 0),
                bar("A", 120_000),
                BarTime {
                    session_start_ms: 180_000,
                    session_end_ms: 300_000,
                    ..bar("A", 180_000)
                },
            ],
            60_000,
        )
        .unwrap();
        assert_eq!(t.next_bar(0), Ok(None));
        assert_eq!(t.next_bar(1), Ok(None));
        assert_eq!(t.next_bar(99), Err(TimelineError::UnknownIndex));
    }
    #[test]
    fn duplicate_unaligned_and_conflicting_session_data_fail_closed() {
        assert_eq!(
            Timeline::new(vec![bar("A", 0), bar("A", 0)], 60_000).unwrap_err(),
            TimelineError::DuplicateBar
        );
        assert_eq!(
            Timeline::new(vec![bar("A", 1)], 60_000).unwrap_err(),
            TimelineError::InvalidBar
        );
        assert_eq!(
            Timeline::new(
                vec![
                    bar("A", 0),
                    BarTime {
                        session_end_ms: 240_000,
                        ..bar("A", 60_000)
                    }
                ],
                60_000
            )
            .unwrap_err(),
            TimelineError::InvalidBar
        );
    }
}
