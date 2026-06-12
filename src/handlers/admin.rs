//! Admin API under the reserved `/_mockmesh` prefix: inspect loaded routes
//! and flip simulation state at runtime. Rules are addressed by their full
//! id (percent-encoded, e.g. `GET%20%2Fusers%2F%7Bid%7D`) or by the short
//! `key` shown in the route listing.

use std::sync::Arc;
use std::sync::atomic::Ordering;

use axum::Json;
use axum::Router;
use axum::extract::{Path, Request, State};
use axum::http::{StatusCode, header};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post, put};
use serde::{Deserialize, Serialize};
use tokio::sync::oneshot;

use crate::config::model::{ErrorModeSpec, LatencySpec};
use crate::rules::{MockRule, RuleSource};
use crate::state::{AppState, ReloadRequest};

pub fn router(state: AppState) -> Router<AppState> {
    let mut router = Router::new()
        .route("/health", get(health))
        .route("/routes", get(list_routes))
        .route("/routes/{id}", get(get_route))
        .route(
            "/routes/{id}/overrides",
            put(set_overrides).delete(clear_overrides),
        )
        .route("/routes/{id}/rate-limit/reset", post(reset_rate_limit))
        .route("/reload", post(reload))
        .route("/config", get(effective_config));
    if state.0.admin_token.is_some() {
        router = router.layer(middleware::from_fn_with_state(state, require_token));
    }
    router
}

async fn require_token(State(state): State<AppState>, req: Request, next: Next) -> Response {
    let Some(expected) = state.0.admin_token.as_deref() else {
        return next.run(req).await;
    };
    let provided = req
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "));
    match provided {
        Some(token) if constant_time_eq(token.as_bytes(), expected.as_bytes()) => {
            next.run(req).await
        }
        _ => (
            StatusCode::UNAUTHORIZED,
            Json(serde_json::json!({ "error": "missing or invalid admin token" })),
        )
            .into_response(),
    }
}

/// Constant-time byte comparison. Length is still observable, which is
/// acceptable for a local dev tool token.
fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

async fn health(State(state): State<AppState>) -> Json<serde_json::Value> {
    let table = state.table();
    Json(serde_json::json!({
        "status": "ok",
        "generation": table.generation,
        "routes": table.rules.len(),
        "uptime_secs": state.0.started.elapsed().as_secs(),
    }))
}

#[derive(Serialize)]
struct RouteInfo {
    id: String,
    key: String,
    method: String,
    path: String,
    source: RuleSource,
    response_kind: &'static str,
    behavior: crate::config::model::Behavior,
    overrides: OverrideInfo,
    hits: u64,
}

#[derive(Serialize)]
struct OverrideInfo {
    error_enabled: bool,
    error_mode: Option<ErrorModeSpec>,
    latency: Option<LatencySpec>,
}

fn route_info(rule: &Arc<MockRule>) -> RouteInfo {
    RouteInfo {
        id: rule.id.clone(),
        key: rule.key.clone(),
        method: rule
            .method
            .as_ref()
            .map_or_else(|| "ANY".to_string(), |m| m.to_string()),
        path: rule.path.raw.clone(),
        source: rule.source,
        response_kind: rule.plan.kind(),
        behavior: rule.behavior.clone(),
        overrides: OverrideInfo {
            error_enabled: rule.runtime.error_enabled.load(Ordering::Relaxed),
            error_mode: rule
                .runtime
                .error_override
                .load_full()
                .map(|a| (*a).clone()),
            latency: rule
                .runtime
                .latency_override
                .load_full()
                .map(|a| (*a).clone()),
        },
        hits: rule.runtime.hits.load(Ordering::Relaxed),
    }
}

async fn list_routes(State(state): State<AppState>) -> Json<serde_json::Value> {
    let table = state.table();
    let routes: Vec<RouteInfo> = table.rules.iter().map(route_info).collect();
    Json(serde_json::json!({
        "generation": table.generation,
        "routes": routes,
    }))
}

async fn get_route(State(state): State<AppState>, Path(id): Path<String>) -> Response {
    let table = state.table();
    match table.rule_by_id(&id) {
        Some(rule) => Json(route_info(rule)).into_response(),
        None => unknown_rule(&id),
    }
}

