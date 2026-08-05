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
//! * **`/info`** — pretty-printed server info for curl-debugging by an operator.
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

use arc_swap::ArcSwap;

use axum::{
    extract::State,
    http::{header, StatusCode},
    response::{IntoResponse, Response},
    routing::get,
    Router,
};
use metrics_exporter_prometheus::{PrometheusBuilder, PrometheusHandle};
use metrics_process::Collector;
use serde::Serialize;
use tokio::net::TcpListener;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

use crate::byte_budget::ByteBudget;

/// Live state shared between the HTTP handlers and the boot path.
///
/// `ready` flips to `true` once `ServerLauncher::launch` has bound every
/// listener. The data-path accept loops never touch this field — only
/// `/readyz` reads it.
#[derive(Debug)]
pub struct ObservabilityState {
    pub ready: AtomicBool,
    pub started_at: std::time::Instant,
    pub bound_addrs: ArcSwap<Vec<SocketAddr>>,
}

impl ObservabilityState {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            ready: AtomicBool::new(false),
            started_at: std::time::Instant::now(),
            bound_addrs: ArcSwap::from_pointee(Vec::new()),
        })
    }

    pub fn mark_ready(&self) {
        self.ready.store(true, Ordering::Release);
    }

    pub fn set_bound_addrs(&self, addrs: Vec<SocketAddr>) {
        self.bound_addrs.store(Arc::new(addrs));
    }
}

/// Handle to a running observability server. `shutdown` MUST be called
/// explicitly to stop it — dropping this handle does NOT cancel
/// `listener_task`/`poller_task` (a bare `CancellationToken` has no
/// Drop-triggered cancel; that's `tokio_util::sync::DropGuard`'s job,
/// which this type doesn't use) nor abort the `JoinHandle`s, so an
/// un-shut-down drop leaks both background tasks running detached.
pub struct ObservabilityHandle {
    pub bound_addr: SocketAddr,
    pub state: Arc<ObservabilityState>,
    shutdown: CancellationToken,
    listener_task: JoinHandle<()>,
    poller_task: JoinHandle<()>,
}

