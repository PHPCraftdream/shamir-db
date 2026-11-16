//! Follower-owned, bounded replication metric state.
//!
//! Event-side methods only touch relaxed atomics. The callback copies values
//! and labels at scrape time; the server-security crate owns all rendering.
//! Named streams retain stable rows; leader request lag lives in weakly held guards,
//! while named acknowledgment history is shared. Overflow handles register unique
//! weakly held current-value cells. Active roles are merged; with no active role,
//! the most recently observed role supplies lag. Named leader receipt history
//! persists independently, while completed request lag is always zero. `(other)`
//! emits one zero row after overflow handles expire; reconnect totals survive drops.

use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, LazyLock, OnceLock, Weak};

use shamir_collections::THasher;
use shamir_server_security::observability::{
    install_replication_metrics_source, ReplicationSnapshot, StreamState,
};

const MAX_STREAM_KEYS: usize = 256;
const PRUNE_EVERY: u64 = 64;
const OTHER_DB: &str = "(other)";
const OTHER_REPO: &str = "(other)";

#[derive(Eq, Hash, PartialEq)]
struct StreamKey {
    db: String,
    repo: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum StreamRole {
    Follower,
    Leader,
}

pub(super) struct MetricCell {
    lag_versions: AtomicU64,
    observed_at: AtomicU64,
    last_receipt_ns: AtomicU64,
    acknowledged_bookmark: AtomicU64,
    state_code: AtomicU64,
    reconnects: AtomicU64,
    active: AtomicUsize,
}

impl MetricCell {
    fn new() -> Self {
        Self {
            lag_versions: AtomicU64::new(0),
            observed_at: AtomicU64::new(0),
            last_receipt_ns: AtomicU64::new(0),
            acknowledged_bookmark: AtomicU64::new(u64::MAX),
            state_code: AtomicU64::new(StreamState::Stopped.code()),
            reconnects: AtomicU64::new(0),
            active: AtomicUsize::new(0),
        }
    }

    fn merge_receipt_into(&self, snapshot: &mut MetricValues) {
        let receipt = self.last_receipt_ns.load(Ordering::Relaxed);
        if receipt != 0 && (snapshot.last_receipt_ns == 0 || receipt < snapshot.last_receipt_ns) {
            snapshot.last_receipt_ns = receipt;
        }
    }

    fn merge_into(
        &self,
        snapshot: &mut MetricValues,
        include_values: bool,
        count_reconnects: bool,
    ) {
        if include_values {
            snapshot.lag_versions = snapshot
                .lag_versions
                .max(self.lag_versions.load(Ordering::Relaxed));
            let receipt = self.last_receipt_ns.load(Ordering::Relaxed);
            if receipt != 0 && (snapshot.last_receipt_ns == 0 || receipt < snapshot.last_receipt_ns)
            {
                snapshot.last_receipt_ns = receipt;
            }
            let mut state = self.state_code.load(Ordering::Relaxed);
            if self.active.load(Ordering::Relaxed) != 0 && state == StreamState::Stopped.code() {
                state = StreamState::Starting.code();
            }
            if state_priority(state) > state_priority(snapshot.state_code) {
                snapshot.state_code = state;
            }
        }
        if count_reconnects {
            snapshot.reconnects = snapshot
                .reconnects
                .saturating_add(self.reconnects.load(Ordering::Relaxed));
        }
    }
}

struct LeaderCells {
    history: Arc<MetricCell>,
    requests: scc::HashMap<u64, Weak<MetricCell>, THasher>,
    next_id: AtomicU64,
}

impl LeaderCells {
    fn new() -> Self {
        Self {
            history: Arc::new(MetricCell::new()),
            requests: scc::HashMap::with_hasher(THasher::default()),
            next_id: AtomicU64::new(0),
        }
    }

