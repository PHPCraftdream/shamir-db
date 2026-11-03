//! Brute-force defence: lockout + exponential backoff per
//! `(client_subnet, username_hash)` (spec §5.2.5 NORMATIVE).
//!
//! Two layers of protection compounded:
//!
//! 1. **Backoff** (per request): on each failed `client_proof` verify, the
//!    server records a [`FailureState`] for the `(subnet, username_hash)`
//!    pair. The next request from that pair must be delayed by
//!    `100ms × 2^N` (cap 30s) before the response is released. Reset after
//!    5 minutes of inactivity.
//!
//! 2. **Lockout** (per hour): if the failure count exceeds
//!    `LOCKOUT_THRESHOLD = 50` within `LOCKOUT_WINDOW = 1 hour`, the pair
//!    is moved into a [`LockoutState`] entry that silently rejects all
//!    further requests until expiry. From the wire it's indistinguishable
//!    from a normal `authentication_failed` (spec §8.4 — silent lockout).
//!
//! **Reset on success** (spec §5.2.5 NORMATIVE): a successful SCRAM verify
//! immediately removes the `FailureState` for the pair AND removes any
//! pre-threshold `LockoutState` so legitimate users don't carry forward
//! backoff after a few typos.
//!
//! `username_hash = HMAC-SHA256(lockout_secret, username_nfc)[..16]` per
//! spec §5.2.5; `lockout_secret` is separate from `server_secret` and is
//! NOT rotated (spec IMPL §1.3) so lockout state survives anti-enum
//! secret rotations.
//!
//! ## Pluggability
//!
//! [`LockoutStore`] is a trait so production deployments can back the
//! state with durable storage (per spec IMPL §1.3: persisted batched ≤5s
//! to survive server restarts). The default [`InMemoryLockoutStore`] is
//! suitable for tests and single-process deployments where some lockout
//! drift across restarts is acceptable.
//!
//! ## Snapshot persistence
//!
//! [`InMemoryLockoutStore::with_snapshot_sink`] installs a
//! [`LockoutSnapshotSink`] that is consulted on construction to rehydrate
//! prior state and is later driven by a periodic task (typically every
//! 60 seconds — see `shamir-server::server`) that calls
//! [`InMemoryLockoutStore::persist_snapshot`]. The serialised form is a
//! [`LockoutSnapshot`] value with stable serde shape; the sink backend
//! (redb, file, ...) is the embedder's choice.
//!
//! Trade-off: a hard crash between snapshots loses ≤60 s of new failures,
//! which is acceptable for failed-auth bookkeeping (an attacker who knows
//! the restart cadence still meets the spec §8.6 warmup window on the
//! rate-limiter, and any user who had ALREADY been driven to lockout
//! survives the snapshot). The alternative — fsync on every failed auth —
//! costs one disk write per invalid password attempt, which is too much.

use crate::common::crypto::hmac_sha256;
use crate::common::time::{ns, UnixNanos};
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use std::net::IpAddr;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::Arc;

#[path = "lockout_storage/pair_slot.rs"]
mod pair_slot;
use pair_slot::PairSlot;
#[path = "lockout_storage/pair_index.rs"]
mod pair_index;
use pair_index::{
    compact as compact_pairs, insert as insert_pair, lookup as lookup_pair, remove as remove_pair,
};

/// Per-spec §5.2.5 backoff base: 100 ms × 2^N (capped).
pub const BACKOFF_BASE_MS: u64 = 100;
/// Per-spec §5.2.5 backoff cap: 30 seconds.
pub const BACKOFF_CAP_MS: u64 = 30_000;
/// Per-spec §5.2.5 backoff reset: 5 minutes of inactivity.
pub const BACKOFF_RESET_NS: u64 = 5 * ns::MINUTE;

/// Per-spec §8 table: lockout threshold = 50 fails / hour per pair.
pub const LOCKOUT_THRESHOLD: u32 = 50;
/// Per-spec §8: lockout window for counting failures.
pub const LOCKOUT_WINDOW_NS: u64 = ns::HOUR;
/// Per-spec §8: lockout duration once threshold reached.
pub const LOCKOUT_DURATION_NS: u64 = ns::HOUR;

