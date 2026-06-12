use std::net::SocketAddr;
use std::process::ExitCode;
use std::time::Duration;

use clap::Parser;
use mock_mesh::cli::Cli;
use mock_mesh::loader::{self, LoadedPaths};
use mock_mesh::rules::compile;
use mock_mesh::state::AppState;
use mock_mesh::{build_router, server, watch};
use tokio::sync::mpsc;
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> ExitCode {
    let cli = Cli::parse();

    let filter = EnvFilter::try_from_default_env()
        .or_else(|_| EnvFilter::try_new(&cli.log))
        .unwrap_or_else(|_| EnvFilter::new("info"));
    tracing_subscriber::fmt().with_env_filter(filter).init();

    let paths = LoadedPaths {
        spec: cli.spec.clone(),
        config: cli.config.clone(),
    };

    // Startup parse errors are fatal (unlike reload errors, which keep the
    // previous rule table).
    let docs = match loader::load_all(&paths) {
        Ok(docs) => docs,
        Err(e) => {
            eprintln!("error: {e}");
            return ExitCode::FAILURE;
        }
    };
    let table = match compile::build_table(&docs, None) {
        Ok(table) => table,
        Err(e) => {
            eprintln!("error: {e}");
            return ExitCode::FAILURE;
        }
    };

    if cli.validate {
        print_route_table(&table);
        return ExitCode::SUCCESS;
    }

    let (reload_tx, reload_rx) = mpsc::channel(8);
    let state = AppState::new(
        table,
        paths,
        cli.seed,
        cli.admin_token.clone(),
        reload_tx.clone(),
    );

    tokio::spawn(watch::reload_loop(state.clone(), reload_rx));
    if !cli.no_watch
        && let Err(e) = watch::spawn_watcher(&state, reload_tx)
    {
        tracing::warn!(
            error = %e,
            "file watching unavailable; hot reload via POST /_mockmesh/reload only"
        );
    }

    let app = build_router(state.clone(), cli.max_body_bytes, !cli.no_admin);
    let addr = SocketAddr::new(cli.host, cli.port);
    let listener = match tokio::net::TcpListener::bind(addr).await {
        Ok(l) => l,
        Err(e) => {
            eprintln!("error: cannot bind {addr}: {e}");
            return ExitCode::FAILURE;
        }
    };
    let local = listener
        .local_addr()
        .map(|a| a.to_string())
        .unwrap_or_else(|_| addr.to_string());
    tracing::info!(
        addr = %local,
        routes = state.table().rules.len(),
        admin = !cli.no_admin,
        "mock-mesh listening"
    );
    if !cli.host.is_loopback() && cli.admin_token.is_none() && !cli.no_admin {
        tracing::warn!(
            "admin API is exposed on a non-loopback address without --admin-token; \
             anyone on the network can flip simulations"
        );
    }

    match server::serve(listener, app, Duration::from_secs(cli.shutdown_grace_secs)).await {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("error: server failed: {e}");
            ExitCode::FAILURE
        }
    }
}

fn print_route_table(table: &mock_mesh::rules::MockTable) {
    println!(
        "{} route(s), generation {}",
        table.rules.len(),
        table.generation
    );
    for rule in &table.rules {
        let method = rule.method.as_ref().map_or("ANY", http::Method::as_str);
        let mut notes: Vec<String> = Vec::new();
        if let Some(l) = &rule.behavior.latency {
            notes.push(format!("latency {}ms+{}ms", l.fixed_ms, l.jitter_ms));
        }
        if let Some(r) = &rule.behavior.rate_limit {
            notes.push(format!("rate {}rps burst {}", r.rps, r.burst));
        }
        if rule.behavior.error_mode.is_some() {
            notes.push("error-mode".to_string());
        }
        println!(
            "  {:7} {:40} [{}] {} {}",
            method,
            rule.path.raw,
            rule.plan.kind(),
            rule.key,
            notes.join(", ")
        );
    }
}
