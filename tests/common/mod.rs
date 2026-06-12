#![allow(clippy::unwrap_used, dead_code)]

use std::net::SocketAddr;
use std::path::PathBuf;
use std::time::Duration;

use axum::Router;
use axum::body::Body;
use axum::http::{Request, Response, StatusCode};
use http_body_util::BodyExt;
use mock_mesh::loader::{self, LoadedPaths};
use mock_mesh::rules::compile;
use mock_mesh::state::AppState;
use mock_mesh::{build_router, server, watch};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use tower::ServiceExt;

pub fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name)
}

/// In-process state without reload loop or watcher (oneshot tests).
pub fn make_state(spec: PathBuf, config: Option<PathBuf>) -> AppState {
    make_state_with(spec, config, None, None)
}

pub fn make_state_with(
    spec: PathBuf,
    config: Option<PathBuf>,
    seed: Option<u64>,
    admin_token: Option<String>,
) -> AppState {
    let paths = LoadedPaths { spec, config };
    let docs = loader::load_all(&paths).unwrap();
    let table = compile::build_table(&docs, None).unwrap();
    let (tx, _rx) = mpsc::channel(1);
    AppState::new(table, paths, seed, admin_token, tx)
}

pub fn app(spec: &str, config: Option<&str>) -> Router {
    let state = make_state(fixture(spec), config.map(fixture));
    build_router(state, 1 << 20, true)
}

pub async fn send(app: &Router, method: &str, path: &str) -> Response<Body> {
    let req = Request::builder()
        .method(method)
        .uri(path)
        .body(Body::empty())
        .unwrap();
    app.clone().oneshot(req).await.unwrap()
}

pub async fn send_json(
    app: &Router,
    method: &str,
    path: &str,
    body: serde_json::Value,
) -> Response<Body> {
    let req = Request::builder()
        .method(method)
        .uri(path)
        .header("content-type", "application/json")
        .body(Body::from(serde_json::to_vec(&body).unwrap()))
        .unwrap();
    app.clone().oneshot(req).await.unwrap()
}

pub async fn body_json(response: Response<Body>) -> serde_json::Value {
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    serde_json::from_slice(&bytes).unwrap()
}

pub async fn get_json(app: &Router, path: &str) -> (StatusCode, serde_json::Value) {
    let response = send(app, "GET", path).await;
    let status = response.status();
    (status, body_json(response).await)
}

/// Real-TCP server with reload loop + file watcher, for hot-reload and
/// connection-level (abort/hang/shutdown) tests.
pub struct TestServer {
    pub addr: SocketAddr,
    pub shutdown: CancellationToken,
    pub handle: tokio::task::JoinHandle<std::io::Result<()>>,
}

impl TestServer {
    pub fn url(&self, path: &str) -> String {
        format!("http://{}{}", self.addr, path)
    }
}

pub async fn spawn_server(spec: PathBuf, config: Option<PathBuf>) -> TestServer {
    let paths = LoadedPaths { spec, config };
    let docs = loader::load_all(&paths).unwrap();
    let table = compile::build_table(&docs, None).unwrap();
    let (tx, rx) = mpsc::channel(8);
    let state = AppState::new(table, paths, None, None, tx.clone());
    tokio::spawn(watch::reload_loop(state.clone(), rx));
    watch::spawn_watcher(&state, tx).unwrap();

    let app = build_router(state, 1 << 20, true);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let shutdown = CancellationToken::new();
    let handle = tokio::spawn(server::serve_with_shutdown(
        listener,
        app,
        shutdown.clone(),
        Duration::from_secs(5),
    ));
    TestServer {
        addr,
        shutdown,
        handle,
    }
}
