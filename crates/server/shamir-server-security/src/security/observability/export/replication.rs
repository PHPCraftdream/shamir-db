//! Snapshot-only replication metric export (design `engine-metrics.md` §2.8).
//!
//! The replication owner installs one callback; scraping reads that callback's
//! value snapshots and is the only place that touches the metrics recorder.

use std::sync::OnceLock;
use std::time::{SystemTime, UNIX_EPOCH};

/// Immutable replication values supplied by the owning subsystem at scrape.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ReplicationSnapshot {
    pub db: String,
    pub repo: String,
    pub lag_versions: u64,
    /// Unix timestamp in nanoseconds; zero means no durable receipt yet.
    pub last_receipt_ns: u64,
    pub state_code: u64,
    pub reconnects: u64,
}

/// Closed state set for a replication stream.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StreamState {
    Starting,
    Streaming,
    BackingOff,
    Stopped,
}

impl StreamState {
    pub const fn label(self) -> &'static str {
        match self {
            Self::Starting => "starting",
            Self::Streaming => "streaming",
            Self::BackingOff => "backing_off",
            Self::Stopped => "stopped",
        }
    }

    pub const fn code(self) -> u64 {
        match self {
            Self::Starting => 1,
            Self::Streaming => 2,
            Self::BackingOff => 3,
            Self::Stopped => 4,
        }
    }
}

pub const STREAM_STATES: [StreamState; 4] = [
    StreamState::Starting,
    StreamState::Streaming,
    StreamState::BackingOff,
    StreamState::Stopped,
];

static SOURCE: OnceLock<fn() -> Vec<ReplicationSnapshot>> = OnceLock::new();

/// Install the process-wide snapshot source. The first owner registration wins.
pub fn install_replication_metrics_source(source: fn() -> Vec<ReplicationSnapshot>) {
    let _ = SOURCE.set(source);
}

/// Describe the replication metric families once during observability setup.
pub(crate) fn register_replication_metrics() {
    metrics::describe_gauge!(
        "shamir_replication_lag_versions",
        metrics::Unit::Count,
        "Leader-advertised version minus the follower's durable bookmark"
    );
    metrics::describe_gauge!(
        "shamir_replication_lag_seconds",
        metrics::Unit::Seconds,
        "Seconds since the last durably applied receipt; absent until the first receipt"
    );
    metrics::describe_gauge!(
        "shamir_replication_stream_state",
        metrics::Unit::Count,
        "Follower stream state, one-hot over the closed state set"
    );
    metrics::describe_counter!(
        "shamir_replication_reconnects_total",
        metrics::Unit::Count,
        "Follower transport retry attempts after an established stream fails"
    );
}

/// Render the latest owner snapshots, computing receipt age only at scrape.
pub(crate) fn record_replication_metrics() {
    let Some(source) = SOURCE.get() else {
        return;
    };
    let snapshots = source();
    let now_ns = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| {
            duration.as_nanos().min(u128::from(u64::MAX)) as u64
        });

    for snapshot in snapshots {
        metrics::gauge!(
            "shamir_replication_lag_versions",
            "db" => snapshot.db.clone(),
            "repo" => snapshot.repo.clone()
        )
        .set(snapshot.lag_versions as f64);
        if snapshot.last_receipt_ns != 0 {
            let age_ns = now_ns.saturating_sub(snapshot.last_receipt_ns);
            metrics::gauge!(
                "shamir_replication_lag_seconds",
                "db" => snapshot.db.clone(),
                "repo" => snapshot.repo.clone()
            )
            .set(age_ns as f64 / 1_000_000_000.0);
        }
        for state in STREAM_STATES {
            let value = u8::from(snapshot.state_code == state.code());
            metrics::gauge!(
                "shamir_replication_stream_state",
                "db" => snapshot.db.clone(),
                "repo" => snapshot.repo.clone(),
                "state" => state.label()
            )
            .set(f64::from(value));
        }
        metrics::counter!(
            "shamir_replication_reconnects_total",
            "db" => snapshot.db,
            "repo" => snapshot.repo
        )
        .absolute(snapshot.reconnects);
    }
}
