//! Simulation-only capacity accounting. Broker authorization still belongs to Warden.
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PortfolioCapacity {
    maximum_positions: usize,
    maximum_exposure_micros: i64,
    positions: BTreeMap<String, i64>,
    pending: BTreeMap<String, i64>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CapacityError {
    InvalidInput,
    AlreadyOccupied,
    PositionLimit,
    ExposureLimit,
    MissingReservation,
    FillExceedsReservation,
    ArithmeticOverflow,
}

impl PortfolioCapacity {
    pub fn new(
        maximum_positions: usize,
        maximum_exposure_micros: i64,
    ) -> Result<Self, CapacityError> {
        if maximum_positions == 0 || maximum_exposure_micros <= 0 {
            return Err(CapacityError::InvalidInput);
        }
        Ok(Self {
            maximum_positions,
            maximum_exposure_micros,
            positions: BTreeMap::new(),
            pending: BTreeMap::new(),
        })
    }

    pub fn committed_exposure_micros(&self) -> i128 {
        self.positions
            .values()
            .chain(self.pending.values())
            .map(|v| i128::from(*v))
            .sum()
    }

    pub fn occupied_slots(&self) -> usize {
        self.positions
            .keys()
            .chain(self.pending.keys())
            .collect::<BTreeSet<_>>()
            .len()
    }

    /// Call in the strategy's explicit tie-break order for simultaneous entry signals.
    pub fn reserve(&mut self, symbol: &str, notional_micros: i64) -> Result<(), CapacityError> {
        if symbol.trim().is_empty() || notional_micros <= 0 {
            return Err(CapacityError::InvalidInput);
        }
        if self.positions.contains_key(symbol) || self.pending.contains_key(symbol) {
            return Err(CapacityError::AlreadyOccupied);
        }
        if self.occupied_slots() >= self.maximum_positions {
            return Err(CapacityError::PositionLimit);
        }
        if self.committed_exposure_micros() + i128::from(notional_micros)
            > i128::from(self.maximum_exposure_micros)
        {
            return Err(CapacityError::ExposureLimit);
        }
        self.pending.insert(symbol.to_string(), notional_micros);
        Ok(())
    }

    /// Apply one incremental, deduplicated simulated fill. The engine owns fill IDs.
    /// A partial fill retains the unfilled reservation; a terminal fill releases it.
    pub fn fill(
        &mut self,
        symbol: &str,
        gross_micros: i64,
        terminal: bool,
    ) -> Result<(), CapacityError> {
        if gross_micros <= 0 {
            return Err(CapacityError::InvalidInput);
        }
        let remaining = *self
            .pending
            .get(symbol)
            .ok_or(CapacityError::MissingReservation)?;
        if gross_micros > remaining {
            return Err(CapacityError::FillExceedsReservation);
        }
        let position = self
            .positions
            .get(symbol)
            .copied()
            .unwrap_or(0)
            .checked_add(gross_micros)
            .ok_or(CapacityError::ArithmeticOverflow)?;
        self.positions.insert(symbol.to_string(), position);
        if terminal || gross_micros == remaining {
            self.pending.remove(symbol);
        } else {
            self.pending
                .insert(symbol.to_string(), remaining - gross_micros);
        }
        Ok(())
    }

    /// Rejection/expiry cancels only the unfilled remainder, never the filled position.
    pub fn release_pending(&mut self, symbol: &str) -> Result<(), CapacityError> {
        self.pending
            .remove(symbol)
            .map(|_| ())
            .ok_or(CapacityError::MissingReservation)
    }

    /// Mark-to-market may exceed the limit; record reality and reject future entries.
    /// Zero removes a fully closed position, but does not cancel an outstanding order.
    pub fn mark_position(
        &mut self,
        symbol: &str,
        notional_micros: i64,
    ) -> Result<(), CapacityError> {
        if notional_micros < 0 || !self.positions.contains_key(symbol) {
            return Err(CapacityError::InvalidInput);
        }
        if notional_micros == 0 {
            self.positions.remove(symbol);
        } else {
            self.positions.insert(symbol.to_string(), notional_micros);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn pending_orders_consume_slots_and_exposure() {
        let mut p = PortfolioCapacity::new(4, 500).unwrap();
        for symbol in ["A", "B", "C", "D"] {
            p.reserve(symbol, 100).unwrap();
        }
        assert_eq!(p.reserve("E", 100), Err(CapacityError::PositionLimit));
        assert_eq!(p.reserve("A", 100), Err(CapacityError::AlreadyOccupied));
        assert_eq!(p.committed_exposure_micros(), 400);
        let mut p = PortfolioCapacity::new(4, 150).unwrap();
        p.reserve("A", 100).unwrap();
        assert_eq!(p.reserve("B", 100), Err(CapacityError::ExposureLimit));
    }
    #[test]
    fn partial_fills_and_cancellation_preserve_real_exposure() {
        let mut p = PortfolioCapacity::new(2, 200).unwrap();
        p.reserve("A", 100).unwrap();
        p.fill("A", 40, false).unwrap();
        assert_eq!(p.committed_exposure_micros(), 100);
        assert_eq!(p.occupied_slots(), 1);
        p.release_pending("A").unwrap();
        assert_eq!(p.committed_exposure_micros(), 40);
        assert_eq!(p.occupied_slots(), 1);
        p.mark_position("A", 0).unwrap();
        assert_eq!(p.occupied_slots(), 0);
    }
    #[test]
    fn invalid_fill_is_atomic_and_market_moves_block_new_entries() {
        let mut p = PortfolioCapacity::new(2, 200).unwrap();
        p.reserve("A", 100).unwrap();
        let before = p.clone();
        assert_eq!(
            p.fill("A", 101, true),
            Err(CapacityError::FillExceedsReservation)
        );
        assert_eq!(p, before);
        p.fill("A", 80, true).unwrap();
        assert_eq!(p.committed_exposure_micros(), 80);
        assert_eq!(
            p.fill("A", 80, true),
            Err(CapacityError::MissingReservation)
        );
        p.mark_position("A", 250).unwrap();
        assert_eq!(p.reserve("B", 1), Err(CapacityError::ExposureLimit));
        assert_eq!(p.committed_exposure_micros(), 250);
    }
}