/// Compose the per-pair key: `username_hash = HMAC(lockout_secret, username_nfc)[..16]`,
/// concatenated with subnet.
pub fn username_hash(lockout_secret: &[u8; 32], username_nfc: &[u8]) -> [u8; 16] {
    let full = hmac_sha256(lockout_secret, username_nfc);
    let mut out = [0u8; 16];
    out.copy_from_slice(&full[..16]);
    out
}

/// Reduce a client IP to its rate-limit subnet:
/// - IPv4: `/24`
/// - IPv6: `/64`
pub fn subnet_of(ip: IpAddr) -> Subnet {
    match ip {
        IpAddr::V4(v4) => {
            let oct = v4.octets();
            Subnet::V4([oct[0], oct[1], oct[2]])
        }
        IpAddr::V6(v6) => {
            if let Some(v4) = v6.to_ipv4_mapped() {
                let oct = v4.octets();
                return Subnet::V4([oct[0], oct[1], oct[2]]);
            }
            let oct = v6.octets();
            let mut prefix = [0u8; 8];
            prefix.copy_from_slice(&oct[..8]);
            Subnet::V6(prefix)
        }
    }
}

/// Subnet identifier used as the per-pair lockout key.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Subnet {
    /// IPv4 /24 prefix.
    V4([u8; 3]),
    /// IPv6 /64 prefix.
    V6([u8; 8]),
}

/// Per-pair key: `(subnet, username_hash)`.
pub type PairKey = (Subnet, [u8; 16]);

/// One entry of failure state per `(subnet, username_hash)`.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct FailureState {
    /// Number of failed attempts in the current burst.
    pub count: u32,
    /// Last failure timestamp (unix nanos).
    pub last_fail_at_ns: u64,
}

impl FailureState {
    /// Compute backoff duration this entry currently dictates: `100ms × 2^count`,
    /// capped at 30s.
    pub fn backoff_ms(&self) -> u64 {
        let exp = self.count.saturating_sub(1);
        // count=1 → 100*2^0 = 100, count=2 → 200, ..., count=8 → 12800, count=9 → 25600,
        // count=10 → 30000 (capped). Use saturating shift to avoid overflow.
        let raw = BACKOFF_BASE_MS.saturating_mul(1u64 << exp.min(20));
        raw.min(BACKOFF_CAP_MS)
    }

    /// Whether this entry is stale (no failure within `BACKOFF_RESET_NS`).
    pub fn is_stale(&self, now_ns: u64) -> bool {
        now_ns.saturating_sub(self.last_fail_at_ns) >= BACKOFF_RESET_NS
    }
}

/// Per-pair lockout state (entered when threshold reached).
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct LockoutState {
    /// When the lockout was triggered (unix nanos).
    pub triggered_at_ns: u64,
    /// Lockout duration in nanos (typically `LOCKOUT_DURATION_NS`).
    pub duration_ns: u64,
}

impl LockoutState {
    /// Whether the lockout is still active at `now_ns`.
    pub fn is_active(&self, now_ns: u64) -> bool {
        now_ns < self.triggered_at_ns.saturating_add(self.duration_ns)
    }
}

/// Decision returned by [`LockoutStore::register_failure`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FailureOutcome {
    /// Pair is now locked out (silent reject all further requests until expiry).
    LockedOut,
    /// Pair has a backoff requirement (caller should sleep at least this long
    /// before responding).
    Backoff {
        /// Minimum delay in milliseconds before the next response is sent.
        delay_ms: u64,
    },
}

/// Read-only snapshot of the current pair failure state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PairStatus {
    /// Active lock expiry; expired lockouts are reported as absent.
    pub locked_until_ns: Option<u64>,
    /// Active backoff expiry, if a failure is recent enough.
    pub backoff_until_ns: Option<u64>,
    /// Timestamp of the latest failure, including stale failures.
    pub last_fail_at_ns: Option<u64>,
}

/// Pluggable backend for failure / lockout state. Production should back
/// this with durable storage (spec IMPL §1.3 batched ≤5s).
pub trait LockoutStore: Send + Sync {
    /// Register a failed authentication attempt for `(subnet, username_hash)`.
    /// Returns the resulting backoff (or `LockedOut` if threshold reached).
    fn register_failure(&self, key: PairKey, now_ns: u64) -> FailureOutcome;

    /// Reset on success (spec §5.2.5 NORMATIVE): clear FailureState AND
    /// any pre-threshold LockoutState for this pair.
    fn reset_on_success(&self, key: PairKey);

