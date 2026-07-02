#![allow(clippy::unwrap_used)]

mod common;

use axum::http::StatusCode;
use common::*;
use http_body_util::BodyExt;
use serde_json::json;

#[tokio::test]
async fn spec_example_served_verbatim() {
    let app = app("petstore.yaml", None);
    let (status, body) = get_json(&app, "/pets").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        body,
        json!([{ "id": 1, "name": "rex" }, { "id": 2, "name": "bella" }])
    );
}

#[tokio::test]
async fn json_spec_works_like_yaml() {
    let app = app("petstore.json", None);
    let (status, body) = get_json(&app, "/pets").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body[0]["name"], "rex");
}

#[tokio::test]
async fn schema_synthesis_for_templated_route() {
    let app = app("petstore.yaml", None);
    let (status, body) = get_json(&app, "/pets/123").await;
    assert_eq!(status, StatusCode::OK);
    let id = body["id"].as_i64().unwrap();
    assert!((1..=1000).contains(&id));
    assert!(body["name"].is_string());
    // $ref-resolved nested object with a format
    if let Some(owner) = body.get("owner").filter(|o| !o.is_null()) {
        assert!(owner["email"].as_str().unwrap().contains('@'));
    }
}

#[tokio::test]
async fn post_returns_lowest_2xx() {
    let app = app("petstore.yaml", None);
    let response = send_json(&app, "POST", "/pets", json!({ "name": "x" })).await;
    assert_eq!(response.status(), StatusCode::CREATED);
}

#[tokio::test]
async fn no_content_responses_are_empty() {
    let app = app("petstore.yaml", None);
    let response = send(&app, "DELETE", "/pets/9").await;
    assert_eq!(response.status(), StatusCode::NO_CONTENT);
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    assert!(bytes.is_empty());
}

#[tokio::test]
async fn exact_path_beats_template() {
    // /pets/special is exact; /pets/{petId} is templated.
    let app = app("petstore.yaml", None);
    let (_, body) = get_json(&app, "/pets/special").await;
    assert_eq!(body, json!({ "special": true }));
}

#[tokio::test]
async fn unmatched_route_is_structured_404() {
    let app = app("petstore.yaml", None);
    let (status, body) = get_json(&app, "/nope").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(body["error"], "no mock rule matched");
    assert_eq!(body["path"], "/nope");
}

#[tokio::test]
async fn wrong_method_is_404() {
    let app = app("petstore.yaml", None);
    let response = send(&app, "PATCH", "/pets").await;
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn trailing_slash_matches() {
    let app = app("petstore.yaml", None);
    let (status, _) = get_json(&app, "/pets/").await;
    assert_eq!(status, StatusCode::OK);
}

#[tokio::test]
async fn config_fixed_response_overrides_spec() {
    let app = app("petstore.yaml", Some("config-basic.yaml"));
    let response = send(&app, "GET", "/pets/special").await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()["x-mock"], "true");
    let body = body_json(response).await;
    assert_eq!(body, json!({ "overridden": true }));
}

#[tokio::test]
async fn config_only_route_served() {
    let app = app("petstore.yaml", Some("config-basic.yaml"));
    let (status, body) = get_json(&app, "/internal/flags").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["dark_mode"], true);
}

#[tokio::test]
async fn default_response_key_served_as_200() {
    let app = app("petstore.yaml", None);
    let (status, body) = get_json(&app, "/health").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["ok"], true);
}

#[tokio::test]
async fn schema_zoo_generates_valid_shapes() {
    let app = app("schema-zoo.yaml", None);
    let (status, body) = get_json(&app, "/zoo").await;
    assert_eq!(status, StatusCode::OK);

    let uuid = body["uuid"].as_str().unwrap();
    assert_eq!(uuid.len(), 36);
    assert!(body["email"].as_str().unwrap().contains('@'));
    assert!(body["when"].as_str().unwrap().ends_with('Z'));
    // oneOf picks the first member: integer pinned to 7
    assert_eq!(body["choice"], json!(7));
    assert!(["open", "closed", "pending"].contains(&body["status"].as_str().unwrap()));
    let score = body["score"].as_f64().unwrap();
    assert!((0.0..=10.0).contains(&score));
    let pets = body["pets"].as_array().unwrap();
    assert!((2..=3).contains(&pets.len()));
    for pet in pets {
        assert!((1..=9).contains(&pet["id"].as_i64().unwrap()));
        assert!(pet["kind"].is_string());
    }
}

#[tokio::test]
async fn array_length_config_sizes_root_array_only() {
    let app = app("schema-zoo.yaml", Some("config-array-length.yaml"));

    // configured route: root array gets exactly the configured length
    let (status, body) = get_json(&app, "/herd").await;
    assert_eq!(status, StatusCode::OK);
    let herd = body.as_array().unwrap();
    assert_eq!(herd.len(), 12);
    for animal in herd {
        assert!((1..=9).contains(&animal["id"].as_i64().unwrap()));
        assert!(animal["kind"].is_string());
    }

    // unconfigured route: nested array keeps schema minItems/maxItems
    let (_, body) = get_json(&app, "/zoo").await;
    let pets = body["pets"].as_array().unwrap();
    assert!((2..=3).contains(&pets.len()));
}

#[tokio::test]
async fn array_length_with_seed_stays_deterministic() {
    let make_app = || {
        let state = make_state_with(
            fixture("schema-zoo.yaml"),
            Some(fixture("config-array-length.yaml")),
            Some(99),
            None,
        );
        mock_mesh::build_router(state, 1 << 20, false)
    };
    let app = make_app();
    let (_, a) = get_json(&app, "/herd").await;
    let (_, b) = get_json(&app, "/herd").await;
    assert_eq!(a.as_array().unwrap().len(), 12);
    assert_eq!(a, b);

    // across "restarts" (fresh state, same seed)
    let (_, c) = get_json(&make_app(), "/herd").await;
    assert_eq!(a, c);
}

#[tokio::test]
async fn circular_schema_terminates() {
    let app = app("schema-zoo.yaml", None);
    let (status, body) = get_json(&app, "/node").await;
    assert_eq!(status, StatusCode::OK);
    // must terminate in null rather than recursing forever
    let mut cur = &body;
    let mut hops = 0;
    while cur.is_object() {
        cur = &cur["next"];
        hops += 1;
        assert!(hops < 32, "circular $ref did not terminate");
    }
}

#[tokio::test]
async fn seeded_responses_are_byte_identical() {
    let state = make_state_with(fixture("petstore.yaml"), None, Some(1234), None);
    let app = mock_mesh::build_router(state, 1 << 20, false);
    let (_, a) = get_json(&app, "/pets/1").await;
    let (_, b) = get_json(&app, "/pets/1").await;
    assert_eq!(a, b);

    // and across "restarts" (fresh state, same seed)
    let state2 = make_state_with(fixture("petstore.yaml"), None, Some(1234), None);
    let app2 = mock_mesh::build_router(state2, 1 << 20, false);
    let (_, c) = get_json(&app2, "/pets/1").await;
    assert_eq!(a, c);
}

#[tokio::test]
async fn admin_disabled_router_has_no_admin_routes() {
    let state = make_state(fixture("petstore.yaml"), None);
    let app = mock_mesh::build_router(state, 1 << 20, false);
    let (status, _) = get_json(&app, "/_mockmesh/health").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}
