use super::{scheduler, TradeAssemblyService};
use serde_json::Value;

pub(crate) fn status(service: &TradeAssemblyService) -> Value {
    scheduler::status(service)
}

pub(crate) fn run(service: &TradeAssemblyService, body: Value) -> Value {
    scheduler::run(service, body)
}
