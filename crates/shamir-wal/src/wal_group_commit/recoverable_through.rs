//! Atomic target coalescing; successful generation evidence outlives waiters.

use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use shamir_storage::error::{DbError, DbResult};
use tokio::sync::Notify;

use super::group_commit::WalGroupCommit;
use super::wal_coverage::WalCoverage;
use crate::wal_sink::WalSink;

static SYNC_NOW_CALLS: AtomicU64 = AtomicU64::new(0);
static SYNC_LEADER_ROUNDS: AtomicU64 = AtomicU64::new(0);
static SYNC_NOW_ERRORS: AtomicU64 = AtomicU64::new(0);
static SYNC_NOW_DURATION_NS: AtomicU64 = AtomicU64::new(0);

/// Demanders retain only a target and error epoch on their own future stack.
/// No queue, oneshot, clock or allocation per demand. One detached worker owns
/// replay allocation/I/O and sparse coverage, never the append leader.
pub struct SyncCoordinator {
    pub(super) coverage: OnceLock<Arc<dyn WalCoverage>>,
    sink: Arc<WalSink>,
    leader: AtomicBool,
    pub(super) dirty_since_sync: AtomicBool,
    pub(super) written_generation: AtomicU64,
    requested: AtomicU64,
    successful: AtomicU64,
    has_success: AtomicBool,
    failure_epoch: AtomicU64,
    // ERROR PATH ONLY. Contention model: written once per failed sync cycle
    // by the single worker, read once by a waiter observing a failure epoch;
    // never held across `.await`; not a hot path. Enforced property: the
    // worker stores `(epoch.load()+1, cause)` strictly before the guard drop
    // bumps `failure_epoch` to that value, so a waiter only accepts a cause
    // whose stored cycle equals the epoch it read just before cloning the
    // slot. With interleaved failures the waiter (still uncovered) may report
    // a failure later than the one that first woke it, or fall back to the
    // generic text; it never reports a cause older than the epoch it read.
    failure_cause: Mutex<Option<(u64, Arc<DbError>)>>,
    pub(super) done: Notify,
    waiters: AtomicUsize,
    pub(super) rounds: AtomicU64,
    metric_calls: AtomicU64,
    metric_coalesced: AtomicU64,
    metric_errors: AtomicU64,
    metric_duration_ns: AtomicU64,
}

impl SyncCoordinator {
    pub fn new(sink: Arc<WalSink>) -> Self {
        Self {
            coverage: OnceLock::new(),
            sink,
            leader: AtomicBool::new(false),
            dirty_since_sync: AtomicBool::new(false),
            written_generation: AtomicU64::new(0),
            requested: AtomicU64::new(0),
            successful: AtomicU64::new(0),
            has_success: AtomicBool::new(false),
            failure_epoch: AtomicU64::new(0),
            failure_cause: Mutex::new(None),
            done: Notify::new(),
            waiters: AtomicUsize::new(0),
            rounds: AtomicU64::new(0),
            metric_calls: AtomicU64::new(0),
            metric_coalesced: AtomicU64::new(0),
            metric_errors: AtomicU64::new(0),
            metric_duration_ns: AtomicU64::new(0),
        }
    }

    fn covers(&self, target: u64) -> bool {
        self.has_success.load(Ordering::Acquire)
            && self.successful.load(Ordering::Acquire) >= target
    }

    fn start(self: &Arc<Self>) {
        if self
            .leader
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .is_err()
        {
            return;
        }
        if self.covers(self.requested.load(Ordering::SeqCst)) {
            self.leader.store(false, Ordering::SeqCst);
            self.done.notify_waiters();
            if !self.covers(self.requested.load(Ordering::SeqCst)) {
                self.start();
            }
            return;
        }
        let worker = Arc::clone(self);
        // fire-and-forget: detached by design; a demander cannot cancel fsync.
        // The RAII guard releases/wakes even on unwind or runtime cancellation.
        let guard = WorkerGuard {
            coordinator: Arc::clone(self),
            armed: true,
        };
        drop(tokio::spawn(async move { worker.run(guard).await }));
    }

