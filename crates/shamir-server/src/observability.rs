//! Observability HTTP server — `/healthz`, `/readyz`, `/metrics`, `/info`.
//!
//! Bound to a separate small TCP port (default `127.0.0.1:9090`, loopback
//! by convention so it isn't reachable from the internet without an
//! explicit reverse-proxy mapping). Designed to be safe to expose to a
//! Prometheus scraper or a Kubernetes liveness/readiness probe:
//!
//! * **`/healthz`** — boolean alive. Always responds `200 OK` if the
//!   process is responding to HTTP. K8s liveness probe should be wired
//!   here. Intentionally trivial — never reads `/proc`, never depends on
//!   any other subsystem — so it can't flake under transient pressure.
//!
//! * **`/readyz`** — boolean ready. `200 OK` once the boot path has
//!   bound every listener, `503 Service Unavailable` until then. Pair
//!   with the load balancer's traffic gating + K8s readinessProbe so a
//!   freshly-spawned pod doesn't receive requests before its listeners
//!   are bound.
//!
//! * **`/metrics`** — Prometheus text-format dump. Includes the standard
//!   `process_*` series (CPU seconds, RSS, threads, fd count, disk I/O)
//!   driven by `metrics-process`, plus application-level counters and
//!   gauges registered by other modules. A background poller refreshes
//!   the process metrics every 5 s; HTTP requests just render the
//!   recorder's current snapshot (~ns work).
//!
//! * **`/info`** — pretty JSON for curl-debugging by an operator.
//!   Snapshots a few interesting fields out of the registry. Optional
//!   convenience.
//!
//! ## Non-blocking guarantees
//!
//! The HTTP listener runs on its own `tokio::spawn` task — no influence
//! on the data-path accept loops. Process-metric collection is one
//! `metrics_process::Collector::collect()` call every 5 s on a separate
//! tokio interval — total cost ~30-50 µs every 5 s = ~0.001 % CPU.

use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use axum::{
    extract::State,
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::get,
    Json, Router,
};
use metrics_exporter_prometheus::{PrometheusBuilder, PrometheusHandle};
use metrics_process::Collector;
use serde::Serialize;
use tokio::net::TcpListener;
use tokio::sync::Notify;
use tokio::task::JoinHandle;

/// Live state shared between the HTTP handlers and the boot path.
///
/// `ready` flips to `true` once `ServerLauncher::launch` has bound every
/// listener. The data-path accept loops never touch this field — only
/// `/readyz` reads it.
#[derive(Debug)]
pub struct ObservabilityState {
    pub ready: AtomicBool,
    pub started_at: std::time::Instant,
    pub bound_addrs: parking_lot::RwLock<Vec<SocketAddr>>,
}

impl ObservabilityState {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            ready: AtomicBool::new(false),
            started_at: std::time::Instant::now(),
            bound_addrs: parking_lot::RwLock::new(Vec::new()),
        })
    }

    pub fn mark_ready(&self) {
        self.ready.store(true, Ordering::Release);
    }

    pub fn set_bound_addrs(&self, addrs: Vec<SocketAddr>) {
        *self.bound_addrs.write() = addrs;
    }
}

/// Handle to a running observability server. Drop or call `shutdown`
/// to stop it.
pub struct ObservabilityHandle {
    pub bound_addr: SocketAddr,
    pub state: Arc<ObservabilityState>,
    shutdown: Arc<Notify>,
    listener_task: JoinHandle<()>,
    poller_task: JoinHandle<()>,
}

impl ObservabilityHandle {
    /// Stop the HTTP listener and the process-metrics poller.
    pub async fn shutdown(self) {
        self.shutdown.notify_waiters();
        let _ = self.listener_task.await;
        let _ = self.poller_task.await;
    }
}

/// Errors raised by [`spawn`].
#[derive(Debug, thiserror::Error)]
pub enum ObservabilityError {
    /// Couldn't bind the HTTP listener.
    #[error("observability bind: {0}")]
    Bind(std::io::Error),
    /// Prometheus recorder install failed (only one global recorder per
    /// process — second `spawn` call would error here in tests).
    #[error("prometheus recorder install: {0}")]
    RecorderInstall(String),
}

