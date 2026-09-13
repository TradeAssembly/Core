// Copyright (c) 2026 OptionLab LLC. All rights reserved.

use super::{ServiceResponse, TradeAssemblyService};
use crate::ports::{AuthorityContext, IdempotencyKey, ObjectOwner, ObjectScope, SideEffectContext};
use serde_json::Value;

pub(super) fn authorize_private_request(
    service: &TradeAssemblyService,
    method: &str,
    path: &str,
    body: &Value,
) -> Option<ServiceResponse> {
    service.invocation_owner()?;
    let body = body
        .get("request")
        .filter(|request| request.is_object())
        .unwrap_or(body);
    let method = method.to_ascii_uppercase();
    let path = super::normalize_path(path);

    let nested_strategy_id = body
        .get("scope")
        .and_then(|scope| string_field(scope, &["strategyId", "strategy_id"]));
    let strategy_id = if method == "GET" && path.starts_with("/strategies/") {
        path.trim_start_matches("/strategies/")
            .split('/')
            .next()
            .filter(|value| !value.is_empty())
            .map(str::to_string)
    } else if path.starts_with("/product/strategies/")
        && !matches!(
            path.as_str(),
            "/product/strategies/create"
                | "/product/strategies/home"
                | "/product/strategies/ai-draft"
        )
    {
        string_field(body, &["strategyId", "strategy_id"]).or(nested_strategy_id)
    } else {
        string_field(body, &["strategyId", "strategy_id"])
            .or(nested_strategy_id)
            .filter(|_| {
                matches!(
                    path.as_str(),
                    "/dataset-ingestions"
                        | "/backtests"
                        | "/robustness-runs"
                        | "/derivatives-analyses"
                        | "/research-comparisons"
                        | "/product/strategy-execution-configs/save"
                        | "/product/strategy-execution-configs/activation-readiness"
                        | "/product/strategy-execution-activations/activate"
                        | "/product/sightline/proposals/create"
                )
            })
    };
    if let Some(response) = guard(service, "strategy", strategy_id.clone()) {
        return Some(response);
    }

    if matches!(
        path.as_str(),
        "/product/strategies/proposals/review" | "/product/strategies/proposals/apply"
    ) {
        if let Some(response) = guard_child(
            service,
            "strategy_proposal",
            string_field(body, &["proposalId", "proposal_id"]),
            "strategy",
            strategy_id,
            true,
        ) {
            return Some(response);
        }
    }

    if method == "POST" && path == "/research-comparisons" {
        for source_run_id in string_array_field(body, &["sourceRunIds", "source_run_ids"]) {
            if !service.object_is_visible("backtest_run", &source_run_id)
                && !service.object_is_visible("robustness_run", &source_run_id)
            {
                return Some(ServiceResponse::object_unavailable());
            }
        }
    }

    let dataset_id = path_id(&path, "/dataset-ingestions/").or_else(|| {
        string_field(
            body,
            &["datasetId", "dataset_id", "ingestionId", "ingestion_id"],
        )
    });
    if path.starts_with("/dataset-ingestions/") {
        if let Some(response) = guard(service, "dataset_ingestion", dataset_id) {
            return Some(response);
        }
    }

    for (prefix, object_type, fields) in [
        (
            "/backtests/",
            "backtest_run",
            &[
                "runId",
                "run_id",
                "backtestId",
                "backtest_id",
                "sourceRunId",
                "source_run_id",
            ][..],
        ),
        (
            "/robustness-runs/",
            "robustness_run",
            &["runId", "run_id", "robustnessRunId"][..],
        ),
        (
            "/derivatives-analyses/",
            "derivatives_analysis",
            &["analysisId", "analysis_id"][..],
        ),
        (
            "/research-comparisons/",
            "research_comparison",
            &["comparisonId", "comparison_id"][..],
        ),
        (
            "/plugins/instances/",
            "plugin_instance",
            &["instanceRef", "instance_ref"][..],
        ),
        ("/exports/", "export", &["exportId", "export_id"][..]),
    ] {
        if path.starts_with(prefix) {
            let object_id = path_id(&path, prefix).or_else(|| string_field(body, fields));
            if let Some(response) = guard(service, object_type, object_id) {
                return Some(response);
            }
        }
    }

    for (process_path, object_type, fields) in [
        (
            "/backtests:process",
            "backtest_run",
            &["runId", "run_id"][..],
        ),
        (
            "/robustness-runs:process",
            "robustness_run",
            &["runId", "run_id"][..],
        ),
    ] {
        if method == "POST" && path == process_path {
            let object_id = string_field(body, fields);
            if object_id.is_none() {
                return Some(ServiceResponse::object_unavailable());
            }
            if let Some(response) = guard(service, object_type, object_id) {
                return Some(response);
            }
        }
    }

    if matches!(
        path.as_str(),
        "/product/providers/update-instance"
            | "/product/providers/enable-instance"
            | "/product/providers/disable-instance"
            | "/product/providers/credentials/test"
    ) || path.ends_with("/configuration")
        || path.ends_with("/credentials")
        || path.ends_with("/credentials/test")
    {
        let legacy_provider_ref = path
            .strip_prefix("/providers/")
            .and_then(|suffix| suffix.strip_suffix("/credentials/test"))
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string);
        if let Some(response) = guard(
            service,
            "plugin_instance",
            string_field(
                body,
                &["instanceRef", "instance_ref", "providerRef", "provider_ref"],
            )
            .or(legacy_provider_ref),
        ) {
            return Some(response);
        }
    }

    if matches!(
        path.as_str(),
        "/product/strategy-execution-configs/activation-readiness"
            | "/product/strategy-execution-activations/activate"
    ) {
        if let Some(response) = guard(
            service,
            "execution_config",
            string_field(body, &["configId", "config_id"]),
        ) {
            return Some(response);
        }
    }

    if path.starts_with("/product/strategy-execution-activations/")
        && path != "/product/strategy-execution-activations/activate"
    {
        let path_activation_id = path_id(&path, "/product/strategy-execution-activations/")
            .filter(|value| !matches!(value.as_str(), "activate" | "control" | "deactivate"));
        if let Some(response) = guard(
            service,
            "execution_activation",
            string_field(body, &["activationId", "activation_id"]).or(path_activation_id),
        ) {
            return Some(response);
        }
    }

    if path.starts_with("/journal/") || path.contains("attribution-journal") {
        if let Some(response) = guard(
            service,
            "execution_activation",
            string_field(body, &["activationId", "activation_id"]),
        ) {
            return Some(response);
        }
        if let Some(response) = guard(
            service,
            "strategy",
            string_field(body, &["strategyId", "strategy_id"]),
        ) {
            return Some(response);
        }
    }

    None
}