/// PUT semantics: the payload *replaces* the whole override set. Omitted
/// fields are cleared, `enabled` defaults to true.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct OverridePayload {
    #[serde(default)]
    error_mode: Option<ErrorModeSpec>,
    #[serde(default)]
    latency: Option<LatencySpec>,
    #[serde(default = "default_true")]
    enabled: bool,
}

fn default_true() -> bool {
    true
}

async fn set_overrides(
    State(state): State<AppState>,
    Path(id): Path<String>,
    payload: Result<Json<OverridePayload>, axum::extract::rejection::JsonRejection>,
) -> Response {
    let Json(payload) = match payload {
        Ok(p) => p,
        Err(e) => {
            return (
                StatusCode::UNPROCESSABLE_ENTITY,
                Json(serde_json::json!({ "error": e.body_text() })),
            )
                .into_response();
        }
    };
    if let Some(ErrorModeSpec::Status {
        code, probability, ..
    }) = &payload.error_mode
    {
        let prob_ok = probability.is_none_or(|p| (0.0..=1.0).contains(&p));
        if StatusCode::from_u16(*code).is_err() || !prob_ok {
            return (
                StatusCode::UNPROCESSABLE_ENTITY,
                Json(serde_json::json!({ "error": "invalid status code or probability" })),
            )
                .into_response();
        }
    }
    let table = state.table();
    let Some(rule) = table.rule_by_id(&id) else {
        return unknown_rule(&id);
    };
    rule.runtime
        .error_override
        .store(payload.error_mode.map(Arc::new));
    rule.runtime
        .latency_override
        .store(payload.latency.map(Arc::new));
    rule.runtime
        .error_enabled
        .store(payload.enabled, Ordering::Relaxed);
    Json(route_info(rule)).into_response()
}

async fn clear_overrides(State(state): State<AppState>, Path(id): Path<String>) -> Response {
    let table = state.table();
    let Some(rule) = table.rule_by_id(&id) else {
        return unknown_rule(&id);
    };
    rule.runtime.clear_overrides();
    Json(route_info(rule)).into_response()
}

async fn reset_rate_limit(State(state): State<AppState>, Path(id): Path<String>) -> Response {
    let table = state.table();
    let Some(rule) = table.rule_by_id(&id) else {
        return unknown_rule(&id);
    };
    match rule.runtime.bucket.load_full() {
        Some(bucket) => {
            bucket.reset();
            Json(serde_json::json!({ "reset": true, "available": bucket.available() }))
                .into_response()
        }
        None => (
            StatusCode::CONFLICT,
            Json(serde_json::json!({ "error": "rule has no rate limit configured" })),
        )
            .into_response(),
    }
}

async fn reload(State(state): State<AppState>) -> Response {
    let (tx, rx) = oneshot::channel();
    if state
        .0
        .reload_tx
        .send(ReloadRequest { respond: Some(tx) })
        .await
        .is_err()
    {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(serde_json::json!({ "error": "reload loop is not running" })),
        )
            .into_response();
    }
    match rx.await {
        Ok(Ok(generation)) => Json(serde_json::json!({ "generation": generation })).into_response(),
        Ok(Err(message)) => (
            StatusCode::UNPROCESSABLE_ENTITY,
            Json(serde_json::json!({ "error": message })),
        )
            .into_response(),
        Err(_) => (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(serde_json::json!({ "error": "reload loop dropped the request" })),
        )
            .into_response(),
    }
}

async fn effective_config(State(state): State<AppState>) -> Json<serde_json::Value> {
    let table = state.table();
    Json(serde_json::json!({
        "generation": table.generation,
        "config": &*table.config,
    }))
}

fn unknown_rule(id: &str) -> Response {
    (
        StatusCode::NOT_FOUND,
        Json(serde_json::json!({ "error": format!("no rule with id or key {id:?}") })),
    )
        .into_response()
}

#[cfg(test)]
mod tests {
    use super::constant_time_eq;

    #[test]
    fn ct_eq_basics() {
        assert!(constant_time_eq(b"secret", b"secret"));
        assert!(!constant_time_eq(b"secret", b"secreT"));
        assert!(!constant_time_eq(b"secret", b"secre"));
        assert!(constant_time_eq(b"", b""));
    }
}
