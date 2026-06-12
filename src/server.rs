//! Custom accept loop instead of `axum::serve`, for two things axum can't
//! do from a handler: resetting the TCP connection (abort simulation) and
//! a bounded-grace shutdown drain.

use std::io;
use std::time::Duration;

use axum::Router;
use hyper_util::rt::{TokioExecutor, TokioIo};
use hyper_util::server::conn::auto::Builder as ConnBuilder;
use tokio::net::{TcpListener, TcpStream};
use tokio_util::sync::CancellationToken;
use tokio_util::task::TaskTracker;
use tower::ServiceExt;

/// Injected into every request's extensions; the abort error mode uses it
/// to tear down the owning connection with an RST.
#[derive(Clone)]
pub struct ConnKiller(CancellationToken);

impl ConnKiller {
    pub fn kill(&self) {
        self.0.cancel();
    }
}

pub async fn serve(listener: TcpListener, app: Router, shutdown_grace: Duration) -> io::Result<()> {
    let shutdown = CancellationToken::new();
    spawn_signal_listener(shutdown.clone());
    serve_with_shutdown(listener, app, shutdown, shutdown_grace).await
}

pub async fn serve_with_shutdown(
    listener: TcpListener,
    app: Router,
    shutdown: CancellationToken,
    shutdown_grace: Duration,
) -> io::Result<()> {
    let tracker = TaskTracker::new();
    loop {
        tokio::select! {
            _ = shutdown.cancelled() => break,
            accepted = listener.accept() => {
                let (stream, peer) = match accepted {
                    Ok(pair) => pair,
                    Err(e) => {
                        tracing::warn!(error = %e, "accept failed");
                        continue;
                    }
                };
                tracing::debug!(%peer, "connection accepted");
                tracker.spawn(handle_connection(stream, app.clone(), shutdown.clone()));
            }
        }
    }

    tracker.close();
    tokio::select! {
        _ = tracker.wait() => {}
        _ = tokio::time::sleep(shutdown_grace) => {
            tracing::warn!(
                grace_secs = shutdown_grace.as_secs(),
                "shutdown grace elapsed; abandoning remaining connections"
            );
        }
    }
    Ok(())
}

async fn handle_connection(stream: TcpStream, app: Router, shutdown: CancellationToken) {
    let kill_token = CancellationToken::new();
    let killer = ConnKiller(kill_token.clone());
    #[cfg(unix)]
    let raw_fd = std::os::fd::AsRawFd::as_raw_fd(&stream);

    let service =
        hyper::service::service_fn(move |mut req: hyper::Request<hyper::body::Incoming>| {
            req.extensions_mut().insert(killer.clone());
            let app = app.clone();
            async move { app.oneshot(req.map(axum::body::Body::new)).await }
        });

    let builder = ConnBuilder::new(TokioExecutor::new());
    let conn = builder.serve_connection_with_upgrades(TokioIo::new(stream), service);
    tokio::pin!(conn);

    let abort = |reason: &str| {
        // Make the close send an RST instead of a FIN so clients observe a
        // genuine "connection reset by peer".
        //
        // SAFETY (unix): the fd belongs to the TcpStream owned by `conn`,
        // which is still alive at this point; the borrowed fd does not
        // outlive this call.
        #[cfg(unix)]
        {
            let fd = unsafe { std::os::fd::BorrowedFd::borrow_raw(raw_fd) };
            let sock = socket2::SockRef::from(&fd);
            if let Err(e) = sock.set_linger(Some(Duration::ZERO)) {
                tracing::warn!(error = %e, "failed to set SO_LINGER for abort");
            }
        }
        tracing::debug!(reason, "aborting connection");
    };

    tokio::select! {
        result = conn.as_mut() => {
            if let Err(e) = result {
                tracing::debug!(error = %e, "connection ended with error");
            }
        }
        _ = kill_token.cancelled() => abort("error_mode: abort"),
        _ = shutdown.cancelled() => {
            // Stop accepting new requests on this connection, then keep
            // driving in-flight ones; still abortable.
            conn.as_mut().graceful_shutdown();
            tokio::select! {
                result = conn.as_mut() => {
                    if let Err(e) = result {
                        tracing::debug!(error = %e, "connection ended with error");
                    }
                }
                _ = kill_token.cancelled() => abort("error_mode: abort"),
            }
        }
    }
}

fn spawn_signal_listener(shutdown: CancellationToken) {
    tokio::spawn(async move {
        let ctrl_c = async {
            if let Err(e) = tokio::signal::ctrl_c().await {
                tracing::error!(error = %e, "failed to listen for ctrl-c");
                std::future::pending::<()>().await;
            }
        };
        #[cfg(unix)]
        let terminate = async {
            match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
                Ok(mut sig) => {
                    sig.recv().await;
                }
                Err(e) => {
                    tracing::error!(error = %e, "failed to listen for SIGTERM");
                    std::future::pending::<()>().await;
                }
            }
        };
        #[cfg(not(unix))]
        let terminate = std::future::pending::<()>();

        tokio::select! {
            _ = ctrl_c => {}
            _ = terminate => {}
        }
        tracing::info!("shutdown signal received");
        shutdown.cancel();
    });
}
