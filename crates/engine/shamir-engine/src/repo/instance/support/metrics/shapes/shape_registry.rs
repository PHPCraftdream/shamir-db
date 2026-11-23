use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::Arc;

use arc_swap::ArcSwap;
use scc::HashMap;
use shamir_collections::THasher;

use super::shape_row::ShapeRow;
use super::shape_stats_snapshot::{ShapeCounters, ShapeStatsSnapshot};

/// Hard cap on distinct tracked shapes per repository.
pub const SHAPE_ROWS_CAP: usize = 256;
/// Trim target: enough headroom to amortise evictions under load.
pub const SHAPE_ROWS_LOW_WATER: usize = 224;

/// Bounded attempts a snapshot makes to take [`ShapeState::exclusive`] before
/// falling back to an unguarded (best-effort) copy. A trim holds the guard
/// for at most [`SHAPE_ROWS_CAP`] removals, so this spin always ends in practice.
const SNAPSHOT_GUARD_ATTEMPTS: u32 = 64;

/// One replaceable generation of shape accounting.
///
/// Everything that must agree with everything else — the row map, the
/// `other` fold bucket, the `tracked` reservation mirror and the `exclusive`
/// cold-path guard — lives here, so a recorder that pins one generation
/// and a snapshot that loads one generation can never mix generations.
///
/// The `tracked` mirror is reserved before an insertion, so `tracked` never
/// exceeds [`SHAPE_ROWS_CAP`] and the map can never hold more rows than the
/// mirror. Every execution is counted exactly once per state: in a live row,
/// or folded into `other` when no slot is available or a row is evicted.
///
/// That count-once invariant — `sum(rows.executions) + other.executions ==
/// executions recorded since the last reset` — is a *quiescent* property: it
/// holds only while no recorder is concurrently writing. A recorder that adds
/// to a row after the copy, or inserts a new row after the copy, is missing
/// from both places. What the guard below guarantees under concurrency is
/// narrower but true: no counter is counted twice across a trim.
///
/// A snapshot copies the rows under the same cold-path guard the trimmer
/// takes, so a trim cannot fold evicted counters into `(other)` between the
/// row copy and the `(other)` read.
struct ShapeState {
    rows: HashMap<[u8; 16], ShapeRow, THasher>,
    other: ShapeRow,
    tracked: AtomicUsize,
    /// Cold-path *exclusion* between the trimmer and the snapshot. A recorder
    /// never *waits* on it: the recorder that finds the registry full makes a
    /// single `compare_exchange` attempt and, if it loses, returns
    /// immediately. The one caller that wins that CAS performs the eviction
    /// synchronously (see [`trim`](ShapeState::trim)), and a snapshot taken
    /// while it runs exhausts [`SNAPSHOT_GUARD_ATTEMPTS`] attempts and falls
    /// back to the documented best-effort copy.
    exclusive: AtomicBool,
}

impl ShapeState {
    fn new() -> Self {
        Self {
            rows: HashMap::with_hasher(THasher::default()),
            other: ShapeRow::new(),
            tracked: AtomicUsize::new(0),
            exclusive: AtomicBool::new(false),
        }
    }

