//! Per-server activity gauges (R4b-1, `operational-views.md` §5.4 / D14):
//! the four unlabeled `shamir_active_*` gauges, rendered at scrape time
//! from a per-server source instead of the process-global Prometheus
//! recorder.
//!
//! Why not `metrics::gauge!` like the other exporters: the Prometheus
//! recorder is installed once per process, so multiple servers in one
//! integration-test process share a single global handle. A concurrent
//! scrape of server B could observe server A's counters written through
//! that shared recorder. The activity gauges therefore bypass the global
//! recorder entirely: [`render_activity_series`] appends four typed gauge
//! text series after the global `PrometheusHandle::render()` output in
//! the `/metrics` handler, sourced from the owning server's own
//! [`ObservabilityState`](crate::security::observability::ObservabilityState).
//!
//! The source is installed exactly once at production boot (see
//! `server_launcher/launch.rs`) and holds the request handler only by
//! `Weak`, so the observability state cannot keep the handler alive past
//! shutdown and never reports a stale server's mirrors after it is
//! dropped. While the handler is alive the gauges are exact live mirrors
//! of its `activity_counts()` snapshot — no `len()` on scc maps;
//! lock-free for the reader (the scrape reads only atomic mirrors and
//! takes no lock a registry writer takes; registry insert/remove still
//! take brief internal shard locks). No per-user or per-session labels
//! (privacy + cardinality, D3); no descriptors are registered with the
//! global recorder, so the appended HELP/TYPE blocks can never duplicate
//! a recorder-rendered descriptor.
//!
//! The `shamir_cancellations_total` counter family is per-server for the
//! same reason as the gauges: the process-global Prometheus recorder is
//! shared between servers in one process. It has exactly four fixed
//! `result` label values and no per-user or per-session label
//! (cardinality + privacy), and it is monotonic for the life of the
//! server.

use std::fmt;
use std::sync::Arc;

/// Live mirror counts of one running server's activity registries,
/// sampled by the per-state source at scrape time. Exact live mirrors:
/// every counted row of the activity view corresponds to one gauge unit.
/// The cancellation counters cover every cancellation operation:
/// requests, interactive transactions and cursors.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ActivityMetricsSnapshot {
    /// Requests currently in flight (the handler's in-flight table).
    pub requests: u64,
    /// Currently open sessions (the session store).
    pub sessions: u64,
    /// Interactive transactions currently open, including those awaiting
    /// commit.
    pub transactions: u64,
    /// Currently open cursors (the cursor registry).
    pub cursors: u64,
    /// Cancellation attempts, counted per result. Index order is FIXED and
    /// shared with [`CANCEL_RESULT_LABELS`]: 0 `cancelled`, 1
    /// `already_finished`, 2 `already_committing`, 3 `not_cancellable`.
    pub cancellations: [u64; 4],
}

/// The four exported series in render order: `(name, help)`. Unlabeled:
/// no per-user or per-session labels (D3 privacy + D14 cardinality cap).
pub(crate) const ACTIVITY_GAUGES: [(&str, &str); 4] = [
    (
        "shamir_active_requests",
        "Requests currently in flight in this server's in-flight table",
    ),
    (
        "shamir_active_sessions",
        "Sessions currently open in this server's session store",
    ),
    (
        "shamir_active_transactions",
        "Interactive transactions currently open, including awaiting-commit",
    ),
    (
        "shamir_active_cursors",
        "Cursors currently open in this server's cursor registry",
    ),
];

/// The four `result` label values of `shamir_cancellations_total`, in the
/// fixed index order of [`ActivityMetricsSnapshot::cancellations`].
pub(crate) const CANCEL_RESULT_LABELS: [&str; 4] = [
    "cancelled",
    "already_finished",
    "already_committing",
    "not_cancellable",
];

/// Per-server source closure installed on the owning server's
/// [`ObservabilityState`](crate::security::observability::ObservabilityState)
/// at boot. A newtype only so `ObservabilityState` can keep deriving
/// `Debug` across a `dyn Fn` field; the closure itself is opaque.
#[derive(Clone)]
pub(crate) struct ActivitySource(Arc<dyn Fn() -> ActivityMetricsSnapshot + Send + Sync>);

impl fmt::Debug for ActivitySource {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("ActivitySource").field(&"<dyn Fn>").finish()
    }
}

impl ActivitySource {
    pub(crate) fn new(source: Arc<dyn Fn() -> ActivityMetricsSnapshot + Send + Sync>) -> Self {
        Self(source)
    }

    /// Sample the live mirrors. Called once per `/metrics` scrape; the
    /// closure is lock-free (atomic mirror reads in the handler).
    pub(crate) fn sample(&self) -> ActivityMetricsSnapshot {
        (self.0)()
    }
}

/// Render the four unlabeled gauge series plus the four labelled
/// `shamir_cancellations_total` counter series in Prometheus text format,
/// ready to append after the global recorder's render output. Values come
/// straight from the snapshot; zero-touch servers without an installed
/// source render explicit zeros so scrapers see each family from the
/// first scrape.
pub(crate) fn render_activity_series(snapshot: &ActivityMetricsSnapshot) -> String {
    let values = [
        snapshot.requests,
        snapshot.sessions,
        snapshot.transactions,
        snapshot.cursors,
    ];
    let mut out = String::new();
    for ((name, help), value) in ACTIVITY_GAUGES.into_iter().zip(values) {
        out.push_str(&format!(
            "# HELP {name} {help}\n# TYPE {name} gauge\n{name} {value}\n"
        ));
    }
    out.push_str(
        "# HELP shamir_cancellations_total Cancellation attempts by result\n\
         # TYPE shamir_cancellations_total counter\n",
    );
    for (label, value) in CANCEL_RESULT_LABELS.into_iter().zip(snapshot.cancellations) {
        out.push_str(&format!(
            "shamir_cancellations_total{{result=\"{label}\"}} {value}\n"
        ));
    }
    out
}
