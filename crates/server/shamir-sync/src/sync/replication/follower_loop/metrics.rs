//! Follower-owned, bounded replication metric state.
//!
//! Event-side methods only touch relaxed atomics. The callback copies values
//! and labels at scrape time; the server-security crate owns all rendering.

use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, LazyLock};

#[cfg(not(test))]
use std::sync::OnceLock;

use shamir_collections::THasher;
use shamir_server_security::observability::{ReplicationSnapshot, StreamState};
#[cfg(test)]
use shamir_server_security::observability::install_replication_metrics_source;

const MAX_STREAM_KEYS: usize = 256;
const OTHER_DB: &str = "(other)";
const OTHER_REPO: &str = "(other)";

#[derive(Eq, Hash, PartialEq)]
struct StreamKey {
    db: String,
    repo: String,
    overflow: bool,
}

pub(super) struct MetricCell {
    lag_versions: AtomicU64,
    last_receipt_ns: AtomicU64,
    state_code: AtomicU64,
    reconnects: AtomicU64,
    overflow: bool,
}

impl MetricCell {
    fn new(overflow: bool) -> Self {
        Self {
            lag_versions: AtomicU64::new(0),
            last_receipt_ns: AtomicU64::new(0),
            state_code: AtomicU64::new(StreamState::Starting.code()),
            reconnects: AtomicU64::new(0),
            overflow,
        }
    }

    fn snapshot(&self, db: &str, repo: &str) -> ReplicationSnapshot {
        ReplicationSnapshot {
            db: db.to_owned(),
            repo: repo.to_owned(),
            lag_versions: self.lag_versions.load(Ordering::Relaxed),
            last_receipt_ns: self.last_receipt_ns.load(Ordering::Relaxed),
            state_code: self.state_code.load(Ordering::Relaxed),
            reconnects: self.reconnects.load(Ordering::Relaxed),
        }
    }
}

pub(super) struct ReplicationRegistry {
    cells: scc::HashMap<StreamKey, Arc<MetricCell>, THasher>,
    registered: AtomicUsize,
    limit: usize,
}

impl ReplicationRegistry {
    pub(super) fn new(limit: usize) -> Self {
        Self {
            cells: scc::HashMap::with_hasher(THasher::default()),
            registered: AtomicUsize::new(0),
            limit,
        }
    }

    #[cfg(test)]
    pub(super) fn register(&self, db: &str, repo: &str) -> (Arc<MetricCell>, bool) {
        let key = StreamKey {
            db: db.to_owned(),
            repo: repo.to_owned(),
            overflow: false,
        };
        match self.cells.entry_sync(key) {
            scc::hash_map::Entry::Occupied(entry) => {
                let cell = Arc::clone(entry.get());
                let overflow = cell.overflow;
                (cell, overflow)
            }
            scc::hash_map::Entry::Vacant(entry) => {
                let reserved = self
                    .registered
                    .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |count| {
                        (count < self.limit).then_some(count + 1)
                    })
                    .is_ok();
                if reserved {
                    let cell = Arc::new(MetricCell::new(false));
                    let result = Arc::clone(&cell);
                    let _entry = entry.insert_entry(cell);
                    (result, false)
                } else {
                    drop(entry);
                    self.other_cell()
                }
            }
        }
    }

    fn other_cell(&self) -> (Arc<MetricCell>, bool) {
        let key = StreamKey {
            db: OTHER_DB.to_owned(),
            repo: OTHER_REPO.to_owned(),
            overflow: true,
        };
        match self.cells.entry_sync(key) {
            scc::hash_map::Entry::Occupied(entry) => (Arc::clone(entry.get()), true),
            scc::hash_map::Entry::Vacant(entry) => {
                let cell = Arc::new(MetricCell::new(true));
                let result = Arc::clone(&cell);
                let _entry = entry.insert_entry(cell);
                (result, true)
            }
        }
    }

    fn snapshots(&self) -> Vec<ReplicationSnapshot> {
        let mut snapshots = Vec::new();
        self.cells.iter_sync(|key, cell| {
            let (db, repo) = if cell.overflow {
                (OTHER_DB, OTHER_REPO)
            } else {
                (key.db.as_str(), key.repo.as_str())
            };
            snapshots.push(cell.snapshot(db, repo));
            true
        });
        snapshots
    }

    #[cfg(test)]
    pub(super) fn registered_count(&self) -> usize {
        self.registered.load(Ordering::Relaxed)
    }
}

static REGISTRY: LazyLock<ReplicationRegistry> =
    LazyLock::new(|| ReplicationRegistry::new(MAX_STREAM_KEYS));

#[cfg(test)]
fn snapshot_source() -> Vec<ReplicationSnapshot> {
    REGISTRY.snapshots()
}

#[cfg(test)]
fn install_snapshot_source() {
    install_replication_metrics_source(snapshot_source);
}

#[cfg(not(test))]
static SOURCE_INSTALLED: OnceLock<()> = OnceLock::new();

#[cfg(not(test))]
fn snapshot_source() -> Vec<ReplicationSnapshot> {
    REGISTRY.snapshots()
}

#[cfg(not(test))]
fn install_snapshot_source() {
    SOURCE_INSTALLED.get_or_init(|| install_replication_metrics_source(snapshot_source));
}

/// Per-loop handle resolved once at loop entry; all subsequent updates are
/// direct atomic operations with no key lookup or key allocation.
pub(super) struct StreamMetrics {
    cell: Arc<MetricCell>,
    overflow: bool,
}

impl StreamMetrics {
    pub(super) fn start(
        db_name: &str,
        repo: &str,
        #[cfg(test)] registry: Option<&ReplicationRegistry>,
    ) -> Self {
        #[cfg(test)]
        install_snapshot_source();
        #[cfg(not(test))]
        install_snapshot_source();

        #[cfg(test)]
        let (cell, overflow) = match registry {
            Some(registry) => registry.register(db_name, repo),
            None => REGISTRY.register(db_name, repo),
        };
        #[cfg(not(test))]
        let (cell, overflow) = REGISTRY.register(db_name, repo);

        cell.state_code
            .store(StreamState::Starting.code(), Ordering::Relaxed);
        Self { cell, overflow }
    }

    pub(super) fn set_state(&self, state: StreamState) {
        // `(other)` represents several streams; the latest state is the
        // bounded approximation available without tracking per-overflow keys.
        self.cell
            .state_code
            .store(state.code(), Ordering::Relaxed);
    }

    pub(super) fn set_lag(&self, lag_versions: u64) {
        if self.overflow {
            self.cell
                .lag_versions
                .fetch_max(lag_versions, Ordering::Relaxed);
        } else {
            self.cell
                .lag_versions
                .store(lag_versions, Ordering::Relaxed);
        }
    }

    pub(super) fn note_durable_progress(&self, lag_versions: u64, receipt_ns: u64) {
        self.set_lag(lag_versions);
        if self.overflow {
            self.cell
                .last_receipt_ns
                .fetch_max(receipt_ns, Ordering::Relaxed);
        } else {
            self.cell
                .last_receipt_ns
                .store(receipt_ns, Ordering::Relaxed);
        }
    }

    pub(super) fn note_reconnect_attempt(&self) {
        self.cell.reconnects.fetch_add(1, Ordering::Relaxed);
    }
}

impl Drop for StreamMetrics {
    fn drop(&mut self) {
        self.set_state(StreamState::Stopped);
    }
}

/// Unix time used only after the durable bookmark has advanced.
pub(super) fn unix_now_ns() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |duration| duration.as_nanos().min(u128::from(u64::MAX)) as u64)
}