    /// Record one data-op execution under its shape key.
    ///
    /// `between` runs after the slot reservation (or cap decision) and
    /// before the map insert. Production passes a no-op; the tests pass a
    /// hand-shake so a recorder can be parked in exactly that window
    /// without sleeping.
    ///
    /// A recorder touches the `exclusive` guard only on the `current >=
    /// SHAPE_ROWS_CAP` arm: it folds the execution into `other` and calls
    /// [`trim`](Self::trim), which makes one CAS attempt and returns
    /// immediately on a loss — no recorder waits. The one caller that wins
    /// that CAS pays the whole eviction synchronously: one traversal of at
    /// most [`SHAPE_ROWS_CAP`] rows, one sort, and up to `SHAPE_ROWS_CAP -
    /// SHAPE_ROWS_LOW_WATER` removals with a merge and a mirror decrement
    /// each. That cost is amortised and only paid by a *new* key arriving
    /// while the registry is full: a stable set of shapes never trims, and
    /// after a trim roughly `SHAPE_ROWS_CAP - SHAPE_ROWS_LOW_WATER` new keys
    /// insert without one.
    fn record(
        &self,
        key: [u8; 16],
        elapsed_ns: u64,
        failed: bool,
        tick: u64,
        between: impl FnOnce(),
    ) {
        if self
            .rows
            .read_sync(&key, |_, row| row.observe(elapsed_ns, failed, tick))
            .is_some()
        {
            return;
        }
        loop {
            let current = self.tracked.load(Ordering::Acquire);
            if current >= SHAPE_ROWS_CAP {
                self.other.observe(elapsed_ns, failed, tick);
                self.trim();
                return;
            }
            if self
                .tracked
                .compare_exchange_weak(current, current + 1, Ordering::AcqRel, Ordering::Acquire)
                .is_ok()
            {
                break;
            }
        }
        between();
        let row = ShapeRow::new();
        row.observe(elapsed_ns, failed, tick);
        if let Err((key, _)) = self.rows.insert_sync(key, row) {
            // A concurrent insert won the key. Its row already took this
            // execution's reservation, so fold the execution there; if the
            // winner was evicted meanwhile, `other` keeps the count.
            if self
                .rows
                .read_sync(&key, |_, winner| winner.add(elapsed_ns, failed, tick))
                .is_none()
            {
                self.other.observe(elapsed_ns, failed, tick);
            }
            self.release_reservation();
        }
    }

    /// Number of slots reserved (equivalently: rows held) in this state.
    fn tracked(&self) -> usize {
        self.tracked.load(Ordering::Acquire)
    }

    /// Copy of every row, the `other` bucket and the mirror of this state.
    ///
    /// The copy is taken under the same cold-path guard as [`trim`](Self::trim),
    /// so a trim cannot fold evicted counters into `(other)` between the row
    /// copy and the `(other)` read — that is all the guard buys: no counter is
    /// counted twice across a trim. The accounting equality
    /// `sum(rows.executions) + other.executions == executions recorded since
    /// the last reset` is a *quiescent* property on top of that and holds
    /// only while no recorder is concurrently writing (a recorder that adds to
    /// a row after the copy, or inserts a new row after the copy, is missing
    /// from both places). A snapshot that cannot take the guard within
    /// [`SNAPSHOT_GUARD_ATTEMPTS`] attempts returns the documented best-effort
    /// copy.
    fn snapshot(&self) -> ShapeStatsSnapshot {
        self.snapshot_with(|| {})
    }

    /// [`snapshot`](Self::snapshot) with a hand-shake hole: `between` runs
    /// after the row copy and before the `(other)`/mirror reads, so a test can
    /// park a snapshot inside that window without sleeping.
    ///
    /// The guard, when taken, is held across the copy, `between` and the
    /// `(other)`/mirror reads, and released by [`ExclusiveGuard`] on the way
    /// out — a panic in `between` still releases it. As in
    /// [`snapshot`](Self::snapshot), the rows-plus-`(other)` equality holds
    /// only while no recorder writes concurrently; the guard guarantees the
    /// narrower property that no counter is counted twice across a trim.
    fn snapshot_with(&self, between: impl FnOnce()) -> ShapeStatsSnapshot {
        let guard = ExclusiveGuard::acquire_bounded(&self.exclusive);
        let guarded = guard.is_some();
        let mut rows: Vec<([u8; 16], ShapeCounters)> = Vec::new();
        self.rows.iter_sync(|key, row| {
            rows.push((*key, row.counters()));
            true
        });
        between();
        let snapshot = ShapeStatsSnapshot {
            rows,
            other: self.other.counters(),
            tracked: self.tracked.load(Ordering::Relaxed) as u64,
        };
        if guarded {
            drop(guard);
        }
        snapshot
    }

