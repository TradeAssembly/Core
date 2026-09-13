use super::{api_result, execution, TradeAssemblyService};
use serde_json::{json, Value};

pub(crate) fn status(service: &TradeAssemblyService) -> Value {
    execution::scheduler_status(service)
}

pub(crate) fn start(service: &TradeAssemblyService, body: Value) -> Value {
    api_result(execution::scheduler_start(service, body))
}

pub(crate) fn stop(service: &TradeAssemblyService) -> Value {
    api_result(execution::scheduler_stop(service))
}

pub(crate) fn run(service: &TradeAssemblyService, body: Value) -> Value {
    let response = execution::scheduler_run(service, body);
    if response.get("lease").is_none() {
        return api_result(json!({
            "cycles": response.get("cycles").cloned().unwrap_or(json!(1)),
            "iterations": response.get("iterations").cloned().unwrap_or(json!(1)),
            "status": response.get("status").cloned().unwrap_or(json!("complete")),
            "lease": {"acquired": false, "duplicate": false, "idle": true},
            "ticks": response.get("ticks").cloned().unwrap_or(json!([])),
        }));
    }
    api_result(response)
}
