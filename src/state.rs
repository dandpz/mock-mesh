use std::sync::Arc;
use std::time::Instant;

use arc_swap::ArcSwap;
use tokio::sync::{mpsc, oneshot};

use crate::loader::LoadedPaths;
use crate::rules::MockTable;

/// Reload request consumed by the single reload loop. `respond` carries the
/// result back to the admin API; the file watcher doesn't need one.
pub struct ReloadRequest {
    pub respond: Option<oneshot::Sender<Result<u64, String>>>,
}

pub struct AppStateInner {
    /// Immutable rule snapshot; readers get wait-free, never-torn loads.
    pub table: ArcSwap<MockTable>,
    pub paths: LoadedPaths,
    pub seed: Option<u64>,
    pub admin_token: Option<String>,
    pub started: Instant,
    pub reload_tx: mpsc::Sender<ReloadRequest>,
}

#[derive(Clone)]
pub struct AppState(pub Arc<AppStateInner>);

impl AppState {
    pub fn new(
        table: MockTable,
        paths: LoadedPaths,
        seed: Option<u64>,
        admin_token: Option<String>,
        reload_tx: mpsc::Sender<ReloadRequest>,
    ) -> Self {
        Self(Arc::new(AppStateInner {
            table: ArcSwap::from_pointee(table),
            paths,
            seed,
            admin_token,
            started: Instant::now(),
            reload_tx,
        }))
    }

    pub fn table(&self) -> arc_swap::Guard<Arc<MockTable>> {
        self.0.table.load()
    }
}