impl ObservabilityHandle {
    /// Stop the HTTP listener and the process-metrics poller.
    ///
    /// F-68 (#895) cluster D / task #124 added timestamped `tracing` events
    /// around each of the two `JoinHandle` awaits individually, suspecting
    /// `axum::serve`'s graceful shutdown waiting on a lingering keep-alive
    /// connection (`listener_task`). Task #922 / F-68b used that
    /// instrumentation on a real ubuntu-latest CI hang
    /// (`metrics_exposes_unbounded_sentinel_when_no_byte_budget` /
    /// `metrics_exposes_finite_byte_budget_gauges`, run `30757334929`) and it
    /// disproved that hypothesis: the log showed `listener_task.await`
    /// returning in ~100µs, then hanging forever on `poller_task.await` with
    /// no further output. Root cause: `poller_task` (below, in
    /// `spawn_with_byte_budget`) used to wait on `shutdown_for_poller
    /// .notified()` inside a `tokio::select!` — `Notify::notify_waiters()`
    /// only wakes tasks that are actively polling `.notified()` AT THE
    /// MOMENT it is called; it stores no permit. Same class of race
    /// `Scheduler`'s own doc comment (`scheduler.rs`, near its
    /// `shutdown_tx` field) already names precisely: if `notify_waiters()`
    /// fires before the spawned task has been polled for the first time
    /// (not yet inside its `select!` at all) — plausible here since
    /// `tokio::spawn` only schedules the task, it doesn't run it
    /// synchronously — the notify is silently dropped and the task waits
    /// out its next `interval.tick()` (5s here) instead. The same is also
    /// true of the narrower "between loop iterations" window (each
    /// `select!` re-evaluation creates a *new* `Notified` future with its
    /// own dead zone) — either window reproduces the observed hang;
    /// `CancellationToken`'s fix below closes both by construction, so
    /// distinguishing which one fired in the specific CI repro isn't load
    /// bearing for the fix. This is the exact same lossy-`Notify` class
    /// already documented and fixed once in this crate for the root
    /// shutdown signal (see `ServerHandle::shutdown_token`'s doc comment in
    /// `server/server_handle.rs`, and `Scheduler`'s broadcast-based fix) —
    /// this call site was missed during that remediation. Fixed here by
    /// switching to `tokio_util::sync::
    /// CancellationToken`, whose `cancel()` is a persistent flag: any
    /// `.cancelled()` future (existing or newly created) resolves
    /// immediately once cancelled, closing the race by construction. The
    /// `tracing` instrumentation is kept (still useful if some other stall
    /// appears here later).
    pub async fn shutdown(self) {
        let shutdown_started = std::time::Instant::now();
        tracing::debug!("ObservabilityHandle::shutdown: enter, calling cancel()");
        self.shutdown.cancel();
        tracing::debug!(
            elapsed = ?shutdown_started.elapsed(),
            "ObservabilityHandle::shutdown: cancel() returned"
        );

        let listener_wait_started = std::time::Instant::now();
        tracing::debug!("ObservabilityHandle::shutdown: awaiting listener_task");
        let _ = self.listener_task.await;
        tracing::debug!(
            elapsed = ?listener_wait_started.elapsed(),
            "ObservabilityHandle::shutdown: listener_task.await returned \
             (axum::serve's with_graceful_shutdown resolved)"
        );

        let poller_wait_started = std::time::Instant::now();
        tracing::debug!("ObservabilityHandle::shutdown: awaiting poller_task");
        let _ = self.poller_task.await;
        tracing::debug!(
            elapsed = ?poller_wait_started.elapsed(),
            "ObservabilityHandle::shutdown: poller_task.await returned"
        );

        tracing::debug!(
            total_elapsed = ?shutdown_started.elapsed(),
            "ObservabilityHandle::shutdown: exit"
        );
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
    /// M-tier audit M5: refused to bind /metrics + friends to a
    /// non-loopback address without an explicit opt-in. /metrics
    /// exposes `auth_attempts_total` labelled by result, including
    /// `locked_out` — a useful side-channel for distributed credential
    /// probing. Operators that need a public endpoint must set
    /// `allow_public_metrics = true` (and front it with auth at the
    /// reverse proxy).
    #[error(
        "observability bind {0} is non-loopback but allow_public_metrics \
         was not set — refusing to expose /metrics publicly"
    )]
    NonLoopbackBindRejected(SocketAddr),
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
///
/// `tx_metrics` — optional transaction metrics to bridge into Prometheus
/// gauges. Snapshotted every 5 s in the background poller. Pass `None`
/// when no RepoTxGate is running yet; the gauges are still registered at
/// zero so Prometheus scrapers see them from the first scrape.
///
/// `byte_budget` — F-29 (#822): optional RI-15 global in-flight
/// response-byte budget to bridge into Prometheus gauges
/// (`shamir_inflight_response_bytes_used`/`_cap`), snapshotted every 5 s
/// in the same background poller alongside `tx_metrics`. Pass `None` in
/// contexts that don't construct a `ByteBudget` (e.g. tests that call
/// `spawn` directly without a full server boot) — the gauges are still
/// registered at zero so Prometheus scrapers see them from the first
/// scrape.
///
/// `shamir_db` — #984: optional `Arc<ShamirDb>` used to compute the
/// `shamir_degraded_indexes_total` gauge (count of indexes stuck in
/// `Building` state across all open tables). Snapshotted every 5 s in
/// the same background poller. Pass `None` in contexts without a live
/// database (tests) — the gauge is still registered at zero.
///
/// # `/readyz` decision (#984)
///
/// `/readyz` stays **strictly binary and unchanged**: it still checks
/// only "listeners bound". Rationale: (a) the module doc above
/// documents that `/readyz` must stay cheap and subsystem-independent —
/// wiring in a `ShamirDb` traversal would violate that invariant; (b) a
/// degraded index is a *data-quality* issue, not a *boot-readiness*
/// issue — the server is perfectly able to serve traffic, it just has
/// a stuck index that needs `doctor::repair()`. Wiring it into
/// `/readyz` would take a pod out of rotation for a non-fatal data
/// issue, which is the wrong operational response. The
/// `shamir_degraded_indexes_total` gauge on `/metrics` is the correct
/// push signal: alert on it, but don't change traffic routing.
///
/// `allow_public_metrics` — M-tier audit M5. When `false` (default for
/// callers), a non-loopback `addr` triggers
/// [`ObservabilityError::NonLoopbackBindRejected`] before any socket
/// bind. /metrics exposes counters such as `auth_attempts_total{result =
/// "locked_out"}`, which is a useful signal for an attacker probing a
/// large user-base. Operators that need a publicly-scraped endpoint
/// MUST opt in explicitly and front the port with reverse-proxy auth
/// (bearer token, mTLS, or IP allowlist).
pub async fn spawn(
    addr: SocketAddr,
    state: Arc<ObservabilityState>,
    install_recorder: bool,
    tx_metrics: Option<Arc<shamir_tx::TxMetrics>>,
    allow_public_metrics: bool,
) -> Result<ObservabilityHandle, ObservabilityError> {
    spawn_with_byte_budget(
        addr,
        state,
        install_recorder,
        tx_metrics,
        None,
        None,
        allow_public_metrics,
    )
    .await
}