    /// Read current failure state without altering it. Stale failures preserve
    /// the timestamp for begin-time ordering, but have no active backoff or lock.
    fn pair_status(&self, key: PairKey, now_ns: u64) -> PairStatus;

    /// Check whether the pair is currently locked out.
    fn is_locked_out(&self, key: PairKey, now_ns: u64) -> bool;

    /// Check the current backoff requirement (returns `0` if no backoff).
    fn current_backoff_ms(&self, key: PairKey, now_ns: u64) -> u64;

    /// Admin: explicitly clear lockout for a user (across ALL subnets).
    /// Spec §12.3 `unlockUser` — must clear both `lockout_state` AND
    /// `auth_failures` so the user doesn't re-enter high backoff
    /// immediately.
    fn admin_unlock_user(&self, username_hash: [u8; 16]);

    /// Background GC: drop entries older than `BACKOFF_RESET_NS`.
    fn gc(&self, now_ns: u64);
}

/// Serialisable point-in-time copy of all lockout state, used by
/// [`LockoutSnapshotSink`] for durable persistence across restarts.
///
/// The wire format is `serde`-tagged so future revisions can add a
/// version field without breaking deserialisation. Entries that are
/// stale at load time are dropped silently by
/// [`InMemoryLockoutStore::with_snapshot`].
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct LockoutSnapshot {
    /// Per-pair failure state (`(subnet, username_hash) -> count + last_ts`).
    pub failures: Vec<(PairKey, FailureState)>,
    /// Per-pair active lockouts (triggered_at + duration).
    pub lockouts: Vec<(PairKey, LockoutState)>,
    /// Cumulative lockout-events counter (informational metric only).
    pub total_lockouts: u64,
    /// Wall-clock at which the snapshot was taken (unix nanos). Allows
    /// the loader to discard everything older than `LOCKOUT_DURATION_NS`
    /// without consulting a clock skew tolerance.
    pub captured_at_ns: u64,
}

/// Backend that materialises [`LockoutSnapshot`]s. The embedder picks the
/// concrete adapter (redb, file, ...); shamir-connect itself stays free of
/// storage dependencies.
///
/// Implementations MUST be idempotent: calling [`Self::save`] with the
/// same snapshot value twice is allowed. [`Self::load`] returns `Ok(None)`
/// when there is no prior snapshot (e.g. a fresh data dir).
pub trait LockoutSnapshotSink: Send + Sync {
    /// Persist `snapshot` durably. Errors are returned to the caller so
    /// the periodic task can log them; the in-memory state is never
    /// dropped on failure.
    fn save(&self, snapshot: &LockoutSnapshot) -> Result<(), LockoutSnapshotError>;

    /// Load the most-recent snapshot if one exists. `Ok(None)` for a
    /// brand-new store.
    fn load(&self) -> Result<Option<LockoutSnapshot>, LockoutSnapshotError>;
}

/// Error type returned by [`LockoutSnapshotSink`] implementations.
#[derive(Debug, thiserror::Error)]
pub enum LockoutSnapshotError {
    /// Underlying storage refused the read/write (disk full, permission
    /// denied, fsync failure, etc.).
    #[error("storage: {0}")]
    Storage(String),
    /// Encoding / decoding failure — usually a malformed prior snapshot
    /// after a format change. Treated as "no snapshot" by the loader.
    #[error("encoding: {0}")]
    Encoding(String),
}

/// Fixed memory and source quota limits for the shared pair table.
pub const LOCKOUT_PAIR_CAPACITY: usize = 8_192;
/// Maximum pairs retained per base prefix (IPv4 /24, IPv6 /64).
pub const LOCKOUT_BASE_QUOTA: usize = 64;
/// Maximum pairs retained per parent prefix (IPv4 /16, IPv6 /48).
pub const LOCKOUT_PARENT_QUOTA: usize = 512;
/// Reason a new pair could not be retained in the bounded table.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PairDropReason {
    /// The base-prefix or parent-prefix quota was exhausted.
    PairQuota,
    /// The global pair table had no evictable slot.
    TableFull,
}

/// Receives absolute pair-store occupancy and rejected-admission reasons.
/// Callbacks run after the slots mutex is released and must be fast/nonblocking.
pub type PairStoreObserver = dyn Fn(usize, Option<PairDropReason>) + Send + Sync;