    fn register_request(&self) -> Arc<MetricCell> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        // Amortized: one scan per PRUNE_EVERY registrations bounds the map between scrapes.
        if id % PRUNE_EVERY == PRUNE_EVERY - 1 {
            self.prune_requests();
        }
        let cell = Arc::new(MetricCell::new());
        let inserted = self.requests.insert_sync(id, Arc::downgrade(&cell));
        debug_assert!(inserted.is_ok());
        cell
    }

    fn merge_requests_into(&self, snapshot: &mut MetricValues) -> bool {
        let mut active = false;
        self.requests.iter_sync(|_, weak| {
            if let Some(cell) = weak.upgrade() {
                if cell.active.load(Ordering::Relaxed) != 0 {
                    active = true;
                    cell.merge_into(snapshot, true, false);
                }
            }
            true
        });
        self.prune_requests();
        active
    }

    fn prune_requests(&self) {
        let mut expired = Vec::new();
        self.requests.iter_sync(|id, cell| {
            if cell.strong_count() == 0 {
                expired.push(*id);
            }
            true
        });
        for id in expired {
            self.requests.remove_sync(&id);
        }
    }
}

struct NamedCells {
    follower: Arc<MetricCell>,
    leader: LeaderCells,
}

impl NamedCells {
    fn new() -> Self {
        Self {
            follower: Arc::new(MetricCell::new()),
            leader: LeaderCells::new(),
        }
    }
}

#[derive(Clone, Copy)]
struct MetricValues {
    lag_versions: u64,
    last_receipt_ns: u64,
    state_code: u64,
    reconnects: u64,
}

impl MetricValues {
    fn new() -> Self {
        Self {
            lag_versions: 0,
            last_receipt_ns: 0,
            state_code: StreamState::Stopped.code(),
            reconnects: 0,
        }
    }

    fn merge(&mut self, other: Self) {
        self.lag_versions = self.lag_versions.max(other.lag_versions);
        if other.last_receipt_ns != 0
            && (self.last_receipt_ns == 0 || other.last_receipt_ns < self.last_receipt_ns)
        {
            self.last_receipt_ns = other.last_receipt_ns;
        }
        if state_priority(other.state_code) > state_priority(self.state_code) {
            self.state_code = other.state_code;
        }
        self.reconnects = self.reconnects.saturating_add(other.reconnects);
    }

    fn into_snapshot(self, db: &str, repo: &str) -> ReplicationSnapshot {
        ReplicationSnapshot {
            db: db.to_owned(),
            repo: repo.to_owned(),
            lag_versions: self.lag_versions,
            last_receipt_ns: self.last_receipt_ns,
            state_code: self.state_code,
            reconnects: self.reconnects,
        }
    }
}

fn state_priority(code: u64) -> u8 {
    match code {
        code if code == StreamState::Stopped.code() => 0,
        code if code == StreamState::Streaming.code() => 1,
        code if code == StreamState::Starting.code() => 2,
        code if code == StreamState::BackingOff.code() => 3,
        _ => 4,
    }
}

pub(super) struct ReplicationRegistry {
    cells: scc::HashMap<StreamKey, NamedCells, THasher>,
    registered: AtomicUsize,
    limit: usize,
    // Weak overflow cells are pruned only on registration and scrape.
    overflow_cells: scc::HashMap<u64, std::sync::Weak<MetricCell>, THasher>,
    overflow_next_id: AtomicU64,
    overflow_seen: AtomicBool,
    overflow_reconnects: Arc<AtomicU64>,
}

impl ReplicationRegistry {
    pub(super) fn new(limit: usize) -> Self {
        Self {
            cells: scc::HashMap::with_hasher(THasher::default()),
            registered: AtomicUsize::new(0),
            limit,
            overflow_cells: scc::HashMap::with_hasher(THasher::default()),
            overflow_next_id: AtomicU64::new(0),
            overflow_seen: AtomicBool::new(false),
            overflow_reconnects: Arc::new(AtomicU64::new(0)),
        }
    }

