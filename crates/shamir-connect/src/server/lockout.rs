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
use crate::common::time::ns;
use dashmap::DashMap;
use serde::{Deserialize, Serialize};
use std::net::IpAddr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

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

/// Pluggable backend for failure / lockout state. Production should back
/// this with durable storage (spec IMPL §1.3 batched ≤5s).
pub trait LockoutStore: Send + Sync {
    /// Register a failed authentication attempt for `(subnet, username_hash)`.
    /// Returns the resulting backoff (or `LockedOut` if threshold reached).
    fn register_failure(&self, key: PairKey, now_ns: u64) -> FailureOutcome;

    /// Reset on success (spec §5.2.5 NORMATIVE): clear FailureState AND
    /// any pre-threshold LockoutState for this pair.
    fn reset_on_success(&self, key: PairKey);

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

/// In-memory lockout store — `DashMap<PairKey, FailureState>` plus a
/// secondary map for active lockouts. Concurrent-safe.
///
/// Optionally backed by a [`LockoutSnapshotSink`] for durable
/// persistence across restarts; see
/// [`Self::with_snapshot_sink`] / [`Self::persist_snapshot`].
#[derive(Default)]
pub struct InMemoryLockoutStore {
    failures: DashMap<PairKey, FailureState>,
    lockouts: DashMap<PairKey, LockoutState>,
    /// Optional metric: total locked-out events ever observed.
    total_lockouts: AtomicU64,
    /// Optional durable backend. `None` for in-memory-only deployments
    /// (default and most tests).
    snapshot_sink: Option<Arc<dyn LockoutSnapshotSink>>,
}

impl core::fmt::Debug for InMemoryLockoutStore {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("InMemoryLockoutStore")
            .field("failures", &self.failures.len())
            .field("lockouts", &self.lockouts.len())
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

impl InMemoryLockoutStore {
    /// Create empty store with no durable backing.
    pub fn new() -> Self {
        Self::default()
    }

    /// Create a store from an explicit snapshot. Stale entries
    /// (failures past `BACKOFF_RESET_NS`, lockouts past expiry) are
    /// discarded against `now_ns = snapshot.captured_at_ns` so the
    /// loaded state matches what `gc` would have produced at the
    /// capture instant.
    ///
    /// `total_lockouts` is preserved verbatim — it's a cumulative
    /// metric, not a real-time decision input.
    pub fn with_snapshot(snapshot: LockoutSnapshot) -> Self {
        let store = Self::default();
        store.rehydrate(snapshot);
        store
    }

    /// Create a store backed by `sink`. On construction, the sink is
    /// consulted for a prior snapshot and the store is rehydrated from
    /// it (errors are logged at `warn` and the store starts empty).
    ///
    /// Subsequent calls to [`Self::persist_snapshot`] write through the
    /// same sink.
    pub fn with_snapshot_sink(sink: Arc<dyn LockoutSnapshotSink>) -> Self {
        let store = Self {
            snapshot_sink: Some(sink.clone()),
            ..Self::default()
        };
        match sink.load() {
            Ok(Some(snap)) => store.rehydrate(snap),
            Ok(None) => {}
            Err(e) => {
                log::warn!("lockout snapshot load failed; starting empty: {e}");
            }
        }
        store
    }

    /// Number of distinct `(subnet, user)` pairs currently in failure state.
    pub fn failure_pair_count(&self) -> usize {
        self.failures.len()
    }

    /// Number of currently-active lockouts.
    pub fn active_lockout_count(&self) -> usize {
        self.lockouts.len()
    }

    /// Total lockout events triggered (cumulative).
    pub fn total_lockouts(&self) -> u64 {
        self.total_lockouts.load(Ordering::Relaxed)
    }

    /// Capture a point-in-time copy of the entire store for the
    /// snapshot path. Holds map shards only long enough to clone
    /// `(key, value)` pairs; no locks are held across an .await
    /// boundary (this is a synchronous function).
    pub fn snapshot(&self) -> LockoutSnapshot {
        let captured_at_ns = crate::common::time::UnixNanos::now().as_u64();
        let mut failures = Vec::with_capacity(self.failures.len());
        for entry in self.failures.iter() {
            failures.push((*entry.key(), *entry.value()));
        }
        let mut lockouts = Vec::with_capacity(self.lockouts.len());
        for entry in self.lockouts.iter() {
            lockouts.push((*entry.key(), *entry.value()));
        }
        LockoutSnapshot {
            failures,
            lockouts,
            total_lockouts: self.total_lockouts.load(Ordering::Relaxed),
            captured_at_ns,
        }
    }

