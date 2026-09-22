// Copyright (c) 2026 OptionLab LLC. All rights reserved.

use crate::auth;
use serde_json::{json, Map, Value};

pub const MCP_PROTOCOL_VERSION: &str = "2025-11-25";
pub const DEFAULT_DB: &str = ".tradeassembly/tradeassembly.db";
pub const DEFAULT_STUDIO_BASE_URL: &str = "http://127.0.0.1:3001";

const VERSION: &str = "0.1.0";
const NO_ADVICE_NOTICE: &str = "TradeAssembly provides deterministic machinery, local journaling, broker/data interfaces, and replay tools. It does not tell users what to trade, when to trade, or how much to trade.";
const SENSITIVE_KEYS: &[&str] = &[
    "access_token",
    "api_key",
    "api_secret",
    "authorization",
    "bearer",
    "client_secret",
    "code_verifier",
    "password",
    "private_key",
    "refresh_token",
    "secret",
    "token",
];

#[derive(Debug, PartialEq, Eq)]
pub struct StdioResult {
    pub exit_code: i32,
    pub stdout: String,
    pub stderr: String,
}

#[derive(Clone, Debug)]
struct ToolSpec {
    name: &'static str,
    title: &'static str,
    description: &'static str,
    input_schema: Value,
    read_only: bool,
    destructive: bool,
    idempotent: bool,
}

impl ToolSpec {
    fn definition(&self) -> Value {
        json!({
            "name": self.name,
            "title": self.title,
            "description": self.description,
            "inputSchema": self.input_schema,
            "annotations": {
                "readOnlyHint": self.read_only,
                "destructiveHint": self.destructive,
                "idempotentHint": self.idempotent,
                "openWorldHint": false
            }
        })
    }
}

pub fn tool_definitions() -> Value {
    Value::Array(tool_specs().iter().map(ToolSpec::definition).collect())
}

/// Tool discovery for a verified runner child. Generic local MCP uses
/// [`tool_definitions`] unchanged; this narrow surface advertises only the
/// deployment's durable MCP allowlist.
pub fn tool_definitions_for_agent_execution_context(allowlist: &[String]) -> Value {
    Value::Array(
        tool_specs()
            .iter()
            .filter(|tool| allowlist.iter().any(|allowed| allowed == tool.name))
            .map(ToolSpec::definition)
            .collect(),
    )
}

pub fn tool_names() -> Vec<&'static str> {
    tool_specs().into_iter().map(|tool| tool.name).collect()
}

pub fn call_tool(name: &str, arguments: Value) -> Value {
    if matches!(
        name,
        "tradeassembly.agent.session.attach" | "tradeassembly.agent.session.detach"
    ) {
        return tool_error(
            name,
            "external_agent_connection_required",
            "This action requires a persistent MCP stdio connection.",
            None,
        );
    }
    if matches!(
        name,
        "tradeassembly.account.login"
            | "tradeassembly.account.login.status"
            | "tradeassembly.account.status"
            | "tradeassembly.setup.inspect"
            | "tradeassembly.plugin.oauth.start"
            | "tradeassembly.plugin.oauth.status"
            | "tradeassembly.plugin.oauth.disconnect"
    ) || name.starts_with("tradeassembly.broker.")
        || name.starts_with("tradeassembly.onboarding.")
    {
        return tool_error(
            name,
            "service_required",
            "Use the local MCP connection for onboarding.",
            None,
        );
    }
    if !tool_specs().iter().any(|tool| tool.name == name) {
        return tool_error(
            name,
            "unknown_mcp_tool",
            &format!("unknown MCP tool: {name}"),
            Some(redact_sensitive_payload(arguments)),
        );
    }
    if name.starts_with("tradeassembly.robustness.")
        || name.starts_with("tradeassembly.derivatives.")
        || name.starts_with("tradeassembly.comparison.")
        || name == "tradeassembly.scenario.valuation"
        || name.starts_with("tradeassembly.monte_carlo.")
    {
        return tool_error(
            name,
            "service_required",
            "Use TradeAssemblyService-backed MCP execution for durable research operations; a request-shaped payload is never a durable research result.",
            Some(redact_sensitive_payload(arguments)),
        );
    }
    let payload = match name {
        "tradeassembly.health" => health_tool(&arguments),
        "tradeassembly.setup.status" => setup_status(&arguments),
        "tradeassembly.account.status" => auth::local_account_status(
            &string_arg(&arguments, "profile", "local"),
            &string_arg(&arguments, "studio_base_url", DEFAULT_STUDIO_BASE_URL),
        ),
        "tradeassembly.account.login" => auth::local_account_login(
            &string_arg(&arguments, "profile", "local"),
            &string_arg(&arguments, "studio_base_url", DEFAULT_STUDIO_BASE_URL),
        ),
        "tradeassembly.install.claim" => auth::local_install_claim(
            &string_arg(&arguments, "profile", "local"),
            arguments.get("install_id").and_then(Value::as_str),
            &string_arg(&arguments, "studio_base_url", DEFAULT_STUDIO_BASE_URL),
        ),
        "tradeassembly.telemetry.emit" => auth::local_telemetry_emit(
            &string_arg(&arguments, "profile", "local"),
            &string_arg(&arguments, "event", "first_backtest"),
            arguments.get("install_id").and_then(Value::as_str),
            &string_arg(&arguments, "studio_base_url", DEFAULT_STUDIO_BASE_URL),
        ),
        "tradeassembly.telemetry.opt_out" => auth::local_telemetry_opt_out(
            &string_arg(&arguments, "profile", "local"),
            arguments
                .get("telemetry_opt_out")
                .and_then(Value::as_bool)
                .unwrap_or(true),
            &string_arg(&arguments, "studio_base_url", DEFAULT_STUDIO_BASE_URL),
        ),
        "tradeassembly.telemetry.replay" => auth::local_telemetry_replay(
            &string_arg(&arguments, "profile", "local"),
            &string_arg(&arguments, "studio_base_url", DEFAULT_STUDIO_BASE_URL),
        ),
        "tradeassembly.studio.link" => studio_link(&arguments),
        "tradeassembly.plugin.status" => plugin_status(&arguments),
        "tradeassembly.indicator.status" => indicator_status(&arguments),
        "tradeassembly.execution.run" => execution_run(&arguments),
        "tradeassembly.execution.replay" => json!({
            "schemaVersion": "tradeassembly.mcp.execution_replay.v1",
            "ok": true,
            "db": string_arg(&arguments, "db", DEFAULT_DB),
            "events": []
        }),
        "tradeassembly.credential.status" => json!({
            "schemaVersion": "tradeassembly.mcp.credential_status.v1",
            "ok": true,
            "providerRef": string_arg(&arguments, "provider_ref", "plugin-instance"),
            "db": string_arg(&arguments, "db", DEFAULT_DB),
            "credentialPosture": "local/customer-managed"
        }),
        "tradeassembly.credential.test" => json!({
            "schemaVersion": "tradeassembly.mcp.credential_test.v1",
            "ok": false,
            "providerRef": string_arg(&arguments, "provider_ref", "plugin-instance"),
            "db": string_arg(&arguments, "db", DEFAULT_DB),
            "message": "credentials not configured"
        }),
        "tradeassembly.strategy.list" => json!({
            "schemaVersion": "tradeassembly.mcp.strategy_list.v1",
            "ok": true,
            "db": string_arg(&arguments, "db", DEFAULT_DB),
            "strategies": []
        }),
        "tradeassembly.strategy.get" => json!({
            "schemaVersion": "tradeassembly.mcp.strategy_get.v1",
            "ok": true,
            "db": string_arg(&arguments, "db", DEFAULT_DB),
            "strategyId": string_arg(&arguments, "strategy_id", "")
        }),
        "tradeassembly.strategy.create" => json!({
            "schemaVersion": "tradeassembly.mcp.strategy_create.v1",
            "ok": true,
            "db": string_arg(&arguments, "db", DEFAULT_DB),
            "mode": string_arg(&arguments, "mode", "blank"),
            "templateId": optional_string_arg(&arguments, "template_id"),
            "name": optional_string_arg(&arguments, "name")
        }),
        "tradeassembly.strategy.save_draft" => json!({
            "schemaVersion": "tradeassembly.mcp.strategy_save_draft.v1",
            "ok": true,
            "db": string_arg(&arguments, "db", DEFAULT_DB),
            "strategyId": optional_string_arg(&arguments, "strategy_id"),
            "name": string_arg(&arguments, "name", "Local strategy"),
            "spec": arguments.get("spec").cloned().unwrap_or_else(|| json!({}))
        }),
        "tradeassembly.strategy.draft.select_node" => json!({
            "schemaVersion": "tradeassembly.mcp.strategy_draft_select_node.v1",
            "ok": true,
            "db": string_arg(&arguments, "db", DEFAULT_DB),
            "strategyId": string_arg(&arguments, "strategy_id", "strat_local_btc_demo"),
            "nodeRef": string_arg(&arguments, "node_ref", "strategy.root")
        }),
        "tradeassembly.strategy.draft.propose_patch" => json!({
            "schemaVersion": "tradeassembly.mcp.strategy_draft_propose_patch.v1",
            "ok": true,
            "db": string_arg(&arguments, "db", DEFAULT_DB),
            "strategyId": string_arg(&arguments, "strategy_id", "strat_local_btc_demo"),
            "actor": {"kind": "agent", "id": "mcp.agent"},
            "patch": arguments.get("patch").or_else(|| arguments.get("specPatch")).cloned().unwrap_or_else(|| json!({}))
        }),
        "tradeassembly.strategy.draft.review_patch" => json!({
            "schemaVersion": "tradeassembly.mcp.strategy_draft_review_patch.v1",
            "ok": false,
            "error": "agent_cannot_apply_strategy_patch"
        }),
        "tradeassembly.strategy.version.publish" => json!({
            "schemaVersion": "tradeassembly.mcp.strategy_version_publish.v1",
            "ok": false,
            "error": "agent_cannot_publish_strategy_version"
        }),
        "tradeassembly.strategy.validate" => json!({
            "schemaVersion": "tradeassembly.mcp.strategy_validate.v1",
            "ok": true,
            "db": string_arg(&arguments, "db", DEFAULT_DB),
            "specFile": string_arg(&arguments, "spec_file", ""),
            "errors": []
        }),
        "tradeassembly.plugin.list" => json!({
            "schemaVersion": "tradeassembly.mcp.plugin_list.v1",
            "ok": true,
            "db": string_arg(&arguments, "db", DEFAULT_DB),
            "plugins": [],
            "entitlementProfile": "local_full"
        }),
        "tradeassembly.plugin.capability_resolve" => json!({
            "schemaVersion": "tradeassembly.mcp.plugin_capability_resolve.v1",
            "ok": false,
            "db": string_arg(&arguments, "db", DEFAULT_DB),
            "requirement": {
                "capability": string_arg(&arguments, "capability", ""),
                "mode": string_arg(&arguments, "mode", "paper"),
                "instrumentType": string_arg(&arguments, "instrument_type", "equity"),
                "operation": string_arg(&arguments, "operation", ""),
                "strategyId": optional_string_arg(&arguments, "strategy_id"),
                "pluginRef": optional_string_arg(&arguments, "plugin_ref"),
                "accountRef": optional_string_arg(&arguments, "account_ref"),
                "purpose": optional_string_arg(&arguments, "purpose")
            },
            "entitlementDecision": {
                "allowed": false,
                "source": "standalone_mcp_helper",
                "profileId": "unavailable"
            },
            "candidates": [],
            "blockers": [{"code": "service_required", "message": "Use TradeAssemblyService-backed MCP execution for capability resolution."}],
            "noSilentFallback": true,
            "noAdviceNotice": NO_ADVICE_NOTICE
        }),
        "tradeassembly.provider.list" => json!({
            "schemaVersion": "tradeassembly.mcp.provider_list.v1",
            "ok": true,
            "db": string_arg(&arguments, "db", DEFAULT_DB),
            "providers": []
        }),
        "tradeassembly.plugin.capability_matrix" => json!({
            "schemaVersion": "tradeassembly.mcp.plugin_capability_matrix.v1",
            "ok": false,
            "db": string_arg(&arguments, "db", DEFAULT_DB),
            "ref": string_arg(&arguments, "ref", ""),
            "capabilities": [],
            "compatible": false,
            "blockers": [{"code": "service_required", "message": "Use TradeAssemblyService-backed MCP execution for plugin capability matrix."}]
        }),
        "tradeassembly.provider.capability_matrix" => json!({
            "schemaVersion": "tradeassembly.mcp.provider_capability_matrix.v1",
            "ok": false,
            "db": string_arg(&arguments, "db", DEFAULT_DB),
            "ref": string_arg(&arguments, "ref", ""),
            "legacyAliasFor": "tradeassembly.plugin.capability_matrix",
            "capabilities": [],
            "compatible": false,
            "blockers": [{"code": "service_required", "message": "Use TradeAssemblyService-backed MCP execution for plugin capability matrix."}]
        }),
        "tradeassembly.provider.pack_compatibility" => json!({
            "schemaVersion": "tradeassembly.mcp.provider_pack_compatibility.v1",
            "ok": false,
            "db": string_arg(&arguments, "db", DEFAULT_DB),
            "ref": string_arg(&arguments, "ref", ""),
            "manifest": arguments.get("manifest").cloned().unwrap_or_else(|| json!({})),
            "capabilities": [],
            "compatible": false,
            "blockers": [{"code": "service_required", "message": "Use TradeAssemblyService-backed MCP execution for plugin provider pack compatibility."}]
        }),
        "tradeassembly.backtest.run"
        | "tradeassembly.backtest.get"
        | "tradeassembly.backtest.list"
        | "tradeassembly.backtest.cancel"
        | "tradeassembly.backtest.retry"
        | "tradeassembly.backtest.process"
        | "tradeassembly.backtest.replay" => json!({
            "ok": false,
            "error": {
                "code": "service_required",
                "message": "Use TradeAssemblyService-backed MCP execution for durable backtest lifecycle operations."
            }
        }),
        "tradeassembly.backtest.export" => json!({
            "schemaVersion": "tradeassembly.mcp.backtest_export.v1",
            "ok": true,
            "db": string_arg(&arguments, "db", DEFAULT_DB),
            "backtestId": string_arg(&arguments, "backtest_id", ""),
            "exportKind": string_arg(&arguments, "export_kind", "reportJson")
        }),
        "tradeassembly.dataset_ingestion.create"
        | "tradeassembly.dataset_ingestion.list"
        | "tradeassembly.dataset_ingestion.get"
        | "tradeassembly.dataset_ingestion.status"
        | "tradeassembly.dataset_ingestion.cancel"
        | "tradeassembly.dataset_ingestion.verify" => json!({
            "ok": false,
            "error": {
                "code": "service_required",
                "message": "Use TradeAssemblyService-backed MCP execution for historical dataset ingestion."
            }
        }),
        "tradeassembly.report.envelope" => json!({
            "schemaVersion": "tradeassembly.mcp.report_envelope.v1",
            "ok": true,
            "db": string_arg(&arguments, "db", DEFAULT_DB),
            "kind": string_arg(&arguments, "kind", "backtest"),
            "studioBaseUrl": string_arg(&arguments, "studio_base_url", DEFAULT_STUDIO_BASE_URL),
            "noAdviceNotice": NO_ADVICE_NOTICE
        }),
        "tradeassembly.scenario.valuation" => json!({
            "ok": false,
            "error": {
                "code": "service_required",
                "message": "Use tradeassembly.derivatives.create with a service-backed exact completed source run."
            }
        }),
        "tradeassembly.portfolio_risk.run"
        | "tradeassembly.portfolio_risk.report"
        | "tradeassembly.portfolio_risk.replay"
        | "tradeassembly.portfolio_risk.export" => json!({
            "schemaVersion": "tradeassembly.mcp.portfolio_risk.v1",
            "ok": true,
            "db": string_arg(&arguments, "db", DEFAULT_DB),
            "strategyId": string_arg(&arguments, "strategy_id", "strat_local_btc_demo"),
            "scopeKind": string_arg(&arguments, "scope_kind", "strategy"),
            "studioBaseUrl": string_arg(&arguments, "studio_base_url", DEFAULT_STUDIO_BASE_URL),
            "noAdviceNotice": NO_ADVICE_NOTICE
        }),
        "tradeassembly.lifecycle_calendar.list"
        | "tradeassembly.lifecycle_calendar.inspect"
        | "tradeassembly.lifecycle_calendar.export"
        | "tradeassembly.lifecycle_calendar.replay" => json!({
            "schemaVersion": "tradeassembly.mcp.lifecycle_calendar.v1",
            "ok": true,
            "db": string_arg(&arguments, "db", DEFAULT_DB),
            "kind": "lifecycle_calendar",
            "strategyId": string_arg(&arguments, "strategy_id", "strat_local_btc_demo"),
            "timelineId": string_arg(&arguments, "timeline_id", "lifecycle_calendar_latest"),
            "studioBaseUrl": string_arg(&arguments, "studio_base_url", DEFAULT_STUDIO_BASE_URL),
            "noAdviceNotice": NO_ADVICE_NOTICE
        }),
        "tradeassembly.journal.list"
        | "tradeassembly.journal.export"
        | "tradeassembly.journal.replay" => {
            return tool_error(
                name,
                "service_required",
                "Use the authenticated MCP connection for journal access.",
                None,
            )
        }
        _ => json!({"ok": true}),
    };
    call_tool_with_payload(name, payload)
}

