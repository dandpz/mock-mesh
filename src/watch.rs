//! Hot reload: a notify watcher on the *parent directories* of the spec and
//! config files (editors and configmap mounts replace files by rename, which
//! makes inode watches go stale), a 300ms debounce, and one single-consumer
//! reload loop shared with the admin `POST /reload` endpoint so reloads can
//! never race each other.

use std::collections::HashSet;
use std::ffi::OsString;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use notify::{RecursiveMode, Watcher};
use tokio::sync::mpsc;

use crate::error::LoadError;
use crate::rules::compile;
use crate::state::{AppState, ReloadRequest};
use crate::{loader, state};

const DEBOUNCE: Duration = Duration::from_millis(300);

/// Single consumer for all reload requests (watcher + admin API).
pub async fn reload_loop(state: AppState, mut rx: mpsc::Receiver<ReloadRequest>) {
    while let Some(req) = rx.recv().await {
        let result = reload_once(&state);
        match &result {
            Ok(generation) => {
                tracing::info!(generation, "rules reloaded");
            }
            Err(e) => {
                tracing::error!(error = %e, "reload failed; keeping previous rules");
            }
        }
        if let Some(respond) = req.respond {
            let _ = respond.send(result.map_err(|e| e.to_string()));
        }
    }
}

pub fn reload_once(state: &AppState) -> Result<u64, LoadError> {
    let docs = loader::load_all(&state.0.paths)?;
    let prev = state.0.table.load();
    let table = compile::build_table(&docs, Some(&prev))?;
    let generation = table.generation;
    state.0.table.store(Arc::new(table));
    Ok(generation)
}

/// Watch spec/config files and feed debounced reload requests into the loop.
pub fn spawn_watcher(
    state: &AppState,
    reload_tx: mpsc::Sender<state::ReloadRequest>,
) -> notify::Result<()> {
    let mut files: Vec<PathBuf> = vec![state.0.paths.spec.clone()];
    if let Some(config) = &state.0.paths.config {
        files.push(config.clone());
    }

    let names: HashSet<OsString> = files
        .iter()
        .filter_map(|p| p.file_name().map(OsString::from))
        .collect();
    let dirs: HashSet<PathBuf> = files
        .iter()
        .filter_map(|p| {
            let parent = p.parent()?;
            Some(if parent.as_os_str().is_empty() {
                PathBuf::from(".")
            } else {
                parent.to_path_buf()
            })
        })
        .collect();

    let (event_tx, mut event_rx) = mpsc::channel::<()>(64);
    let mut watcher = notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
        let Ok(event) = res else { return };
        let relevant = event
            .paths
            .iter()
            .any(|p| p.file_name().is_some_and(|n| names.contains(n)));
        if relevant {
            // Full channel just means a reload is already queued.
            let _ = event_tx.try_send(());
        }
    })?;
    for dir in &dirs {
        watcher.watch(dir, RecursiveMode::NonRecursive)?;
        tracing::debug!(dir = %dir.display(), "watching for changes");
    }

    tokio::spawn(async move {
        // Owns the watcher so it lives as long as the server.
        let _watcher = watcher;
        while event_rx.recv().await.is_some() {
            // Debounce: editors fire bursts of events per save.
            while let Ok(Some(())) = tokio::time::timeout(DEBOUNCE, event_rx.recv()).await {}
            if reload_tx
                .send(ReloadRequest { respond: None })
                .await
                .is_err()
            {
                break;
            }
        }
    });
    Ok(())
}