    async fn run(self: Arc<Self>, mut guard: WorkerGuard) {
        loop {
            // Capture target BEFORE replay. Appends acknowledged before this
            // acquire are complete bytes; exact replay can include additional
            // bytes, but target success never promises a later generation.
            // Capture-clear BEFORE target; writes during I/O re-arm dirty.
            self.dirty_since_sync.store(false, Ordering::Release);
            let target = self.written_generation.load(Ordering::Acquire);
            let started = Instant::now();
            let result = async {
                if self.coverage.get().is_none() || matches!(self.sink.as_ref(), WalSink::Mem(_)) {
                    return self.sink.sync().await;
                }
                // Shared with rotation, including different captured files.
                // Ordinary appends never acquire this sync-only gate.
                let _sync = match self.sink.as_ref() {
                    WalSink::File(segments) => Some(segments.sync_gate.lock().await),
                    WalSink::Mem(_) => None,
                };
                let (entries, active) = self.sink.sync_snapshot().await?;
                let versions: Vec<_> = entries.iter().map(|e| e.commit_version).collect();
                let generation = self.coverage.get().map(|coverage| {
                    coverage.written(&versions, false);
                    coverage.capture()
                });
                self.sink.sync_captured(active.as_deref()).await?;
                if let (Some(coverage), Some(generation)) = (self.coverage.get(), generation) {
                    coverage.synced(generation);
                }
                Ok::<(), DbError>(())
            }
            .await;
            let ns = u64::try_from(started.elapsed().as_nanos()).unwrap_or(u64::MAX);
            self.rounds.fetch_add(1, Ordering::Relaxed);
            self.metric_duration_ns.fetch_add(ns, Ordering::Relaxed);
            SYNC_LEADER_ROUNDS.fetch_add(1, Ordering::Relaxed);
            SYNC_NOW_DURATION_NS.fetch_add(ns, Ordering::Relaxed);
            if let Err(err) = result {
                // Preserve the underlying sink error for `demand()`'s failure
                // branch. Store BEFORE returning so the guard drop below
                // publishes the new failure epoch only after the matching
                // cause is in place (a waiter that sees the epoch bump must
                // also see this cause).
                let cycle = self.failure_epoch.load(Ordering::Acquire) + 1;
                *self.failure_cause.lock().expect("failure cause mutex") =
                    Some((cycle, Arc::new(err)));
                // Drop publishes failure and relinquishes leadership; do not
                // retry a permanently failing disk in this worker loop.
                return;
            }
            self.successful.fetch_max(target, Ordering::Release);
            self.has_success.store(true, Ordering::Release);
            self.done.notify_waiters();
            if self.requested.load(Ordering::SeqCst) > target {
                continue;
            }
            guard.armed = false;
            self.leader.store(false, Ordering::SeqCst);
            // SeqCst pairs request publication with leadership release so
            // both sides cannot miss each other across independent atomics.
            // Close release/request race: a demand either wins start itself,
            // or this recheck schedules the newer target after releasing.
            if self.requested.load(Ordering::SeqCst) > target {
                self.start();
            }
            return;
        }
    }

    /// cancel-safe: yes â€” target publication belongs to detached work.
    async fn demand(self: &Arc<Self>) -> DbResult<u64> {
        // Legacy wrapper: stringify the owned cause to keep the PUBLIC
        // `sync_now()` DbResult surface's documented shape (same precedent
        // as the legacy `DbResult` entry points in wal_ticket.rs). Internal
        // callers use `demand_inner` to keep the original error.
        self.demand_inner()
            .await
            .map_err(|cause| DbError::Storage(format!("coalesced WAL sync failed: {cause}")))
    }