/// Routes durable research tools through the canonical service HTTP facade.
/// The standalone helper above intentionally never fabricates durable results
/// because it has no service or storage context.
pub fn call_service_tool(
    service: &crate::service::TradeAssemblyService,
    name: &str,
    arguments: Value,
) -> Value {
    let response = match name {
        "tradeassembly.agent.session.attach" | "tradeassembly.agent.session.detach" => {
            return tool_error(
                name,
                "external_agent_connection_required",
                "This action requires a persistent MCP stdio connection.",
                None,
            );
        }
        "tradeassembly.robustness.report" => {
            if !has_only_keys(&arguments, &["run_id", "studio_base_url", "db"]) {
                return tool_error(
                    name,
                    "robustness_arguments_invalid",
                    "Robustness report arguments do not match the tool schema.",
                    None,
                );
            }
            let Some(run_id) = required_nonempty_string(&arguments, "run_id") else {
                return tool_error(
                    name,
                    "robustness_run_id_required",
                    "Robustness run ID is required.",
                    None,
                );
            };
            service.handle_http_from_source(
                "mcp",
                "GET",
                &format!("/robustness-runs/{run_id}/report"),
                json!({}),
            )
        }
        "tradeassembly.robustness.replay" => {
            if !has_only_keys(
                &arguments,
                &[
                    "run_id",
                    "idempotency_key",
                    "actor",
                    "studio_base_url",
                    "db",
                ],
            ) {
                return tool_error(
                    name,
                    "robustness_arguments_invalid",
                    "Robustness replay arguments do not match the tool schema.",
                    None,
                );
            }
            let Some(run_id) = required_nonempty_string(&arguments, "run_id") else {
                return tool_error(
                    name,
                    "robustness_run_id_required",
                    "Robustness run ID is required.",
                    None,
                );
            };
            let Some(idempotency_key) = required_nonempty_string(&arguments, "idempotency_key")
            else {
                return tool_error(
                    name,
                    "robustness_idempotency_key_required",
                    "Robustness replay requires an idempotency key.",
                    None,
                );
            };
            let Some(actor) = optional_nonempty_string(&arguments, "actor", "local-user") else {
                return tool_error(
                    name,
                    "robustness_authority_invalid",
                    "Robustness replay authority actor must be a non-empty string.",
                    None,
                );
            };
            service.handle_http_from_source(
                "mcp",
                "POST",
                &format!("/robustness-runs/{run_id}/replay"),
                json!({
                    "idempotencyKey": idempotency_key,
                    "authorityContext": {
                        "actor": actor,
                        "surface": "mcp",
                    },
                }),
            )
        }
        "tradeassembly.robustness.export" => {
            if !has_only_keys(
                &arguments,
                &[
                    "run_id",
                    "format",
                    "idempotency_key",
                    "actor",
                    "studio_base_url",
                    "db",
                ],
            ) {
                return tool_error(
                    name,
                    "robustness_arguments_invalid",
                    "Robustness export arguments do not match the tool schema.",
                    None,
                );
            }
            let Some(run_id) = required_nonempty_string(&arguments, "run_id") else {
                return tool_error(
                    name,
                    "robustness_run_id_required",
                    "Robustness run ID is required.",
                    None,
                );
            };
            let Some(format) = required_nonempty_string(&arguments, "format") else {
                return tool_error(
                    name,
                    "robustness_export_format_required",
                    "Robustness export format is required.",
                    None,
                );
            };
            if !matches!(format.as_str(), "json" | "csv") {
                return tool_error(
                    name,
                    "robustness_export_format_invalid",
                    "Robustness export format must be json or csv.",
                    None,
                );
            }
            let Some(idempotency_key) = required_nonempty_string(&arguments, "idempotency_key")
            else {
                return tool_error(
                    name,
                    "robustness_idempotency_key_required",
                    "Robustness export requires an idempotency key.",
                    None,
                );
            };
            let Some(actor) = optional_nonempty_string(&arguments, "actor", "local-user") else {
                return tool_error(
                    name,
                    "robustness_authority_invalid",
                    "Robustness export authority actor must be a non-empty string.",
                    None,
                );
            };
            service.handle_http_from_source(
                "mcp",
                "POST",
                &format!("/robustness-runs/{run_id}/export"),
                json!({
                    "format": format,
                    "idempotencyKey": idempotency_key,
                    "authorityContext": {
                        "actor": actor,
                        "surface": "mcp",
                    },
                }),
            )
        }
        "tradeassembly.derivatives.create" => {
            let Some(request) = arguments.get("request").cloned().filter(Value::is_object) else {
                return tool_error(
                    name,
                    "derivatives_request_invalid",
                    "Derivatives analysis requires an object request.",
                    None,
                );
            };
            service.handle_http_from_source("mcp", "POST", "/derivatives-analyses", request)
        }
        "tradeassembly.derivatives.list" => {
            service.handle_http_from_source("mcp", "GET", "/derivatives-analyses", json!({}))
        }
        "tradeassembly.derivatives.get" => {
            let Some(analysis_id) = required_nonempty_string(&arguments, "analysis_id") else {
                return tool_error(
                    name,
                    "derivatives_analysis_id_required",
                    "Derivatives analysis ID is required.",
                    None,
                );
            };
            service.handle_http_from_source(
                "mcp",
                "GET",
                &format!("/derivatives-analyses/{analysis_id}"),
                json!({}),
            )
        }
        "tradeassembly.derivatives.export" => {
            let Some(analysis_id) = required_nonempty_string(&arguments, "analysis_id") else {
                return tool_error(
                    name,
                    "derivatives_analysis_id_required",
                    "Derivatives analysis ID is required.",
                    None,
                );
            };
            let Some(format) = required_nonempty_string(&arguments, "format") else {
                return tool_error(
                    name,
                    "derivatives_export_format_required",
                    "Derivatives export format is required.",
                    None,
                );
            };
            if !matches!(format.as_str(), "json" | "csv") {
                return tool_error(
                    name,
                    "derivatives_export_format_invalid",
                    "Derivatives export format must be json or csv.",
                    None,
                );
            }
            service.handle_http_from_source(
                "mcp",
                "GET",
                &format!("/derivatives-analyses/{analysis_id}/exports/{format}"),
                json!({}),
            )
        }
        "tradeassembly.comparison.create" => {
            let Some(request) = arguments.get("request").cloned().filter(Value::is_object) else {
                return tool_error(
                    name,
                    "comparison_request_invalid",
                    "Research comparison requires an object request.",
                    None,
                );
            };
            service.handle_http_from_source("mcp", "POST", "/research-comparisons", request)
        }
        "tradeassembly.comparison.list" => {
            service.handle_http_from_source("mcp", "GET", "/research-comparisons", json!({}))
        }
        "tradeassembly.comparison.get" => {
            let Some(comparison_id) = required_nonempty_string(&arguments, "comparison_id") else {
                return tool_error(
                    name,
                    "comparison_id_required",
                    "Research comparison ID is required.",
                    None,
                );
            };
            service.handle_http_from_source(
                "mcp",
                "GET",
                &format!("/research-comparisons/{comparison_id}"),
                json!({}),
            )
        }
        "tradeassembly.comparison.export" => {
            let Some(comparison_id) = required_nonempty_string(&arguments, "comparison_id") else {
                return tool_error(
                    name,
                    "comparison_id_required",
                    "Research comparison ID is required.",
                    None,
                );
            };
            let Some(format) = required_nonempty_string(&arguments, "format") else {
                return tool_error(
                    name,
                    "comparison_export_format_required",
                    "Research comparison export format is required.",
                    None,
                );
            };
            if !matches!(format.as_str(), "json" | "csv") {
                return tool_error(
                    name,
                    "comparison_export_kind_invalid",
                    "Research comparison export format must be json or csv.",
                    None,
                );
            }
            service.handle_http_from_source(
                "mcp",
                "GET",
                &format!("/research-comparisons/{comparison_id}/exports/{format}"),
                json!({}),
            )
        }
        _ => return service.call_mcp_tool(name, arguments),
    };
    if response.status >= 400 {
        let fallback_code = if name.starts_with("tradeassembly.robustness.") {
            "robustness_failed"
        } else if name.starts_with("tradeassembly.comparison.") {
            "comparison_failed"
        } else {
            "derivatives_analysis_failed"
        };
        let code = response
            .body
            .pointer("/error/code")
            .and_then(Value::as_str)
            .unwrap_or(fallback_code)
            .to_string();
        return tool_error(
            name,
            &code,
            if name.starts_with("tradeassembly.robustness.") {
                "Robustness command failed."
            } else if name.starts_with("tradeassembly.comparison.") {
                "Research comparison command failed."
            } else {
                "Derivatives analysis command failed."
            },
            Some(response.body),
        );
    }
    call_tool_with_payload(name, response.body)
}

pub fn call_tool_with_payload(_name: &str, payload: Value) -> Value {
    let redacted = redact_sensitive_payload(payload);
    json!({
        "content": [{"type": "text", "text": serde_json::to_string(&redacted).unwrap_or_else(|_| "{}".to_string())}],
        "structuredContent": redacted,
        "isError": false
    })
}

pub fn execution_run_arguments(arguments: &Value) -> Value {
    json!({
        "db": string_arg(arguments, "db", DEFAULT_DB),
        "activation_id": optional_string_arg(arguments, "activation_id"),
        "submit_exit": bool_arg(arguments, "submit_exit", false),
        "submit_orders": bool_arg(arguments, "submit_orders", false)
    })
}

pub fn serve_stdio_text(input: &str, db: &str, studio_base_url: &str) -> StdioResult {
    let service = crate::service::TradeAssemblyService::new(db)
        .with_studio_base_url(studio_base_url)
        .unwrap_or_else(|_| crate::service::TradeAssemblyService::new(db));
    serve_stdio_text_with_service(&service, input, db, studio_base_url)
}

#[doc(hidden)]
pub fn serve_stdio_text_for_test(input: &str, db: &str, studio_base_url: &str) -> StdioResult {
    let service = crate::service::TradeAssemblyService::test_local(db)
        .with_studio_base_url(studio_base_url)
        .unwrap_or_else(|_| crate::service::TradeAssemblyService::test_local(db));
    serve_stdio_text_with_service(&service, input, db, studio_base_url)
}

fn serve_stdio_text_with_service(
    service: &crate::service::TradeAssemblyService,
    input: &str,
    db: &str,
    studio_base_url: &str,
) -> StdioResult {
    let mut stdout = String::new();
    let mut stderr = String::new();
    for line in input.lines().map(str::trim).filter(|line| !line.is_empty()) {
        let response = match serde_json::from_str::<Value>(line) {
            Ok(Value::Object(message)) => {
                handle_message(service, Value::Object(message), db, studio_base_url)
            }
            Ok(_) => Some(protocol_error(
                Value::Null,
                -32600,
                "MCP message must be a JSON object",
            )),
            Err(error) => Some(protocol_error(
                Value::Null,
                -32700,
                &format!("invalid JSON: {error}"),
            )),
        };
        if let Some(response) = response {
            match serde_json::to_string(&response) {
                Ok(encoded) => {
                    stdout.push_str(&encoded);
                    stdout.push('\n');
                }
                Err(error) => {
                    stderr.push_str(&format!("tradeassembly MCP server error: {error}\n"));
                    stdout.push_str(
                        &serde_json::to_string(&protocol_error(
                            Value::Null,
                            -32603,
                            "internal serialization error",
                        ))
                        .unwrap(),
                    );
                    stdout.push('\n');
                }
            }
        }
    }
    StdioResult {
        exit_code: 0,
        stdout,
        stderr,
    }
}

fn handle_message(
    service: &crate::service::TradeAssemblyService,
    message: Value,
    db: &str,
    studio_base_url: &str,
) -> Option<Value> {
    let request_id = message.get("id").cloned().unwrap_or(Value::Null);
    let method = message.get("method").and_then(Value::as_str).unwrap_or("");
    let params = message.get("params").cloned().unwrap_or_else(|| json!({}));
    match method {
        "initialize" => Some(json!({
            "jsonrpc": "2.0",
            "id": request_id,
            "result": {
                "protocolVersion": requested_protocol_version(&params),
                "capabilities": {"tools": {"listChanged": false}},
                "serverInfo": {
                    "name": "tradeassembly",
                    "title": "TradeAssembly",
                    "version": VERSION,
                    "description": "Local-first TradeAssembly strategy research, replay, and automation MCP server."
                },
                "instructions": NO_ADVICE_NOTICE
            }
        })),
        "notifications/initialized" => None,
        "ping" => Some(json!({"jsonrpc": "2.0", "id": request_id, "result": {}})),
        "tools/list" => Some(
            json!({"jsonrpc": "2.0", "id": request_id, "result": {"tools": tool_definitions()}}),
        ),
        "tools/call" => {
            let name = params.get("name").and_then(Value::as_str);
            let Some(name) = name else {
                return Some(protocol_error(
                    request_id,
                    -32602,
                    "tools/call requires params.name",
                ));
            };
            let mut arguments = params
                .get("arguments")
                .and_then(Value::as_object)
                .cloned()
                .unwrap_or_default();
            arguments
                .entry("db".to_string())
                .or_insert_with(|| Value::String(db.to_string()));
            if matches!(
                name,
                "tradeassembly.health"
                    | "tradeassembly.setup.status"
                    | "tradeassembly.studio.link"
                    | "tradeassembly.report.envelope"
                    | "tradeassembly.backtest.report"
                    | "tradeassembly.backtest.get"
                    | "tradeassembly.backtest.replay"
                    | "tradeassembly.backtest.export"
                    | "tradeassembly.robustness.get"
                    | "tradeassembly.robustness.report"
                    | "tradeassembly.robustness.replay"
                    | "tradeassembly.robustness.export"
                    | "tradeassembly.scenario.valuation"
                    | "tradeassembly.portfolio_risk.run"
                    | "tradeassembly.portfolio_risk.report"
                    | "tradeassembly.portfolio_risk.replay"
                    | "tradeassembly.portfolio_risk.export"
                    | "tradeassembly.fill_quality.inspect"
                    | "tradeassembly.fill_quality.report"
                    | "tradeassembly.fill_quality.replay"
                    | "tradeassembly.fill_quality.export"
                    | "tradeassembly.attribution_journal.review"
                    | "tradeassembly.attribution_journal.report"
                    | "tradeassembly.attribution_journal.inspect"
                    | "tradeassembly.attribution_journal.replay"
                    | "tradeassembly.attribution_journal.export"
                    | "tradeassembly.lifecycle_calendar.list"
                    | "tradeassembly.lifecycle_calendar.inspect"
                    | "tradeassembly.lifecycle_calendar.export"
                    | "tradeassembly.lifecycle_calendar.replay"
                    | "tradeassembly.research_notebook.create"
                    | "tradeassembly.research_notebook.compose"
                    | "tradeassembly.research_notebook.attach"
                    | "tradeassembly.research_notebook.list"
                    | "tradeassembly.research_notebook.inspect"
                    | "tradeassembly.research_notebook.export"
                    | "tradeassembly.research_notebook.replay"
            ) {
                // The process-bound origin is authoritative. Never honor a
                // caller-supplied per-call origin.
                arguments.insert(
                    "studio_base_url".to_string(),
                    Value::String(studio_base_url.to_string()),
                );
            }
            Some(
                json!({"jsonrpc": "2.0", "id": request_id, "result": call_service_tool(service, name, Value::Object(arguments))}),
            )
        }
        _ if request_id.is_null() => None,
        _ => Some(protocol_error(
            request_id,
            -32601,
            &format!("unknown MCP method: {method}"),
        )),
    }
}