    fn release_reservation(&self) {
        let _ = self
            .tracked
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |current| {
                current.checked_sub(1)
            });
    }

    /// Evict the lowest recency ticks down to [`SHAPE_ROWS_LOW_WATER`].
    ///
    /// Called by [`record`](Self::record), and only on its `current >=
    /// SHAPE_ROWS_CAP` arm — a *new* key arriving while the registry is full.
    /// Single-flight per state: one `compare_exchange` attempt, and a losing
    /// caller returns immediately without waiting.
    ///
    /// The winner runs the eviction synchronously: one traversal of at most
    /// [`SHAPE_ROWS_CAP`] rows, one sort, and up to `SHAPE_ROWS_CAP -
    /// SHAPE_ROWS_LOW_WATER` removals with a merge into `other` and a mirror
    /// decrement each. The cost is amortised: a stable set of shapes never
    /// trims, and after a trim roughly `SHAPE_ROWS_CAP - SHAPE_ROWS_LOW_WATER`
    /// new keys insert without one.
    fn trim(&self) {
        let Some(_guard) = ExclusiveGuard::try_acquire(&self.exclusive) else {
            return;
        };
        let mut ticks: Vec<(u64, [u8; 16])> = Vec::new();
        self.rows.iter_sync(|key, row| {
            ticks.push((row.tick(), *key));
            true
        });
        if ticks.len() > SHAPE_ROWS_LOW_WATER {
            ticks.sort_unstable_by_key(|(tick, _)| *tick);
            let excess = ticks.len() - SHAPE_ROWS_LOW_WATER;
            for (_, key) in ticks.into_iter().take(excess) {
                if let Some((_, row)) = self.rows.remove_sync(&key) {
                    self.other.merge(&row);
                    self.release_reservation();
                }
            }
        }
    }
}

/// Releases [`ShapeState::exclusive`] on drop, so a panic while the guard is
/// held cannot disable trimming for the life of the state.
struct ExclusiveGuard<'a> {
    flag: &'a AtomicBool,
}

impl<'a> ExclusiveGuard<'a> {
    /// One CAS attempt: the trimmer must never wait.
    fn try_acquire(flag: &'a AtomicBool) -> Option<Self> {
        if flag
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
        {
            Some(Self { flag })
        } else {
            None
        }
    }

    /// Up to [`SNAPSHOT_GUARD_ATTEMPTS`] CAS attempts with `spin_loop`, then
    /// `None`.
    ///
    /// The retry is bounded on purpose: a snapshot is a cold-path read that
    /// must never stall, so one that cannot take the guard within the bound
    /// copies anyway and its result is best-effort. A trim holding the guard
    /// that long means pathological churn; counters stay monotone regardless,
    /// only the rows-vs-`(other)` split may then straddle the trim.
    fn acquire_bounded(flag: &'a AtomicBool) -> Option<Self> {
        for _ in 0..SNAPSHOT_GUARD_ATTEMPTS {
            if let Some(guard) = Self::try_acquire(flag) {
                return Some(guard);
            }
            std::hint::spin_loop();
        }
        None
    }
}

impl Drop for ExclusiveGuard<'_> {
    fn drop(&mut self) {
        self.flag.store(false, Ordering::Release);
    }
}

