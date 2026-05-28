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

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::Ipv4Addr;

    fn key(subnet: u8, user: u8) -> PairKey {
        (Subnet::V4([10, 0, subnet]), [user; 16])
    }

    #[test]
    fn first_failure_returns_100ms_backoff() {
        let s = InMemoryLockoutStore::new();
        let now = 1_000_000_000;
        match s.register_failure(key(1, 1), now) {
            FailureOutcome::Backoff { delay_ms } => assert_eq!(delay_ms, 100),
            FailureOutcome::LockedOut => panic!("first failure must not lock out"),
        }
    }

    #[test]
    fn backoff_doubles_per_failure() {
        let s = InMemoryLockoutStore::new();
        let now = 1_000_000_000;
        let k = key(1, 1);

        let expected = [
            100u64, 200, 400, 800, 1600, 3200, 6400, 12800, 25600, 30000, 30000,
        ];
        for (i, &want) in expected.iter().enumerate() {
            let got = match s.register_failure(k, now + (i as u64) * ns::SECOND) {
                FailureOutcome::Backoff { delay_ms } => delay_ms,
                FailureOutcome::LockedOut => 0,
            };
            assert_eq!(
                got,
                want,
                "failure #{} expected {}ms got {}ms",
                i + 1,
                want,
                got
            );
        }
    }

    #[test]
    fn lockout_after_threshold() {
        let s = InMemoryLockoutStore::new();
        let now = 1_000_000_000;
        let k = key(1, 1);

        // 49 failures: still backoff.
        for i in 0..49 {
            let outcome = s.register_failure(k, now + (i as u64) * ns::SECOND);
            assert!(matches!(outcome, FailureOutcome::Backoff { .. }));
        }

        // 50th failure: locked out.
        let outcome = s.register_failure(k, now + 49 * ns::SECOND);
        assert_eq!(outcome, FailureOutcome::LockedOut);
        assert!(s.is_locked_out(k, now + 49 * ns::SECOND));
        assert_eq!(s.total_lockouts(), 1);
    }

    #[test]
    fn lockout_expires_after_duration() {
        let s = InMemoryLockoutStore::new();
        let now = 1_000_000_000;
        let k = key(1, 1);
        for i in 0..50 {
            s.register_failure(k, now + (i as u64) * ns::SECOND);
        }
        let trigger_ts = now + 49 * ns::SECOND;
        assert!(s.is_locked_out(k, now + 50 * ns::SECOND));

        // Expiry: triggered_at + duration. Just after that → unlocked.
        let after = trigger_ts + LOCKOUT_DURATION_NS + 1;
        assert!(!s.is_locked_out(k, after));
    }

    #[test]
    fn reset_on_success_clears_failure_and_lockout() {
        let s = InMemoryLockoutStore::new();
        let now = 1_000_000_000;
        let k = key(1, 1);
        s.register_failure(k, now);
        s.register_failure(k, now);
        assert!(s.current_backoff_ms(k, now) > 0);

        s.reset_on_success(k);
        assert_eq!(s.current_backoff_ms(k, now), 0);
        assert!(!s.is_locked_out(k, now));
    }

    #[test]
    fn backoff_resets_after_inactivity_window() {
        let s = InMemoryLockoutStore::new();
        let now = 1_000_000_000;
        let k = key(1, 1);
        s.register_failure(k, now);
        s.register_failure(k, now); // backoff = 200ms

        // 6 minutes later → BACKOFF_RESET_NS exceeded → next failure is treated as fresh
        let later = now + 6 * ns::MINUTE;
        let outcome = s.register_failure(k, later);
        assert_eq!(outcome, FailureOutcome::Backoff { delay_ms: 100 });
    }

    #[test]
    fn admin_unlock_user_clears_all_subnets() {
        let s = InMemoryLockoutStore::new();
        let now = 1_000_000_000;
        let user = [0xaau8; 16];
        let k1 = (Subnet::V4([10, 0, 1]), user);
        let k2 = (Subnet::V4([10, 0, 2]), user);
        for _ in 0..50 {
            s.register_failure(k1, now);
        }
        s.register_failure(k2, now);
        assert!(s.is_locked_out(k1, now));
        assert!(s.current_backoff_ms(k2, now) > 0);

        s.admin_unlock_user(user);
        assert!(!s.is_locked_out(k1, now));
        assert_eq!(s.current_backoff_ms(k2, now), 0);
    }

    #[test]
    fn gc_removes_stale_entries() {
        let s = InMemoryLockoutStore::new();
        let now = 1_000_000_000;
        let k = key(1, 1);
        s.register_failure(k, now);
        assert_eq!(s.failure_pair_count(), 1);

        let later = now + 6 * ns::MINUTE;
        s.gc(later);
        assert_eq!(s.failure_pair_count(), 0);
    }

    #[test]
    fn subnet_of_v4_takes_24_bit_prefix() {
        let s = subnet_of(IpAddr::V4(Ipv4Addr::new(10, 0, 1, 200)));
        assert_eq!(s, Subnet::V4([10, 0, 1]));
    }

    #[test]
    fn username_hash_is_deterministic() {
        let secret = [0xaau8; 32];
        let h1 = username_hash(&secret, b"alice");
        let h2 = username_hash(&secret, b"alice");
        assert_eq!(h1, h2);

        let h3 = username_hash(&secret, b"bob");
        assert_ne!(h1, h3);
    }

    #[test]
    fn username_hash_separates_lockout_from_server_secret() {
        // Different lockout_secrets must produce different hashes for the
        // same username — defends against secret-rotation orphan state.
        let h1 = username_hash(&[0x01u8; 32], b"alice");
        let h2 = username_hash(&[0x02u8; 32], b"alice");
        assert_ne!(h1, h2);
    }

    // -------------------------------------------------------------------
    // Snapshot persistence tests (Option A — periodic durable snapshot).
    // -------------------------------------------------------------------

    /// In-memory sink for snapshot round-trip tests. Stores the latest
    /// saved snapshot in a `Mutex<Option<…>>`.
    struct MemSink(std::sync::Mutex<Option<LockoutSnapshot>>);
    impl MemSink {
        fn new() -> Arc<Self> {
            Arc::new(Self(std::sync::Mutex::new(None)))
        }
        fn preload(snap: LockoutSnapshot) -> Arc<Self> {
            Arc::new(Self(std::sync::Mutex::new(Some(snap))))
        }
    }
    impl LockoutSnapshotSink for MemSink {
        fn save(&self, snapshot: &LockoutSnapshot) -> Result<(), LockoutSnapshotError> {
            *self.0.lock().unwrap() = Some(snapshot.clone());
            Ok(())
        }
        fn load(&self) -> Result<Option<LockoutSnapshot>, LockoutSnapshotError> {
            Ok(self.0.lock().unwrap().clone())
        }
    }

    #[test]
    fn snapshot_captures_failures_and_lockouts() {
        let s = InMemoryLockoutStore::new();
        let now = 1_000_000_000;
        s.register_failure(key(1, 1), now);
        s.register_failure(key(1, 1), now);
        s.register_failure(key(2, 7), now);

        let snap = s.snapshot();
        assert_eq!(snap.failures.len(), 2);
        assert!(snap.lockouts.is_empty());

        // Drive one pair into LockedOut.
        for i in 0..50 {
            s.register_failure(key(3, 9), now + (i as u64) * ns::SECOND);
        }
        let snap2 = s.snapshot();
        assert_eq!(snap2.lockouts.len(), 1);
        assert!(snap2.total_lockouts >= 1);
    }

    #[test]
    fn lockout_state_survives_snapshot_roundtrip() {
        // Use wall-clock-ish timestamps so the snapshot's
        // `captured_at_ns = UnixNanos::now()` doesn't make the rehydrated
        // entries look stale.
        let s = InMemoryLockoutStore::new();
        let now = crate::common::time::UnixNanos::now().as_u64();
        let k = key(1, 1);

        // Drive to lockout.
        for i in 0..50 {
            s.register_failure(k, now + (i as u64) * ns::SECOND);
        }
        assert!(s.is_locked_out(k, now + 49 * ns::SECOND));

        // Round-trip through msgpack (the durable encoding used by
        // the redb-backed sink in shamir-server).
        let snap = s.snapshot();
        let bytes = rmp_serde::to_vec_named(&snap).expect("encode");
        let restored: LockoutSnapshot = rmp_serde::from_slice(&bytes).expect("decode");

        let s2 = InMemoryLockoutStore::with_snapshot(restored);
        // Probe at the capture instant (well inside lockout duration).
        let probe_ns = snap.captured_at_ns;
        assert!(
            s2.is_locked_out(k, probe_ns),
            "lockout must survive snapshot round-trip"
        );
        assert!(s2.total_lockouts() >= 1);
    }

    #[test]
    fn failure_backoff_survives_snapshot_roundtrip() {
        let s = InMemoryLockoutStore::new();
        // Use wall-clock-ish timestamps so rehydrate's freshness check
        // does not discard the entry.
        let now = crate::common::time::UnixNanos::now().as_u64();
        let k = key(1, 1);
        s.register_failure(k, now);
        s.register_failure(k, now);
        let pre_backoff = s.current_backoff_ms(k, now);
        assert!(pre_backoff > 0);

        let snap = s.snapshot();
        let bytes = rmp_serde::to_vec_named(&snap).expect("encode");
        let restored: LockoutSnapshot = rmp_serde::from_slice(&bytes).expect("decode");

        let s2 = InMemoryLockoutStore::with_snapshot(restored);
        // At the captured instant the backoff requirement is preserved
        // verbatim (count and last_fail_at_ns both round-trip).
        let probe_ns = snap.captured_at_ns;
        assert_eq!(s2.current_backoff_ms(k, probe_ns), pre_backoff);
    }

    #[test]
    fn rehydrate_drops_stale_failures_and_expired_lockouts() {
        // Capture a snapshot in the past, then rehydrate as-if at that
        // point: stale entries (older than BACKOFF_RESET_NS at capture)
        // are dropped, expired lockouts likewise.
        //
        // `captured_at_ns` must be > LOCKOUT_DURATION_NS so the "expired
        // lockout" arithmetic stays in range.
        let captured_at_ns = 10 * ns::HOUR;
        let stale_ts = captured_at_ns - 10 * ns::MINUTE; // > BACKOFF_RESET_NS old
        let fresh_ts = captured_at_ns - ns::MINUTE;

        let snap = LockoutSnapshot {
            failures: vec![
                // Stale: > BACKOFF_RESET_NS old at capture.
                (
                    key(1, 1),
                    FailureState {
                        count: 3,
                        last_fail_at_ns: stale_ts,
                    },
                ),
                // Fresh: still within window.
                (
                    key(2, 2),
                    FailureState {
                        count: 2,
                        last_fail_at_ns: fresh_ts,
                    },
                ),
            ],
            lockouts: vec![
                // Expired: triggered LOCKOUT_DURATION_NS + 1s ago.
                (
                    key(3, 3),
                    LockoutState {
                        triggered_at_ns: captured_at_ns - LOCKOUT_DURATION_NS - ns::SECOND,
                        duration_ns: LOCKOUT_DURATION_NS,
                    },
                ),
                // Still active.
                (
                    key(4, 4),
                    LockoutState {
                        triggered_at_ns: captured_at_ns - ns::MINUTE,
                        duration_ns: LOCKOUT_DURATION_NS,
                    },
                ),
            ],
            total_lockouts: 42,
            captured_at_ns,
        };

        let s = InMemoryLockoutStore::with_snapshot(snap);
        assert_eq!(s.failure_pair_count(), 1, "stale failure must be dropped");
        assert_eq!(
            s.active_lockout_count(),
            1,
            "expired lockout must be dropped"
        );
        // Metric is preserved verbatim — it's a counter, not a real-time
        // decision input.
        assert_eq!(s.total_lockouts(), 42);
    }

    #[test]
    fn with_snapshot_sink_rehydrates_from_sink() {
        // Construct a synthetic snapshot, install it in a sink, then a
        // fresh store created `with_snapshot_sink` must mirror it.
        let now = 5_000_000_000;
        let k = key(7, 7);
        let snap = LockoutSnapshot {
            failures: vec![(
                k,
                FailureState {
                    count: 4,
                    last_fail_at_ns: now,
                },
            )],
            lockouts: vec![],
            total_lockouts: 0,
            captured_at_ns: now,
        };
        let sink = MemSink::preload(snap);

        let s = InMemoryLockoutStore::with_snapshot_sink(sink);
        // count=4 → backoff = 100 * 2^3 = 800
        assert_eq!(s.current_backoff_ms(k, now), 800);
    }

    #[test]
    fn persist_snapshot_writes_through_sink() {
        let sink = MemSink::new();
        let s = InMemoryLockoutStore::with_snapshot_sink(sink.clone());
        s.register_failure(key(1, 1), 1_000_000_000);
        s.register_failure(key(1, 1), 1_000_000_000);

        let wrote = s.persist_snapshot().expect("persist must succeed");
        assert!(wrote, "sink installed → persist returns true");

        let stored = sink.0.lock().unwrap().clone().expect("snapshot stored");
        assert_eq!(stored.failures.len(), 1);
        assert_eq!(stored.failures[0].1.count, 2);
    }

    #[test]
    fn persist_snapshot_without_sink_is_noop() {
        let s = InMemoryLockoutStore::new();
        s.register_failure(key(1, 1), 1_000_000_000);
        let wrote = s.persist_snapshot().expect("noop must succeed");
        assert!(!wrote, "no sink → persist returns false");
    }

    #[test]
    fn snapshot_json_roundtrip() {
        // Belt-and-suspenders: the docs promise the snapshot is
        // serde-compatible. Verify against a second codec (JSON) so a
        // codec-specific quirk in rmp can't mask a missing derive.
        let s = InMemoryLockoutStore::new();
        s.register_failure(key(1, 1), 1_000_000_000);
        let snap = s.snapshot();
        let json = serde_json::to_vec(&snap).expect("json encode");
        let restored: LockoutSnapshot = serde_json::from_slice(&json).expect("json decode");
        assert_eq!(restored.failures.len(), snap.failures.len());
        assert_eq!(restored.total_lockouts, snap.total_lockouts);
    }
}
