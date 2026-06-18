//! mock-mesh: a single-binary, high-throughput mock HTTP server driven by
//! an OpenAPI spec, with latency, rate-limit and error-state simulation.

pub mod cli;
pub mod config;
pub mod error;
pub mod fake;
pub mod handlers;
pub mod loader;
pub mod openapi;
pub mod rules;
pub mod server;
pub mod simulate;
pub mod skill;
pub mod state;
pub mod watch;

use axum::Router;
use axum::extract::DefaultBodyLimit;

use crate::state::AppState;

/// Build the full application router. Exposed so integration tests can
/// drive it with `tower::ServiceExt::oneshot` without binding a socket.
pub fn build_router(state: AppState, max_body_bytes: usize, admin_enabled: bool) -> Router {
    let mut router = Router::new();
    if admin_enabled {
        router = router.nest("/_mockmesh", handlers::admin::router(state.clone()));
    }
    router
        .fallback(handlers::mock::handle)
        .layer(DefaultBodyLimit::max(max_body_bytes))
        .with_state(state)
}