/// In-memory lockout store backed by a fixed, bounded pair table.
pub struct InMemoryLockoutStore {
    // This lock is acquired on every handshake, including the pre-challenge
    // lock check. Its critical section is bounded and allocation-free; quota
    // scans occur only on new-pair admission, and the single mutex is needed
    // to coordinate cross-slot quota decisions.
    slots: Mutex<Box<[PairSlot]>>,
    occupied_pairs: AtomicUsize,
    /// Optional metric: total locked-out events ever observed.
    total_lockouts: AtomicU64,
    /// Optional durable backend. `None` for in-memory-only deployments.
    snapshot_sink: Option<Arc<dyn LockoutSnapshotSink>>,
    observer: Option<Arc<PairStoreObserver>>,
}

impl Drop for InMemoryLockoutStore {
    fn drop(&mut self) {
        if let Some(observer) = &self.observer {
            observer(0, None);
        }
    }
}

impl core::fmt::Debug for InMemoryLockoutStore {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("InMemoryLockoutStore")
            .field("pairs", &self.failure_pair_count())
            .field(
                "total_lockouts",
                &self.total_lockouts.load(Ordering::Relaxed),
            )
            .field(
                "snapshot_sink",
                &self.snapshot_sink.as_ref().map(|_| "<sink>"),
            )
            .finish()
    }
}

impl Default for InMemoryLockoutStore {
    fn default() -> Self {
        let slots = vec![PairSlot::default(); LOCKOUT_PAIR_CAPACITY].into_boxed_slice();
        Self {
            slots: Mutex::new(slots),
            occupied_pairs: AtomicUsize::new(0),
            total_lockouts: AtomicU64::new(0),
            snapshot_sink: None,
            observer: None,
        }
    }
}

impl InMemoryLockoutStore {
    /// Create an empty in-memory lockout store.
    pub fn new() -> Self {
        Self::default()
    }

    /// Restore lockout state from a previously captured snapshot.
    pub fn with_snapshot(snapshot: LockoutSnapshot) -> Self {
        let mut store = Self::default();
        store.rehydrate(snapshot);
        store
    }

    /// Create a store backed by a snapshot sink, loading prior state if available.
    pub fn with_snapshot_sink(sink: Arc<dyn LockoutSnapshotSink>) -> Self {
        Self::with_snapshot_sink_and_observer(sink, None)
    }

    /// Create a snapshot-backed store with an optional pair metrics observer.
    pub fn with_snapshot_sink_and_observer(
        sink: Arc<dyn LockoutSnapshotSink>,
        observer: Option<Arc<PairStoreObserver>>,
    ) -> Self {
        let mut store = Self::default();
        store.snapshot_sink = Some(sink.clone());
        store.observer = observer;
        match sink.load() {
            Ok(Some(snapshot)) => store.rehydrate(snapshot),
            Ok(None) => {}
            Err(error) => log::warn!("lockout snapshot load failed; starting empty: {error}"),
        }
        store
    }

    /// Install an observer on an existing store. The observer receives the
    /// current absolute occupancy immediately and after every occupancy change.
    pub fn with_observer(mut self, observer: Arc<PairStoreObserver>) -> Self {
        if let Some(previous) = &self.observer {
            previous(0, None);
        }
        observer(self.occupied_pair_count(), None);
        self.observer = Some(observer);
        self
    }

    /// Count occupied pair slots, including lockout-only entries.
    pub fn occupied_pair_count(&self) -> usize {
        self.occupied_pairs.load(Ordering::Relaxed)
    }

    /// Notify the observer with this store's absolute occupancy.
    fn report(&self, dropped: Option<PairDropReason>) {
        if let Some(observer) = &self.observer {
            observer(self.occupied_pair_count(), dropped);
        }
    }

    /// Count pairs with recorded failures.
    pub fn failure_pair_count(&self) -> usize {
        self.slots
            .lock()
            .iter()
            .filter(|slot| slot.occupied && slot.failure.count != 0)
            .count()
    }

    /// Number of pairs with a recorded lockout (active or not yet collected).
    pub fn active_lockout_count(&self) -> usize {
        self.slots
            .lock()
            .iter()
            .filter(|slot| slot.occupied && slot.lockout.is_some())
            .count()
    }

    /// Total lockout events observed since start.
    pub fn total_lockouts(&self) -> u64 {
        self.total_lockouts.load(Ordering::Relaxed)
    }