fn requested_protocol_version(params: &Value) -> &'static str {
    match params.get("protocolVersion").and_then(Value::as_str) {
        Some(MCP_PROTOCOL_VERSION) => MCP_PROTOCOL_VERSION,
        _ => MCP_PROTOCOL_VERSION,
    }
}

fn tool_specs() -> Vec<ToolSpec> {
    vec![
        tool_with_metadata("tradeassembly.agent.session.attach", "Attach External Agent", "Bind this MCP connection to an active external-client deployment and activation. Requires authenticated ownership and existing delegated authority; no token is returned. The client owns its agent loop.", object_schema([("deployment_id", string_schema("Existing external-client deployment ID.", None)), ("activation_id", string_schema("Existing active execution activation ID.", None)), ("idempotency_key", string_schema("Connection attachment retry key.", None))], ["deployment_id", "activation_id", "idempotency_key"]), false, false, true),
        tool_with_metadata("tradeassembly.agent.session.detach", "Detach External Agent", "Release only this MCP connection's agent lease. This does not cancel orders; reconcile outstanding outcomes before reconnecting.", object_schema([], []), false, false, true),
        tool("tradeassembly.onboarding.start", "Connect TradeAssembly", "Start resumable browser setup. Open browserUrl, then poll onboarding.status. Never accepts credentials. Does not activate trading.", object_schema([("mode", enum_string_schema("Owner-selected broker account mode.", &["paper", "live"])), ("environment", enum_string_schema("Deployment environment; defaults to the packaged connection profile.", &["staging", "production"])), ("relay", json!({"type":"boolean","description":"Also verify the selected Relay subscription.","default":false})), ("idempotency_key", string_schema("Stable setup request identifier.", None)), ("instanceRef", string_schema("Optional existing broker instance.", None))], ["mode", "idempotency_key"]), false),
        tool("tradeassembly.onboarding.status", "Connection Progress", "Inspect and reconcile browser setup. Readiness requires verified service results.", object_schema([("onboardingId", string_schema("Reference returned by onboarding.start.", None))], ["onboardingId"]), false),
        tool("tradeassembly.onboarding.cancel", "Cancel Connection Setup", "Cancel this setup attempt without disconnecting an existing account.", object_schema([("onboardingId", string_schema("Reference returned by onboarding.start.", None))], ["onboardingId"]), false),
        tool("tradeassembly.health", "TradeAssembly MCP Health", "Check local TradeAssembly MCP health and safety posture.", object_schema([("db", db_property()), ("studio_base_url", studio_property())], []), true),
        tool("tradeassembly.auth.status", "OIDC Session Status", "Inspect the redacted OIDC session status for this MCP process.", json!({"type": "object", "properties": {}, "additionalProperties": false}), true),
        tool("tradeassembly.setup.status", "TradeAssembly Setup Status", "Summarize local setup, MCP transport, Studio URL, and credential posture. Use surface=agent for agent-only acceptance; omitted surface retains Studio checks.", object_schema([("db", db_property()), ("studio_base_url", studio_property()), ("surface", enum_string_schema("Requested setup surface; defaults to studio.", &["agent", "studio"]))], []), true),
        tool("tradeassembly.account.status", "Account Status", "Read local account, install, entitlement, telemetry, and credential-custody posture without hosted services.", object_schema([("profile", string_schema("Config profile.", Some("local"))), ("studio_base_url", studio_property())], []), true),
        tool("tradeassembly.account.login", "Sign In", "Open browser sign-in without an existing session. Returns promptly; use account.login.status to check completion. Never accepts passwords or tokens.", object_schema([], []), false),
        tool("tradeassembly.account.login.status", "Sign-in Progress", "Check browser sign-in completion without returning credentials.", object_schema([], []), true),
        tool("tradeassembly.setup.inspect", "Inspect Setup", "Inspect setup without starting automation. Returns evidence and one next action; private state requires a verified identity. Use surface=agent for agent-only acceptance; omitted surface retains Studio checks. Broker and restart verification remain required.", object_schema([("surface", enum_string_schema("Requested setup surface; defaults to studio.", &["agent", "studio"]))], []), true),
        tool("tradeassembly.broker.connect", "Connect Broker", "Prepare browser-based broker setup, creating a connection if needed. Choose paper or live explicitly. The authenticated MCP session supplies the actor; authority_context is optional compatibility input and its actor is ignored. Never put credentials in tool arguments.", object_schema([("instanceRef", string_schema("Optional existing broker instance.", None)), ("pluginRef", string_schema("Optional installed Alpaca plugin identifier.", None)), ("mode", enum_string_schema("Owner-selected environment.", &["paper", "live"])), ("idempotency_key", string_schema("Command idempotency key.", None)), ("authority_context", object_like_schema("Optional compatibility authority context; actor is bound from the authenticated session."))], ["mode", "idempotency_key"]), false),
        tool("tradeassembly.broker.status", "Broker Connection Status", "Read durable connection facts; installation and credential presence do not prove account connectivity.", object_schema([("instanceRef", string_schema("Optional broker instance.", None))], []), true),
        tool("tradeassembly.broker.verify", "Verify Broker Connection", "Read account health and record verification for the explicit environment. The authenticated MCP session supplies the actor; authority_context is optional compatibility input and its actor is ignored. Never places an order; declared capabilities are not execution proof.", object_schema([("instanceRef", string_schema("Broker instance.", None)), ("mode", enum_string_schema("Owner-selected environment.", &["paper", "live"])), ("idempotency_key", string_schema("Command idempotency key.", None)), ("authority_context", object_like_schema("Optional compatibility authority context; actor is bound from the authenticated session."))], ["instanceRef", "mode", "idempotency_key"]), false),
        tool("tradeassembly.install.claim", "Install Claim", "Claim the local install id to the local identity profile with idempotency evidence.", object_schema([("profile", string_schema("Config profile.", Some("local"))), ("install_id", string_schema("Optional install id.", None)), ("studio_base_url", studio_property())], []), false),
        tool("tradeassembly.telemetry.emit", "Emit Telemetry", "Emit a sanitized local telemetry event through the local relay contract.", object_schema([("profile", string_schema("Config profile.", Some("local"))), ("event", string_schema("Telemetry event type.", Some("first_backtest"))), ("install_id", string_schema("Optional install id.", None)), ("studio_base_url", studio_property())], []), false),
        tool("tradeassembly.telemetry.opt_out", "Telemetry Opt Out", "Toggle local telemetry opt-out state and return relay behavior evidence.", object_schema([("profile", string_schema("Config profile.", Some("local"))), ("telemetry_opt_out", boolean_schema("Whether telemetry is opted out.", Some(true))), ("studio_base_url", studio_property())], []), false),
        tool("tradeassembly.telemetry.replay", "Telemetry Replay", "Return local sanitized telemetry replay/debug refs.", object_schema([("profile", string_schema("Config profile.", Some("local"))), ("studio_base_url", studio_property())], []), true),
        tool("tradeassembly.studio.link", "TradeAssembly Studio Link", "Return a local or hosted Studio deep link.", object_schema([("path", string_schema("Studio path to open.", Some("/"))), ("db", db_property()), ("studio_base_url", studio_property())], []), true),
        tool_with_metadata("studio.deployment.create", "Create Agent Deployment", "Create one local AgentDeployment through the durable runner. Requires explicit authority context and idempotency; no broker credentials are accepted or returned.", agent_deployment_create_schema(), false, false, true),
        tool_with_metadata("studio.deployment.start", "Start Agent Deployment", "Set one local AgentDeployment desired state to active. This records authority and idempotency but does not itself submit an order.", agent_deployment_state_schema(), false, false, true),
        tool_with_metadata("studio.deployment.pause", "Pause Agent Deployment", "Set one local AgentDeployment desired state to paused. This records authority and idempotency but does not cancel or submit broker orders.", agent_deployment_state_schema(), false, false, true),
        tool_with_metadata("studio.deployment.stop", "Stop Agent Deployment", "Set one local AgentDeployment desired state to stopped. This records authority and idempotency but does not cancel or submit broker orders.", agent_deployment_state_schema(), false, false, true),
        tool("studio.deployment.inspect", "Inspect Agent Deployments", "Read local AgentDeployment records with prompts, workspaces, runtime profiles, and credentials redacted.", agent_deployment_inspect_schema(), true),
        tool("studio.agent_run.inspect", "Inspect Agent Run", "Read one local AgentRun with opaque Codex session references and credentials redacted.", agent_run_inspect_schema(), true),
        tool("studio.agent_run.events", "Agent Run Events", "Replay ordered, redacted lifecycle evidence for one local agent deployment.", agent_run_events_schema(), true),
        tool("studio.agent_run.health", "Agent Run Health", "Read redacted local AgentRun health and pending-reconciliation state.", agent_run_health_schema(), true),
        tool_with_metadata("studio.agent_run.recover", "Recover Agent Run", "Acknowledge deterministic reconciliation for one quarantined run and release only a future tick. Requires acknowledge_reconciled=true, authority, and idempotency; it never submits or replays an order.", agent_run_recover_schema(), false, false, true),
        tool("studio.execution.external_receipt.append", "Append External Broker Receipt", "Append a redacted receipt for an order placed through a customer-configured broker interface. This tool never receives credentials.", object_schema([("deployment_id", string_schema("Agent deployment ID.", None)), ("run_id", string_schema("Agent run ID.", None)), ("broker", string_schema("Broker identifier.", Some("alpaca"))), ("environment", string_schema("paper or live.", Some("paper"))), ("client_order_id", string_schema("Customer broker client order ID.", None)), ("broker_order_id", string_schema("Optional broker order ID.", None)), ("event_type", string_schema("Broker receipt event type.", Some("order_accepted"))), ("occurred_at_ms", integer_schema("Broker observation time in Unix milliseconds.", None)), ("receipt", object_like_schema("Redacted broker receipt facts.")), ("source_ref", string_schema("Optional customer-side evidence reference.", None)), ("idempotency_key", string_schema("Command idempotency key.", None)), ("authority_context", object_like_schema("Actor/run authority context.")), ("db", db_property())], ["deployment_id", "run_id", "broker", "environment", "client_order_id", "event_type", "receipt", "idempotency_key", "authority_context"]), false),
        tool("studio.execution.external_receipt.inspect", "Inspect External Broker Receipt", "Inspect redacted external broker receipt evidence for one deployment and client order ID.", object_schema([("deployment_id", string_schema("Agent deployment ID.", None)), ("client_order_id", string_schema("Customer broker client order ID.", None)), ("db", db_property())], ["deployment_id", "client_order_id"]), true),
        tool("studio.execution.external_receipt.reconcile", "Reconcile External Broker Receipt", "Record an explicit reconciliation decision for a redacted external broker receipt. This tool never submits or retries a broker order.", object_schema([("deployment_id", string_schema("Agent deployment ID.", None)), ("client_order_id", string_schema("Customer broker client order ID.", None)), ("resolution", string_schema("reviewed or unresolved.", Some("reviewed"))), ("idempotency_key", string_schema("Command idempotency key.", None)), ("authority_context", object_like_schema("Actor/run authority context.")), ("db", db_property())], ["deployment_id", "client_order_id", "idempotency_key", "authority_context"]), false),
        tool("tradeassembly.sightline.session_state", "Sightline Session State", "Read the room, surfaces, selection, agent presence, proposals, approvals, and events bound to the pinned Sightline contracts.", object_schema([("db", db_property()), ("strategy_id", string_schema("Optional strategy ID.", Some("strat_local_btc_demo"))), ("surface_id", string_schema("Optional surface ID.", None))], []), true),
        tool("tradeassembly.sightline.get_current_selection", "Sightline Current Selection", "Read the current redacted semantic selection and context through the pinned Sightline contracts.", object_schema([("db", db_property()), ("strategy_id", string_schema("Optional strategy ID.", Some("strat_local_btc_demo"))), ("surface_id", string_schema("Optional surface ID.", None)), ("selection_id", string_schema("Optional selection ID.", None))], []), true),
        tool("tradeassembly.sightline.get_context", "Sightline Context", "Read agent-safe semantic context for the current or requested selection.", object_schema([("db", db_property()), ("strategy_id", string_schema("Optional strategy ID.", Some("strat_local_btc_demo"))), ("surface_id", string_schema("Optional surface ID.", None)), ("selection_id", string_schema("Optional selection ID.", None)), ("context_level", integer_schema("Requested context level; local MVP caps at 2.", Some(2)))], []), true),
        tool("tradeassembly.sightline.create_proposal", "Sightline Create Proposal", "Create a Sightline-compatible proposal that routes through TradeAssembly StrategyDraft proposal review.", object_schema([("db", db_property()), ("strategy_id", string_schema("Strategy ID.", Some("strat_local_btc_demo"))), ("selection_id", string_schema("Optional selection ID.", None)), ("summary", string_schema("Proposal summary.", Some("Agent proposed StrategySpec edit"))), ("purpose", string_schema("Controlled purpose.", Some("strategy_authoring"))), ("patch", object_like_schema("StrategySpec patch object.")), ("actor", object_like_schema("Agent principal.")), ("sightline_refs", array_schema())], ["strategy_id", "patch"]), false),
        tool("tradeassembly.sightline.request_approval", "Sightline Request Approval", "Create a visible approval prompt; TradeAssembly APF still enforces protected actions.", object_schema([("db", db_property()), ("strategy_id", string_schema("Optional strategy ID.", Some("strat_local_btc_demo"))), ("action", string_schema("Requested action.", Some("review_proposal"))), ("authority_level", string_schema("mutate, runtime, or high_authority.", Some("mutate"))), ("summary", string_schema("Approval summary.", Some("Review requested"))), ("actor", object_like_schema("Agent principal."))], []), false),
        tool("tradeassembly.sightline.navigate_surface", "Sightline Navigate Surface", "Request audited semantic navigation or focus for a registered Studio surface.", object_schema([("db", db_property()), ("strategy_id", string_schema("Optional strategy ID.", Some("strat_local_btc_demo"))), ("surface_id", string_schema("Optional surface ID.", None)), ("route", string_schema("Target route.", Some("/app/strategies/strat_local_btc_demo/builder"))), ("node_id", string_schema("Optional node ID.", None)), ("actor", object_like_schema("Agent principal."))], []), false),
        tool("tradeassembly.sightline.list_events", "Sightline Events", "List local room/session events bound to the pinned Sightline event contract.", object_schema([("db", db_property()), ("strategy_id", string_schema("Optional strategy ID.", Some("strat_local_btc_demo"))), ("limit", integer_schema("Maximum events.", Some(50)))], []), true),
        tool("tradeassembly.strategy.list", "List Strategies", "List local strategies.", object_schema([("db", db_property())], []), true),
        tool("tradeassembly.strategy.get", "Get Strategy", "Get a local strategy workspace.", object_schema([("strategy_id", string_schema("Strategy ID.", None)), ("db", db_property())], ["strategy_id"]), true),
        tool("tradeassembly.journal.list", "List Journal Events", "List journal events visible to the authenticated owner. Legacy unowned records remain conservatively strategy-scoped.", object_schema([("db", db_property())], []), true),
        tool("tradeassembly.journal.export", "Export Journal", "Export authenticated-owner journal events as canonical JSON for caller-directed transport; this tool does not write files.", object_schema([("db", db_property())], []), true),
        tool("tradeassembly.journal.replay", "Replay Journal", "Replay authenticated-owner journal events and return deterministic stored-event counts without evaluating strategy logic.", object_schema([("db", db_property())], []), true),
        tool("tradeassembly.strategy.schema", "Strategy Authoring Schema", "Read the embedded canonical StrategySpec JSON Schema before authoring owner-supplied rules. No source files are required. Publication requires owner acknowledgment and never activates execution.", object_schema([], []), true),
        tool("tradeassembly.strategy.create", "Create Strategy", "Create a local strategy draft from a template or blank mode. Blank drafts contain no trading rules and are not research-ready. Use tradeassembly.strategy.schema to author the owner's rules.", object_schema([("db", db_property()), ("mode", string_schema("Creation mode.", Some("blank"))), ("template_id", string_schema("Optional template ID.", None)), ("name", string_schema("Optional strategy name.", None))], []), false),
        tool("tradeassembly.strategy.save_draft", "Save Strategy Draft", "Save a user-defined strategy draft.", object_schema([("db", db_property()), ("strategy_id", string_schema("Optional existing strategy ID.", None)), ("name", string_schema("Strategy name.", Some("Local strategy"))), ("spec", object_like_schema("StrategySpec draft."))], []), false),
        tool("tradeassembly.strategy.draft.select_node", "Select Strategy Draft Node", "Select a redacted StrategySpec node for agent context.", object_schema([("db", db_property()), ("strategy_id", string_schema("Strategy ID.", Some("strat_local_btc_demo"))), ("node_ref", string_schema("Strategy semantic node ref.", Some("strategy.root")))], ["strategy_id", "node_ref"]), true),
        tool("tradeassembly.strategy.draft.propose_patch", "Propose Strategy Draft Patch", "Create a reviewable StrategyDraftChangeSet proposal. Agents may propose but not apply.", object_schema([("db", db_property()), ("strategy_id", string_schema("Strategy ID.", Some("strat_local_btc_demo"))), ("expected_draft_hash", string_schema("Expected draft hash.", None)), ("selection_ref", string_schema("Optional semantic selection ref.", None)), ("summary", string_schema("Proposal summary.", Some("Agent proposed strategy edit"))), ("purpose", string_schema("Controlled purpose.", Some("strategy_authoring"))), ("patch", object_like_schema("StrategySpec patch object.")), ("evidence_refs", array_schema())], ["strategy_id", "patch"]), false),
        tool("tradeassembly.strategy.draft.review_patch", "Review Strategy Draft Patch", "Review a strategy patch proposal. MCP agent calls cannot apply accepted patches.", object_schema([("db", db_property()), ("strategy_id", string_schema("Strategy ID.", Some("strat_local_btc_demo"))), ("proposal_id", string_schema("Proposal ID.", None)), ("decision", string_schema("Review decision.", Some("reject")))], ["strategy_id", "proposal_id", "decision"]), false),
        tool("tradeassembly.strategy.version.publish", "Publish Strategy Version", "Publish the expected strategy draft as an immutable StrategyVersion. Requires explicit user authority; agent calls are denied.", object_schema([("db", db_property()), ("strategy_id", string_schema("Strategy ID.", Some("strat_local_btc_demo"))), ("expected_draft_hash", string_schema("Exact hash of the draft being published.", None)), ("actor", object_like_schema("User authority context."))], ["strategy_id", "expected_draft_hash", "actor"]), false),
        tool("tradeassembly.strategy.validate", "Validate StrategySpec", "Validate inline spec JSON without saving it, or validate an owned saved draft by strategy_id. Returns structured diagnostics. Does not read files, publish, or activate.", object_schema([("spec", object_like_schema("StrategySpec JSON to validate without persistence.")), ("strategy_id", string_schema("Owned saved draft to validate when spec is omitted.", None)), ("db", db_property())], []), true),
        tool("tradeassembly.plugin.list", "List Plugins", "List installed local plugins, operation contracts, entitlement posture, and capability resolver summaries.", object_schema([("db", db_property())], []), true),
        tool("tradeassembly.plugin.capability_resolve", "Resolve Plugin Capability", "Resolve which plugin operations can satisfy a strategy capability requirement under the active entitlement profile.", object_schema([("db", db_property()), ("capability", string_schema("Capability id required by the strategy.", None)), ("mode", string_schema("Execution mode, such as paper or live.", Some("paper"))), ("instrument_type", string_schema("Instrument type, such as equity, option, crypto, or future.", Some("equity"))), ("operation", string_schema("Optional operation id or strategy action.", None)), ("strategy_id", string_schema("Optional strategy id.", None)), ("plugin_ref", string_schema("Optional preferred plugin ref.", None)), ("account_ref", string_schema("Optional account ref.", None)), ("purpose", string_schema("Optional purpose code for APF receipts.", None))], ["capability"]), true),
        tool("tradeassembly.plugin.capability_graph_resolve", "Resolve Capability Graph", "Resolve a complete strategy capability graph through the shared deterministic resolver.", object_schema([("db", db_property()), ("strategy_id", string_schema("Strategy ID.", Some("strat_local_btc_demo"))), ("strategy_version_id", string_schema("Published StrategyVersion ID.", None)), ("mode", string_schema("Readiness mode.", Some("paper"))), ("evaluation_epoch", string_schema("RFC3339 evaluation epoch.", None)), ("requirements", json!({"type":"array","items":{"type":"object","additionalProperties":true},"description":"Optional explicit capability requirement objects; omit to derive the published strategy requirements."})), ("bindings", object_like_schema("Explicit requirement bindings.")), ("configured_fallbacks", object_like_schema("Ordered configured fallback bindings."))], []), true),
        tool("tradeassembly.plugin.capability_revision_save", "Save Capability Graph Revision", "Persist immutable capability bindings and their resolved graph snapshot.", object_schema([("db", db_property()), ("config_id", string_schema("Execution config ID.", None)), ("strategy_id", string_schema("Strategy ID.", Some("strat_local_btc_demo"))), ("strategy_version_id", string_schema("Published StrategyVersion ID.", None)), ("mode", string_schema("Readiness mode.", Some("paper"))), ("evaluation_epoch", string_schema("RFC3339 evaluation epoch.", None)), ("requirements", json!({"type":"array","items":{"type":"object","additionalProperties":true},"description":"Optional explicit capability requirement objects; omit to derive the published strategy requirements."})), ("bindings", object_like_schema("Explicit requirement bindings.")), ("configured_fallbacks", object_like_schema("Ordered configured fallback bindings.")), ("idempotency_key", string_schema("Idempotency key.", None))], []), false),
        tool("tradeassembly.plugin.capability_revision_get", "Get Capability Graph Revision", "Read an immutable capability graph revision.", object_schema([("db", db_property()), ("revision_id", string_schema("Capability revision ID.", None))], ["revision_id"]), true),
        tool("tradeassembly.plugin.capability_revision_check", "Check Capability Graph Revision", "Re-evaluate current plugin facts and report whether a saved graph revision remains current.", object_schema([("db", db_property()), ("revision_id", string_schema("Capability revision ID.", None)), ("evaluation_epoch", string_schema("RFC3339 evaluation epoch.", None))], ["revision_id"]), true),
        tool("tradeassembly.plugin.capability_matrix", "Plugin Capability Matrix", "Inspect installed plugin capability metadata by plugin ref or compatibility provider ref.", object_schema([("ref", string_schema("Plugin or provider ref.", None)), ("db", db_property())], ["ref"]), true),
        tool("tradeassembly.provider.list", "List Plugin Provider Metadata", "List compatibility provider metadata behind installed plugins.", object_schema([("db", db_property())], []), true),
        tool("tradeassembly.provider.capability_matrix", "Legacy Provider Capability Matrix Alias", "Compatibility alias for tradeassembly.plugin.capability_matrix.", object_schema([("ref", string_schema("Provider ref.", None)), ("db", db_property())], ["ref"]), true),
        tool("tradeassembly.provider.pack_compatibility", "Plugin Provider Pack Compatibility", "Check compatibility provider metadata for an instrument pack manifest.", object_schema([("ref", string_schema("Provider or plugin ref.", None)), ("manifest", object_like_schema("Instrument pack manifest.")), ("db", db_property()), ("strategy_id", string_schema("Optional strategy ID.", None)), ("max_conformance_age_seconds", integer_schema("Max conformance age in seconds.", Some(86400)))], ["ref", "manifest"]), true),
        tool("tradeassembly.credential.status", "Credential Status", "Show local credential status without returning raw secrets.", object_schema([("provider_ref", string_schema("Installed plugin instance ref.", Some("plugin-instance"))), ("db", db_property())], []), true),
        tool("tradeassembly.credential.test", "Credential Test", "Test plugin credential availability without placing orders.", object_schema([("provider_ref", string_schema("Installed plugin instance ref.", Some("plugin-instance"))), ("db", db_property())], []), true),
        tool_with_metadata("tradeassembly.plugin.oauth.start", "Start Plugin OAuth", "Start a request-scoped OAuth handoff for an owned plugin instance. Credentials and provider client secrets are never accepted or returned.", object_schema([("instanceRef", string_schema("Owned plugin instance ref.", None)), ("environment", string_schema("Plugin-declared deployment environment.", None)), ("scopes", oauth_scopes_schema()), ("idempotencyKey", string_schema("Stable key for retrying this exact handoff request.", None))], ["instanceRef", "environment", "scopes", "idempotencyKey"]), false, false, true),
        tool_with_metadata("tradeassembly.plugin.oauth.status", "Plugin OAuth Status", "Poll an owned plugin OAuth handoff and store provider credentials only after relay verification. No token is returned. Use a fresh idempotencyKey for each poll.", object_schema([("instanceRef", string_schema("Owned plugin instance ref.", None)), ("connectionId", string_schema("Opaque relay connection id.", None)), ("idempotencyKey", string_schema("Fresh key for this status poll, or the same key when retrying it.", None))], ["instanceRef", "connectionId", "idempotencyKey"]), false, false, true),
        tool_with_metadata("tradeassembly.plugin.oauth.disconnect", "Disconnect Plugin OAuth", "Revoke local credentials and invalidate pending OAuth handoffs for an owned plugin instance.", object_schema([("instanceRef", string_schema("Owned plugin instance ref.", None)), ("idempotencyKey", string_schema("Stable key for retrying this disconnect request.", None))], ["instanceRef", "idempotencyKey"]), false, true, true),
        tool("tradeassembly.plugin.status", "Plugin Status", "Summarize plugin command availability.", object_schema([("registry_path", string_schema("Optional plugin registry path.", None))], []), true),
        tool("tradeassembly.indicator.status", "Indicator Status", "Summarize indicator surface availability.", object_schema([("db", db_property())], []), true),
        tool("tradeassembly.backtest.run", "Run Backtest", "Create a durable deterministic BacktestRun for one immutable StrategyVersion and dataset snapshot.", object_schema([("db", db_property()), ("request", backtest_request_schema()), ("strategy_id", string_schema("Strategy ID for compatibility configuration.", None)), ("strategy_version_id", string_schema("Immutable StrategyVersion ID.", None)), ("dataset_id", string_schema("Immutable dataset snapshot ID.", None)), ("purpose", string_schema("Controlled purpose.", Some("strategy_backtest_research"))), ("client", string_schema("Client context.", Some("self"))), ("idempotency_key", string_schema("Request-equivalence idempotency key.", None))], ["idempotency_key"]), false),
        tool("tradeassembly.backtest.get", "Get Backtest", "Read one durable BacktestRun and its current attempt.", object_schema([("run_id", string_schema("Backtest run ID.", None)), ("db", db_property())], ["run_id"]), true),
        tool("tradeassembly.backtest.report", "Get Backtest Report", "Project one integrity-verified completed BacktestRun into its deterministic report.", object_schema([("backtest_id", string_schema("Backtest ID.", None)), ("db", db_property())], ["backtest_id"]), true),
        tool("tradeassembly.backtest.export", "Export Backtest Report", "Export one verified deterministic backtest report as manifest JSON, report JSON, trades CSV, orders and fills CSV, positions and ledger CSV, or equity CSV.", object_schema([("backtest_id", string_schema("Backtest ID.", None)), ("export_kind", string_schema("Export kind.", Some("reportJson"))), ("db", db_property())], ["backtest_id"]), false),
        tool("tradeassembly.backtest.list", "List Backtests", "List persisted local backtests.", object_schema([("db", db_property())], []), true),
        tool("tradeassembly.backtest.cancel", "Cancel Backtest", "Cancel a queued or running BacktestRun; late worker completion is fenced.", object_schema([("run_id", string_schema("Backtest run ID.", None)), ("idempotency_key", string_schema("Command idempotency key.", None)), ("db", db_property())], ["run_id", "idempotency_key"]), false),
        tool("tradeassembly.backtest.retry", "Retry Backtest", "Retry a failed BacktestRun as a new immutable attempt.", object_schema([("run_id", string_schema("Backtest run ID.", None)), ("idempotency_key", string_schema("Command idempotency key.", None)), ("db", db_property())], ["run_id", "idempotency_key"]), false),
        tool("tradeassembly.backtest.process", "Process Backtest", "Claim and process only the authenticated owner's specified durable BacktestRun.", object_schema([("run_id", string_schema("Exact authorized backtest run ID.", None)), ("worker", string_schema("Stable worker identity.", Some("local-backtest-worker"))), ("idempotency_key", string_schema("Command idempotency key.", None)), ("db", db_property())], ["run_id", "idempotency_key"]), false),
        tool("tradeassembly.backtest.replay", "Replay Backtest", "Recompute a persisted backtest from pinned inputs and fail closed on mismatch.", object_schema([("run_id", string_schema("Backtest run ID.", None)), ("db", db_property())], ["run_id"]), true),
        tool("tradeassembly.dataset_ingestion.create", "Create Dataset Ingestion", "Resolve an exact historical-data plugin operation, acquire typed observations, validate quality, and persist an immutable content-addressed dataset snapshot.", dataset_ingestion_create_schema(), false),
        tool("tradeassembly.dataset_ingestion.list", "List Dataset Ingestions", "List historical dataset ingestion lifecycle records.", object_schema([("db", db_property())], []), true),
        tool("tradeassembly.dataset_ingestion.get", "Get Dataset Ingestion", "Get an integrity-checked dataset summary; request a bounded observation page explicitly.", dataset_ingestion_get_schema(), true),
        tool("tradeassembly.dataset_ingestion.status", "Dataset Ingestion Status", "Read the current state of one dataset ingestion.", object_schema([("ingestion_id", string_schema("Dataset ingestion ID.", None)), ("db", db_property())], ["ingestion_id"]), true),
        tool("tradeassembly.dataset_ingestion.cancel", "Cancel Dataset Ingestion", "Cancel a nonterminal dataset ingestion using the initiating authority context.", object_schema([("ingestion_id", string_schema("Dataset ingestion ID.", None)), ("idempotency_key", string_schema("Cancellation idempotency key.", None)), ("authority_context", object_like_schema("Initiating authority context.")), ("db", db_property())], ["ingestion_id", "idempotency_key", "authority_context"]), false),
        tool("tradeassembly.dataset_ingestion.verify", "Verify Dataset Ingestion", "Re-read one immutable dataset snapshot and fail closed on any integrity mismatch.", object_schema([("ingestion_id", string_schema("Dataset ingestion ID.", None)), ("db", db_property())], ["ingestion_id"]), true),
        tool("tradeassembly.report.envelope", "Report Envelope", "Return an agent-facing report envelope with artifact refs and Studio links.", object_schema([("db", db_property()), ("kind", string_schema("Report kind.", Some("backtest"))), ("strategy_id", string_schema("Optional strategy ID.", None)), ("backtest_id", string_schema("Optional backtest ID.", None)), ("checkpoint_id", string_schema("Optional checkpoint ID.", None)), ("selector_run", object_like_schema("Optional selector run payload.")), ("studio_base_url", studio_property())], []), true),
        tool("tradeassembly.scenario.valuation", "Scenario Valuation Compatibility Alias", "Compatibility alias for durable derivatives analysis. Service-backed execution requires an exact completed source run and never fabricates scenario results.", object_schema([("request", object_like_schema("Complete derivatives analysis request.")), ("db", db_property())], ["request"]), false),
        tool("tradeassembly.monte_carlo.run", "Run Monte Carlo Compatibility Alias", "Compatibility alias for a durable Monte Carlo robustness run. Service-backed execution requires an exact completed source run.", robustness_run_schema(), false),
        tool("tradeassembly.monte_carlo.status", "Monte Carlo Status Compatibility Alias", "Read the durable lifecycle state of a Monte Carlo robustness run.", object_schema([("run_id", string_schema("Durable robustness run ID.", None)), ("db", db_property())], ["run_id"]), true),
        tool("tradeassembly.monte_carlo.report", "Monte Carlo Report Compatibility Alias", "Read the persisted result for a completed Monte Carlo robustness run.", object_schema([("run_id", string_schema("Durable robustness run ID.", None)), ("db", db_property())], ["run_id"]), true),
        tool("tradeassembly.monte_carlo.replay", "Monte Carlo Replay Compatibility Alias", "Replay a completed Monte Carlo robustness run from persisted immutable inputs.", object_schema([("run_id", string_schema("Durable robustness run ID.", None)), ("idempotency_key", string_schema("Caller-controlled replay idempotency key.", None)), ("actor", string_schema("Initiating authority actor.", Some("local-user"))), ("db", db_property())], ["run_id", "idempotency_key"]), false),
        tool("tradeassembly.monte_carlo.export", "Monte Carlo Export Compatibility Alias", "Export a completed Monte Carlo robustness result as deterministic JSON or CSV.", object_schema([("run_id", string_schema("Durable robustness run ID.", None)), ("format", enum_string_schema("Export format.", &["json", "csv"])), ("idempotency_key", string_schema("Caller-controlled export idempotency key.", None)), ("actor", string_schema("Initiating authority actor.", Some("local-user"))), ("db", db_property())], ["run_id", "format", "idempotency_key"]), false),
        tool_with_metadata("tradeassembly.robustness.run", "Run Robustness Study", "Create a durable deterministic robustness run from one completed immutable backtest and a typed study request. Returns a run lifecycle record, never a fabricated result.", robustness_run_schema(), false, false, true),
        tool_with_metadata("tradeassembly.robustness.list", "List Robustness Runs", "List durable robustness run lifecycle records and terminal result references.", object_schema([("db", db_property())], []), true, false, false),
        tool_with_metadata("tradeassembly.robustness.get", "Get Robustness Run", "Read one durable robustness run, its manifest, attempts, lifecycle events, and result reference when completed.", object_schema([("run_id", string_schema("Durable robustness run ID.", None)), ("studio_base_url", studio_property()), ("db", db_property())], ["run_id"]), true, false, false),
        tool_with_metadata("tradeassembly.robustness.report", "Get Robustness Report", "Read the persisted manifest and result for one completed robustness run. The adapter performs no calculations.", object_schema([("run_id", string_schema("Durable robustness run ID.", None)), ("studio_base_url", studio_property()), ("db", db_property())], ["run_id"]), true, false, false),
        tool_with_metadata("tradeassembly.robustness.replay", "Replay Robustness Run", "Recompute one completed robustness run from persisted immutable inputs and fail closed on any mismatch.", object_schema([("run_id", string_schema("Durable robustness run ID.", None)), ("idempotency_key", string_schema("Caller-controlled idempotency key for the replay command.", None)), ("actor", string_schema("Initiating authority actor.", Some("local-user"))), ("studio_base_url", studio_property()), ("db", db_property())], ["run_id", "idempotency_key"]), false, false, true),
        tool_with_metadata("tradeassembly.robustness.export", "Export Robustness Report", "Write a content-addressed JSON or CSV export from one completed persisted robustness result.", object_schema([("run_id", string_schema("Durable robustness run ID.", None)), ("format", enum_string_schema("Export format.", &["json", "csv"])), ("idempotency_key", string_schema("Caller-controlled idempotency key for the export command.", None)), ("actor", string_schema("Initiating authority actor.", Some("local-user"))), ("studio_base_url", studio_property()), ("db", db_property())], ["run_id", "format", "idempotency_key"]), false, false, true),
        tool_with_metadata("tradeassembly.robustness.process", "Process Robustness Run", "Claim and process one queued robustness run using a stable worker identity. Processing is durable, replay-safe, and returns the persisted lifecycle record.", object_schema([("worker", string_schema("Stable worker identity used for the durable queue lease.", None)), ("db", db_property())], ["worker"]), false, false, true),
        tool_with_metadata("tradeassembly.robustness.cancel", "Cancel Robustness Run", "Cancel one queued or running robustness run with an idempotent command. Late worker completion is fenced by the durable lifecycle.", object_schema([("run_id", string_schema("Durable robustness run ID.", None)), ("idempotency_key", string_schema("Caller-controlled idempotency key for this cancellation command.", None)), ("actor", string_schema("Initiating authority actor.", Some("local-user"))), ("surface", string_schema("Initiating surface.", Some("mcp"))), ("db", db_property())], ["run_id", "idempotency_key"]), false, true, true),
        tool_with_metadata("tradeassembly.robustness.retry", "Retry Robustness Run", "Retry one failed robustness run as a new immutable attempt with durable idempotency and preserved prior evidence.", object_schema([("run_id", string_schema("Durable robustness run ID.", None)), ("idempotency_key", string_schema("Caller-controlled idempotency key for this retry command.", None)), ("actor", string_schema("Initiating authority actor.", Some("local-user"))), ("surface", string_schema("Initiating surface.", Some("mcp"))), ("db", db_property())], ["run_id", "idempotency_key"]), false, false, true),
        tool_with_metadata("tradeassembly.derivatives.create", "Create Derivatives Analysis", "Create a durable derivatives analysis from one completed immutable backtest. All marks, lifecycle evidence, and provenance are resolved by the service; caller values cannot become observed evidence.", derivatives_create_schema(), false, false, true),
        tool_with_metadata("tradeassembly.derivatives.list", "List Derivatives Analyses", "List durable derivatives analyses with immutable source bindings and export references.", object_schema([("db", db_property())], []), true, false, false),
        tool_with_metadata("tradeassembly.derivatives.get", "Get Derivatives Analysis", "Read one durable derivatives analysis and integrity-checked source bindings.", object_schema([("analysis_id", string_schema("Durable derivatives analysis ID.", None)), ("db", db_property())], ["analysis_id"]), true, false, false),
        tool_with_metadata("tradeassembly.derivatives.export", "Export Derivatives Analysis", "Read the immutable JSON or CSV export for one derivatives analysis.", object_schema([("analysis_id", string_schema("Durable derivatives analysis ID.", None)), ("format", enum_string_schema("Export format.", &["json", "csv"])), ("db", db_property())], ["analysis_id", "format"]), true, false, false),
        tool_with_metadata("tradeassembly.comparison.create", "Create Research Comparison", "Create an immutable comparison from two to five completed, integrity-verified research runs. The service resolves source evidence and compatibility; the adapter never calculates metrics.", comparison_create_schema(), false, false, true),
        tool_with_metadata("tradeassembly.comparison.list", "List Research Comparisons", "List durable immutable research comparison records.", object_schema([("db", db_property())], []), true, false, false),
        tool_with_metadata("tradeassembly.comparison.get", "Get Research Comparison", "Read one durable comparison with its exact source manifest and integrity-checked artifact.", object_schema([("comparison_id", string_schema("Durable research comparison ID.", None)), ("db", db_property())], ["comparison_id"]), true, false, false),
        tool_with_metadata("tradeassembly.comparison.export", "Export Research Comparison", "Read the immutable JSON or CSV export for one research comparison.", object_schema([("comparison_id", string_schema("Durable research comparison ID.", None)), ("format", enum_string_schema("Export format.", &["json", "csv"])), ("db", db_property())], ["comparison_id", "format"]), true, false, false),
        tool("tradeassembly.portfolio_risk.run", "Run Portfolio Risk Overlay", "Compute a local portfolio risk overlay and return Studio-linked evidence.", object_schema([("db", db_property()), ("strategy_id", string_schema("Strategy ID.", Some("strat_local_btc_demo"))), ("scope_kind", string_schema("Scope kind.", Some("strategy"))), ("studio_base_url", studio_property())], []), false),
        tool("tradeassembly.portfolio_risk.report", "Portfolio Risk Report", "Return exposure tables, concentration warnings, threshold comparisons, replay refs, exports, and Studio links.", object_schema([("db", db_property()), ("strategy_id", string_schema("Strategy ID.", Some("strat_local_btc_demo"))), ("scope_kind", string_schema("Scope kind.", Some("strategy"))), ("studio_base_url", studio_property())], []), true),
        tool("tradeassembly.portfolio_risk.replay", "Portfolio Risk Replay", "Return portfolio risk replay refs and commands.", object_schema([("db", db_property()), ("strategy_id", string_schema("Strategy ID.", Some("strat_local_btc_demo"))), ("scope_kind", string_schema("Scope kind.", Some("strategy"))), ("studio_base_url", studio_property())], []), true),
        tool("tradeassembly.portfolio_risk.export", "Portfolio Risk Export", "Return portfolio risk export refs for JSON, CSV, HTML, and SVG artifacts.", object_schema([("db", db_property()), ("strategy_id", string_schema("Strategy ID.", Some("strat_local_btc_demo"))), ("scope_kind", string_schema("Scope kind.", Some("strategy"))), ("studio_base_url", studio_property())], []), false),
        tool("tradeassembly.fill_quality.inspect", "Inspect Fill Quality", "Inspect per-fill slippage, quote comparison, warnings, provenance, and Studio links.", object_schema([("db", db_property()), ("strategy_id", string_schema("Strategy ID.", Some("strat_local_btc_demo"))), ("analysis_id", string_schema("Fill quality analysis ID.", Some("fill_quality_latest"))), ("benchmark", object_like_schema("Optional user-selected benchmark.")), ("studio_base_url", studio_property())], []), true),
        tool("tradeassembly.fill_quality.report", "Fill Quality Report", "Return fill quality summary, per-fill rows, warnings, replay refs, exports, and Studio links.", object_schema([("db", db_property()), ("strategy_id", string_schema("Strategy ID.", Some("strat_local_btc_demo"))), ("analysis_id", string_schema("Fill quality analysis ID.", Some("fill_quality_latest"))), ("benchmark", object_like_schema("Optional user-selected benchmark.")), ("studio_base_url", studio_property())], []), true),
        tool("tradeassembly.fill_quality.replay", "Fill Quality Replay", "Return fill quality replay refs and commands.", object_schema([("db", db_property()), ("strategy_id", string_schema("Strategy ID.", Some("strat_local_btc_demo"))), ("analysis_id", string_schema("Fill quality analysis ID.", Some("fill_quality_latest"))), ("studio_base_url", studio_property())], []), true),
        tool("tradeassembly.fill_quality.export", "Fill Quality Export", "Return fill quality export refs for JSON and CSV artifacts.", object_schema([("db", db_property()), ("strategy_id", string_schema("Strategy ID.", Some("strat_local_btc_demo"))), ("analysis_id", string_schema("Fill quality analysis ID.", Some("fill_quality_latest"))), ("studio_base_url", studio_property())], []), false),
        tool("tradeassembly.attribution_journal.review", "Review Attribution Journal", "Compute a local attribution and trade journal review with stable refs, warnings, replay refs, exports, and Studio links.", object_schema([("db", db_property()), ("strategy_id", string_schema("Strategy ID.", Some("strat_local_btc_demo"))), ("review_id", string_schema("Review ID.", Some("attribution_journal_latest"))), ("studio_base_url", studio_property())], []), true),
        tool("tradeassembly.attribution_journal.report", "Attribution Journal Report", "Return trade journal rows, attribution tables, P&L summary, warnings, replay refs, exports, and Studio links.", object_schema([("db", db_property()), ("strategy_id", string_schema("Strategy ID.", Some("strat_local_btc_demo"))), ("review_id", string_schema("Review ID.", Some("attribution_journal_latest"))), ("studio_base_url", studio_property())], []), true),
        tool("tradeassembly.attribution_journal.inspect", "Inspect Attribution Journal", "Inspect local attribution provenance, journal rows, warning summary, and replay review packet.", object_schema([("db", db_property()), ("strategy_id", string_schema("Strategy ID.", Some("strat_local_btc_demo"))), ("review_id", string_schema("Review ID.", Some("attribution_journal_latest"))), ("studio_base_url", studio_property())], []), true),
        tool("tradeassembly.attribution_journal.replay", "Replay Attribution Journal", "Return deterministic attribution journal replay refs and commands.", object_schema([("db", db_property()), ("strategy_id", string_schema("Strategy ID.", Some("strat_local_btc_demo"))), ("review_id", string_schema("Review ID.", Some("attribution_journal_latest"))), ("studio_base_url", studio_property())], []), true),
        tool("tradeassembly.attribution_journal.export", "Export Attribution Journal", "Return attribution journal JSON, CSV, and replay export refs.", object_schema([("db", db_property()), ("strategy_id", string_schema("Strategy ID.", Some("strat_local_btc_demo"))), ("review_id", string_schema("Review ID.", Some("attribution_journal_latest"))), ("studio_base_url", studio_property())], []), false),
        tool("tradeassembly.lifecycle_calendar.list", "Lifecycle Calendar List", "Return strategy lifecycle calendar events, filters, warnings, provenance, replay refs, exports, and Studio links.", object_schema([("db", db_property()), ("strategy_id", string_schema("Strategy ID.", Some("strat_local_btc_demo"))), ("timeline_id", string_schema("Lifecycle calendar timeline ID.", Some("lifecycle_calendar_latest"))), ("studio_base_url", studio_property())], []), true),
        tool("tradeassembly.lifecycle_calendar.inspect", "Inspect Lifecycle Calendar", "Inspect a strategy lifecycle calendar with affected positions, source provenance, and warnings.", object_schema([("db", db_property()), ("strategy_id", string_schema("Strategy ID.", Some("strat_local_btc_demo"))), ("timeline_id", string_schema("Lifecycle calendar timeline ID.", Some("lifecycle_calendar_latest"))), ("studio_base_url", studio_property())], []), true),
        tool("tradeassembly.lifecycle_calendar.export", "Lifecycle Calendar Export", "Return lifecycle calendar export refs for JSON, CSV, and calendar artifacts.", object_schema([("db", db_property()), ("strategy_id", string_schema("Strategy ID.", Some("strat_local_btc_demo"))), ("timeline_id", string_schema("Lifecycle calendar timeline ID.", Some("lifecycle_calendar_latest"))), ("studio_base_url", studio_property())], []), false),
        tool("tradeassembly.lifecycle_calendar.replay", "Lifecycle Calendar Replay", "Return lifecycle calendar replay refs and commands.", object_schema([("db", db_property()), ("strategy_id", string_schema("Strategy ID.", Some("strat_local_btc_demo"))), ("timeline_id", string_schema("Lifecycle calendar timeline ID.", Some("lifecycle_calendar_latest"))), ("studio_base_url", studio_property())], []), true),
        tool("tradeassembly.research_notebook.create", "Create Research Notebook", "Create an immutable strategy-scoped notebook from durable artifact selectors. Core resolves all evidence refs and hashes.", object_schema([("db", db_property()), ("strategy_id", string_schema("Strategy ID.", Some("strat_local_btc_demo"))), ("artifact_selectors", array_schema()), ("title", string_schema("Optional notebook title.", None)), ("studio_base_url", studio_property())], ["artifact_selectors"]), false),
        tool("tradeassembly.research_notebook.compose", "Compose Research Notebook", "Compose an immutable notebook from durable artifact selectors. Caller-supplied hashes and URIs are ignored.", object_schema([("db", db_property()), ("strategy_id", string_schema("Strategy ID.", Some("strat_local_btc_demo"))), ("artifact_selectors", array_schema()), ("title", string_schema("Optional notebook title.", None)), ("studio_base_url", studio_property())], ["artifact_selectors"]), false),
        tool("tradeassembly.research_notebook.attach", "Attach Notebook Artifact", "Compatibility compose operation using durable artifact selectors only.", object_schema([("db", db_property()), ("strategy_id", string_schema("Strategy ID.", Some("strat_local_btc_demo"))), ("artifact_selectors", array_schema()), ("title", string_schema("Optional notebook title.", None)), ("studio_base_url", studio_property())], ["artifact_selectors"]), false),
        tool("tradeassembly.research_notebook.list", "List Research Notebooks", "Return local research notebook refs and the latest notebook payload for a strategy.", object_schema([("db", db_property()), ("strategy_id", string_schema("Strategy ID.", Some("strat_local_btc_demo"))), ("studio_base_url", studio_property())], []), true),
        tool("tradeassembly.research_notebook.inspect", "Inspect Research Notebook", "Inspect a local research notebook with warnings, artifact refs, assumptions, exports, replay refs, and Studio links.", object_schema([("db", db_property()), ("strategy_id", string_schema("Strategy ID.", Some("strat_local_btc_demo"))), ("notebook_id", string_schema("Notebook ID.", None)), ("studio_base_url", studio_property())], []), true),
        tool("tradeassembly.research_notebook.export", "Export Research Notebook", "Return research notebook JSON, Markdown, and HTML export refs.", object_schema([("db", db_property()), ("strategy_id", string_schema("Strategy ID.", Some("strat_local_btc_demo"))), ("notebook_id", string_schema("Notebook ID.", None)), ("studio_base_url", studio_property())], []), false),
        tool("tradeassembly.research_notebook.replay", "Replay Research Notebook", "Return deterministic replay refs and commands for a local research notebook.", object_schema([("db", db_property()), ("strategy_id", string_schema("Strategy ID.", Some("strat_local_btc_demo"))), ("notebook_id", string_schema("Notebook ID.", None)), ("studio_base_url", studio_property())], []), true),
        tool("tradeassembly.execution.config.save", "Save Execution Config", "Save a strategy execution configuration for a locked StrategyVersion, connection-selected paper or live mode, symbol, timeframe, and risk limits.", object_schema([("db", db_property()), ("strategy_id", string_schema("Strategy ID.", Some("strat_local_btc_demo"))), ("version_id", string_schema("Optional StrategyVersion ID.", None)), ("provider_ref", string_schema("Installed plugin instance ref.", Some("sim"))), ("mode", string_schema("Execution mode selected by the configured connection: paper or live.", Some("paper"))), ("orchestrator", enum_string_schema("Evaluation owner; external_agent records authorization without scheduling deterministic ticks.", &["deterministic", "external_agent"])), ("account_ref", string_schema("Exact broker account reference from connection verification.", None)), ("data_provider_ref", string_schema("Optional installed market-data plugin instance.", None)), ("data_account_ref", string_schema("Exact data-provider account reference when required.", None)), ("asset_class", string_schema("Explicit instrument asset class.", None)), ("idempotency_key", string_schema("Stable configuration command key.", None)), ("risk_limits", object_like_schema("Execution risk limits.")), ("symbol", string_schema("Optional symbol.", Some("BTC/USD"))), ("timeframe", string_schema("Optional timeframe.", Some("1m")))], []), false),
        tool("tradeassembly.execution.live_mandate.issue", "Issue Local Live Mandate", "Issue an immutable local Live mandate for an exact verified execution configuration. User approval is explicit; no order is placed.", object_schema([("configId", string_schema("Exact Live execution config ID.", None)), ("expiresAtMs", integer_schema("Explicit expiry in Unix milliseconds.", None)), ("delegateDeploymentId", string_schema("Optional exact local agent deployment delegate.", None)), ("idempotencyKey", string_schema("Stable retry key.", None))], ["configId", "expiresAtMs", "idempotencyKey"]), false),
        tool("tradeassembly.execution.live_mandate.revoke", "Revoke Local Live Mandate", "Revoke one immutable local Live mandate. No broker call is made.", object_schema([( "mandateId", string_schema("Mandate ID.", None)), ("idempotencyKey", string_schema("Stable retry key.", None))], ["mandateId", "idempotencyKey"]), false),
        tool("tradeassembly.execution.live_mandate.status", "Read Local Live Mandate", "Read one local Live mandate after exact owner or verified agent-delegate checks.", object_schema([( "mandateId", string_schema("Mandate ID.", None))], ["mandateId"]), true),
        tool("tradeassembly.execution.readiness", "Execution Readiness", "Check whether an execution configuration can be activated and return blocking prerequisites.", object_schema([("db", db_property()), ("config_id", string_schema("Execution config ID.", None)), ("strategy_id", string_schema("Optional strategy ID.", Some("strat_local_btc_demo"))), ("mode", string_schema("Optional mode override.", Some("paper"))), ("local_live_mandate_id", string_schema("Optional exact local Live mandate to revalidate for this configuration and caller.", None))], []), true),
        tool("tradeassembly.execution.activate", "Activate Execution", "Activate a ready connection-selected execution config and create durable execution run, checkpoint, APF evidence, and replay refs.", object_schema([("db", db_property()), ("config_id", string_schema("Execution config ID.", None)), ("strategy_id", string_schema("Optional strategy ID.", Some("strat_local_btc_demo"))), ("idempotency_key", string_schema("Required idempotency key for activation.", None)), ("acknowledgement_ids", array_schema()), ("local_live_mandate_id", string_schema("Required for Live: exact owner-issued local mandate; never caller-authored authority.", None))], []), false),
        tool("tradeassembly.execution.control", "Control Execution", "Apply an audited execution lifecycle control: pause_entries, resume_entries, stop_after_flat, stop, cancel_orders, liquidate, manual_override, or emergency_stop.", object_schema([("db", db_property()), ("activation_id", string_schema("Activation ID.", None)), ("action", string_schema("Execution control action.", Some("pause_entries"))), ("idempotency_key", string_schema("Stable idempotency key for retry-safe control.", None))], ["activation_id", "action"]), false),
        tool("tradeassembly.execution.deactivate", "Deactivate Execution", "Deactivate a selected activation and persist replayable stop evidence.", object_schema([("db", db_property()), ("activation_id", string_schema("Activation ID.", None))], ["activation_id"]), false),
        tool("tradeassembly.execution.scheduler.start", "Start Execution Scheduler", "Start local scheduler state for a selected activation. Use scheduler.run for bounded worker ticks.", object_schema([("db", db_property()), ("activation_id", string_schema("Activation ID.", None)), ("interval_seconds", integer_schema("Scheduler interval seconds.", Some(60)))], ["activation_id"]), false),
        tool("tradeassembly.execution.scheduler.run", "Run Execution Scheduler", "Run bounded scheduler ticks through the control plane for active executions.", object_schema([("db", db_property()), ("activation_id", string_schema("Optional activation ID.", None)), ("worker_id", string_schema("Worker ID.", Some("worker-local"))), ("max_cycles", integer_schema("Maximum cycles to run.", Some(1)))], []), false),
        tool("tradeassembly.execution.scheduler.stop", "Stop Execution Scheduler", "Stop local scheduler state.", object_schema([("db", db_property())], []), false),
        tool("tradeassembly.execution.status", "Execution Status", "Read one activation-scoped execution dashboard snapshot, APF audit receipts, scheduler state, orders, positions, and replay refs.", object_schema([("db", db_property()), ("strategy_id", string_schema("Optional strategy ID.", Some("strat_local_btc_demo"))), ("activation_id", string_schema("Optional explicit activation ID.", None)), ("after_sequence", integer_schema("Return events after this sequence.", Some(0))), ("event_limit", integer_schema("Maximum ordered events to return.", Some(100)))], []), true),
        tool("tradeassembly.execution.run", "Run Execution Plan", "Run an activation through the execution service. Defaults to dry-run/no order submission.", object_schema([("db", db_property()), ("activation_id", string_schema("Optional activation ID.", None)), ("submit_exit", boolean_schema("Submit exit logic.", Some(false))), ("submit_orders", boolean_schema("Submit broker orders. Defaults false for MCP safety.", Some(false)))], []), false),
        tool("tradeassembly.execution.replay", "Execution Replay", "Return activation-scoped local replay report evidence.", object_schema([("db", db_property()), ("strategy_id", string_schema("Optional strategy ID.", Some("strat_local_btc_demo"))), ("activation_id", string_schema("Optional explicit activation ID.", None)), ("after_sequence", integer_schema("Return events after this sequence.", Some(0))), ("event_limit", integer_schema("Maximum ordered events to return.", Some(100)))], []), true),
    ]
}

