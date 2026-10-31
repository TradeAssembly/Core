//! Additive typed execution preparation; normal activation owns the atomic run.
use super::*;
use crate::service::{data_plans::PlanScope, order_plans};
use serde::{Deserialize, Serialize};
pub(crate) const VERSION: &str = "tradeassembly.typed_execution.v2";
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConfigCommand {
    pub scope: PlanScope,
    pub capability_graph_revision: String,
    pub broker_requirement_ref: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub automatic_execution: Option<super::typed_cycle::AutomaticConfig>,
}
pub(crate) fn is_typed(v: &Value) -> bool {
    v["executionContractVersion"] == VERSION
}
fn text<'a>(v: &'a Value, k: &str) -> Result<&'a str, String> {
    v[k].as_str()
        .filter(|s| !s.is_empty())
        .ok_or_else(|| format!("typed_execution_{k}_required"))
}
fn build(s: &TradeAssemblyService, c: &ConfigCommand) -> Result<Value, String> {
    if let Some(a) = &c.automatic_execution {
        a.validate()?;
    }
    let owner = s
        .invocation_owner()
        .ok_or("typed_execution_owner_required")?;
    s.require_object("strategy", &c.scope.strategy_id)
        .map_err(|_| "typed_execution_not_available")?;
    if c.broker_requirement_ref.is_empty() || c.broker_requirement_ref.len() > 128 {
        return Err("typed_execution_broker_requirement_invalid".into());
    }
    let policy = order_plans::published_policy(s, &c.scope)?;
    let version = crate::service::strategy::strategy_versions(s, &c.scope.strategy_id)
        .into_iter()
        .find(|v| {
            v["id"] == c.scope.strategy_version_id && v["specHash"] == c.scope.strategy_spec_hash
        })
        .ok_or("typed_execution_published_version_required")?;
    if version["immutable"] != true
        || version["publishedAt"].as_str().is_none_or(|v| v.is_empty())
        || version["spec"]["stages"]["order_strategy"]["enabled"] != true
        || !version["spec"]["capability_requirements"]["required"]
            .as_array()
            .is_some_and(|rs| {
                rs.iter().any(|r| {
                    r["requirement_id"] == c.broker_requirement_ref
                        && r["stage_refs"]
                            .as_array()
                            .is_some_and(|refs| refs.iter().any(|v| v == "order_strategy"))
                })
            })
    {
        return Err("typed_execution_declared_broker_required".into());
    }

    let managed_exit_policy = order_plans::management::policy(s, &c.scope)?;
    if let Some(automatic) = &c.automatic_execution {
        if let Some(policy) = &managed_exit_policy {
            order_plans::management::validate_automatic_policy(policy)?;
        }
        if managed_exit_policy.is_some() != automatic.management.is_some() {
            return Err("typed_automatic_management_policy_required".into());
        }
    }
    let selected_template = super::super::selection_plans::template(s, &c.scope)?;
    let selected = selected_template.is_some();
    if c.automatic_execution.is_some() {
        let entry_only = if let Some((_, template)) = &selected_template {
            matches!(
                &template.exit,
                crate::selection_strategy::RuleTemplate::IsTrue {
                    value: crate::selection_strategy::OperandTemplate::Boolean { value: false }
                }
            )
        } else {
            super::super::data_strategy::program(s, &c.scope)?.is_some_and(|program| {
                matches!(
                    program.exit,
                    crate::data_strategy::Rule::IsTrue {
                        value: crate::data_strategy::Operand::Boolean { value: false }
                    }
                )
            })
        };
        if managed_exit_policy.is_none()
            && (!entry_only || version["spec"]["stages"]["exit_policy"]["enabled"] != false)
        {
            return Err("typed_cycle_exit_admission_not_supported".into());
        }
    }
    if !selected && super::super::data_strategy::program(s, &c.scope)?.is_none() {
        return Err("typed_execution_program_required".into());
    }
    let now = s.runtime().clock.trusted_now_ms()?;
    let epoch = chrono::DateTime::from_timestamp_millis(now)
        .ok_or("typed_execution_clock_invalid")?
        .to_rfc3339();
    let rev = capability_graph::revision_for_context(
        s,
        &c.capability_graph_revision,
        &c.scope.strategy_id,
        &c.scope.strategy_version_id,
        "paper",
        &epoch,
    )
    .map_err(|_| "typed_execution_graph_stale")?;
    if rev["strategySpecHash"] != c.scope.strategy_spec_hash || rev["graph"]["ok"] != true {
        return Err("typed_execution_graph_stale".into());
    }
    let node = rev["graph"]["nodes"]
        .as_array()
        .and_then(|ns| ns.iter().find(|n| n["nodeId"] == c.broker_requirement_ref))
        .ok_or("typed_execution_broker_unresolved")?;
    let binding = &node["selected"];
    if binding["operationId"] != crate::broker_submission::v2::SUBMIT_OPERATION
        || binding["operation"]["capability"] != crate::broker_submission::v2::SUBMIT_CAPABILITY
        || binding["operation"]["effect"] != "write"
    {
        return Err("typed_execution_v2_broker_required".into());
    }
    let witness = crate::broker_submission::account_identity::current(
        s.runtime().plugins.as_ref(),
        s.runtime().plugin_packages.as_ref(),
        s.runtime().credentials.as_ref(),
        binding,
        now,
        "paper",
    )?;
    let package = s
        .runtime()
        .plugin_packages
        .get(text(
            &s.runtime()
                .plugins
                .get_instance(text(binding, "pluginInstanceRef")?)?
                .ok_or("typed_execution_instance_missing")?,
            "activePackageSha256",
        )?)?
        .ok_or("typed_execution_package_missing")?;
    if !package.manifest["operations"]
        .as_array()
        .is_some_and(|ops| {
            ops.iter().any(|o| {
                o["id"] == crate::broker_submission::v2::LOOKUP_OPERATION
                    && o["capability"] == crate::broker_submission::v2::LOOKUP_CAPABILITY
                    && o["effect"] == "read"
            })
        })
    {
        return Err("typed_execution_lookup_required".into());
    }
    let id = format!(
        "cfg_typed_{}",
        typed_hash(
            &json!({"command":c,"owner":owner,"account":witness.identity,"brokerBinding":witness.binding_sha256,"policy":policy})
        )?
    );
    let mut result = json!({"kind":"ExecutionConfig","configId":id,"executionContractVersion":VERSION,"typedExecution":c,"orchestrator":"typed_pipeline","strategyId":c.scope.strategy_id,"strategyVersionId":c.scope.strategy_version_id,"strategySpecHash":c.scope.strategy_spec_hash,"capabilityGraphRevisionId":c.capability_graph_revision,"capabilityGraphFingerprint":rev["graphFingerprint"],"mode":"paper","accountRef":witness.account_ref,"providerRef":binding["pluginRef"],"canonicalAccount":witness.identity,"brokerAccountBindingSha256":witness.binding_sha256,"orderPlanPolicySha256":typed_hash(&policy)?,"orderPlanBrokerNode":c.broker_requirement_ref,"selectionPolicy":if selected{"selected_receipt_head"}else{"published_static"},"symbol":Value::Null,"timeframe":Value::Null,"calendarRef":Value::Null,"dataFreshnessPolicy":{"maxAgeMs":policy.max_snapshot_age_ms},"riskLimits":{"typedPolicySha256":typed_hash(&policy)?},"schedulerIntervalSeconds":60,"warmup":{"source":"typed_data_plan_readiness"},"status":"configured"});
    if let Some(p) = managed_exit_policy {
        result["managedExitPolicySha256"] = json!(typed_hash(&p)?);
    }
    Ok(result)
}
pub(super) fn save(s: &TradeAssemblyService, body: &Value) -> Value {
    let result = (|| {
        let c: ConfigCommand = serde_json::from_value(body["typedExecution"].clone())
            .map_err(|_| "typed_execution_config_invalid")?;
        if body["strategyId"] != c.scope.strategy_id
            || body["accountMode"] != "paper"
            || body["mode"] != "paper"
        {
            return Err("typed_execution_scope_or_mode_mismatch".into());
        }
        let config = build(s, &c)?;
        let id = text(&config, "configId")?;
        s.bind_inherited_object("execution_config", id, "strategy", &c.scope.strategy_id)
            .map_err(|_| "typed_execution_not_available")?;
        let ctx = control_side_effect_context(body, "typed-config");
        s.runtime()
            .storage
            .put_json_if_absent(CONFIGS_NS, id, config.clone(), &ctx)?;
        Ok::<_, String>(
            json!({"configId":id,"id":id,"item":config,"readiness":readiness(s,&config)}),
        )
    })();
    result.unwrap_or_else(|code| json!({"status":"blocked","error":{"code":code}}))
}
pub(in crate::service) fn current(
    s: &TradeAssemblyService,
    config: &Value,
) -> Result<Value, String> {
    let c: ConfigCommand = serde_json::from_value(config["typedExecution"].clone())
        .map_err(|_| "typed_execution_config_invalid")?;
    s.require_object("execution_config", text(config, "configId")?)
        .map_err(|_| "typed_execution_not_available")?;
    let fresh = build(s, &c)?;
    if fresh != *config {
        return Err("typed_execution_config_stale".into());
    }
    Ok(fresh)
}
pub(super) fn readiness(s: &TradeAssemblyService, config: &Value) -> Value {
    let result = current(s, config).and_then(|_| {
        let (issuer, subject) = s
            .responsible_human_identity()
            .ok_or("typed_execution_human_required")?;
        let now = s.runtime().clock.trusted_now_ms()?;
        let identity = s.runtime().identity.resolve(subject, now)?;
        if identity.issuer != issuer || identity.expires_at_ms <= now {
            return Err("typed_execution_human_stale".into());
        }
        Ok(())
    });
    match result {
        Ok(()) => {
            let mut result = json!({"ready":true,"blockedReasons":[],"capabilityGraphRevision":s.runtime().storage.get_json("capability_graph_revisions",config["capabilityGraphRevisionId"].as_str().unwrap_or_default()).ok().flatten(),"executionContractVersion":VERSION,"continuousExecution":false,"entryDecisionRequired":true});
            if !config["typedExecution"]["automaticExecution"].is_null() {
                result["scheduledFinancialEntrySupported"] = json!(true);
                let management =
                    !config["typedExecution"]["automaticExecution"]["management"].is_null();
                result["automaticExecutionScope"] = json!(if management {
                    "same_occurrence_entry_and_owned_position_management"
                } else {
                    "entry_only"
                });
                result["syntheticExitAdmissionSupported"] = json!(management);
                result["automaticReversalSupported"] = json!(false);
                result["nextSessionRolloverSupported"] = json!(false);
                result["localSessionWorkerStartRequired"] = json!(management);
            }
            result
        }
        Err(code) => {
            json!({"ready":false,"blockedReasons":[{"code":code}],"executionContractVersion":VERSION})
        }
    }
}

fn typed_hash<T: Serialize>(v: &T) -> Result<String, String> {
    crate::data_pipeline::canonical_sha256(v).map_err(|_| "typed_execution_hash_invalid".into())
}
