// Copyright (c) 2026 OptionLab LLC. All rights reserved.

use super::{broker_onboarding, scheduler, TradeAssemblyService};
use serde_json::{json, Value};

pub(super) fn snapshot(service: &TradeAssemblyService) -> Value {
    let instances = broker_onboarding::status(service, json!({}));
    let connections = instances["instances"]
        .as_array()
        .map(|items| {
            items.iter().map(|item| json!({
            "instanceRef": item["instanceRef"],
            "pluginRef": item["pluginRef"],
            "configured": item["credentialStatus"]["configured"].as_bool().unwrap_or(false),
            "enabled": item["enabled"],
            "accountVerified": item["accountVerified"],
            "verificationStale": item["verificationStale"],
            "verificationReceipt": item["verificationReceipt"],
            "restartObservation": item["restartObservation"],
            "studioObservation": item["studioObservation"],
            "health": item["health"],
            "mode": item["mode"],
            "nextAction": item["nextAction"],
        })).collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let deployments = crate::agent_runner::deployments(&service.runtime());
    let scheduler_record = scheduler::status(service);
    let scheduler_recorded_running = scheduler_record["running"].as_bool().unwrap_or(false);
    let (deployment_count, active_count, readable) = match deployments {
        Ok(items) => (
            items.len(),
            items
                .iter()
                .filter(|item| item.desired_state == "active")
                .count(),
            true,
        ),
        Err(_) => (0, 0, false),
    };
    json!({
        "connections": connections,
        "automation": {
            "inspected": readable,
            "deploymentCount": if readable { json!(deployment_count) } else { Value::Null },
            "activeDeploymentCount": if readable { json!(active_count) } else { Value::Null },
            "startedBySetup": false,
            "runtimeVerified": false,
            "schedulerRecordedRunning": scheduler_recorded_running,
        },
        "existingState": !connections.is_empty() || deployment_count > 0 || scheduler_recorded_running,
    })
}
