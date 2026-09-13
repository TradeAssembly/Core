// Copyright (c) 2026 OptionLab LLC. All rights reserved.

use crate::service::{ServiceResponse, TradeAssemblyService};
use axum::{
    body::Body,
    extract::State,
    http::{Method, Request, StatusCode},
    response::{IntoResponse, Response},
    routing::any,
    Json, Router,
};
use serde_json::{json, Value};
use std::net::SocketAddr;
use tower_http::cors::{Any, CorsLayer};

#[derive(Clone)]
pub struct HttpState {
    service: TradeAssemblyService,
}

pub fn build_router(service: TradeAssemblyService) -> Router {
    let state = HttpState { service };
    Router::new()
        .fallback(any(dispatch))
        .layer(
            CorsLayer::new()
                .allow_origin(Any)
                .allow_methods(Any)
                .allow_headers(Any),
        )
        .with_state(state)
}

pub async fn serve(service: TradeAssemblyService, host: &str, port: u16) -> Result<(), String> {
    let addr: SocketAddr = format!("{host}:{port}")
        .parse()
        .map_err(|error| format!("invalid listen address: {error}"))?;
    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .map_err(|error| error.to_string())?;
    axum::serve(listener, build_router(service))
        .await
        .map_err(|error| error.to_string())
}

async fn dispatch(
    State(state): State<HttpState>,
    method: Method,
    request: Request<Body>,
) -> Response {
    let path = request
        .uri()
        .path_and_query()
        .map(|value| value.as_str().to_string())
        .unwrap_or_else(|| "/".to_string());
    let route_path = path.split('?').next().unwrap_or("");
    let is_graphql = route_path == "/graphql";
    if !route_is_documented(method.as_str(), route_path) {
        return into_response(ServiceResponse::not_found(route_path));
    }
    let studio_bearer = request
        .headers()
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
        .map(str::to_string);
    let bytes = match axum::body::to_bytes(request.into_body(), 1024 * 1024).await {
        Ok(bytes) => bytes,
        Err(error) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({"detail": format!("invalid request body: {error}")})),
            )
                .into_response()
        }
    };
    let body = if bytes.is_empty() {
        json!({})
    } else {
        serde_json::from_slice::<Value>(&bytes).unwrap_or_else(|_| json!({}))
    };
    if is_graphql && crate::graphql::is_standard_introspection(&body) {
        return (StatusCode::OK, Json(crate::graphql::execute(body).await)).into_response();
    }
    let service = state.service.clone();
    let method = method.to_string();
    let response = tokio::task::spawn_blocking(move || {
        if is_graphql {
            service.handle_studio_graphql(studio_bearer.as_deref(), body)
        } else {
            service.handle_http(&method, &path, body)
        }
    })
    .await
    .unwrap_or_else(|error| {
        ServiceResponse::internal_error(&format!("HTTP service worker failed: {error}"))
    });
    into_response(response)
}

fn into_response(response: ServiceResponse) -> Response {
    let status = StatusCode::from_u16(response.status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
    (status, Json(response.body)).into_response()
}

pub fn documented_routes() -> Vec<&'static str> {
    HTTP_ROUTE_REGISTRY.to_vec()
}

pub fn route_is_documented(method: &str, path: &str) -> bool {
    HTTP_ROUTE_REGISTRY.iter().any(|route| {
        let Some((route_method, route_path)) = route.split_once(' ') else {
            return false;
        };
        route_method == method.to_ascii_uppercase() && path_matches(route_path, path)
    })
}

fn path_matches(template: &str, path: &str) -> bool {
    let template_segments = template
        .trim_matches('/')
        .split('/')
        .filter(|segment| !segment.is_empty())
        .collect::<Vec<_>>();
    let path_segments = path
        .trim_matches('/')
        .split('/')
        .filter(|segment| !segment.is_empty())
        .collect::<Vec<_>>();
    template_segments.len() == path_segments.len()
        && template_segments
            .iter()
            .zip(path_segments)
            .all(|(template, actual)| segment_matches(template, actual))
}

