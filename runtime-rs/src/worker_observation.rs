// Copyright (c) 2026 OptionLab LLC. All rights reserved.

//! Bounded read-only worker evidence, never an execution/admission grant.
use crate::ports::LeaseClaim;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WorkerObservation {
    pub schema_version: String,
    pub observation_id: String,
    pub activation_id: String,
    pub process_boot_id: String,
    pub worker_id: String,
    pub state: String,
    pub observed_at_ms: i64,
    pub expires_at_ms: i64,
}

pub fn project_worker(
    activation_id: &str,
    process_boot_id: &str,
    now_ms: i64,
    lease_resource: &str,
    observation: Result<Option<WorkerObservation>, String>,
    lease: Result<Option<LeaseClaim>, String>,
) -> Value {
    let unavailable = || json!({"status":"unavailable","reason":"worker_observation_unavailable"});
    let observation = match observation {
        Ok(Some(record))
            if record.activation_id == activation_id
                && record.schema_version == "tradeassembly.worker_observation.v1" =>
        {
            record
        }
        _ => return unavailable(),
    };
    // A previous process's record is diagnostic history, never current liveness.
    if observation.process_boot_id != process_boot_id {
        return json!({"status":"recovering","reason":"previous_process_observation"});
    }
    if observation.worker_id.is_empty()
        || observation.observation_id.is_empty()
        || observation.observed_at_ms < 0
        || observation.observed_at_ms > now_ms
        || observation.expires_at_ms <= observation.observed_at_ms
        || !matches!(
            observation.state.as_str(),
            "waiting" | "evaluating" | "stopped"
        )
    {
        return unavailable();
    }
    let lease = match lease {
        Ok(Some(claim)) if claim.resource != lease_resource => json!({"status":"unavailable"}),
        Ok(Some(claim)) if claim.expires_at_ms > now_ms => json!({
            "status":"held","owner":claim.owner,"fencingToken":claim.fencing_token,
            "expiresAtMs":claim.expires_at_ms,
            "matchesWorker":claim.owner == observation.worker_id,
        }),
        Ok(Some(claim)) => json!({"status":"expired","expiresAtMs":claim.expires_at_ms}),
        Ok(None) => json!({"status":"idle"}),
        Err(_) => json!({"status":"unavailable"}),
    };
    let status = if observation.state == "stopped" {
        "stopped"
    } else if observation.expires_at_ms <= now_ms {
        "stale"
    } else if lease["status"] == "unavailable" {
        "unavailable"
    } else if observation.state == "evaluating"
        && lease["status"] == "held"
        && lease["matchesWorker"] == true
    {
        "evaluating"
    } else {
        "observed"
    };
    json!({
        "status":status,
        "observedAtMs":observation.observed_at_ms,
        "expiresAtMs":observation.expires_at_ms,
        "observationId":observation.observation_id,
        "processBootId":observation.process_boot_id,
        "workerId":observation.worker_id,
        "observedState":observation.state,
        "evaluationLease":lease,
        "reason": match status {
            "stale" => "worker_check_in_expired",
            "unavailable" => "evaluation_lease_unavailable",
            "stopped" => "worker_loop_stopped",
            _ => "recent_worker_check_in",
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    fn observation() -> WorkerObservation {
        WorkerObservation {
            schema_version: "tradeassembly.worker_observation.v1".into(),
            observation_id: "observation-1".into(),
            activation_id: "activation-1".into(),
            process_boot_id: "boot-current".into(),
            worker_id: "worker-current".into(),
            state: "waiting".into(),
            observed_at_ms: 1_000,
            expires_at_ms: 2_000,
        }
    }
    fn project(
        record: WorkerObservation,
        now: i64,
        lease: Result<Option<LeaseClaim>, String>,
    ) -> Value {
        project_worker(
            "activation-1",
            "boot-current",
            now,
            "tick",
            Ok(Some(record)),
            lease,
        )
    }
    #[test]
    fn waiting_without_a_tick_lease_is_a_recent_check_in_not_proof_of_execution() {
        let value = project(observation(), 1_500, Ok(None));
        assert_eq!(value["status"], "observed");
        assert_eq!(value["evaluationLease"]["status"], "idle");
    }
    #[test]
    fn expiry_is_inclusive_and_clock_rollback_is_unavailable() {
        assert_eq!(project(observation(), 2_000, Ok(None))["status"], "stale");
        assert_eq!(
            project(observation(), 999, Ok(None))["status"],
            "unavailable"
        );
    }
    #[test]
    fn restart_and_foreign_activation_do_not_leak_or_claim_current_evidence() {
        let mut record = observation();
        record.process_boot_id = "previous-boot".into();
        let value = project(record, 1_500, Ok(None));
        assert_eq!(
            value,
            json!({"status":"recovering","reason":"previous_process_observation"})
        );
        let mut record = observation();
        record.activation_id = "foreign-activation".into();
        let value = project(record, 1_500, Ok(None));
        assert_eq!(value["status"], "unavailable");
        assert!(value.get("workerId").is_none());
    }
    #[test]
    fn evaluation_requires_current_matching_unexpired_lease() {
        let mut record = observation();
        record.state = "evaluating".into();
        let lease = LeaseClaim {
            resource: "tick".into(),
            owner: record.worker_id.clone(),
            fencing_token: 7,
            expires_at_ms: 1_800,
        };
        assert_eq!(
            project(record.clone(), 1_500, Ok(Some(lease.clone())))["status"],
            "evaluating"
        );
        assert_eq!(
            project(record.clone(), 1_800, Ok(Some(lease.clone())))["evaluationLease"]["status"],
            "expired"
        );
        let mut foreign = lease;
        foreign.owner = "another-worker".into();
        assert_eq!(
            project(record.clone(), 1_500, Ok(Some(foreign)))["status"],
            "observed"
        );
        assert_eq!(
            project(record, 1_500, Err("adapter unavailable".into()))["status"],
            "unavailable"
        );
    }
    #[test]
    fn missing_or_invalid_observations_fail_closed() {
        assert_eq!(
            project_worker(
                "activation-1",
                "boot-current",
                1_500,
                "tick",
                Ok(None),
                Ok(None)
            )["status"],
            "unavailable"
        );
        let mut record = observation();
        record.expires_at_ms = record.observed_at_ms;
        assert_eq!(project(record, 1_500, Ok(None))["status"], "unavailable");
    }
    #[test]
    fn explicit_stop_wins_over_recent_timestamps() {
        let mut record = observation();
        record.state = "stopped".into();
        assert_eq!(project(record, 1_500, Ok(None))["status"], "stopped");
    }
}