impl TradeAssemblyService {
    pub(crate) fn claim_local_seed_strategies(&self) {
        for strategy_id in ["strat_local_btc_demo", "strat_local_crypto_replay"] {
            let _ = self.bind_owned_object("strategy", strategy_id);
        }
    }

    pub(crate) fn claim_local_seed_plugin_instances(&self) {
        for instance_ref in ["sim", "local-data", "alpaca-paper"] {
            let _ = self.bind_owned_object("plugin_instance", instance_ref);
            let _ = self.bind_inherited_object(
                "credential_reference",
                instance_ref,
                "plugin_instance",
                instance_ref,
            );
        }
    }

    pub(crate) fn invocation_owner(&self) -> Option<ObjectOwner> {
        self.invocation_principal
            .as_ref()
            .map(|principal| ObjectOwner {
                issuer: principal.issuer.clone(),
                subject: principal.subject.clone(),
                tenant_ref: format!("personal:{}:{}", principal.issuer, principal.subject),
            })
    }

    pub(crate) fn bind_owned_object(
        &self,
        object_type: &str,
        object_id: &str,
    ) -> Result<(), String> {
        let Some(owner) = self.invocation_owner() else {
            return Ok(());
        };
        self.bind_object_scope(ObjectScope {
            object_type: object_type.to_string(),
            object_id: object_id.to_string(),
            owner,
            parent_type: None,
            parent_id: None,
        })
    }

    pub(crate) fn bind_inherited_object(
        &self,
        object_type: &str,
        object_id: &str,
        parent_type: &str,
        parent_id: &str,
    ) -> Result<(), String> {
        if self.invocation_owner().is_none() {
            return Ok(());
        }
        let parent = self
            .runtime
            .object_authorization
            .scope(parent_type, parent_id)?
            .ok_or_else(|| "object_not_available".to_string())?;
        if !self.owner_matches(&parent.owner) {
            return Err("object_not_available".to_string());
        }
        self.bind_object_scope(ObjectScope {
            object_type: object_type.to_string(),
            object_id: object_id.to_string(),
            owner: parent.owner,
            parent_type: Some(parent_type.to_string()),
            parent_id: Some(parent_id.to_string()),
        })
    }

