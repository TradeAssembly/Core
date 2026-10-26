// Copyright (c) 2026 OptionLab LLC. All rights reserved.

/// A read-only view of an existing durable schedule. It does not prove that a
/// worker is alive, acquire ownership, or grant authority to execute anything.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ScheduleObservation {
    pub state: String,
    pub due_at_ms: i64,
}

impl ScheduleObservation {
    pub fn next_due_at_ms(&self) -> Option<i64> {
        if matches!(self.state.as_str(), "pending" | "leased") && self.due_at_ms >= 0 {
            Some(self.due_at_ms)
        } else {
            None
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ExecutionTiming {
    Scheduled(i64),
    NotScheduled,
    Unavailable,
}

/// Project timing from durable evidence; never create or claim work to read it.
pub fn execution_timing(
    state: &str,
    orchestrator: &str,
    scheduler_running: bool,
    read: impl FnOnce() -> Result<Option<ScheduleObservation>, String>,
) -> ExecutionTiming {
    if matches!(state, "stopped" | "failed" | "emergency_stopped") {
        return ExecutionTiming::NotScheduled;
    }
    if !matches!(state, "active" | "paused") {
        return ExecutionTiming::Unavailable;
    }
    if orchestrator == "external_agent" {
        return ExecutionTiming::NotScheduled;
    }
    if orchestrator != "deterministic" {
        return ExecutionTiming::Unavailable;
    }
    if !scheduler_running {
        return ExecutionTiming::NotScheduled;
    }
    match read() {
        Ok(Some(record)) => match record.next_due_at_ms() {
            Some(due_at_ms) => ExecutionTiming::Scheduled(due_at_ms),
            None if matches!(record.state.as_str(), "complete" | "canceled") => {
                ExecutionTiming::NotScheduled
            }
            None => ExecutionTiming::Unavailable,
        },
        Ok(None) | Err(_) => ExecutionTiming::Unavailable,
    }
}

#[cfg(test)]
mod tests {
    use super::{execution_timing, ExecutionTiming, ScheduleObservation};

    #[test]
    fn external_and_stopped_executions_do_not_consult_a_deterministic_schedule() {
        for state in ["active", "paused"] {
            assert_eq!(
                execution_timing(state, "external_agent", true, || panic!("must not read")),
                ExecutionTiming::NotScheduled
            );
        }
        for state in ["stopped", "failed", "emergency_stopped"] {
            assert_eq!(
                execution_timing(state, "deterministic", true, || panic!(
                    "stale running flag must not read"
                )),
                ExecutionTiming::NotScheduled
            );
        }
    }

    #[test]
    fn missing_failed_and_unknown_evidence_remain_unavailable() {
        assert_eq!(
            execution_timing("active", "deterministic", true, || Ok(None)),
            ExecutionTiming::Unavailable
        );
        assert_eq!(
            execution_timing("active", "deterministic", true, || Err(
                "storage unavailable".to_string()
            )),
            ExecutionTiming::Unavailable
        );
        assert_eq!(
            execution_timing("active", "unknown", true, || panic!("unknown owner")),
            ExecutionTiming::Unavailable
        );
        assert_eq!(
            execution_timing("unknown", "deterministic", true, || panic!("unknown state")),
            ExecutionTiming::Unavailable
        );
    }

    #[test]
    fn overdue_work_is_not_rescheduled_from_read_time_and_paused_entries_keep_existing_timing() {
        for state in ["active", "paused"] {
            assert_eq!(
                execution_timing(state, "deterministic", true, || Ok(Some(
                    ScheduleObservation {
                        state: "pending".to_string(),
                        due_at_ms: 1_000
                    }
                ))),
                ExecutionTiming::Scheduled(1_000)
            );
        }
        assert_eq!(
            execution_timing("active", "deterministic", false, || panic!(
                "disabled scheduler"
            )),
            ExecutionTiming::NotScheduled
        );
    }

    #[test]
    fn pending_and_inflight_work_retain_the_original_due_time() {
        for state in ["pending", "leased"] {
            let record = ScheduleObservation {
                state: state.to_string(),
                due_at_ms: 1_798_000_000_000,
            };
            assert_eq!(record.next_due_at_ms(), Some(1_798_000_000_000));
        }
    }

    #[test]
    fn completed_canceled_unknown_and_invalid_records_do_not_claim_a_next_tick() {
        for state in ["complete", "canceled", "unknown", ""] {
            assert_eq!(
                ScheduleObservation {
                    state: state.to_string(),
                    due_at_ms: 100,
                }
                .next_due_at_ms(),
                None
            );
        }
        assert_eq!(
            ScheduleObservation {
                state: "pending".to_string(),
                due_at_ms: -1,
            }
            .next_due_at_ms(),
            None
        );
    }
}