/// Per-repository bounded registry of canonical shape counters.
///
/// All accounting for one generation lives in a single `ShapeState`
/// published through an [`ArcSwap`]. A recorder pins the state it started on
/// with one `load_full`, so its whole reserve-insert sequence — and only
/// that — is charged to one state; [`snapshot`](Self::snapshot) likewise
/// pins one state and copies rows, `other` and `tracked` from it only. A
/// concurrent reset can never yield old rows next to a new mirror, and the
/// copy is taken under the same cold-path guard as the trimmer, so the rows
/// and `(other)` belong to one logical state instead of straddling a trim.
///
/// [`reset`](Self::reset) *replaces* the state instead of clearing it in
/// place. A reset discards the old state, and observations pinned to it are
/// discarded **with** it — the "may be lost" rule of brief 108 item 6. That
/// is what bounds the new state: a recorder parked between its reservation
/// and its insert writes into the discarded state, so it can neither push
/// the live state past [`SHAPE_ROWS_CAP`] nor leave anything behind.
pub struct ShapeRegistry {
    state: ArcSwap<ShapeState>,
    tick: AtomicU64,
}

impl Default for ShapeRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl ShapeRegistry {
    pub fn new() -> Self {
        Self {
            state: ArcSwap::from_pointee(ShapeState::new()),
            tick: AtomicU64::new(0),
        }
    }

    /// Record one data-op execution under its shape key.
    pub fn record(&self, key: [u8; 16], elapsed_ns: u64, failed: bool) {
        self.record_with(key, elapsed_ns, failed, || {});
    }

    /// Test hand-shake seam: identical to [`record`](Self::record) but calls
    /// `between` after the slot reservation and before the map insert, so a
    /// test can park a recorder inside the reserve-insert window.
    #[cfg(test)]
    pub(crate) fn record_paused(
        &self,
        key: [u8; 16],
        elapsed_ns: u64,
        failed: bool,
        between: impl FnOnce(),
    ) {
        self.record_with(key, elapsed_ns, failed, between);
    }

    /// Test hand-shake seam: identical to [`snapshot`](Self::snapshot) but
    /// calls `between` after the row copy and before the `(other)`/mirror
    /// reads, so a test can park a snapshot inside that window.
    #[cfg(test)]
    pub(crate) fn snapshot_paused(&self, between: impl FnOnce()) -> ShapeStatsSnapshot {
        let state = self.state.load_full();
        state.snapshot_with(between)
    }

    fn record_with(&self, key: [u8; 16], elapsed_ns: u64, failed: bool, between: impl FnOnce()) {
        // The pin: every step below is charged to this one state, even if a
        // `reset` publishes a fresh one in the meantime.
        let state = self.state.load_full();
        let tick = self.tick.fetch_add(1, Ordering::Relaxed);
        state.record(key, elapsed_ns, failed, tick, between);
    }

    /// Number of shape rows currently tracked.
    pub fn tracked(&self) -> usize {
        self.state.load().tracked()
    }

    /// Discard every shape and the `other` bucket by publishing a fresh,
    /// empty state. Recorders pinned to the discarded state finish against
    /// it — their observations are dropped together with it. The swap is a
    /// single `ArcSwap` store, so a concurrent snapshot pins exactly one
    /// generation and can never read parts of two.
    pub fn reset(&self) {
        self.state.store(Arc::new(ShapeState::new()));
    }

    /// Cold-path copy of every tracked shape and the `other` bucket, taken
    /// from a single state and under the same cold-path guard as the
    /// trimmer, so the parts belong to one logical generation: a concurrent
    /// reset yields exactly one generation, never a mix, and a concurrent
    /// trim cannot move counters into `(other)` between the row copy and the
    /// `(other)` read (which would count them twice).
    ///
    /// The accounting equality `sum(rows.executions) + other.executions ==
    /// executions recorded since the last reset` is a *quiescent* property: it
    /// holds only while no recorder is concurrently writing, because a
    /// recorder that adds to a row after the copy, or inserts a new row after
    /// the copy, is missing from both places. What the guard guarantees under
    /// concurrency is narrower: no counter is counted twice across a trim. A
    /// snapshot that cannot take the guard in its bounded number of attempts
    /// returns the documented best-effort copy.
    pub fn snapshot(&self) -> ShapeStatsSnapshot {
        let state = self.state.load_full();
        state.snapshot()
    }
}
