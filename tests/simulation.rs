#![allow(clippy::unwrap_used)]

mod common;

use std::time::Instant;

use axum::http::StatusCode;
use common::*;
use serde_json::json;

#[tokio::test]
async fn latency_is_injected() {
    let app = app("petstore.yaml", Some("config-chaos.yaml"));
    let start = Instant::now();
    let (status, _) = get_json(&app, "/pets").await;
    assert_eq!(status, StatusCode::OK);
    let elapsed = start.elapsed().as_millis();
    assert!(elapsed >= 150, "expected >=150ms latency, got {elapsed}ms");
}

#[tokio::test]
async fn token_bucket_throttles_burst() {
    let app = app("petstore.yaml", Some("config-chaos.yaml"));
    // burst=2 → first two pass, third is limited
    let (s1, _) = get_json(&app, "/pets/1").await;
    let (s2, _) = get_json(&app, "/pets/2").await;
    assert_eq!(s1, StatusCode::OK);
    assert_eq!(s2, StatusCode::OK);
    let response = send(&app, "GET", "/pets/3").await;
    assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);
    let retry_after = response.headers()["retry-after"].to_str().unwrap();
    assert_eq!(retry_after, "2"); // ceil(1 / 0.5 rps)
    let body = body_json(response).await;
    assert_eq!(body["error"], "rate_limited");
}

#[tokio::test]
async fn probabilistic_rejection_at_one_always_fires() {
    let app = app("petstore.yaml", Some("config-chaos.yaml"));
    for _ in 0..5 {
        let (status, body) = get_json(&app, "/always-limited").await;
        assert_eq!(status, StatusCode::TOO_MANY_REQUESTS);
        assert_eq!(body["error"], "rate_limited");
    }
}

#[tokio::test]
async fn forced_error_status_with_custom_body() {
    let app = app("petstore.yaml", Some("config-chaos.yaml"));
    let response = send_json(&app, "POST", "/pets", json!({ "name": "x" })).await;
    assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
    let body = body_json(response).await;
    assert_eq!(
        body,
        json!({ "error": "internal", "message": "simulated failure" })
    );
}

#[tokio::test]
async fn hang_holds_then_times_out() {
    let app = app("petstore.yaml", Some("config-chaos.yaml"));
    let start = Instant::now();
    let (status, body) = get_json(&app, "/pets/special").await;
    assert!(start.elapsed().as_millis() >= 1000);
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(body["error"], "simulated hang elapsed");
}

#[tokio::test]
async fn abort_without_connection_falls_back_to_500() {
    // In-process oneshot has no TCP connection to kill; the handler
    // degrades to an empty 500.
    let app = app("petstore.yaml", Some("config-chaos.yaml"));
    let response = send(&app, "GET", "/health").await;
    assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
}