    /// Capture the current lockout state for persistence.
    pub fn snapshot(&self) -> LockoutSnapshot {
        let captured_at_ns = UnixNanos::now().as_u64();
        let mut failures = Vec::with_capacity(LOCKOUT_PAIR_CAPACITY);
        let mut lockouts = Vec::with_capacity(LOCKOUT_PAIR_CAPACITY);
        {
            let slots = self.slots.lock();
            for slot in slots.iter().filter(|slot| slot.occupied) {
                if slot.failure.count != 0 {
                    failures.push((slot.key, slot.failure));
                }
                if let Some(lock) = slot.lockout {
                    lockouts.push((slot.key, lock));
                }
            }
        }
        LockoutSnapshot {
            failures,
            lockouts,
            total_lockouts: self.total_lockouts(),
            captured_at_ns,
        }
    }

    /// Persist a snapshot when a sink is configured; returns false without one.
    pub fn persist_snapshot(&self) -> Result<bool, LockoutSnapshotError> {
        let Some(sink) = self.snapshot_sink.as_ref() else {
            return Ok(false);
        };
        sink.save(&self.snapshot())?;
        Ok(true)
    }

    fn rehydrate(&mut self, snapshot: LockoutSnapshot) {
        let now = snapshot.captured_at_ns;
        let mut failures = snapshot.failures;
        failures.sort_unstable_by(|a, b| {
            b.1.count
                .cmp(&a.1.count)
                .then_with(|| b.1.last_fail_at_ns.cmp(&a.1.last_fail_at_ns))
        });
        let mut locks = snapshot.lockouts;
        locks.sort_unstable_by_key(|(_, lock)| std::cmp::Reverse(lock.triggered_at_ns));

        let slots = self.slots.get_mut();
        for (key, lock) in locks
            .iter()
            .filter(|(_, lock)| lock.is_active(now))
            .copied()
        {
            let failure = FailureState {
                count: 0,
                last_fail_at_ns: 0,
            };
            let _ = find_or_admit(slots, key, failure, Some(lock), now, false);
        }
        for (key, failure) in failures
            .iter()
            .filter(|(_, failure)| !failure.is_stale(now))
            .copied()
        {
            let index = lookup_pair(slots, key);
            if let Some(index) = index {
                if (failure.count, failure.last_fail_at_ns)
                    > (
                        slots[index].failure.count,
                        slots[index].failure.last_fail_at_ns,
                    )
                {
                    slots[index].failure = failure;
                }
            } else {
                let _ = find_or_admit(slots, key, failure, None, now, false);
            }
        }
        self.total_lockouts
            .store(snapshot.total_lockouts, Ordering::Relaxed);
        self.occupied_pairs
            .store(count_occupied(slots), Ordering::Relaxed);
        if let Some(observer) = &self.observer {
            observer(self.occupied_pair_count(), None);
        }
    }
}

fn source_base(subnet: Subnet) -> (u8, [u8; 6]) {
    match subnet {
        Subnet::V4(v) => (4, [v[0], v[1], 0, 0, 0, 0]),
        Subnet::V6(v) => (6, [v[0], v[1], v[2], v[3], v[4], v[5]]),
    }
}

fn count_occupied(slots: &[PairSlot]) -> usize {
    slots.iter().filter(|slot| slot.occupied).count()
}

/// Why `key` could not be admitted: an exhausted base/parent quota, else a full table.
fn pair_drop_reason(slots: &[PairSlot], key: PairKey, now_ns: u64) -> PairDropReason {
    let parent = source_base(key.0);
    let mut base_count = 0;
    let mut parent_count = 0;
    for slot in slots.iter().filter(|slot| !pair_is_free(slot, now_ns)) {
        base_count += (slot.key.0 == key.0) as usize;
        parent_count += (source_base(slot.key.0) == parent) as usize;
    }
    if base_count >= LOCKOUT_BASE_QUOTA || parent_count >= LOCKOUT_PARENT_QUOTA {
        PairDropReason::PairQuota
    } else {
        PairDropReason::TableFull
    }
}

fn pair_is_free(slot: &PairSlot, now: u64) -> bool {
    !slot.occupied
        || (slot.failure.is_stale(now) && slot.lockout.is_none_or(|lock| !lock.is_active(now)))
}