fn tool(
    name: &'static str,
    title: &'static str,
    description: &'static str,
    input_schema: Value,
    read_only: bool,
) -> ToolSpec {
    tool_with_metadata(
        name,
        title,
        description,
        input_schema,
        read_only,
        false,
        false,
    )
}

fn tool_with_metadata(
    name: &'static str,
    title: &'static str,
    description: &'static str,
    input_schema: Value,
    read_only: bool,
    destructive: bool,
    idempotent: bool,
) -> ToolSpec {
    ToolSpec {
        name,
        title,
        description,
        input_schema,
        read_only,
        destructive,
        idempotent,
    }
}

fn health_tool(arguments: &Value) -> Value {
    json!({
        "schemaVersion": "tradeassembly.mcp.health.v1",
        "ok": true,
        "service": "tradeassembly-mcp",
        "version": VERSION,
        "protocolVersion": MCP_PROTOCOL_VERSION,
        "transport": "stdio",
        "db": string_arg(arguments, "db", DEFAULT_DB),
        "studioBaseUrl": string_arg(arguments, "studio_base_url", DEFAULT_STUDIO_BASE_URL),
        "toolCount": tool_specs().len(),
        "credentialPosture": "local/self-hosted/customer-managed only; no TradeAssembly.org credential custody in v0",
        "noAdviceNotice": NO_ADVICE_NOTICE
    })
}