    pub(crate) fn require_object(
        &self,
        object_type: &str,
        object_id: &str,
    ) -> Result<(), ServiceResponse> {
        let Some(_) = self.invocation_owner() else {
            return Ok(());
        };
        let scope = self
            .runtime
            .object_authorization
            .scope(object_type, object_id)
            .map_err(|_| ServiceResponse::internal_error("object authorization unavailable"))?;
        if scope.is_some_and(|scope| self.owner_matches(&scope.owner)) {
            Ok(())
        } else {
            Err(ServiceResponse::object_unavailable())
        }
    }

    pub(crate) fn object_is_visible(&self, object_type: &str, object_id: &str) -> bool {
        let Some(_) = self.invocation_owner() else {
            return true;
        };
        self.runtime
            .object_authorization
            .scope(object_type, object_id)
            .ok()
            .flatten()
            .is_some_and(|scope| self.owner_matches(&scope.owner))
    }

    pub(crate) fn filter_visible_values(
        &self,
        object_type: &str,
        values: Vec<Value>,
        id_fields: &[&str],
    ) -> Vec<Value> {
        if self.invocation_owner().is_none() {
            return values;
        }
        values
            .into_iter()
            .filter(|value| {
                id_fields
                    .iter()
                    .find_map(|field| value.get(*field).and_then(Value::as_str))
                    .is_some_and(|id| self.object_is_visible(object_type, id))
            })
            .collect()
    }

    fn bind_object_scope(&self, scope: ObjectScope) -> Result<(), String> {
        let context = SideEffectContext::new(
            AuthorityContext {
                actor: scope.owner.subject.clone(),
                surface: "object_authorization".to_string(),
                account_mode: "paper".to_string(),
            },
            IdempotencyKey::new(format!(
                "object-scope:{}:{}",
                scope.object_type, scope.object_id
            ))?,
        );
        self.runtime
            .object_authorization
            .bind(&scope, &context)
            .map(|_| ())
    }

    fn owner_matches(&self, owner: &ObjectOwner) -> bool {
        self.invocation_owner()
            .is_some_and(|candidate| candidate == *owner)
    }
}

fn string_field(value: &Value, fields: &[&str]) -> Option<String> {
    fields
        .iter()
        .find_map(|field| value.get(*field).and_then(Value::as_str))
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

fn string_array_field(value: &Value, fields: &[&str]) -> Vec<String> {
    fields
        .iter()
        .find_map(|field| value.get(*field).and_then(Value::as_array))
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .collect()
}

fn path_id(path: &str, prefix: &str) -> Option<String> {
    path.strip_prefix(prefix)
        .and_then(|suffix| suffix.split('/').next())
        .and_then(|segment| segment.split(':').next())
        .map(str::trim)
        .filter(|value| !value.is_empty() && !value.contains(['{', '}']))
        .map(str::to_string)
}

fn guard(
    service: &TradeAssemblyService,
    object_type: &str,
    object_id: Option<String>,
) -> Option<ServiceResponse> {
    object_id.and_then(|id| service.require_object(object_type, &id).err())
}

fn guard_child(
    service: &TradeAssemblyService,
    object_type: &str,
    object_id: Option<String>,
    parent_type: &str,
    parent_id: Option<String>,
    require_existing: bool,
) -> Option<ServiceResponse> {
    let _ = service.invocation_owner()?;
    let (Some(object_id), Some(parent_id)) = (object_id, parent_id) else {
        return require_existing.then(ServiceResponse::object_unavailable);
    };
    match service
        .runtime
        .object_authorization
        .scope(object_type, &object_id)
    {
        Ok(Some(scope))
            if service.owner_matches(&scope.owner)
                && scope.parent_type.as_deref() == Some(parent_type)
                && scope.parent_id.as_deref() == Some(parent_id.as_str()) =>
        {
            None
        }
        Ok(None) if !require_existing => None,
        Ok(_) => Some(ServiceResponse::object_unavailable()),
        Err(_) => Some(ServiceResponse::internal_error(
            "object authorization unavailable",
        )),
    }
}
