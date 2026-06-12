//! The catch-all fallback handler — the hot path. Per request:
//! one wait-free table load, one hash lookup (static paths), an atomic hit
//! count, then simulation + response. Fixed/example bodies are pre-serialized
//! `Bytes`, so the common case allocates nothing but the response envelope.

use std::sync::atomic::Ordering;

use axum::body::Body;
use axum::extract::State;
use axum::http::{Request, Response, StatusCode, header};
use axum::response::IntoResponse;
use bytes::Bytes;
use rand::rngs::SmallRng;
use rand::{RngExt, SeedableRng};

use crate::config::model::RateLimitSpec;
use crate::fake::GenCtx;
use crate::rules::{MockRule, ResponsePlan, fnv1a64, matcher};
use crate::server::ConnKiller;
use crate::simulate::error_mode::{self, ErrorAction};
use crate::simulate::latency;
use crate::state::AppState;

pub async fn handle(State(state): State<AppState>, req: Request<Body>) -> Response<Body> {
    let table = state.table();
    let Some(rule) = matcher::find_rule(&table, req.method(), req.uri().path()) else {
        return json_response(
            StatusCode::NOT_FOUND,
            &serde_json::json!({
                "error": "no mock rule matched",
                "method": req.method().as_str(),
                "path": req.uri().path(),
            }),
        );
    };
    rule.runtime.hits.fetch_add(1, Ordering::Relaxed);

    // Chaos decisions are always non-deterministic; --seed only pins fake
    // bodies. Otherwise a probabilistic error mode would fire either always
    // or never.
    let mut chaos_rng = SmallRng::from_rng(&mut rand::rng());

    // 1. Error switch — abort/hang must preempt everything else.
    if rule.runtime.error_enabled.load(Ordering::Relaxed) {
        let effective = rule
            .runtime
            .error_override
            .load_full()
            .map(|a| (*a).clone())
            .or_else(|| rule.behavior.error_mode.clone());
        if let Some(spec) = effective
            && let Some(action) = error_mode::decide(&spec, &mut chaos_rng)
        {
            match action {
                ErrorAction::Respond { status, body } => {
                    let status =
                        StatusCode::from_u16(status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
                    return match body {
                        Some(v) => json_response(status, &v),
                        None => json_response(
                            status,
                            &serde_json::json!({ "error": "simulated failure" }),
                        ),
                    };
                }
                ErrorAction::Hang(max) => {
                    tokio::time::sleep(max).await;
                    return json_response(
                        StatusCode::SERVICE_UNAVAILABLE,
                        &serde_json::json!({ "error": "simulated hang elapsed" }),
                    );
                }
                ErrorAction::Abort => {
                    if let Some(killer) = req.extensions().get::<ConnKiller>() {
                        killer.kill();
                        // The connection task is being torn down with
                        // SO_LINGER(0); park until this future is dropped.
                        std::future::pending::<()>().await;
                        unreachable!("request future outlived its aborted connection");
                    }
                    // No connection to kill (e.g. in-process tests): the
                    // closest observable behavior is an empty 500.
                    return empty_response(StatusCode::INTERNAL_SERVER_ERROR);
                }
            }
        }
    }

    // 2. Rate limit — a throttled request doesn't pay the latency cost.
    if let Some(spec) = &rule.behavior.rate_limit {
        if let Some(bucket) = rule.runtime.bucket.load_full()
            && !bucket.try_acquire()
        {
            return rate_limited(spec, bucket.retry_after_secs());
        }
        if spec.reject_probability > 0.0 && chaos_rng.random_bool(spec.reject_probability) {
            return rate_limited(spec, 1);
        }
    }

    // 3. Latency.
    let effective_latency = rule
        .runtime
        .latency_override
        .load_full()
        .map(|a| (*a).clone())
        .or_else(|| rule.behavior.latency.clone());
    if let Some(spec) = effective_latency {
        latency::apply(&spec, &mut chaos_rng).await;
    }

    // 4. Body.
    build_response(rule, &state)
}

fn build_response(rule: &MockRule, state: &AppState) -> Response<Body> {
    match &rule.plan {
        ResponsePlan::Fixed {
            status,
            headers,
            body,
            content_type,
        } => {
            let mut builder = Response::builder().status(*status);
            if !body.is_empty() {
                builder = builder.header(header::CONTENT_TYPE, content_type);
            }
            for (name, value) in headers {
                builder = builder.header(name, value);
            }
            builder
                .body(Body::from(body.clone()))
                .unwrap_or_else(|_| fallback_500())
        }
        ResponsePlan::Example {
            status,
            body,
            content_type,
        } => Response::builder()
            .status(*status)
            .header(header::CONTENT_TYPE, content_type)
            .body(Body::from(body.clone()))
            .unwrap_or_else(|_| fallback_500()),
        ResponsePlan::Schema {
            status,
            schema,
            root,
        } => {
            // Seeded runs derive the RNG from (seed, rule id): identical
            // bodies per endpoint across requests and restarts.
            let rng = match state.0.seed {
                Some(seed) => SmallRng::seed_from_u64(seed ^ fnv1a64(rule.id.as_bytes())),
                None => SmallRng::from_rng(&mut rand::rng()),
            };
            let mut ctx = GenCtx::new(rng, root);
            let value = crate::fake::generate(schema, &mut ctx);
            let body = serde_json::to_vec(&value).unwrap_or_else(|_| b"null".to_vec());
            Response::builder()
                .status(*status)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(body))
                .unwrap_or_else(|_| fallback_500())
        }
        ResponsePlan::Empty { status } => empty_response(*status),
    }
}

fn rate_limited(spec: &RateLimitSpec, retry_after_secs: u64) -> Response<Body> {
    let status =
        StatusCode::from_u16(spec.response_status).unwrap_or(StatusCode::TOO_MANY_REQUESTS);
    let body = serde_json::json!({ "error": "rate_limited" });
    let bytes = serde_json::to_vec(&body).unwrap_or_default();
    Response::builder()
        .status(status)
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::RETRY_AFTER, retry_after_secs)
        .body(Body::from(bytes))
        .unwrap_or_else(|_| fallback_500())
}

pub fn json_response(status: StatusCode, value: &serde_json::Value) -> Response<Body> {
    let bytes = serde_json::to_vec(value).unwrap_or_else(|_| b"{}".to_vec());
    Response::builder()
        .status(status)
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(bytes))
        .unwrap_or_else(|_| fallback_500())
}

fn empty_response(status: StatusCode) -> Response<Body> {
    Response::builder()
        .status(status)
        .body(Body::from(Bytes::new()))
        .unwrap_or_else(|_| fallback_500())
}

/// Last-resort response when even response building fails; cannot panic.
fn fallback_500() -> Response<Body> {
    StatusCode::INTERNAL_SERVER_ERROR.into_response()
}