    /// cancel-safe; target publication belongs to detached work.
    async fn demand_inner(self: &Arc<Self>) -> Result<u64, Arc<DbError>> {
        let epoch = self.failure_epoch.load(Ordering::Acquire);
        let target = self.written_generation.load(Ordering::Acquire);
        self.requested.fetch_max(target, Ordering::SeqCst);
        SYNC_NOW_CALLS.fetch_add(1, Ordering::Relaxed);
        self.metric_calls.fetch_add(1, Ordering::Relaxed);
        self.waiters.fetch_add(1, Ordering::Release);
        self.done.notify_waiters();
        let _guard = SyncWaitGuard(self);
        let mut counted_coalesced = false;
        loop {
            let notified = self.done.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            // Proven success wins over ANY later error. Never classify an
            // already covered target by the outcome of an unrelated cycle.
            if self.covers(target) {
                return Ok(self.coverage.get().map_or(0, |c| c.recoverable_through()));
            }
            if self.failure_epoch.load(Ordering::Acquire) != epoch {
                if self.covers(target) {
                    continue;
                }
                // `observed` is re-read just before cloning the slot; a later
                // failure may have bumped it since the wake-up above. A
                // matching cycle proves the slot holds that epoch's cause.
                let observed = self.failure_epoch.load(Ordering::Acquire);
                let stored = self
                    .failure_cause
                    .lock()
                    .expect("failure cause mutex")
                    .clone();
                if let Some((cycle, cause)) = stored {
                    // A later failure may already have replaced the slot;
                    // then the cycles differ and we fall through to the
                    // generic text instead of misattributing.
                    if cycle == observed {
                        return Err(cause);
                    }
                }
                // Epoch bumped without a matching stored cause (guard dropped
                // on the cancellation path, or a later failure raced in):
                // fall back to the generic text.
                return Err(Arc::new(DbError::Storage(
                    "coalesced WAL sync failed or worker stopped".into(),
                )));
            }
            if !counted_coalesced && self.leader.load(Ordering::Acquire) {
                self.metric_coalesced.fetch_add(1, Ordering::Relaxed);
                counted_coalesced = true;
            }
            self.start();
            notified.await;
        }
    }
}

// Allocated once when the completion tracker installs its observer, not per
// commit. Coverage publication precedes the wake, including abort-only holes.
struct ProgressObserver {
    coverage: Arc<dyn WalCoverage>,
    coordinator: Arc<SyncCoordinator>,
}
impl WalCoverage for ProgressObserver {
    fn written(&self, versions: &[u64], memory: bool) {
        self.coverage.written(versions, memory);
    }
    fn capture(&self) -> u64 {
        self.coverage.capture()
    }
    fn synced(&self, generation: u64) {
        self.coverage.synced(generation);
    }
    fn enable_memory(&self) {
        self.coverage.enable_memory();
    }
    fn memory_completed(&self, through: u64) {
        self.coverage.memory_completed(through);
        self.coordinator.done.notify_waiters();
    }
    fn settle(&self, version: u64) {
        self.coverage.settle(version);
        self.coordinator.done.notify_waiters();
    }
    fn certify_exclusive_open(&self, through: u64) {
        self.coverage.certify_exclusive_open(through);
        self.coordinator.done.notify_waiters();
    }
    fn recoverable_through(&self) -> u64 {
        self.coverage.recoverable_through()
    }
}

struct WorkerGuard {
    coordinator: Arc<SyncCoordinator>,
    armed: bool,
}
impl Drop for WorkerGuard {
    fn drop(&mut self) {
        if self.armed {
            self.coordinator
                .dirty_since_sync
                .store(true, Ordering::Release);
            self.coordinator
                .metric_errors
                .fetch_add(1, Ordering::Relaxed);
            SYNC_NOW_ERRORS.fetch_add(1, Ordering::Relaxed);
            self.coordinator
                .failure_epoch
                .fetch_add(1, Ordering::Release);
            self.coordinator.leader.store(false, Ordering::SeqCst);
            self.coordinator.done.notify_waiters();
        }
    }
}

struct SyncWaitGuard<'a>(&'a SyncCoordinator);
impl Drop for SyncWaitGuard<'_> {
    fn drop(&mut self) {
        self.0.waiters.fetch_sub(1, Ordering::Relaxed);
    }
}