fn setup_status(arguments: &Value) -> Value {
    json!({
        "schemaVersion": "tradeassembly.mcp.setup_status.v1",
        "ok": true,
        "db": string_arg(arguments, "db", DEFAULT_DB),
        "studioBaseUrl": string_arg(arguments, "studio_base_url", DEFAULT_STUDIO_BASE_URL),
        "mcp": {
            "serveCommand": "tradeassembly mcp serve",
            "transport": "stdio",
            "protocolVersion": MCP_PROTOCOL_VERSION
        },
        "credentialBackends": {
            "default": "local-file",
            "supported": ["local-file", "env", "macos-keychain", "memory", "aws-sm"],
            "custody": "user-managed"
        },
        "noAdviceNotice": NO_ADVICE_NOTICE
    })
}

fn studio_link(arguments: &Value) -> Value {
    let base = string_arg(arguments, "studio_base_url", DEFAULT_STUDIO_BASE_URL);
    let path = string_arg(arguments, "path", "/");
    let normalized_path = if path.starts_with('/') {
        path
    } else {
        format!("/{path}")
    };
    json!({
        "schemaVersion": "tradeassembly.mcp.studio_link.v1",
        "ok": true,
        "db": string_arg(arguments, "db", DEFAULT_DB),
        "studioBaseUrl": base,
        "href": format!("{}{}", base.trim_end_matches('/'), normalized_path),
        "noAdviceNotice": NO_ADVICE_NOTICE
    })
}