/// Same as [`spawn`], plus an optional [`ByteBudget`] to bridge into the
/// `shamir_inflight_response_bytes_used`/`_cap` gauges (F-29, #822), and an
/// optional `Arc<shamir_db::ShamirDb>` to bridge into the
/// `shamir_degraded_indexes_total` gauge (#984). Split out so `spawn`'s
/// existing call sites/signature (already used directly by
/// `observability_http.rs`'s tests) don't need updating just to add these
/// new optional arguments.
#[allow(clippy::too_many_arguments)]
pub async fn spawn_with_byte_budget(
    addr: SocketAddr,
    state: Arc<ObservabilityState>,
    install_recorder: bool,
    tx_metrics: Option<Arc<shamir_tx::TxMetrics>>,
    byte_budget: Option<ByteBudget>,
    shamir_db: Option<Arc<shamir_db::ShamirDb>>,
    allow_public_metrics: bool,
) -> Result<ObservabilityHandle, ObservabilityError> {
    // 0. M-tier audit M5: reject non-loopback binds without explicit
    //    opt-in. Done BEFORE the recorder install / TcpListener::bind
    //    so a misconfigured server fails fast and doesn't briefly
    //    expose a port.
    if !addr.ip().is_loopback() && !allow_public_metrics {
        return Err(ObservabilityError::NonLoopbackBindRejected(addr));
    }

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

    // Tx metrics — gauges snapshotted from TxMetrics every poller cycle.
    metrics::describe_gauge!(
        "shamir_tx_started_total",
        metrics::Unit::Count,
        "Transactions started"
    );
    metrics::describe_gauge!(
        "shamir_tx_committed_total",
        metrics::Unit::Count,
        "Transactions committed"
    );
    metrics::describe_gauge!(
        "shamir_tx_aborted_ssi_total",
        metrics::Unit::Count,
        "SSI conflict aborts"
    );
    metrics::describe_gauge!(
        "shamir_tx_aborted_expired_total",
        metrics::Unit::Count,
        "Expired tx aborts"
    );
    metrics::describe_gauge!(
        "shamir_tx_aborted_storage_total",
        metrics::Unit::Count,
        "Storage error aborts"
    );
    metrics::describe_gauge!("shamir_gc_runs_total", metrics::Unit::Count, "GC runs");
    metrics::describe_gauge!(
        "shamir_gc_entries_deleted_total",
        metrics::Unit::Count,
        "GC entries deleted"
    );

    // RI-15 / F-29 (#822) — global in-flight response-byte budget gauges.
    // `used()`/`cap()` are cheap `AtomicUsize` loads (see `byte_budget.rs`);
    // snapshotted every poller cycle below alongside the tx/gc gauges.
    metrics::describe_gauge!(
        "shamir_inflight_response_bytes_used",
        metrics::Unit::Bytes,
        "Bytes currently reserved against the RI-15 global in-flight \
         response-byte budget (sum across every concurrently-executing \
         batch/connection)"
    );
    metrics::describe_gauge!(
        "shamir_inflight_response_bytes_cap",
        metrics::Unit::Bytes,
        "Configured cap (bytes) on the RI-15 global in-flight \
         response-byte budget. -1 means the budget is unbounded (operator \
         explicitly opted out via `max_inflight_response_bytes: null`) — \
         distinguishable from a legitimate cap because a real cap can \
         never be negative"
    );

    // #984 — degraded (non-Ready) index count gauge. O(number of
    // indexes across ALREADY-OPEN tables), in-memory only, zero store
    // reads. Does NOT force-open closed tables — so the gauge reflects
    // currently-open tables only (an index on a table that has never
    // been accessed since boot is invisible until first query opens it).
    metrics::describe_gauge!(
        "shamir_degraded_indexes_total",
        metrics::Unit::Count,
        "Count of indexes NOT in Ready state (stuck-Building) across \
         currently-open tables only. Zero store reads — inspects in-memory \
         index registries. Does NOT open closed tables; an index on a \
         table that has never been queried since boot is invisible here \
         until first access opens it. Non-zero means an operator should \
         run doctor::repair() to rebuild the stuck index(es)."
    );

    // Zero-touch registration so gauges appear in /metrics from first scrape.
    metrics::gauge!("shamir_tx_started_total").set(0.0);
    metrics::gauge!("shamir_tx_committed_total").set(0.0);
    metrics::gauge!("shamir_tx_aborted_ssi_total").set(0.0);
    metrics::gauge!("shamir_tx_aborted_expired_total").set(0.0);
    metrics::gauge!("shamir_tx_aborted_storage_total").set(0.0);
    metrics::gauge!("shamir_gc_runs_total").set(0.0);
    metrics::gauge!("shamir_gc_entries_deleted_total").set(0.0);
    metrics::gauge!("shamir_inflight_response_bytes_used").set(0.0);
    metrics::gauge!("shamir_inflight_response_bytes_cap").set(
        byte_budget
            .as_ref()
            .and_then(ByteBudget::cap)
            .map(|c| c as f64)
            .unwrap_or(-1.0),
    );
    metrics::gauge!("shamir_degraded_indexes_total").set(0.0);

    // 4. Background poller: refresh process metrics every 5 s. Cheap
    // (~30-50 µs of work). The first collect() is invoked synchronously
    // so /metrics returns useful data immediately.
    collector.collect();
    // F-68b (#922): `CancellationToken`, not `Notify::notify_waiters()` — see
    // `ObservabilityHandle::shutdown`'s doc comment (this file) for the
    // lossy-`Notify` missed-wakeup bug this replaced (confirmed root cause of
    // a real 600s ubuntu-latest CI hang) and the precedent in
    // `server/server_handle.rs`'s `shutdown_token` doc comment.
    let shutdown = CancellationToken::new();
    let shutdown_for_poller = shutdown.clone();
    let tx_metrics_clone = tx_metrics;
    let byte_budget_clone = byte_budget;
    let shamir_db_clone = shamir_db;
    let poller_task = tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(5));
        // Burn the immediate first tick — we already collected once.
        interval.tick().await;
        loop {
            tokio::select! {
                biased;
                _ = shutdown_for_poller.cancelled() => break,
                _ = interval.tick() => {
                    collector.collect();
                    if let Some(ref tx_m) = tx_metrics_clone {
                        let snap = tx_m.snapshot();
                        metrics::gauge!("shamir_tx_started_total").set(snap.txs_started as f64);
                        metrics::gauge!("shamir_tx_committed_total").set(snap.txs_committed as f64);
                        metrics::gauge!("shamir_tx_aborted_ssi_total").set(snap.txs_aborted_ssi as f64);
                        metrics::gauge!("shamir_tx_aborted_expired_total").set(snap.txs_aborted_expired as f64);
                        metrics::gauge!("shamir_tx_aborted_storage_total").set(snap.txs_aborted_storage as f64);
                        metrics::gauge!("shamir_gc_runs_total").set(snap.gc_runs as f64);
                        metrics::gauge!("shamir_gc_entries_deleted_total").set(snap.gc_entries_deleted as f64);
                    }
                    // RI-15 / F-29 (#822): `used()` moves every request, so it
                    // (unlike `cap()`, fixed at boot and already set once at
                    // registration above) needs a fresh snapshot every cycle.
                    if let Some(ref bb) = byte_budget_clone {
                        metrics::gauge!("shamir_inflight_response_bytes_used").set(bb.used() as f64);
                    }
                    // #984: degraded-index count — O(indexes) in-memory walk,
                    // zero store reads.
                    if let Some(ref db) = shamir_db_clone {
                        let degraded = db.degraded_index_count().await;
                        metrics::gauge!("shamir_degraded_indexes_total")
                            .set(degraded as f64);
                    }
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
    let bound_addr = listener.local_addr().map_err(ObservabilityError::Bind)?;
    tracing::info!(bound_addr = %bound_addr, "observability HTTP server bound");

    let shutdown_for_serve = shutdown.clone();
    let listener_task = tokio::spawn(async move {
        let serve = axum::serve(listener, app);
        let shutdown_signal = async move {
            shutdown_for_serve.cancelled().await;
            // F-68 (#895) cluster D / task #124 suspected this
            // `with_graceful_shutdown` signal point as the hang source
            // (axum/hyper's graceful shutdown waiting on a lingering
            // keep-alive connection). Task #922 / F-68b confirmed via a real
            // CI hang's `tracing` log that this resolves in ~100µs — the
            // actual hang was in the poller task's now-fixed lossy `Notify`
            // wait (see `ObservabilityHandle::shutdown`'s doc comment).
            // `CancellationToken::cancelled()` closes the same class of race
            // here too (previously `Notify::notified()`), even though this
            // specific await was not observed to hang in the confirmed
            // repro.
            tracing::debug!(
                "observability listener_task: shutdown signal fired, \
                 waiting for axum::serve's graceful shutdown to drain \
                 open connections"
            );
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
            .load()
            .iter()
            .map(|a| a.to_string())
            .collect(),
        ready: s.state.ready.load(Ordering::Acquire),
    };
    let bytes = rmp_serde::to_vec_named(&body).unwrap_or_default();
    (
        StatusCode::OK,
        [(header::CONTENT_TYPE, "application/msgpack")],
        bytes,
    )
        .into_response()
}