impl WalGroupCommit {
    /// Install exactly once before admission.
    pub fn install_coverage(&self, coverage: Arc<dyn WalCoverage>) -> DbResult<()> {
        if self.is_mem_sink() {
            coverage.enable_memory();
        }
        self.sync
            .coverage
            .set(coverage)
            .map_err(|_| DbError::Internal("WAL coverage already installed".into()))
    }
    pub fn coverage_observer(&self) -> Option<Arc<dyn WalCoverage>> {
        self.sync.coverage.get().map(|coverage| {
            Arc::new(ProgressObserver {
                coverage: Arc::clone(coverage),
                coordinator: Arc::clone(&self.sync),
            }) as Arc<dyn WalCoverage>
        })
    }
    pub fn recoverable_through(&self) -> u64 {
        self.sync
            .coverage
            .get()
            .map_or(0, |coverage| coverage.recoverable_through())
    }
    pub fn settle(&self, version: u64) {
        if let Some(coverage) = self.sync.coverage.get() {
            coverage.settle(version);
        }
        self.sync.done.notify_waiters();
    }
    /// cancel-safe: yes â€” observation only.
    pub async fn wait_recoverable(&self, version: u64, timeout: Duration) -> bool {
        tokio::time::timeout(timeout, async {
            loop {
                let notified = self.sync.done.notified();
                tokio::pin!(notified);
                notified.as_mut().enable();
                if self.recoverable_through() >= version {
                    return true;
                }
                notified.await;
            }
        })
        .await
        .unwrap_or(false)
    }
    pub fn max_committed_version(&self) -> u64 {
        self.sink.max_committed()
    }
    pub fn is_mem_sink(&self) -> bool {
        matches!(self.sink.as_ref(), WalSink::Mem(_))
    }
    #[doc(hidden)]
    pub fn sync_waiters(&self) -> usize {
        self.sync.waiters.load(Ordering::Relaxed)
    }
    /// Test phase handshake: register demands before releasing a paused sink.
    #[doc(hidden)]
    pub async fn wait_sync_waiters(&self, count: usize) {
        loop {
            let notified = self.sync.done.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            if self.sync.waiters.load(Ordering::Acquire) == count {
                return;
            }
            notified.await;
        }
    }
    pub fn sync_now_calls() -> u64 {
        SYNC_NOW_CALLS.load(Ordering::Relaxed)
    }
    pub fn sync_rounds() -> u64 {
        SYNC_LEADER_ROUNDS.load(Ordering::Relaxed)
    }
    pub fn sync_errors() -> u64 {
        SYNC_NOW_ERRORS.load(Ordering::Relaxed)
    }
    pub fn sync_duration_ns() -> u64 {
        SYNC_NOW_DURATION_NS.load(Ordering::Relaxed)
    }
    /// Atomic demand count, including cancelled and already-covered demands.
    pub fn instance_sync_calls(&self) -> u64 {
        self.sync.metric_calls.load(Ordering::Relaxed)
    }
    pub fn instance_sync_rounds(&self) -> u64 {
        self.sync.rounds.load(Ordering::Relaxed)
    }
    pub fn instance_sync_coalesced(&self) -> u64 {
        self.sync.metric_coalesced.load(Ordering::Relaxed)
    }
    pub fn instance_sync_errors(&self) -> u64 {
        self.sync.metric_errors.load(Ordering::Relaxed)
    }
    pub fn instance_sync_duration_ns(&self) -> u64 {
        self.sync.metric_duration_ns.load(Ordering::Relaxed)
    }

    /// cancel-safe: yes â€” dropping the waiter cannot cancel its cycle.
    pub async fn sync_now(&self) -> DbResult<u64> {
        self.sync.demand().await
    }

    /// The group-commit window's own fsync. A Mem sink has nothing to
    /// coalesce and no detached worker: finish in the leader's poll
    /// (current-thread commit atomicity; R follows W via `memory_completed`).
    /// File sinks share the coalesced worker with timer and pull demands.
    pub(super) async fn sync_window(&self) -> Result<(), Arc<DbError>> {
        if self.is_mem_sink() {
            let res = self.sink.sync().await;
            self.sync.rounds.fetch_add(1, Ordering::Relaxed);
            if res.is_ok() {
                self.sync.dirty_since_sync.store(false, Ordering::Release);
            }
            return res.map_err(Arc::new);
        }
        self.sync.demand_inner().await.map(|_| ())
    }
}