fn plugin_status(arguments: &Value) -> Value {
    json!({
        "schemaVersion": "tradeassembly.mcp.plugin_status.v1",
        "ok": true,
        "registryPath": optional_string_arg(arguments, "registry_path"),
        "status": "plugin install, disable, uninstall, invoke, and provider compatibility commands are available through shared application commands",
        "tools": [
            "tradeassembly.plugin.install",
            "tradeassembly.plugin.disable",
            "tradeassembly.plugin.uninstall",
            "tradeassembly.plugin.invoke",
            "tradeassembly.plugin.list",
            "tradeassembly.plugin.capability_resolve",
            "tradeassembly.plugin.capability_matrix",
            "tradeassembly.provider.capability_matrix",
            "tradeassembly.provider.pack_compatibility"
        ]
    })
}

fn indicator_status(arguments: &Value) -> Value {
    json!({
        "schemaVersion": "tradeassembly.mcp.indicator_status.v1",
        "ok": true,
        "db": string_arg(arguments, "db", DEFAULT_DB),
        "status": "indicator requests are available through provider and market-data command surfaces; MCP exposes status only in v0",
        "credentialPosture": "provider-backed indicator calls must use local/customer-managed credentials"
    })
}

fn execution_run(arguments: &Value) -> Value {
    let call = execution_run_arguments(arguments);
    json!({
        "schemaVersion": "tradeassembly.mcp.execution_run.v1",
        "ok": true,
        "db": call["db"],
        "activationId": call["activation_id"],
        "submitExit": call["submit_exit"],
        "submitOrders": call["submit_orders"]
    })
}

pub(crate) fn redact_sensitive_payload(payload: Value) -> Value {
    match payload {
        Value::Object(values) => Value::Object(
            values
                .into_iter()
                .map(|(key, value)| {
                    if is_sensitive_key(&key) {
                        (key, Value::String("***redacted***".to_string()))
                    } else {
                        (key, redact_sensitive_payload(value))
                    }
                })
                .collect(),
        ),
        Value::Array(items) => {
            Value::Array(items.into_iter().map(redact_sensitive_payload).collect())
        }
        other => other,
    }
}

fn is_sensitive_key(key: &str) -> bool {
    let normalized = key.replace('-', "_").to_ascii_lowercase();
    SENSITIVE_KEYS.contains(&normalized.as_str())
        || normalized.ends_with("_token")
        || normalized.ends_with("_secret")
}

pub fn tool_error(name: &str, code: &str, message: &str, details: Option<Value>) -> Value {
    let details = details.map(redact_sensitive_payload);
    let mut payload = json!({
        "ok": false,
        "error": {
            "code": code,
            "message": message,
            "retryable": false
        },
        "tool": name,
        "noAdviceNotice": NO_ADVICE_NOTICE
    });
    if let Some(details) = details {
        payload["details"] = details;
    }
    json!({
        "content": [{"type": "text", "text": serde_json::to_string(&payload).unwrap_or_else(|_| "{}".to_string())}],
        "structuredContent": payload,
        "isError": true
    })
}

fn protocol_error(request_id: Value, code: i64, message: &str) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": request_id,
        "error": {
            "code": code,
            "message": message
        }
    })
}

fn object_schema<const N: usize, const R: usize>(
    properties: [(&str, Value); N],
    required: [&str; R],
) -> Value {
    let mut schema = Map::from_iter([
        ("type".to_string(), Value::String("object".to_string())),
        ("properties".to_string(), object(properties)),
        ("additionalProperties".to_string(), Value::Bool(false)),
    ]);
    if !required.is_empty() {
        schema.insert(
            "required".to_string(),
            Value::Array(
                required
                    .iter()
                    .map(|item| Value::String((*item).to_string()))
                    .collect(),
            ),
        );
    }
    Value::Object(schema)
}

fn oauth_scopes_schema() -> Value {
    json!({
        "type": "array",
        "description": "Requested scopes declared by the installed plugin manifest.",
        "items": {"type": "string", "minLength": 1, "maxLength": 128},
        "maxItems": 64,
        "uniqueItems": true
    })
}

fn agent_deployment_create_schema() -> Value {
    object_schema(
        [
            ("deployment", agent_deployment_schema()),
            (
                "idempotency_key",
                string_schema("Caller-controlled lifecycle idempotency key.", None),
            ),
            ("authority_context", agent_authority_schema()),
            ("db", db_property()),
        ],
        ["deployment", "idempotency_key", "authority_context"],
    )
}

fn agent_deployment_state_schema() -> Value {
    object_schema(
        [
            ("deployment_id", string_schema("Agent deployment ID.", None)),
            (
                "idempotency_key",
                string_schema("Caller-controlled lifecycle idempotency key.", None),
            ),
            ("authority_context", agent_authority_schema()),
            ("db", db_property()),
        ],
        ["deployment_id", "idempotency_key", "authority_context"],
    )
}

fn agent_deployment_inspect_schema() -> Value {
    object_schema(
        [
            (
                "deployment_id",
                string_schema(
                    "Optional agent deployment ID; omit to list local deployments.",
                    None,
                ),
            ),
            ("db", db_property()),
        ],
        [],
    )
}

fn agent_run_inspect_schema() -> Value {
    object_schema(
        [
            ("run_id", string_schema("Durable agent run ID.", None)),
            ("db", db_property()),
        ],
        ["run_id"],
    )
}

fn agent_run_events_schema() -> Value {
    object_schema(
        [
            ("deployment_id", string_schema("Agent deployment ID.", None)),
            (
                "after_sequence",
                integer_schema("Return events strictly after this sequence.", Some(0)),
            ),
            (
                "limit",
                integer_schema(
                    "Maximum redacted events to return; capped at 100.",
                    Some(50),
                ),
            ),
            ("db", db_property()),
        ],
        ["deployment_id"],
    )
}

fn agent_run_health_schema() -> Value {
    object_schema(
        [
            (
                "deployment_id",
                string_schema(
                    "Optional agent deployment ID; omit for all local deployments.",
                    None,
                ),
            ),
            ("db", db_property()),
        ],
        [],
    )
}

fn agent_run_recover_schema() -> Value {
    object_schema(
        [
            ("deployment_id", string_schema("Agent deployment ID.", None)),
            (
                "acknowledge_reconciled",
                boolean_schema(
                    "Must be true after deterministic evidence has been reconciled.",
                    None,
                ),
            ),
            (
                "idempotency_key",
                string_schema("Caller-controlled recovery idempotency key.", None),
            ),
            ("authority_context", agent_authority_schema()),
            ("db", db_property()),
        ],
        [
            "deployment_id",
            "acknowledge_reconciled",
            "idempotency_key",
            "authority_context",
        ],
    )
}