    #[cfg(test)]
    pub(super) fn register(&self, db: &str, repo: &str) -> (Arc<MetricCell>, bool) {
        let (cell, overflow, _) = self.register_role(db, repo, StreamRole::Follower);
        (cell, overflow)
    }

    fn register_role(
        &self,
        db: &str,
        repo: &str,
        role: StreamRole,
    ) -> (Arc<MetricCell>, bool, Option<Arc<MetricCell>>) {
        let key = StreamKey {
            db: db.to_owned(),
            repo: repo.to_owned(),
        };
        match self.cells.entry_sync(key) {
            scc::hash_map::Entry::Occupied(entry) => match role {
                StreamRole::Follower => (Arc::clone(&entry.get().follower), false, None),
                StreamRole::Leader => {
                    let leader = &entry.get().leader;
                    (
                        leader.register_request(),
                        false,
                        Some(Arc::clone(&leader.history)),
                    )
                }
            },
            scc::hash_map::Entry::Vacant(entry) => {
                let reserved = self
                    .registered
                    .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |count| {
                        (count < self.limit).then_some(count + 1)
                    })
                    .is_ok();
                if reserved {
                    let cells = NamedCells::new();
                    let (cell, history) = match role {
                        StreamRole::Follower => (Arc::clone(&cells.follower), None),
                        StreamRole::Leader => (
                            cells.leader.register_request(),
                            Some(Arc::clone(&cells.leader.history)),
                        ),
                    };
                    let _entry = entry.insert_entry(cells);
                    (cell, false, history)
                } else {
                    drop(entry);
                    let (cell, overflow) = self.register_overflow();
                    (cell, overflow, None)
                }
            }
        }
    }

    fn register_overflow(&self) -> (Arc<MetricCell>, bool) {
        self.prune_overflow();
        let cell = Arc::new(MetricCell::new());
        let id = self.overflow_next_id.fetch_add(1, Ordering::Relaxed);
        let inserted = self.overflow_cells.insert_sync(id, Arc::downgrade(&cell));
        debug_assert!(inserted.is_ok());
        self.overflow_seen.store(true, Ordering::Relaxed);
        (cell, true)
    }

    fn prune_overflow(&self) {
        let mut expired = Vec::new();
        self.overflow_cells.iter_sync(|id, cell| {
            if cell.strong_count() == 0 {
                expired.push(*id);
            }
            true
        });
        for id in expired {
            self.overflow_cells.remove_sync(&id);
        }
    }

    fn snapshots(&self) -> Vec<ReplicationSnapshot> {
        let mut snapshots = Vec::with_capacity(self.registered.load(Ordering::Relaxed) + 1);
        self.cells.iter_sync(|key, cells| {
            let mut values = MetricValues::new();
            let mut leader_values = MetricValues::new();
            let follower_active = cells.follower.active.load(Ordering::Relaxed) != 0;
            let leader_active = cells.leader.merge_requests_into(&mut leader_values);
            let (include_follower, include_leader) = if follower_active || leader_active {
                (follower_active, leader_active)
            } else if cells.follower.observed_at.load(Ordering::Relaxed)
                >= cells.leader.history.observed_at.load(Ordering::Relaxed)
            {
                (true, false)
            } else {
                (false, true)
            };
            cells
                .follower
                .merge_into(&mut values, include_follower, true);
            if include_leader && leader_active {
                values.merge(leader_values);
            }
            // Named acknowledgments are repository history, not request lifetime
            // state, so the receipt timestamp survives every request independently.
            cells.leader.history.merge_receipt_into(&mut values);
            snapshots.push(values.into_snapshot(&key.db, &key.repo));
            true
        });

        let mut overflow = MetricValues::new();
        let mut has_overflow = false;
        self.overflow_cells.iter_sync(|_, weak| {
            if let Some(cell) = weak.upgrade() {
                has_overflow = true;
                let active = cell.active.load(Ordering::Relaxed) != 0;
                cell.merge_into(&mut overflow, active, false);
            }
            true
        });
        self.prune_overflow();
        overflow.reconnects = self.overflow_reconnects.load(Ordering::Relaxed);
        if has_overflow || self.overflow_seen.load(Ordering::Relaxed) {
            snapshots.push(overflow.into_snapshot(OTHER_DB, OTHER_REPO));
        }
        snapshots
            .sort_unstable_by(|left, right| (&left.db, &left.repo).cmp(&(&right.db, &right.repo)));
        snapshots
    }

    /// Source construction events are keyed for named repos; overflow sources
    /// contribute to one lifetime aggregate because their identities are not retained.
    pub(crate) fn note_source_created(&self, db: &str, repo: &str, is_reconnect: bool) {
        let (cell, overflow, _) = self.register_role(db, repo, StreamRole::Follower);
        if is_reconnect {
            if overflow {
                self.overflow_reconnects.fetch_add(1, Ordering::Relaxed);
            } else {
                cell.reconnects.fetch_add(1, Ordering::Relaxed);
            }
        }
    }

    #[cfg(test)]
    pub(super) fn snapshot_for_test(&self) -> Vec<ReplicationSnapshot> {
        self.snapshots()
    }

    #[cfg(test)]
    pub(super) fn registered_count(&self) -> usize {
        self.registered.load(Ordering::Relaxed)
    }
}

