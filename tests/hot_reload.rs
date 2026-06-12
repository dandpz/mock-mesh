#![allow(clippy::unwrap_used)]

mod common;

use std::time::{Duration, Instant};

use common::*;
use serde_json::json;

async fn wait_for_generation(client: &reqwest::Client, base: &str, min_generation: u64) -> bool {
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        if let Ok(resp) = client.get(format!("{base}/_mockmesh/health")).send().await
            && let Ok(body) = resp.json::<serde_json::Value>().await
            && body["generation"].as_u64().unwrap_or(0) >= min_generation
        {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    false
}

fn temp_copies() -> (tempfile::TempDir, std::path::PathBuf, std::path::PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let spec = dir.path().join("spec.yaml");
    let config = dir.path().join("config.yaml");
    std::fs::copy(fixture("petstore.yaml"), &spec).unwrap();
    std::fs::write(&config, "endpoints: []\n").unwrap();
    (dir, spec, config)
}

#[tokio::test]
async fn file_change_hot_reloads_rules() {
    let (_dir, spec, config) = temp_copies();
    let server = spawn_server(spec, Some(config.clone())).await;
    let client = reqwest::Client::new();
    let base = format!("http://{}", server.addr);

    // Initially: spec example
    let body: serde_json::Value = client
        .get(server.url("/pets"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(body[0]["name"], "rex");

    // Rewrite the config to override the route, watcher should pick it up.
    std::fs::write(
        &config,
        r#"
endpoints:
  - path: /pets
    method: GET
    response:
      status: 200
      body: { reloaded: true }
"#,
    )
    .unwrap();
    assert!(
        wait_for_generation(&client, &base, 2).await,
        "hot reload did not happen within 5s"
    );

    let body: serde_json::Value = client
        .get(server.url("/pets"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(body, json!({ "reloaded": true }));

    server.shutdown.cancel();
    let _ = server.handle.await;
}

#[tokio::test]
async fn broken_config_keeps_previous_rules() {
    let (_dir, spec, config) = temp_copies();
    let server = spawn_server(spec, Some(config.clone())).await;
    let client = reqwest::Client::new();

    std::fs::write(&config, "endpoints: [ this is : not valid\n").unwrap();
    // Force a reload through the admin API: must report the parse error.
    let resp = client
        .post(server.url("/_mockmesh/reload"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 422);

    // Old rules still served, generation unchanged.
    let health: serde_json::Value = client
        .get(server.url("/_mockmesh/health"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(health["generation"], 1);
    let body: serde_json::Value = client
        .get(server.url("/pets"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(body[0]["name"], "rex");

    server.shutdown.cancel();
    let _ = server.handle.await;
}

#[tokio::test]
async fn admin_reload_bumps_generation() {
    let (_dir, spec, config) = temp_copies();
    let server = spawn_server(spec, Some(config)).await;
    let client = reqwest::Client::new();

    let resp: serde_json::Value = client
        .post(server.url("/_mockmesh/reload"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(resp["generation"], 2);

    server.shutdown.cancel();
    let _ = server.handle.await;
}

#[tokio::test]
async fn overrides_survive_reload() {
    let (_dir, spec, config) = temp_copies();
    let server = spawn_server(spec, Some(config)).await;
    let client = reqwest::Client::new();

    // Find the GET /pets key, set a 503 override.
    let routes: serde_json::Value = client
        .get(server.url("/_mockmesh/routes"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let key = routes["routes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["id"] == "GET /pets")
        .unwrap()["key"]
        .as_str()
        .unwrap()
        .to_string();
    client
        .put(server.url(&format!("/_mockmesh/routes/{key}/overrides")))
        .json(&json!({ "error_mode": { "kind": "status", "code": 503 } }))
        .send()
        .await
        .unwrap();

    // Reload; the override must still be active (runtime carry-over).
    client
        .post(server.url("/_mockmesh/reload"))
        .send()
        .await
        .unwrap();
    let resp = client.get(server.url("/pets")).send().await.unwrap();
    assert_eq!(resp.status(), 503);

    server.shutdown.cancel();
    let _ = server.handle.await;
}

#[tokio::test]
async fn abort_mode_resets_connection() {
    let dir = tempfile::tempdir().unwrap();
    let spec = dir.path().join("spec.yaml");
    let config = dir.path().join("config.yaml");
    std::fs::copy(fixture("petstore.yaml"), &spec).unwrap();
    std::fs::write(
        &config,
        "endpoints:\n  - path: /pets\n    method: GET\n    behavior:\n      error_mode: { kind: abort }\n",
    )
    .unwrap();
    let server = spawn_server(spec, Some(config)).await;

    let client = reqwest::Client::new();
    let result = client
        .get(server.url("/pets"))
        .timeout(Duration::from_secs(5))
        .send()
        .await;
    // The client must observe a transport failure, not an HTTP response.
    assert!(result.is_err(), "expected connection error, got {result:?}");

    server.shutdown.cancel();
    let _ = server.handle.await;
}

#[tokio::test]
async fn graceful_shutdown_finishes_inflight_request() {
    let dir = tempfile::tempdir().unwrap();
    let spec = dir.path().join("spec.yaml");
    let config = dir.path().join("config.yaml");
    std::fs::copy(fixture("petstore.yaml"), &spec).unwrap();
    std::fs::write(
        &config,
        "endpoints:\n  - path: /pets\n    method: GET\n    behavior:\n      latency: { fixed_ms: 500 }\n",
    )
    .unwrap();
    let server = spawn_server(spec, Some(config)).await;
    let client = reqwest::Client::new();

    let url = server.url("/pets");
    let inflight = tokio::spawn(async move { client.get(url).send().await });
    tokio::time::sleep(Duration::from_millis(100)).await;
    server.shutdown.cancel();

    let response = inflight.await.unwrap().unwrap();
    assert_eq!(response.status(), 200);
    let _ = server.handle.await;
}