fn agent_deployment_schema() -> Value {
    let mut schema = object_schema(
        [
            ("executor", enum_string_schema("Agent process owner; defaults to supervised. external_client is never launched by the local supervisor.", &["supervised", "external_client"])),
            (
                "deploymentId",
                string_schema("Durable deployment ID.", None),
            ),
            (
                "systemProjectId",
                string_schema("Owning system project ID.", None),
            ),
            (
                "agentDefinitionVersionId",
                string_schema("Immutable agent definition version ID.", None),
            ),
            (
                "executionConfigVersionId",
                string_schema("Immutable execution configuration version ID.", None),
            ),
            (
                "studioToolAllowlist",
                json!({
                    "type": "array",
                    "description": "Exact MCP tools allowed for this deployment.",
                    "items": {"type": "string"},
                    "minItems": 1,
                    "uniqueItems": true,
                }),
            ),
            (
                "desiredState",
                enum_string_schema(
                    "active, paused, or stopped.",
                    &["active", "paused", "stopped"],
                ),
            ),
            (
                "intervalSeconds",
                json!({"type": "integer", "minimum": 60, "description": "Optional interval evaluator schedule in seconds; required when cronUtc is omitted."}),
            ),
            (
                "cronUtc",
                string_schema(
                    "Optional five-field, minute-resolution UTC cron schedule.",
                    None,
                ),
            ),
            (
                "mode",
                enum_string_schema("paper or live.", &["paper", "live"]),
            ),
            (
                "prompt",
                string_schema(
                    "Nonsecret operational prompt; secrets and broker credentials are rejected.",
                    None,
                ),
            ),
            (
                "workspace",
                string_schema("Existing local workspace path for the Codex runtime.", None),
            ),
            (
                "runtimeProfile",
                string_schema("Optional local Codex runtime profile.", None),
            ),
        ],
        [
            "deploymentId",
            "systemProjectId",
            "agentDefinitionVersionId",
            "executionConfigVersionId",
            "studioToolAllowlist",
            "desiredState",
            "mode",
        ],
    );
    schema["allOf"] = json!([{
        "if": {"properties": {"executor": {"const": "external_client"}}, "required": ["executor"]},
        "else": {"required": ["prompt", "workspace"]}
    }]);
    schema
}

fn agent_authority_schema() -> Value {
    object_schema(
        [
            (
                "actor",
                string_schema("Authenticated local operator or agent identity.", None),
            ),
            (
                "surface",
                string_schema("Invoking surface, such as mcp.", None),
            ),
            (
                "accountMode",
                enum_string_schema(
                    "paper or live; must match the deployment.",
                    &["paper", "live"],
                ),
            ),
        ],
        ["actor", "surface", "accountMode"],
    )
}

/// Keep nested JSON Schema references rooted in the containing tool input.
fn embedded_schema<T: schemars::JsonSchema>(pointer: &str) -> Value {
    fn rebase(value: &mut Value, pointer: &str) {
        match value {
            Value::Object(object) => {
                if let Some(Value::String(reference)) = object.get_mut("$ref") {
                    if let Some(suffix) = reference.strip_prefix('#') {
                        *reference = format!("#{pointer}{suffix}");
                    }
                }
                for child in object.values_mut() {
                    rebase(child, pointer);
                }
            }
            Value::Array(values) => values.iter_mut().for_each(|value| rebase(value, pointer)),
            _ => {}
        }
    }
    let mut schema = serde_json::to_value(schemars::schema_for!(T)).expect("serializable schema");
    schema
        .as_object_mut()
        .expect("object schema")
        .remove("$schema");
    rebase(&mut schema, pointer);
    schema
}

fn backtest_request_schema() -> Value {
    json!({
        "type":"object",
        "description":"Explicit backtest assumptions and immutable bindings. Capital, costs and risk are caller choices, not recommendations. Leave configuration.capabilityGraph identifiers empty to resolve a new backtest graph; dataset provenance retains the ingestion graph. Poll backtest.get after an idempotent create acknowledgment.",
        "properties": {
            "configuration": embedded_schema::<crate::backtest_contracts::BacktestConfiguration>("/properties/request/properties/configuration"),
            "purpose": {"type":"string"},
            "client": {"type":"string"},
            "idempotencyKey": {"type":"string"},
            "evaluationEpoch": {"type":"string"},
            "evidenceRefs": {"type":"array", "items":{"type":"string"}}
        },
        "additionalProperties":true
    })
}

fn dataset_ingestion_get_schema() -> Value {
    object_schema(
        [
            ("ingestion_id", string_schema("Dataset ingestion ID.", None)),
            (
                "observation_offset",
                json!({"type":"integer","minimum":0,"default":0}),
            ),
            (
                "observation_limit",
                json!({"type":"integer","minimum":0,"maximum":1000,"default":0,"description":"Zero returns metadata only; up to 1000 observations per explicit page."}),
            ),
            ("db", db_property()),
        ],
        ["ingestion_id"],
    )
}

fn dataset_ingestion_create_schema() -> Value {
    object_schema(
        [
            ("strategy_id", string_schema("Strategy ID.", None)),
            (
                "strategy_version_id",
                string_schema("Optional immutable strategy version ID.", None),
            ),
            (
                "plugin_instance_ref",
                string_schema("Exact historical-data plugin instance binding.", None),
            ),
            (
                "operation_id",
                string_schema("Exact plugin historical-data operation ID.", None),
            ),
            ("asset_class", string_schema("Dataset asset class.", None)),
            ("instruments", array_schema()),
            ("data_kind", string_schema("bars or option_chain.", None)),
            (
                "granularity",
                string_schema("Requested data granularity.", None),
            ),
            (
                "time_slice",
                embedded_schema::<crate::historical_data::DatasetTimeSlice>(
                    "/properties/time_slice",
                ),
            ),
            ("calendar", string_schema("Trading calendar ID.", None)),
            ("timezone", string_schema("Dataset timezone.", Some("UTC"))),
            (
                "normalization_policy",
                embedded_schema::<crate::historical_data::DatasetNormalizationPolicy>(
                    "/properties/normalization_policy",
                ),
            ),
            (
                "quality_policy",
                embedded_schema::<crate::historical_data::DatasetQualityPolicy>(
                    "/properties/quality_policy",
                ),
            ),
            (
                "max_rows",
                integer_schema("Bounded maximum row count.", Some(100_000)),
            ),
            (
                "capability_graph_revision_id",
                string_schema("Optional immutable GC-09 capability graph revision.", None),
            ),
            (
                "idempotency_key",
                string_schema("Required caller-controlled idempotency key.", None),
            ),
            (
                "authority_context",
                object_like_schema("Caller authority and surface context."),
            ),
            ("db", db_property()),
        ],
        [
            "strategy_id",
            "plugin_instance_ref",
            "operation_id",
            "asset_class",
            "instruments",
            "data_kind",
            "granularity",
            "time_slice",
            "calendar",
            "normalization_policy",
            "quality_policy",
            "idempotency_key",
        ],
    )
}

fn robustness_run_schema() -> Value {
    object_schema(
        [
            (
                "request",
                object_schema(
                    [
                        (
                            "source_run_id",
                            string_schema("Completed immutable backtest run supplying the source evidence.", None),
                        ),
                        (
                            "study_kind",
                            enum_string_schema(
                                "Deterministic robustness study kind.",
                                &[
                                    "monte_carlo",
                                    "parameter_sensitivity",
                                    "slippage_sensitivity",
                                    "fee_sensitivity",
                                    "execution_model_sensitivity",
                                    "walk_forward",
                                    "regime",
                                    "stress",
                                    "capacity_liquidity",
                                ],
                            ),
                        ),
                        (
                            "assumptions",
                            object_like_schema("Typed study assumptions; the selected study kind defines the allowed fields."),
                        ),
                        ("budget", robustness_budget_schema()),
                        (
                            "deterministic_seed",
                            integer_schema("Stable seed for deterministic replay.", None),
                        ),
                        (
                            "idempotency_key",
                            string_schema("Caller-controlled request-equivalence idempotency key.", None),
                        ),
                        (
                            "engine_version",
                            string_schema("Pinned robustness engine version.", Some("robustness-engine-v1")),
                        ),
                        (
                            "client",
                            string_schema("Client context for the durable research record.", Some("self")),
                        ),
                        (
                            "purpose",
                            string_schema("Controlled research purpose.", Some("strategy_robustness_research")),
                        ),
                        (
                            "actor",
                            string_schema("Initiating authority actor.", Some("local-user")),
                        ),
                        (
                            "surface",
                            string_schema("Initiating surface.", Some("mcp")),
                        ),
                    ],
                    [
                        "source_run_id",
                        "study_kind",
                        "assumptions",
                        "budget",
                        "deterministic_seed",
                        "idempotency_key",
                    ],
                ),
            ),
            ("db", db_property()),
        ],
        ["request"],
    )
}

fn robustness_budget_schema() -> Value {
    object_schema(
        [
            (
                "maximum_samples",
                integer_schema("Maximum deterministic samples.", None),
            ),
            (
                "maximum_grid_points",
                integer_schema("Maximum parameter grid points.", None),
            ),
            (
                "maximum_windows",
                integer_schema("Maximum walk-forward windows.", None),
            ),
            (
                "maximum_scenarios",
                integer_schema("Maximum stress or scenario count.", None),
            ),
            (
                "maximum_output_bytes",
                integer_schema("Maximum serialized output bytes.", None),
            ),
            (
                "maximum_attempts",
                integer_schema("Maximum durable attempts.", None),
            ),
        ],
        [
            "maximum_samples",
            "maximum_grid_points",
            "maximum_windows",
            "maximum_scenarios",
            "maximum_output_bytes",
            "maximum_attempts",
        ],
    )
}

fn derivatives_create_schema() -> Value {
    object_schema(
        [
            (
                "request",
                object_schema(
                    [
                        (
                            "sourceRunId",
                            string_schema(
                                "Completed immutable backtest run supplying source evidence.",
                                None,
                            ),
                        ),
                        (
                            "idempotencyKey",
                            string_schema(
                                "Caller-controlled request-equivalence idempotency key.",
                                None,
                            ),
                        ),
                        (
                            "asOfMs",
                            integer_schema(
                                "Optional analysis timestamp; defaults to the source dataset's latest observation.",
                                None,
                            ),
                        ),
                        (
                            "maxMarkAgeMs",
                            integer_schema("Maximum acceptable source mark age.", Some(900_000)),
                        ),
                        (
                            "maxGreekAgeMs",
                            integer_schema("Maximum acceptable source Greek age.", Some(900_000)),
                        ),
                        (
                            "scenarios",
                            json!({
                                "type": "array",
                                "description": "Optional declared stress scenarios. The service binds their provenance to the immutable source run.",
                                "items": {
                                    "type": "object",
                                    "additionalProperties": false,
                                    "properties": {
                                        "scenarioId": {"type": "string"},
                                        "kind": {"type": "string", "enum": ["hypothetical_stress", "historical_stress"]},
                                        "underlyingPricesMicros": {"type": "object", "additionalProperties": {"type": "integer"}},
                                        "volatilityShiftsPpm": {"type": "object", "additionalProperties": {"type": "integer"}},
                                        "elapsedSeconds": {"type": "integer"}
                                    },
                                    "required": ["scenarioId", "kind", "underlyingPricesMicros", "volatilityShiftsPpm", "elapsedSeconds"]
                                }
                            }),
                        ),
                        (
                            "sensitivityGrid",
                            object_like_schema(
                                "Optional declared sensitivity grid; the service computes all values from source evidence.",
                            ),
                        ),
                        (
                            "pricingModel",
                            object_like_schema(
                                "Optional versioned pricing-model declaration; the service records canonical provenance.",
                            ),
                        ),
                    ],
                    ["sourceRunId", "idempotencyKey"],
                ),
            ),
            ("db", db_property()),
        ],
        ["request"],
    )
}

fn comparison_create_schema() -> Value {
    object_schema(
        [
            (
                "request",
                object_schema(
                    [
                        (
                            "sourceRunIds",
                            json!({
                                "type": "array",
                                "description": "Two to five completed immutable research run IDs.",
                                "items": {"type": "string"},
                                "minItems": 2,
                                "maxItems": 5,
                                "uniqueItems": true
                            }),
                        ),
                        (
                            "scope",
                            object_schema(
                                [
                                    (
                                        "strategyId",
                                        string_schema("Owning strategy ID.", None),
                                    ),
                                    (
                                        "allowCrossVersion",
                                        boolean_schema(
                                            "Whether compatible versions of the strategy may be compared.",
                                            Some(false),
                                        ),
                                    ),
                                    (
                                        "compatibleStrategyIds",
                                        json!({
                                            "type": "array",
                                            "description": "Explicit additional compatible strategy IDs.",
                                            "items": {"type": "string"},
                                            "uniqueItems": true
                                        }),
                                    ),
                                ],
                                [
                                    "strategyId",
                                    "allowCrossVersion",
                                    "compatibleStrategyIds",
                                ],
                            ),
                        ),
                        ("policy", comparison_policy_schema()),
                        (
                            "idempotencyKey",
                            string_schema(
                                "Caller-controlled request-equivalence idempotency key.",
                                None,
                            ),
                        ),
                    ],
                    ["sourceRunIds", "scope", "idempotencyKey"],
                ),
            ),
            ("db", db_property()),
        ],
        ["request"],
    )
}

fn comparison_policy_schema() -> Value {
    object_schema(
        [
            (
                "currency",
                enum_string_schema("Currency compatibility handling.", &["block", "warn"]),
            ),
            (
                "instrumentScope",
                enum_string_schema(
                    "Instrument-scope compatibility handling.",
                    &["block", "warn"],
                ),
            ),
            (
                "datasetRange",
                enum_string_schema("Dataset-range compatibility handling.", &["block", "warn"]),
            ),
            (
                "strategyVersion",
                enum_string_schema(
                    "Strategy-version compatibility handling.",
                    &["block", "warn"],
                ),
            ),
            (
                "accounting",
                enum_string_schema("Accounting compatibility handling.", &["block", "warn"]),
            ),
            (
                "fill",
                enum_string_schema("Fill-model compatibility handling.", &["block", "warn"]),
            ),
            (
                "cost",
                enum_string_schema("Cost-model compatibility handling.", &["block", "warn"]),
            ),
            (
                "studySemantics",
                enum_string_schema(
                    "Study-semantics compatibility handling.",
                    &["block", "warn"],
                ),
            ),
        ],
        [
            "currency",
            "instrumentScope",
            "datasetRange",
            "strategyVersion",
            "accounting",
            "fill",
            "cost",
            "studySemantics",
        ],
    )
}

fn object<const N: usize>(properties: [(&str, Value); N]) -> Value {
    Value::Object(Map::from_iter(
        properties
            .into_iter()
            .map(|(key, value)| (key.to_string(), value)),
    ))
}

fn db_property() -> Value {
    string_schema(
        "Path to the local TradeAssembly database.",
        Some(DEFAULT_DB),
    )
}

fn studio_property() -> Value {
    string_schema(
        "Local or hosted TradeAssembly Studio base URL.",
        Some(DEFAULT_STUDIO_BASE_URL),
    )
}

fn string_schema(description: &str, default: Option<&str>) -> Value {
    let mut schema = Map::from_iter([
        ("type".to_string(), Value::String("string".to_string())),
        (
            "description".to_string(),
            Value::String(description.to_string()),
        ),
    ]);
    if let Some(default) = default {
        schema.insert("default".to_string(), Value::String(default.to_string()));
    }
    Value::Object(schema)
}

fn enum_string_schema(description: &str, values: &[&str]) -> Value {
    let mut schema = string_schema(description, None);
    schema["enum"] = Value::Array(
        values
            .iter()
            .map(|value| Value::String((*value).to_string()))
            .collect(),
    );
    schema
}

fn boolean_schema(description: &str, default: Option<bool>) -> Value {
    let mut schema = Map::from_iter([
        ("type".to_string(), Value::String("boolean".to_string())),
        (
            "description".to_string(),
            Value::String(description.to_string()),
        ),
    ]);
    if let Some(default) = default {
        schema.insert("default".to_string(), Value::Bool(default));
    }
    Value::Object(schema)
}

fn integer_schema(description: &str, default: Option<i64>) -> Value {
    let mut schema = Map::from_iter([
        ("type".to_string(), Value::String("integer".to_string())),
        (
            "description".to_string(),
            Value::String(description.to_string()),
        ),
    ]);
    if let Some(default) = default {
        schema.insert("default".to_string(), json!(default));
    }
    Value::Object(schema)
}

fn object_like_schema(description: &str) -> Value {
    json!({"type": "object", "description": description, "additionalProperties": true})
}

fn array_schema() -> Value {
    json!({"type": "array", "items": {"type": "string"}})
}

fn string_arg(arguments: &Value, key: &str, default: &str) -> String {
    arguments
        .get(key)
        .and_then(Value::as_str)
        .unwrap_or(default)
        .to_string()
}

fn required_nonempty_string(arguments: &Value, key: &str) -> Option<String> {
    arguments
        .get(key)
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .map(str::to_string)
}