fn find_or_admit(
    slots: &mut [PairSlot],
    key: PairKey,
    failure: FailureState,
    lockout: Option<LockoutState>,
    now: u64,
    evict: bool,
) -> Option<(usize, bool)> {
    if let Some(index) = lookup_pair(slots, key) {
        if (failure.count, failure.last_fail_at_ns)
            > (
                slots[index].failure.count,
                slots[index].failure.last_fail_at_ns,
            )
        {
            slots[index].failure = failure;
        }
        if let Some(lock) = lockout {
            let replace = slots[index].lockout.is_none_or(|existing| {
                lock.triggered_at_ns.saturating_add(lock.duration_ns)
                    > existing
                        .triggered_at_ns
                        .saturating_add(existing.duration_ns)
            });
            if replace {
                slots[index].lockout = Some(lock);
            }
        }
        return Some((index, true));
    }
    let base = key.0;
    let parent = source_base(base);
    let mut base_count = 0;
    let mut parent_count = 0;
    let mut free = None;
    for (index, slot) in slots.iter().enumerate() {
        if pair_is_free(slot, now) {
            if free.is_none() {
                free = Some(index);
            }
            continue;
        }
        base_count += (slot.key.0 == base) as usize;
        parent_count += (source_base(slot.key.0) == parent) as usize;
    }
    let exhausted_base = base_count >= LOCKOUT_BASE_QUOTA;
    let exhausted_parent = parent_count >= LOCKOUT_PARENT_QUOTA;
    let mut candidate: Option<(usize, u32, u64)> = None;
    for (index, slot) in slots
        .iter()
        .enumerate()
        .filter(|(_, slot)| !pair_is_free(slot, now))
    {
        if (exhausted_base && slot.key.0 != base)
            || (exhausted_parent && source_base(slot.key.0) != parent)
        {
            continue;
        }
        let effective_count = if slot.failure.is_stale(now) {
            0
        } else {
            slot.failure.count
        };
        if slot.lockout.is_some_and(|lock| lock.is_active(now))
            || effective_count >= LOCKOUT_THRESHOLD
        {
            continue;
        }
        if candidate.is_none_or(|(_, count, oldest)| {
            effective_count < count
                || (effective_count == count && slot.failure.last_fail_at_ns < oldest)
        }) {
            candidate = Some((index, effective_count, slot.failure.last_fail_at_ns));
        }
    }
    let chosen = if exhausted_base || exhausted_parent {
        if evict {
            candidate.map(|(i, _, _)| i)
        } else {
            None
        }
    } else if let Some(index) = free {
        Some(index)
    } else if evict {
        candidate.map(|(i, _, _)| i)
    } else {
        None
    };
    let index = chosen?;
    let was_occupied = slots[index].occupied;
    if was_occupied {
        remove_pair(slots, index);
    }
    slots[index].replace_payload(key, failure, lockout);
    insert_pair(slots, index);
    Some((index, was_occupied))
}

impl LockoutStore for InMemoryLockoutStore {
    fn register_failure(&self, key: PairKey, now_ns: u64) -> FailureOutcome {
        let mut slots = self.slots.lock();
        let position = lookup_pair(&slots, key);
        let mut dropped = None;
        let (index, count) = if let Some(index) = position {
            let slot = &mut slots[index];
            slot.failure.count = if slot.failure.is_stale(now_ns) {
                1
            } else {
                slot.failure.count.saturating_add(1)
            };
            slot.failure.last_fail_at_ns = now_ns;
            (Some(index), slot.failure.count)
        } else {
            let failure = FailureState {
                count: 1,
                last_fail_at_ns: now_ns,
            };
            match find_or_admit(&mut slots, key, failure, None, now_ns, true) {
                Some((index, was_occupied)) => {
                    if !was_occupied {
                        self.occupied_pairs.fetch_add(1, Ordering::Relaxed);
                    }
                    (Some(index), 1)
                }
                None => {
                    dropped = Some(pair_drop_reason(&slots, key, now_ns));
                    (None, 1)
                }
            }
        };
        let Some(index) = index else {
            drop(slots);
            if let Some(observer) = &self.observer {
                observer(self.occupied_pair_count(), dropped);
            }
            return FailureOutcome::Backoff { delay_ms: 0 };
        };
        let outcome = if count >= LOCKOUT_THRESHOLD {
            slots[index].lockout = Some(LockoutState {
                triggered_at_ns: now_ns,
                duration_ns: LOCKOUT_DURATION_NS,
            });
            self.total_lockouts.fetch_add(1, Ordering::Relaxed);
            FailureOutcome::LockedOut
        } else {
            FailureOutcome::Backoff {
                delay_ms: FailureState {
                    count,
                    last_fail_at_ns: now_ns,
                }
                .backoff_ms(),
            }
        };
        let occupied = self.occupied_pair_count();
        drop(slots);
        if position.is_none() {
            if let Some(observer) = &self.observer {
                observer(occupied, None);
            }
        }
        outcome
    }