static REGISTRY: LazyLock<ReplicationRegistry> =
    LazyLock::new(|| ReplicationRegistry::new(MAX_STREAM_KEYS));
static SOURCE_INSTALLED: OnceLock<()> = OnceLock::new();
static NEXT_OBSERVATION: AtomicU64 = AtomicU64::new(1);

/// Return an immutable scrape snapshot from the replication owner's registry.
pub fn replication_metrics_snapshot() -> Vec<ReplicationSnapshot> {
    REGISTRY.snapshots()
}

fn install_snapshot_source() {
    SOURCE_INSTALLED
        .get_or_init(|| install_replication_metrics_source(replication_metrics_snapshot));
}

struct OwnerMetrics {
    cell: Arc<MetricCell>,
    ack_history: Option<Arc<MetricCell>>,
}

impl OwnerMetrics {
    fn start(registry: &ReplicationRegistry, db: &str, repo: &str, role: StreamRole) -> Self {
        let (cell, _, ack_history) = registry.register_role(db, repo, role);
        if cell.active.fetch_add(1, Ordering::Relaxed) == 0 {
            cell.state_code
                .store(StreamState::Starting.code(), Ordering::Relaxed);
        }
        Self { cell, ack_history }
    }

    fn set_state(&self, state: StreamState) {
        self.cell.state_code.store(state.code(), Ordering::Relaxed);
    }

    fn set_lag(&self, lag_versions: u64) {
        self.cell
            .lag_versions
            .store(lag_versions, Ordering::Relaxed);
        self.cell.observed_at.fetch_max(
            NEXT_OBSERVATION.fetch_add(1, Ordering::Relaxed),
            Ordering::Relaxed,
        );
    }

    fn note_durable_progress(&self, lag_versions: u64, receipt_ns: u64) {
        self.set_lag(lag_versions);
        self.cell
            .last_receipt_ns
            .store(receipt_ns, Ordering::Relaxed);
    }

