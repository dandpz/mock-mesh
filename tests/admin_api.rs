#![allow(clippy::unwrap_used)]

mod common;

use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use common::*;
use serde_json::json;
use tower::ServiceExt;

async fn rule_key(app: &Router, id: &str) -> String {
    let (_, body) = get_json(app, "/_mockmesh/routes").await;
    body["routes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["id"] == id)
        .unwrap_or_else(|| panic!("rule {id} not in listing"))["key"]
        .as_str()
        .unwrap()
        .to_string()
}

#[tokio::test]
async fn health_reports_generation() {
    let app = app("petstore.yaml", None);
    let (status, body) = get_json(&app, "/_mockmesh/health").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["status"], "ok");
    assert_eq!(body["generation"], 1);
    assert!(body["routes"].as_u64().unwrap() > 0);
}

#[tokio::test]
async fn routes_listing_includes_spec_and_config_rules() {
    let app = app("petstore.yaml", Some("config-basic.yaml"));
    let (_, body) = get_json(&app, "/_mockmesh/routes").await;
    let routes = body["routes"].as_array().unwrap();
    let by_id = |id: &str| routes.iter().find(|r| r["id"] == id);

    let spec_rule = by_id("GET /pets").unwrap();
    assert_eq!(spec_rule["source"], "spec");
    assert_eq!(spec_rule["response_kind"], "example");

    let merged = by_id("GET /pets/special").unwrap();
    assert_eq!(merged["source"], "both");
    assert_eq!(merged["response_kind"], "fixed");

    let config_only = by_id("GET /internal/flags").unwrap();
    assert_eq!(config_only["source"], "config");
}

#[tokio::test]
async fn route_detail_by_key_and_hit_counter() {
    let app = app("petstore.yaml", None);
    let key = rule_key(&app, "GET /pets").await;

    get_json(&app, "/pets").await;
    get_json(&app, "/pets").await;

    let (status, body) = get_json(&app, &format!("/_mockmesh/routes/{key}")).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["id"], "GET /pets");
    assert_eq!(body["hits"], 2);
}

#[tokio::test]
async fn unknown_rule_is_404() {
    let app = app("petstore.yaml", None);
    let (status, _) = get_json(&app, "/_mockmesh/routes/ffffffffffff").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn override_flips_endpoint_to_error_and_back() {
    let app = app("petstore.yaml", None);
    let key = rule_key(&app, "GET /pets").await;

    let response = send_json(
        &app,
        "PUT",
        &format!("/_mockmesh/routes/{key}/overrides"),
        json!({ "error_mode": { "kind": "status", "code": 503 } }),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);

    let (status, _) = get_json(&app, "/pets").await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);

    let response = send(
        &app,
        "DELETE",
        &format!("/_mockmesh/routes/{key}/overrides"),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let (status, _) = get_json(&app, "/pets").await;
    assert_eq!(status, StatusCode::OK);
}

#[tokio::test]
async fn override_latency_applies() {
    let app = app("petstore.yaml", None);
    let key = rule_key(&app, "GET /pets").await;
    send_json(
        &app,
        "PUT",
        &format!("/_mockmesh/routes/{key}/overrides"),
        json!({ "latency": { "fixed_ms": 120 } }),
    )
    .await;
    let start = std::time::Instant::now();
    get_json(&app, "/pets").await;
    assert!(start.elapsed().as_millis() >= 120);
}

#[tokio::test]
async fn disabled_error_switch_suppresses_config_error() {
    // config-chaos forces 500 on POST /pets; disabling the switch restores
    // the spec-derived 201.
    let app = app("petstore.yaml", Some("config-chaos.yaml"));
    let key = rule_key(&app, "POST /pets").await;
    send_json(
        &app,
        "PUT",
        &format!("/_mockmesh/routes/{key}/overrides"),
        json!({ "enabled": false }),
    )
    .await;
    let response = send_json(&app, "POST", "/pets", json!({})).await;
    assert_eq!(response.status(), StatusCode::CREATED);
}

#[tokio::test]
async fn invalid_override_payload_is_422() {
    let app = app("petstore.yaml", None);
    let key = rule_key(&app, "GET /pets").await;
    let response = send_json(
        &app,
        "PUT",
        &format!("/_mockmesh/routes/{key}/overrides"),
        json!({ "error_mode": { "kind": "status", "code": 99 } }),
    )
    .await;
    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);

    let response = send_json(
        &app,
        "PUT",
        &format!("/_mockmesh/routes/{key}/overrides"),
        json!({ "bogus_field": 1 }),
    )
    .await;
    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
}

#[tokio::test]
async fn rate_limit_reset_refills_bucket() {
    let app = app("petstore.yaml", Some("config-chaos.yaml"));
    let key = rule_key(&app, "GET /pets/{petId}").await;

    // exhaust burst=2
    get_json(&app, "/pets/1").await;
    get_json(&app, "/pets/2").await;
    let (status, _) = get_json(&app, "/pets/3").await;
    assert_eq!(status, StatusCode::TOO_MANY_REQUESTS);

    let response = send(
        &app,
        "POST",
        &format!("/_mockmesh/routes/{key}/rate-limit/reset"),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);

    let (status, _) = get_json(&app, "/pets/4").await;
    assert_eq!(status, StatusCode::OK);
}

#[tokio::test]
async fn rate_limit_reset_on_unlimited_rule_is_409() {
    let app = app("petstore.yaml", None);
    let key = rule_key(&app, "GET /pets").await;
    let response = send(
        &app,
        "POST",
        &format!("/_mockmesh/routes/{key}/rate-limit/reset"),
    )
    .await;
    assert_eq!(response.status(), StatusCode::CONFLICT);
}

#[tokio::test]
async fn effective_config_endpoint() {
    let app = app("petstore.yaml", Some("config-basic.yaml"));
    let (status, body) = get_json(&app, "/_mockmesh/config").await;
    assert_eq!(status, StatusCode::OK);
    assert!(body["config"]["endpoints"].as_array().unwrap().len() == 2);
}

#[tokio::test]
async fn admin_token_enforced() {
    let state = make_state_with(
        fixture("petstore.yaml"),
        None,
        None,
        Some("s3cret".to_string()),
    );
    let app = mock_mesh::build_router(state, 1 << 20, true);

    // No token → 401
    let (status, _) = get_json(&app, "/_mockmesh/health").await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    // Wrong token → 401
    let req = Request::builder()
        .uri("/_mockmesh/health")
        .header("authorization", "Bearer wrong")
        .body(Body::empty())
        .unwrap();
    let response = app.clone().oneshot(req).await.unwrap();
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);

    // Correct token → 200
    let req = Request::builder()
        .uri("/_mockmesh/health")
        .header("authorization", "Bearer s3cret")
        .body(Body::empty())
        .unwrap();
    let response = app.clone().oneshot(req).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    // Mock routes stay open without a token
    let (status, _) = get_json(&app, "/pets").await;
    assert_eq!(status, StatusCode::OK);
}