fn optional_nonempty_string(arguments: &Value, key: &str, default: &str) -> Option<String> {
    match arguments.get(key) {
        None => Some(default.to_string()),
        Some(Value::String(value)) if !value.trim().is_empty() => Some(value.to_string()),
        Some(_) => None,
    }
}

fn has_only_keys(arguments: &Value, allowed: &[&str]) -> bool {
    arguments
        .as_object()
        .is_some_and(|values| values.keys().all(|key| allowed.contains(&key.as_str())))
}

fn optional_string_arg(arguments: &Value, key: &str) -> Value {
    arguments
        .get(key)
        .and_then(Value::as_str)
        .map(|value| Value::String(value.to_string()))
        .unwrap_or(Value::Null)
}

fn bool_arg(arguments: &Value, key: &str, default: bool) -> bool {
    arguments
        .get(key)
        .and_then(Value::as_bool)
        .unwrap_or(default)
}

#[cfg(test)]
mod tests {
    use serde_json::{json, Value};

    const REQUIRED_TOOLS: &[&str] = &[
        "tradeassembly.health",
        "tradeassembly.setup.status",
        "tradeassembly.strategy.list",
        "tradeassembly.strategy.get",
        "tradeassembly.journal.list",
        "tradeassembly.journal.export",
        "tradeassembly.journal.replay",
        "tradeassembly.strategy.create",
        "tradeassembly.strategy.save_draft",
        "tradeassembly.strategy.draft.select_node",
        "tradeassembly.strategy.draft.propose_patch",
        "tradeassembly.strategy.draft.review_patch",
        "tradeassembly.strategy.version.publish",
        "tradeassembly.strategy.validate",
        "tradeassembly.credential.status",
        "tradeassembly.provider.list",
        "tradeassembly.plugin.status",
        "tradeassembly.indicator.status",
        "tradeassembly.backtest.run",
        "tradeassembly.backtest.get",
        "tradeassembly.backtest.report",
        "tradeassembly.backtest.export",
        "tradeassembly.backtest.list",
        "tradeassembly.backtest.cancel",
        "tradeassembly.backtest.retry",
        "tradeassembly.backtest.process",
        "tradeassembly.backtest.replay",
        "tradeassembly.report.envelope",
        "tradeassembly.robustness.run",
        "tradeassembly.robustness.list",
        "tradeassembly.robustness.get",
        "tradeassembly.robustness.report",
        "tradeassembly.robustness.replay",
        "tradeassembly.robustness.export",
        "tradeassembly.robustness.process",
        "tradeassembly.robustness.cancel",
        "tradeassembly.robustness.retry",
        "tradeassembly.derivatives.create",
        "tradeassembly.derivatives.list",
        "tradeassembly.derivatives.get",
        "tradeassembly.derivatives.export",
        "tradeassembly.comparison.create",
        "tradeassembly.comparison.list",
        "tradeassembly.comparison.get",
        "tradeassembly.comparison.export",
        "tradeassembly.portfolio_risk.run",
        "tradeassembly.portfolio_risk.report",
        "tradeassembly.portfolio_risk.replay",
        "tradeassembly.portfolio_risk.export",
        "tradeassembly.fill_quality.inspect",
        "tradeassembly.fill_quality.report",
        "tradeassembly.fill_quality.replay",
        "tradeassembly.fill_quality.export",
        "tradeassembly.attribution_journal.review",
        "tradeassembly.attribution_journal.report",
        "tradeassembly.attribution_journal.inspect",
        "tradeassembly.attribution_journal.replay",
        "tradeassembly.attribution_journal.export",
        "tradeassembly.lifecycle_calendar.list",
        "tradeassembly.lifecycle_calendar.inspect",
        "tradeassembly.lifecycle_calendar.export",
        "tradeassembly.lifecycle_calendar.replay",
        "tradeassembly.research_notebook.create",
        "tradeassembly.research_notebook.compose",
        "tradeassembly.research_notebook.attach",
        "tradeassembly.research_notebook.list",
        "tradeassembly.research_notebook.inspect",
        "tradeassembly.research_notebook.export",
        "tradeassembly.research_notebook.replay",
        "tradeassembly.execution.config.save",
        "tradeassembly.execution.readiness",
        "tradeassembly.execution.activate",
        "tradeassembly.execution.control",
        "tradeassembly.execution.deactivate",
        "tradeassembly.execution.scheduler.start",
        "tradeassembly.execution.scheduler.run",
        "tradeassembly.execution.scheduler.stop",
        "tradeassembly.execution.status",
        "tradeassembly.execution.run",
        "tradeassembly.execution.replay",
        "tradeassembly.studio.link",
    ];

    #[test]
    fn tool_definitions_cover_required_agent_surface() {
        let definitions = super::tool_definitions();
        let names = definitions
            .as_array()
            .expect("tools list")
            .iter()
            .map(|tool| tool["name"].as_str().expect("tool name"))
            .collect::<std::collections::BTreeSet<_>>();

        for required in REQUIRED_TOOLS {
            assert!(names.contains(required), "missing MCP tool {required}");
        }
        for definition in definitions.as_array().expect("tools list") {
            assert_eq!(definition["inputSchema"]["type"], "object");
            assert!(!definition["name"].as_str().unwrap().contains('\n'));
        }

        let robustness_run = definitions
            .as_array()
            .expect("tools list")
            .iter()
            .find(|tool| tool["name"] == "tradeassembly.robustness.run")
            .expect("robustness run tool");
        assert_eq!(
            robustness_run["inputSchema"]["required"],
            json!(["request"])
        );
        assert_eq!(
            robustness_run["inputSchema"]["properties"]["request"]["required"],
            json!([
                "source_run_id",
                "study_kind",
                "assumptions",
                "budget",
                "deterministic_seed",
                "idempotency_key"
            ])
        );
        assert_eq!(robustness_run["annotations"]["readOnlyHint"], json!(false));
        assert_eq!(robustness_run["annotations"]["idempotentHint"], json!(true));

        let robustness_cancel = definitions
            .as_array()
            .expect("tools list")
            .iter()
            .find(|tool| tool["name"] == "tradeassembly.robustness.cancel")
            .expect("robustness cancel tool");
        assert_eq!(
            robustness_cancel["annotations"]["destructiveHint"],
            json!(true)
        );
        assert_eq!(
            robustness_cancel["inputSchema"]["required"],
            json!(["run_id", "idempotency_key"])
        );

        for compatibility_alias in [
            "tradeassembly.scenario.valuation",
            "tradeassembly.monte_carlo.run",
            "tradeassembly.monte_carlo.status",
            "tradeassembly.monte_carlo.report",
            "tradeassembly.monte_carlo.replay",
            "tradeassembly.monte_carlo.export",
        ] {
            assert!(
                names.contains(compatibility_alias),
                "compatibility alias missing: {compatibility_alias}"
            );
        }
        assert_eq!(
            super::call_tool("tradeassembly.monte_carlo.run", json!({}))["structuredContent"]
                ["error"]["code"],
            json!("service_required")
        );

        let publish = definitions
            .as_array()
            .expect("tools list")
            .iter()
            .find(|tool| tool["name"] == "tradeassembly.strategy.version.publish")
            .expect("publish tool");
        assert_eq!(
            publish["inputSchema"]["properties"]["expected_draft_hash"]["type"],
            "string"
        );
        assert_eq!(
            publish["inputSchema"]["required"],
            json!(["strategy_id", "expected_draft_hash", "actor"])
        );

        let dataset_create = definitions
            .as_array()
            .expect("tools list")
            .iter()
            .find(|tool| tool["name"] == "tradeassembly.dataset_ingestion.create")
            .expect("dataset ingestion create tool");
        assert!(dataset_create["inputSchema"]["required"]
            .as_array()
            .expect("required fields")
            .contains(&json!("operation_id")));
    }

    #[test]
    fn plugin_oauth_tools_are_discoverable_and_strict() {
        let definitions = super::tool_definitions();
        let definitions = definitions.as_array().expect("tool definitions");
        for (name, required) in [
            (
                "tradeassembly.plugin.oauth.start",
                vec!["instanceRef", "environment", "scopes", "idempotencyKey"],
            ),
            (
                "tradeassembly.plugin.oauth.status",
                vec!["instanceRef", "connectionId", "idempotencyKey"],
            ),
            (
                "tradeassembly.plugin.oauth.disconnect",
                vec!["instanceRef", "idempotencyKey"],
            ),
        ] {
            let definition = definitions
                .iter()
                .find(|definition| definition["name"] == name)
                .unwrap_or_else(|| panic!("missing {name}"));
            assert_eq!(definition["inputSchema"]["type"], json!("object"));
            assert_eq!(
                definition["inputSchema"]["additionalProperties"],
                json!(false)
            );
            assert_eq!(definition["inputSchema"]["required"], json!(required));
            assert_eq!(definition["annotations"]["idempotentHint"], json!(true));
            let properties = definition["inputSchema"]["properties"]
                .as_object()
                .expect("OAuth properties");
            assert!(!properties.keys().any(|key| {
                matches!(
                    key.as_str(),
                    "access_token" | "api_key" | "api_secret" | "token"
                )
            }));
            if name.ends_with(".start") {
                assert_eq!(properties["scopes"]["uniqueItems"], json!(true));
                assert_eq!(properties["scopes"]["items"]["type"], json!("string"));
            }
        }
    }

    #[test]
    fn standalone_plugin_oauth_calls_require_service_dispatch() {
        for name in [
            "tradeassembly.plugin.oauth.start",
            "tradeassembly.plugin.oauth.status",
            "tradeassembly.plugin.oauth.disconnect",
        ] {
            let response = super::call_tool(name, json!({}));
            assert_eq!(
                response["structuredContent"]["error"]["code"],
                json!("service_required")
            );
        }
    }

    #[test]
    fn service_plugin_oauth_dispatch_preserves_anonymous_authorization() {
        let db = temp_db("oauth-mcp-auth");
        let service = crate::service::TradeAssemblyService::test_local(&db);
        let response = service.call_mcp_tool(
            "tradeassembly.plugin.oauth.start",
            json!({
                "instanceRef": "plugin-instance",
                "environment": "staging",
                "scopes": [],
                "idempotencyKey": "oauth-mcp-auth-1"
            }),
        );
        let code = response["error"]["code"]
            .as_str()
            .or_else(|| response["structuredContent"]["error"]["code"].as_str())
            .unwrap_or_default();
        assert!(!code.is_empty(), "anonymous OAuth call must be denied");
        assert_ne!(code, "unknown_mcp_tool");
    }

    #[test]
    fn initialize_list_and_call_over_stdio() {
        let db = temp_db("stdio-init");
        let input = [
            json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {"protocolVersion": super::MCP_PROTOCOL_VERSION}}).to_string(),
            json!({"jsonrpc": "2.0", "method": "notifications/initialized"}).to_string(),
            json!({"jsonrpc": "2.0", "id": 2, "method": "tools/list", "params": {}}).to_string(),
            json!({
                "jsonrpc": "2.0",
                "id": 3,
                "method": "tools/call",
                "params": {
                    "name": "tradeassembly.health",
                    "arguments": {"db": db, "studio_base_url": "http://127.0.0.1:3001"}
                }
            })
            .to_string(),
        ]
        .join("\n");

        let service = crate::service::TradeAssemblyService::test_local(&db);
        let served = super::serve_stdio_text_with_service(
            &service,
            &input,
            &db,
            super::DEFAULT_STUDIO_BASE_URL,
        );
        assert_eq!(served.exit_code, 0);
        assert_eq!(served.stderr, "");
        let responses = served
            .stdout
            .lines()
            .map(|line| serde_json::from_str::<Value>(line).expect("json response"))
            .collect::<Vec<_>>();

        assert_eq!(
            responses
                .iter()
                .map(|response| response["id"].as_i64().unwrap())
                .collect::<Vec<_>>(),
            vec![1, 2, 3]
        );
        assert_eq!(
            responses[0]["result"]["capabilities"]["tools"]["listChanged"],
            json!(false)
        );
        assert!(responses[1]["result"]["tools"]
            .as_array()
            .unwrap()
            .iter()
            .any(|tool| tool["name"] == "tradeassembly.health"));
        let health = &responses[2]["result"]["structuredContent"];
        assert_eq!(health["ok"], json!(true));
        assert_eq!(health["transport"], "stdio");
        assert_eq!(health["db"], db);
    }

    #[test]
    fn tool_call_returns_structured_content_and_redacts_secrets() {
        let db = temp_db("credential-status");
        let raw = json!({
            "providerRef": "alpaca-paper",
            "db": db,
            "api_key": "KEY_SHOULD_NOT_LEAK",
            "nested": {"access_token": "TOKEN_SHOULD_NOT_LEAK", "key_mask": "KEY1...KEY2"}
        });
        let result = super::call_tool_with_payload("tradeassembly.credential.status", raw);
        let structured = &result["structuredContent"];
        let text = result["content"][0]["text"].as_str().unwrap();

        assert_eq!(result["isError"], json!(false));
        assert_eq!(structured["api_key"], "***redacted***");
        assert_eq!(structured["nested"]["access_token"], "***redacted***");
        assert_eq!(structured["nested"]["key_mask"], "KEY1...KEY2");
        assert!(!text.contains("KEY_SHOULD_NOT_LEAK"));
        assert!(!text.contains("TOKEN_SHOULD_NOT_LEAK"));
    }

    #[test]
    fn standalone_robustness_helper_never_fabricates_a_result() {
        let response = super::call_tool(
            "tradeassembly.robustness.run",
            json!({
                "request": {
                    "source_run_id": "backtest_1",
                    "study_kind": "monte_carlo",
                    "assumptions": {},
                    "budget": {},
                    "deterministic_seed": 7,
                    "idempotency_key": "robustness-1"
                }
            }),
        );

        assert_eq!(response["isError"], json!(true));
        assert_eq!(
            response["structuredContent"]["error"]["code"],
            json!("service_required")
        );
        assert!(!response["structuredContent"]
            .as_object()
            .expect("structured error")
            .contains_key("result"));
    }

    #[test]
    fn standalone_derivatives_helper_never_fabricates_an_analysis() {
        let response = super::call_tool(
            "tradeassembly.derivatives.create",
            json!({
                "request": {
                    "sourceRunId": "backtest_1",
                    "idempotencyKey": "derivatives-1"
                }
            }),
        );

        assert_eq!(response["isError"], json!(true));
        assert_eq!(
            response["structuredContent"]["error"]["code"],
            json!("service_required")
        );
        assert!(!response.to_string().contains("outputHash"));
    }

    #[test]
    fn standalone_comparison_helper_never_fabricates_an_artifact() {
        let response = super::call_tool(
            "tradeassembly.comparison.create",
            json!({
                "request": {
                    "sourceRunIds": ["backtest_1", "backtest_2"],
                    "scope": {
                        "strategyId": "strategy_1",
                        "allowCrossVersion": false,
                        "compatibleStrategyIds": []
                    },
                    "idempotencyKey": "comparison-1"
                }
            }),
        );

        assert_eq!(response["isError"], json!(true));
        assert_eq!(
            response["structuredContent"]["error"]["code"],
            json!("service_required")
        );
        assert!(!response.to_string().contains("comparisonHash"));
    }

    #[test]
    fn execution_run_defaults_to_dry_run() {
        let call =
            super::execution_run_arguments(&json!({"db": "local.db", "activation_id": "act_1"}));

        assert_eq!(
            call,
            json!({"db": "local.db", "activation_id": "act_1", "submit_exit": false, "submit_orders": false})
        );
    }

    #[test]
    fn serve_cli_wires_stdio_without_protocol_noise() {
        let db = temp_db("stdio-cli");
        let input = json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "tools/call",
            "params": {"name": "tradeassembly.health", "arguments": {}}
        })
        .to_string();

        let service = crate::service::TradeAssemblyService::test_local(&db);
        let served =
            super::serve_stdio_text_with_service(&service, &input, &db, "http://127.0.0.1:3001");
        assert_eq!(served.exit_code, 0);
        let response = serde_json::from_str::<Value>(served.stdout.trim()).expect("json response");
        assert_eq!(response["id"], json!(1));
        assert_eq!(response["result"]["structuredContent"]["db"], db);
        assert_eq!(
            response["result"]["structuredContent"]["studioBaseUrl"],
            "http://127.0.0.1:3001"
        );
    }

    fn temp_db(name: &str) -> String {
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock before unix epoch")
            .as_nanos();
        std::env::temp_dir()
            .join(format!(
                "tradeassembly-mcp-{name}-{}-{stamp}.db",
                std::process::id()
            ))
            .to_string_lossy()
            .to_string()
    }
}