    fn observe_pull(&self, current_version: u64, reported_bookmark: u64) {
        self.set_lag(current_version.saturating_sub(reported_bookmark));
        let Some(history) = self.ack_history.as_ref() else {
            return;
        };
        history.observed_at.fetch_max(
            NEXT_OBSERVATION.fetch_add(1, Ordering::Relaxed),
            Ordering::Relaxed,
        );
        let mut observed = history.acknowledged_bookmark.load(Ordering::Relaxed);
        loop {
            if observed == u64::MAX {
                // A valid reported bookmark is at most from_version - 1, so MAX is
                // only the sentinel.
                match history.acknowledged_bookmark.compare_exchange_weak(
                    observed,
                    reported_bookmark,
                    Ordering::Relaxed,
                    Ordering::Relaxed,
                ) {
                    Ok(_) => return,
                    Err(current) => observed = current,
                }
            } else if reported_bookmark <= observed {
                return;
            } else {
                match history.acknowledged_bookmark.compare_exchange_weak(
                    observed,
                    reported_bookmark,
                    Ordering::Relaxed,
                    Ordering::Relaxed,
                ) {
                    Ok(_) => {
                        history
                            .last_receipt_ns
                            .fetch_max(unix_now_ns(), Ordering::Relaxed);
                        return;
                    }
                    Err(current) => observed = current,
                }
            }
        }
    }
}

impl Drop for OwnerMetrics {
    fn drop(&mut self) {
        if self.cell.active.fetch_sub(1, Ordering::Relaxed) == 1 {
            self.cell
                .state_code
                .store(StreamState::Stopped.code(), Ordering::Relaxed);
            if self.cell.active.load(Ordering::Relaxed) != 0 {
                let _ = self.cell.state_code.compare_exchange(
                    StreamState::Stopped.code(),
                    StreamState::Starting.code(),
                    Ordering::Relaxed,
                    Ordering::Relaxed,
                );
            }
        }
    }
}

/// Per-loop handle resolved once at loop entry; subsequent updates are atomic.
pub(super) struct StreamMetrics(OwnerMetrics);

impl StreamMetrics {
    pub(super) fn start(
        db_name: &str,
        repo: &str,
        #[cfg(test)] registry: Option<&ReplicationRegistry>,
    ) -> Self {
        install_snapshot_source();
        #[cfg(test)]
        let registry = registry.unwrap_or(&REGISTRY);
        #[cfg(not(test))]
        let registry = &REGISTRY;
        Self(OwnerMetrics::start(
            registry,
            db_name,
            repo,
            StreamRole::Follower,
        ))
    }

    pub(super) fn set_state(&self, state: StreamState) {
        self.0.set_state(state);
    }

    pub(super) fn set_lag(&self, lag_versions: u64) {
        self.0.set_lag(lag_versions);
    }

    pub(super) fn note_durable_progress(&self, lag_versions: u64, receipt_ns: u64) {
        self.0.note_durable_progress(lag_versions, receipt_ns);
    }
}

/// Request-scoped leader bridge; leader/follower snapshots merge without a role label.
pub struct LeaderStreamMetrics(OwnerMetrics);

impl LeaderStreamMetrics {
    /// Start observing one authenticated leader pull request for `(db, repo)`.
    pub fn new(db: &str, repo: &str) -> Self {
        install_snapshot_source();
        Self(OwnerMetrics::start(&REGISTRY, db, repo, StreamRole::Leader))
    }

    #[cfg(test)]
    pub(super) fn new_for_test(db: &str, repo: &str, registry: &ReplicationRegistry) -> Self {
        Self(OwnerMetrics::start(registry, db, repo, StreamRole::Leader))
    }

    /// Observe the successful pull's leader version and request bookmark.
    /// Receipt time advances only when the acknowledged bookmark increases.
    pub fn observe_pull(&self, current_version: u64, reported_bookmark: u64) {
        self.0.observe_pull(current_version, reported_bookmark);
        self.0.set_state(StreamState::Streaming);
    }
}

/// Record a supervisor source construction. Pass `false` for its first source
/// and `true` only when recreating that repo's source; retries do not count.
pub(crate) fn note_source_created(db: &str, repo: &str, is_reconnect: bool) {
    REGISTRY.note_source_created(db, repo, is_reconnect);
}

/// Unix time used only after the durable bookmark has advanced.
pub(super) fn unix_now_ns() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |duration| {
            duration.as_nanos().min(u128::from(u64::MAX)) as u64
        })
}