fn segment_matches(template: &str, actual: &str) -> bool {
    let Some(open) = template.find('{') else {
        return template == actual;
    };
    let Some(close_offset) = template[open..].find('}') else {
        return template == actual;
    };
    let close = open + close_offset;
    let prefix = &template[..open];
    let suffix = &template[close + 1..];
    actual.starts_with(prefix)
        && actual.ends_with(suffix)
        && actual.len() > prefix.len() + suffix.len()
}

const HTTP_ROUTE_REGISTRY: &[&str] = &[
    "DELETE /plugins/instances/{ref}",
    "DELETE /plugins/instances/{ref}/credentials",
    "DELETE /entitlements/{grant_id}",
    "DELETE /providers/{ref}/credentials",
    "GET /admin/providers",
    "GET /admin/control-plane",
    "GET /admin/risk",
    "GET /admin/scheduler",
    "GET /admin/status",
    "GET /admin/workspace",
    "GET /backtests",
    "GET /backtests/{run_id}",
    "GET /backtests/{run_id}/report",
    "GET /dataset-ingestions",
    "GET /dataset-ingestions/{ingestion_id}",
    "GET /capability-graph-revisions/{revision_id}",
    "GET /entitlements",
    "GET /exit-watches",
    "GET /health",
    "GET /journal/events",
    "GET /marketdata/bars",
    "GET /marketdata/instrument-packs",
    "GET /marketdata/instruments/aliases",
    "GET /marketdata/options/chain",
    "GET /marketdata/quote",
    "GET /orders",
    "GET /orders/attempts",
    "GET /plugins/instances",
    "GET /plugins/instances/{ref}",
    "GET /plugins/instances/{ref}/credentials",
    "GET /plugins/manifests",
    "GET /plugins/manifests/{ref}",
    "GET /portfolio/positions",
    "GET /providers",
    "GET /providers/broker-contracts",
    "GET /providers/policy",
    "GET /providers/{ref}/credentials",
    "GET /derivatives-analyses",
    "GET /derivatives-analyses/{analysis_id}",
    "GET /derivatives-analyses/{analysis_id}/exports/{format}",
    "GET /product/sightline/events",
    "GET /product/sightline/session-state",
    "GET /ready",
    "GET /research/artifacts",
    "GET /research-comparisons",
    "GET /research-comparisons/{comparison_id}",
    "GET /research-comparisons/{comparison_id}/exports/{format}",
    "GET /research/jobs",
    "GET /research/sweeps",
    "GET /research/universes",
    "GET /robustness-runs",
    "GET /robustness-runs/{run_id}",
    "GET /robustness-runs/{run_id}/report",
    "GET /risk/reservations",
    "GET /risk/status",
    "GET /runs",
    "GET /scheduler/status",
    "GET /status",
    "GET /strategies",
    "GET /strategies/{strategy_id}",
    "GET /strategy/contracts/indicator-schema",
    "GET /strategy/contracts/schema",
    "GET /strategy/contracts/stages",
    "GET /strategy/contracts/substeps",
    "GET /workspace",
    "POST /backtests",
    "POST /backtests/{run_id}/cancel",
    "POST /backtests/{run_id}/retry",
    "POST /backtests/{run_id}/replay",
    "POST /backtests:process",
    "POST /backtests/{backtest_id}/export",
    "POST /capabilities/resolve",
    "POST /capabilities/graph/resolve",
    "POST /capability-graph-revisions",
    "POST /capability-graph-revisions/currentness",
    "POST /dataset-ingestions",
    "POST /dataset-ingestions/{ingestion_id}/cancel",
    "POST /dataset-ingestions/{ingestion_id}/verify",
    "POST /derivatives-analyses",
    "POST /demo/btc-exit",
    "POST /demo/reset",
    "POST /demo/reset-btc",
    "POST /entitlements/deny",
    "POST /entitlements/grant",
    "POST /execution/fill-quality",
    "POST /execution/lifecycle-calendar",
    "POST /execution/position-lifecycle/{operation}",
    "POST /exit-watches/process",
    "POST /graphql",
    "POST /journal/attribution-review",
    "POST /journal/replay",
    "POST /journal/replay-harness",
    "POST /journal/replay-report",
    "POST /marketdata/conformance",
    "POST /marketdata/indicators",
    "POST /marketdata/instrument-packs/install",
    "POST /marketdata/instrument-packs/validate",
    "POST /marketdata/instruments/resolve",
    "POST /marketdata/options/select",
    "POST /marketdata/selectors/evaluate",
    "POST /marketdata/selectors/explain",
    "POST /orders/reconcile",
    "POST /orders/status-stream",
    "POST /orders/status-stream/watch",
    "POST /orders/{order_id}/status-events",
    "POST /plugins/capabilities/resolve",
    "POST /plugins/instances",
    "POST /plugins/instances/{ref}:disable",
    "POST /plugins/instances/{ref}:enable",
    "POST /plugins/instances/{ref}:rollback",
    "POST /plugins/instances/{ref}:upgrade",
    "POST /plugins/instances/{ref}/credentials",
    "POST /plugins/oauth/start",
    "POST /plugins/oauth/status",
    "POST /plugins/oauth/disconnect",
    "POST /plugins/instances/{ref}/health:refresh",
    "POST /plugins/instances/{ref}/operations/{operation_id}:invoke",
    "POST /plugins/manifests",
    "POST /plugins/packages",
    "POST /portfolio/positions/sync",
    "POST /product/alerts/acknowledge",
    "POST /product/alerts/center",
    "POST /product/connect-workspace",
    "POST /product/discover-workspace",
    "POST /product/plugins/providers",
    "POST /product/providers/credentials/test",
    "POST /product/providers/disable-instance",
    "POST /product/providers/enable-instance",
    "POST /product/providers/list",
    "POST /product/providers/update-instance",
    "POST /product/reports/envelope",
    "POST /product/research/rollup",
    "POST /product/run-center",
    "POST /product/run-center/run-once",
    "POST /product/scheduler/start",
    "POST /product/scheduler/stop",
    "POST /product/share-snapshots/export",
    "POST /product/share-snapshots/view",
    "POST /product/sightline/agents/connect",
    "POST /product/sightline/approvals/request",
    "POST /product/sightline/context/get",
    "POST /product/sightline/navigation/request",
    "POST /product/sightline/nodes/focus",
    "POST /product/sightline/nodes/highlight",
    "POST /product/sightline/presence/update",
    "POST /product/sightline/proposals/create",
    "POST /product/sightline/selections/set",
    "POST /product/sightline/selections/share",
    "POST /product/sightline/strategy-editor/publish",
    "POST /product/sightline/studio-surface/publish",
    "POST /product/sightline/surfaces/register",
    "POST /product/strategies/ai-draft",
    "POST /product/strategies/archive",
    "POST /product/strategies/backtests/export",
    "POST /product/strategies/backtests/run",
    "POST /product/strategies/builder-state",
    "POST /product/strategies/create",
    "POST /product/strategies/duplicate",
    "POST /product/strategies/execution-workspace",
    "POST /product/strategies/fill-quality",
    "POST /product/strategies/fill-quality/export",
    "POST /product/strategies/fill-quality/inspect",
    "POST /product/strategies/fill-quality/replay",
    "POST /product/strategies/fill-quality/report",
    "POST /product/strategies/attribution-journal",
    "POST /product/strategies/attribution-journal/export",
    "POST /product/strategies/attribution-journal/inspect",
    "POST /product/strategies/attribution-journal/replay",
    "POST /product/strategies/attribution-journal/report",
    "POST /product/strategies/attribution-journal/review",
    "POST /product/strategies/get",
    "POST /product/strategies/home",
    "POST /product/strategies/instrument-context",
    "POST /product/strategies/lifecycle-calendar",
    "POST /product/strategies/lifecycle-calendar/export",
    "POST /product/strategies/lifecycle-calendar/inspect",
    "POST /product/strategies/lifecycle-calendar/list",
    "POST /product/strategies/lifecycle-calendar/replay",
    "POST /product/strategies/publish",
    "POST /product/strategies/monte-carlo",
    "POST /product/strategies/monte-carlo/export",
    "POST /product/strategies/monte-carlo/report",
    "POST /product/strategies/monte-carlo/replay",
    "POST /product/strategies/monte-carlo/status",
    "POST /product/strategies/portfolio-risk",
    "POST /product/strategies/portfolio-risk/export",
    "POST /product/strategies/portfolio-risk/report",
    "POST /product/strategies/portfolio-risk/replay",
    "POST /product/strategies/proposals/apply",
    "POST /product/strategies/proposals/create",
    "POST /product/strategies/proposals/review",
    "POST /product/strategies/research-notebook",
    "POST /product/strategies/research-notebook/attach",
    "POST /product/strategies/research-notebook/compose",
    "POST /product/strategies/research-notebook/create",
    "POST /product/strategies/research-notebook/export",
    "POST /product/strategies/research-notebook/inspect",
    "POST /product/strategies/research-notebook/list",
    "POST /product/strategies/research-notebook/replay",
    "POST /product/strategies/research-workspace",
    "POST /product/strategies/research-sweeps/create",
    "POST /product/strategies/research/datasets/create",
    "POST /product/strategies/research/jobs/create",
    "POST /product/strategies/research/jobs/status",
    "POST /product/strategies/research/promote-to-paper",
    "POST /product/strategies/research-runs/create",
    "POST /robustness-runs",
    "POST /robustness-runs/{run_id}/cancel",
    "POST /robustness-runs/{run_id}/retry",
    "POST /robustness-runs/{run_id}/replay",
    "POST /robustness-runs/{run_id}/export",
    "POST /robustness-runs:process",
    "POST /research-comparisons",
    "POST /product/strategies/restore",
    "POST /product/strategies/save-draft",
    "POST /product/strategies/scenario-valuation",
    "POST /product/strategies/semantic-selection",
    "POST /product/strategies/share-snapshots/create",
    "POST /product/strategies/share-snapshots/revoke",
    "POST /product/strategies/share-workspace",
    "POST /product/strategies/validate-draft",
    "POST /product/strategies/validate-expression",
    "POST /product/strategies/version-history",
    "POST /product/strategy-execution-activations/activate",
    "POST /product/strategy-execution-activations/control",
    "POST /product/strategy-execution-activations/deactivate",
    "POST /product/strategy-execution-configs/activation-readiness",
    "POST /product/strategy-execution-configs/save",
    "POST /product/viewer",
    "POST /providers/broker-conformance",
    "POST /providers/{ref}/credentials",
    "POST /providers/{ref}/credentials/test",
    "POST /research/datasets",
    "POST /research/jobs",
    "POST /research/sweeps",
    "POST /research/universes",
    "POST /risk/account/sync",
    "POST /risk/margin-preview",
    "POST /risk/portfolio-overlay",
    "POST /risk/reconcile",
    "POST /risk/stress-test",
    "POST /run",
    "POST /scheduler/run",
    "POST /scheduler/start",
    "POST /scheduler/stop",
    "POST /scheduler/tick",
    "POST /strategies",
    "POST /strategies/draft",
    "POST /strategies/{strategy_id}/backtests",
    "POST /strategy/contracts/capability:resolve",
    "POST /strategy/contracts/compatibility:resolve",
    "POST /strategy/contracts/compile-preview",
    "POST /strategy/contracts/envelope-preview",
    "POST /strategy/contracts/validate",
    "PUT /plugins/instances/{ref}/configuration",
    "PUT /providers/policy",
    "PUT /risk/account",
];