    fn reset_on_success(&self, key: PairKey) {
        let mut slots = self.slots.lock();
        let removed = if let Some(index) = lookup_pair(&slots, key) {
            remove_pair(&mut slots, index);
            slots[index].clear_payload();
            true
        } else {
            false
        };
        if removed {
            self.occupied_pairs.fetch_sub(1, Ordering::Relaxed);
        }
        drop(slots);
        if removed {
            self.report(None);
        }
    }

    fn pair_status(&self, key: PairKey, now_ns: u64) -> PairStatus {
        let slots = self.slots.lock();
        let Some(index) = lookup_pair(&slots, key) else {
            return PairStatus {
                locked_until_ns: None,
                backoff_until_ns: None,
                last_fail_at_ns: None,
            };
        };
        let slot = &slots[index];
        let locked_until_ns = slot
            .lockout
            .filter(|lock| lock.is_active(now_ns))
            .and_then(|lock| lock.triggered_at_ns.checked_add(lock.duration_ns));
        let failure = slot.failure;
        let last_fail_at_ns = (failure.last_fail_at_ns != 0).then_some(failure.last_fail_at_ns);
        let backoff_until_ns = if failure.count != 0 && !failure.is_stale(now_ns) {
            failure
                .backoff_ms()
                .checked_mul(ns::SECOND / 1_000)
                .and_then(|duration| failure.last_fail_at_ns.checked_add(duration))
                .filter(|until| now_ns < *until)
        } else {
            None
        };
        PairStatus {
            locked_until_ns,
            backoff_until_ns,
            last_fail_at_ns,
        }
    }

    fn is_locked_out(&self, key: PairKey, now_ns: u64) -> bool {
        let mut slots = self.slots.lock();
        if let Some(index) = lookup_pair(&slots, key) {
            let slot = &mut slots[index];
            if slot.lockout.is_some_and(|lock| lock.is_active(now_ns)) {
                return true;
            }
            if slot.lockout.is_some() {
                slot.lockout = None;
            }
        }
        false
    }

    fn current_backoff_ms(&self, key: PairKey, now_ns: u64) -> u64 {
        let slots = self.slots.lock();
        lookup_pair(&slots, key)
            .map(|index| &slots[index])
            .filter(|slot| slot.failure.count != 0 && !slot.failure.is_stale(now_ns))
            .map_or(0, |slot| slot.failure.backoff_ms())
    }

    fn admin_unlock_user(&self, username_hash: [u8; 16]) {
        let mut slots = self.slots.lock();
        let mut removed = 0;
        let mut index = 0;
        while index < slots.len() {
            if slots[index].occupied && slots[index].key.1 == username_hash {
                slots[index].clear_payload();
                removed += 1;
            }
            index += 1;
        }
        if removed != 0 {
            compact_pairs(&mut slots);
            self.occupied_pairs.fetch_sub(removed, Ordering::Relaxed);
        }
        drop(slots);
        if removed != 0 {
            self.report(None);
        }
    }

    fn gc(&self, now_ns: u64) {
        let mut slots = self.slots.lock();
        let mut removed = 0;
        let mut index = 0;
        while index < slots.len() {
            if slots[index].occupied {
                if slots[index].failure.is_stale(now_ns) {
                    slots[index].failure = FailureState {
                        count: 0,
                        last_fail_at_ns: 0,
                    };
                }
                if slots[index].failure.count == 0
                    && slots[index]
                        .lockout
                        .is_none_or(|lock| !lock.is_active(now_ns))
                {
                    slots[index].clear_payload();
                    removed += 1;
                } else if slots[index]
                    .lockout
                    .is_some_and(|lock| !lock.is_active(now_ns))
                {
                    slots[index].lockout = None;
                }
            }
            index += 1;
        }
        if removed != 0 {
            compact_pairs(&mut slots);
            self.occupied_pairs.fetch_sub(removed, Ordering::Relaxed);
        }
        drop(slots);
        if removed != 0 {
            self.report(None);
        }
    }
}

// Tests live in crate::server::tests::security::lockout_tests.
