//! Authenticated observation of an original, possibly accepted broker command.
use super::{ServiceResponse, TradeAssemblyService};
use crate::broker_submission::{
    prepare_recovery_plan, BrokerSubmissionDependencies, RecoveryObserver,
};
use crate::local_owner_identity::LocalOwnerIdentity;
use crate::ports::{AuthorityContext, IdempotencyKey, SideEffectContext};
use serde::Deserialize;
use serde_json::{json, Value};

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RecoveryRequest {
    original_idempotency_key: String,
    recovery_idempotency_key: String,
}

pub(super) fn valid_request(body: &Value) -> bool {
    serde_json::from_value::<RecoveryRequest>(body.clone()).is_ok()
}

pub(super) fn recover(service: &TradeAssemblyService, body: Value) -> ServiceResponse {
    // Raw caller fields were checked before the service attached authenticated
    // transport metadata. Never interpret that metadata as recovery parameters.
    let Ok(request) = serde_json::from_value::<RecoveryRequest>(json!({
        "originalIdempotencyKey":body["originalIdempotencyKey"],
        "recoveryIdempotencyKey":body["recoveryIdempotencyKey"]
    })) else {
        return ServiceResponse::error(400, "broker_recovery_request_invalid", false);
    };
    let (Ok(_), Ok(key)) = (
        IdempotencyKey::new(&request.original_idempotency_key),
        IdempotencyKey::new(&request.recovery_idempotency_key),
    ) else {
        return ServiceResponse::error(400, "broker_recovery_key_invalid", false);
    };
    if request.original_idempotency_key == request.recovery_idempotency_key {
        return ServiceResponse::error(400, "broker_recovery_distinct_key_required", false);
    }
    let observer = if let Some(agent) = service.agent_mcp_execution_context() {
        RecoveryObserver::Agent(agent.clone())
    } else if let Some((issuer, subject)) = service.responsible_human_identity() {
        RecoveryObserver::Owner {
            issuer: issuer.into(),
            subject: subject.into(),
        }
    } else {
        return ServiceResponse::error(
            401,
            "broker_recovery_authenticated_observer_required",
            false,
        );
    };
    let Ok(owner) = LocalOwnerIdentity::for_database(std::path::Path::new(service.db())) else {
        return ServiceResponse::error(403, "broker_recovery_owner_unavailable", false);
    };
    let runtime = service.runtime();
    let deps = BrokerSubmissionDependencies {
        storage: runtime.storage.clone(),
        plugins: runtime.plugins.clone(),
        plugin_packages: runtime.plugin_packages.clone(),
        credentials: runtime.credentials.clone(),
        capability_resolver: runtime.capability_resolver.clone(),
        clock: runtime.clock.clone(),
        leases: runtime.leases.clone(),
        owner,
    };
    let plan = match prepare_recovery_plan(deps, &request.original_idempotency_key, observer) {
        Ok(plan) => plan,
        Err(_) => {
            return ServiceResponse::error(403, "broker_recovery_not_authorized_or_bound", false)
        }
    };
    let context = SideEffectContext::new(
        AuthorityContext {
            actor: "authenticated_broker_observer".into(),
            surface: "broker_recovery".into(),
            account_mode: "live".into(),
        },
        key,
    );
    match runtime
        .plugin_operations
        .recover_ambiguous_broker_order(&plan, &context)
    {
        Ok(receipt) => ServiceResponse::ok(json!({"state":"observed", "receipt":receipt})),
        Err(_) => ServiceResponse::error(409, "plugin_order_reconciliation_required", false),
    }
}