/// Spawn the observability HTTP server.
///
/// `addr` is the bind address (typically `127.0.0.1:9090`).
/// `state` is shared with the boot path — caller flips `ready` to `true`
/// after listeners are bound.
///
/// `install_recorder = false` skips installing the Prometheus recorder —
/// useful when multiple test instances run in the same process (recorders
/// are global and only one can be installed). The recorder still answers
/// `/metrics` if a previous call installed one.
pub async fn spawn(
    addr: SocketAddr,
    state: Arc<ObservabilityState>,
    install_recorder: bool,
) -> Result<ObservabilityHandle, ObservabilityError> {
    // 1. Set up the Prometheus recorder (if we're allowed to install one).
    //    `install_recorder()` builds a recorder and `set_global_recorder`s
    //    it in one shot; succeeds at most once per process. Tests that
    //    spawn multiple servers in the same process pass
    //    `install_recorder = false` for the second+ instance.
    let prom_handle = if install_recorder {
        match PrometheusBuilder::new().install_recorder() {
            Ok(h) => Some(h),
            Err(e) => return Err(ObservabilityError::RecorderInstall(e.to_string())),
        }
    } else {
        None
    };

    // 2. Process metrics collector — describes + collects standard
    // `process_*` series.
    let collector = Collector::default();
    collector.describe();

    // 3. Application metrics — describe + register-by-zero-touch so
    // they appear in `/metrics` even before the first real event
    // (otherwise Prometheus scrapers see them only after the first
    // counter increment, which makes Grafana panel discovery flaky).
    //
    // The `metrics::counter!(...).increment(0)` is the canonical way to
    // "register without changing the value" — `describe_*` alone only
    // attaches metadata, the counter itself remains absent from the
    // exporter's output until first touched.
    metrics::describe_counter!(
        "auth_attempts_total",
        metrics::Unit::Count,
        "Number of authentication attempts, bucketed by terminal result \
         label: success / bad_proof / locked_out / unknown_user / \
         rate_limited / unsupported_version / policy / io_or_decode"
    );
    for label in [
        "success",
        "bad_proof",
        "locked_out",
        "unknown_user",
        "rate_limited",
        "unsupported_version",
        "policy",
        "io_or_decode",
    ] {
        metrics::counter!("auth_attempts_total", "result" => label).increment(0);
    }

    // 4. Background poller: refresh process metrics every 5 s. Cheap
    // (~30-50 µs of work). The first collect() is invoked synchronously
    // so /metrics returns useful data immediately.
    collector.collect();
    let shutdown = Arc::new(Notify::new());
    let shutdown_for_poller = shutdown.clone();
    let poller_task = tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(5));
        // Burn the immediate first tick — we already collected once.
        interval.tick().await;
        loop {
            tokio::select! {
                biased;
                _ = shutdown_for_poller.notified() => break,
                _ = interval.tick() => {
                    collector.collect();
                }
            }
        }
    });

    // 4. Build the router.
    let app_state = AppState {
        state: state.clone(),
        prom: prom_handle,
    };
    let app = Router::new()
        .route("/healthz", get(healthz))
        .route("/readyz", get(readyz))
        .route("/metrics", get(metrics_handler))
        .route("/info", get(info_handler))
        .with_state(app_state);

    // 5. Bind + spawn the listener.
    let listener = TcpListener::bind(addr)
        .await
        .map_err(ObservabilityError::Bind)?;
    let bound_addr = listener
        .local_addr()
        .map_err(ObservabilityError::Bind)?;
    tracing::info!(bound_addr = %bound_addr, "observability HTTP server bound");

    let shutdown_for_serve = shutdown.clone();
    let listener_task = tokio::spawn(async move {
        let serve = axum::serve(listener, app);
        let shutdown_signal = async move {
            shutdown_for_serve.notified().await;
        };
        if let Err(e) = serve.with_graceful_shutdown(shutdown_signal).await {
            tracing::warn!(error = %e, "observability server exited with error");
        }
    });

    Ok(ObservabilityHandle {
        bound_addr,
        state,
        shutdown,
        listener_task,
        poller_task,
    })
}

#[derive(Clone)]
struct AppState {
    state: Arc<ObservabilityState>,
    /// `None` when `install_recorder = false` AND no prior install
    /// happened — `/metrics` then returns `503`.
    prom: Option<PrometheusHandle>,
}

// --------------------------------------------------------------------------
// Handlers
// --------------------------------------------------------------------------

async fn healthz() -> &'static str {
    // Trivial — process is alive iff this responds.
    "ok\n"
}

async fn readyz(State(s): State<AppState>) -> Response {
    if s.state.ready.load(Ordering::Acquire) {
        (StatusCode::OK, "ready\n").into_response()
    } else {
        (StatusCode::SERVICE_UNAVAILABLE, "not ready\n").into_response()
    }
}

async fn metrics_handler(State(s): State<AppState>) -> Response {
    match &s.prom {
        Some(h) => (StatusCode::OK, h.render()).into_response(),
        None => (
            StatusCode::SERVICE_UNAVAILABLE,
            "metrics recorder not installed\n",
        )
            .into_response(),
    }
}

#[derive(Serialize)]
struct InfoBody {
    uptime_seconds: u64,
    bound_addrs: Vec<String>,
    ready: bool,
}

async fn info_handler(State(s): State<AppState>) -> Response {
    let body = InfoBody {
        uptime_seconds: s.state.started_at.elapsed().as_secs(),
        bound_addrs: s
            .state
            .bound_addrs
            .read()
            .iter()
            .map(|a| a.to_string())
            .collect(),
        ready: s.state.ready.load(Ordering::Acquire),
    };
    (StatusCode::OK, Json(body)).into_response()
}