    /// Persist the current store via the installed [`LockoutSnapshotSink`].
    /// Returns `Ok(false)` when no sink is installed (in-memory-only
    /// mode); `Ok(true)` after a successful write. Errors are propagated
    /// so the caller can log and rate-limit.
    pub fn persist_snapshot(&self) -> Result<bool, LockoutSnapshotError> {
        let Some(sink) = self.snapshot_sink.as_ref() else {
            return Ok(false);
        };
        let snap = self.snapshot();
        sink.save(&snap)?;
        Ok(true)
    }

    /// Load entries from `snapshot`, discarding stale ones. Idempotent;
    /// existing in-memory state is REPLACED (this is only called from
    /// constructors).
    fn rehydrate(&self, snapshot: LockoutSnapshot) {
        let now_ns = snapshot.captured_at_ns;
        self.failures.clear();
        for (key, state) in snapshot.failures {
            // Drop entries that would already be stale at capture time.
            if !state.is_stale(now_ns) {
                self.failures.insert(key, state);
            }
        }
        self.lockouts.clear();
        for (key, state) in snapshot.lockouts {
            if state.is_active(now_ns) {
                self.lockouts.insert(key, state);
            }
        }
        self.total_lockouts
            .store(snapshot.total_lockouts, Ordering::Relaxed);
    }
}

impl LockoutStore for InMemoryLockoutStore {
    fn register_failure(&self, key: PairKey, now_ns: u64) -> FailureOutcome {
        // Update / insert failure state.
        let mut new_count: u32 = 0;
        self.failures
            .entry(key)
            .and_modify(|f| {
                if f.is_stale(now_ns) {
                    f.count = 1;
                } else {
                    f.count = f.count.saturating_add(1);
                }
                f.last_fail_at_ns = now_ns;
                new_count = f.count;
            })
            .or_insert_with(|| {
                new_count = 1;
                FailureState {
                    count: 1,
                    last_fail_at_ns: now_ns,
                }
            });

        // Check lockout threshold.
        if new_count >= LOCKOUT_THRESHOLD {
            let lock = LockoutState {
                triggered_at_ns: now_ns,
                duration_ns: LOCKOUT_DURATION_NS,
            };
            self.lockouts.insert(key, lock);
            self.total_lockouts.fetch_add(1, Ordering::Relaxed);
            return FailureOutcome::LockedOut;
        }

        FailureOutcome::Backoff {
            delay_ms: FailureState {
                count: new_count,
                last_fail_at_ns: now_ns,
            }
            .backoff_ms(),
        }
    }

    fn reset_on_success(&self, key: PairKey) {
        self.failures.remove(&key);
        self.lockouts.remove(&key);
    }

    fn is_locked_out(&self, key: PairKey, now_ns: u64) -> bool {
        match self.lockouts.get(&key) {
            Some(state) => {
                if state.is_active(now_ns) {
                    true
                } else {
                    drop(state);
                    self.lockouts.remove(&key);
                    false
                }
            }
            None => false,
        }
    }

    fn current_backoff_ms(&self, key: PairKey, now_ns: u64) -> u64 {
        match self.failures.get(&key) {
            Some(f) if !f.is_stale(now_ns) => f.backoff_ms(),
            _ => 0,
        }
    }

    fn admin_unlock_user(&self, username_hash: [u8; 16]) {
        // Remove all entries with matching username_hash across ALL subnets
        // (spec §12.3).
        self.failures.retain(|key, _| key.1 != username_hash);
        self.lockouts.retain(|key, _| key.1 != username_hash);
    }

    fn gc(&self, now_ns: u64) {
        self.failures.retain(|_, f| !f.is_stale(now_ns));
        self.lockouts.retain(|_, l| l.is_active(now_ns));
    }
}

// Tests live in crate::server::tests::lockout_tests.
