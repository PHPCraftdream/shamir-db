//! Per-subnet rate limiting for `auth_init` (spec §8 + §8.6 NORMATIVE).
//!
//! Caps the rate of new authentication attempts to defend against
//! distributed credential-stuffing / enumeration attacks. Independent of
//! the lockout subsystem (which is per-pair, post-failure); this module
//! gates the very first request from a subnet **before** Argon2id is even
//! considered.
//!
//! ## Constants (per spec §8 table)
//!
//! - `RATE_LIMIT_AUTH_INIT_PER_SUBNET = 10/sec` (sliding window).
//! - Subnet granularity: `/24 IPv4` or `/64 IPv6` (same as lockout).
//!
//! ## Restart warmup window (spec §8.6 NORMATIVE)
//!
//! In the first 60 seconds after server start, the rate is divided by 4
//! (= 2.5/sec) until in-memory state warms up from persisted snapshots.
//! Closes the restart-replay window for distributed attackers who would
//! otherwise burst-replay collected probes against a freshly-restarted
//! server with empty rate-limit / lockout state.
//!
//! ## Algorithm
//!
//! Token bucket per subnet, refilled at the configured rate. Each request
//! consumes one token; if the bucket is empty the request is rejected
//! with `rate_limited` (spec §14.4).
//!
//! Pluggable [`RateLimiter`] trait so production can back state with
//! durable storage (per spec IMPL §1.3); reference [`InMemoryRateLimiter`]
//! is fine for single-node deployments where some warmup-window drift on
//! restart is acceptable (the warmup itself defends).
//!
//! ## Snapshot persistence
//!
//! [`InMemoryRateLimiter::with_snapshot_sink`] installs a
//! [`RateLimitSnapshotSink`] (mirroring the lockout subsystem's
//! `LockoutSnapshotSink`) that is consulted on construction to rehydrate
//! the per-subnet token buckets and is later driven by a periodic task
//! (typically the SAME 60s task that snapshots lockout — see
//! `shamir-server::server`) that calls
//! [`InMemoryRateLimiter::persist_snapshot`]. The serialised form is a
//! [`RateLimitSnapshot`] value with stable serde shape; the sink backend
//! (redb, file, ...) is the embedder's choice.
//!
//! ### Rehydration of TIME-DEPENDENT buckets (security note)
//!
//! Token buckets refill against elapsed wall-clock. A naive restore that
//! preserved each bucket's `last_refill_at_ns` from snapshot time would,
//! on the first post-restart `check`, see a huge `elapsed` (= downtime +
//! uptime-since-boot) and refill every bucket to FULL — handing an
//! attacker free tokens for the entire downtime. That is the INSECURE
//! direction.
//!
//! Instead [`InMemoryRateLimiter::with_snapshot`] restores `micro_tokens`
//! verbatim (so a depleted/throttled subnet stays throttled across the
//! restart) but RESETS every bucket's `last_refill_at_ns` to the fresh
//! boot time. The downtime is therefore treated as "no refill happened" —
//! conservative, the SECURE direction: an attacker gains no free refill by
//! inducing a restart. Normal refill logic then catches up from boot. The
//! spec §8.6 warmup window (rate /4 for the first 60s) layers additional
//! defence on top during exactly this recovery interval.

use crate::common::time::ns;
use crate::server::lockout::Subnet;
use dashmap::DashMap;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

/// Spec §8 table: 10 auth_init / sec per subnet.
pub const RATE_LIMIT_AUTH_INIT_PER_SECOND: u32 = 10;
/// Spec §8.6 warmup divisor.
pub const WARMUP_DIVISOR: u32 = 4;
/// Spec §8.6 warmup window.
pub const WARMUP_WINDOW_NS: u64 = 60 * ns::SECOND;

/// Decision returned by [`RateLimiter::check`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RateDecision {
    /// Request is permitted; the bucket has been debited.
    Allowed,
    /// Request rejected — caller emits `rate_limited` error with
    /// `retry_after` (in seconds, rounded up).
    RateLimited {
        /// Seconds until at least one more token is available.
        retry_after_secs: u32,
    },
}

/// Per-subnet sliding-window rate limiter for `auth_init`.
pub trait RateLimiter: Send + Sync {
    /// Check + consume one token for `subnet` at `now_ns`. Honors the spec
    /// §8.6 warmup window if `now_ns < startup_at_ns + WARMUP_WINDOW_NS`.
    fn check(&self, subnet: Subnet, now_ns: u64) -> RateDecision;

    /// Background GC: drop bucket entries with no activity for >5 min.
    fn gc(&self, now_ns: u64);
}

/// Serialisable point-in-time copy of all per-subnet token buckets, used
/// by [`RateLimitSnapshotSink`] for durable persistence across restarts
/// (mirrors the lockout subsystem's `LockoutSnapshot`).
///
/// The wire format is `serde`-tagged so future revisions can add fields
/// without breaking deserialisation. Note: `startup_at_ns` is deliberately
/// NOT carried — the §8.6 warmup anchor is re-armed from the fresh boot
/// instant on restore (see [`InMemoryRateLimiter::with_snapshot`]).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RateLimitSnapshot {
    /// Per-subnet bucket state (`subnet -> (micro_tokens, last_refill_at_ns)`).
    pub buckets: Vec<(Subnet, BucketState)>,
    /// Wall-clock at which the snapshot was taken (unix nanos). Used by the
    /// loader to apply the same idle-GC cutoff that would have applied at
    /// capture time.
    pub captured_at_ns: u64,
}

/// Backend that materialises [`RateLimitSnapshot`]s. The embedder picks the
/// concrete adapter (redb, file, ...); shamir-connect itself stays free of
/// storage dependencies.
///
/// Implementations MUST be idempotent: calling [`Self::save`] with the same
/// snapshot twice is allowed. [`Self::load`] returns `Ok(None)` when there
/// is no prior snapshot (e.g. a fresh data dir).
pub trait RateLimitSnapshotSink: Send + Sync {
    /// Persist `snapshot` durably. Errors are returned to the caller so the
    /// periodic task can log them; the in-memory state is never dropped on
    /// failure.
    fn save(&self, snapshot: &RateLimitSnapshot) -> Result<(), RateLimitSnapshotError>;

    /// Load the most-recent snapshot if one exists. `Ok(None)` for a
    /// brand-new store.
    fn load(&self) -> Result<Option<RateLimitSnapshot>, RateLimitSnapshotError>;
}

/// Error type returned by [`RateLimitSnapshotSink`] implementations.
#[derive(Debug, thiserror::Error)]
pub enum RateLimitSnapshotError {
    /// Underlying storage refused the read/write (disk full, permission
    /// denied, fsync failure, etc.).
    #[error("storage: {0}")]
    Storage(String),
    /// Encoding / decoding failure — usually a malformed prior snapshot
    /// after a format change. Treated as "no snapshot" by the loader.
    #[error("encoding: {0}")]
    Encoding(String),
}

/// In-memory token-bucket rate limiter.
///
/// State per subnet: `(tokens_remaining, last_refill_at_ns)`. Refill rate
/// is `RATE_LIMIT_AUTH_INIT_PER_SECOND` (or `/4` during warmup).
/// `FxHasher` for small fixed-size Subnet keys ([u8;3] for IPv4 /24,
/// [u8;8] for IPv6 /64). DoS resistance for this map is moot — the
/// limiter itself is what protects against DoS.
type SubnetHasher = std::hash::BuildHasherDefault<fxhash::FxHasher>;

/// Token-bucket rate limiter keyed by client subnet.
///
/// Optionally backed by a [`RateLimitSnapshotSink`] for durable
/// persistence across restarts; see
/// [`Self::with_snapshot_sink`] / [`Self::persist_snapshot`].
pub struct InMemoryRateLimiter {
    buckets: DashMap<Subnet, BucketState, SubnetHasher>,
    /// Wall-clock at server-process start. Used to detect warmup window.
    startup_at_ns: u64,
    /// Optional durable backend. `None` for in-memory-only deployments
    /// (default and most tests).
    snapshot_sink: Option<Arc<dyn RateLimitSnapshotSink>>,
}

/// Per-subnet token-bucket state. Public only so it can appear in the
/// serialisable [`RateLimitSnapshot`]; fields are crate-internal.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct BucketState {
    /// Tokens currently in the bucket (fixed-point: tokens × 1e9).
    /// Stored scaled so we can do sub-token refill without floats.
    pub(crate) micro_tokens: u64,
    /// Last refill timestamp.
    pub(crate) last_refill_at_ns: u64,
}

impl BucketState {
    /// Capacity in scaled units (= burst limit × 1e9).
    /// We allow burst of 1 second's worth of tokens.
    const fn capacity_at_rate(rate_per_sec: u32) -> u64 {
        (rate_per_sec as u64) * 1_000_000_000
    }
}

impl InMemoryRateLimiter {
    /// New limiter; `startup_at_ns` should be `UnixNanos::now()` at server
    /// boot.
    pub fn new(startup_at_ns: u64) -> Self {
        Self {
            buckets: DashMap::with_hasher(SubnetHasher::default()),
            startup_at_ns,
            snapshot_sink: None,
        }
    }

    /// Create a limiter from an explicit snapshot, anchored at a FRESH
    /// `startup_at_ns` (the new boot instant). The §8.6 warmup window is
    /// re-armed from `startup_at_ns`, and every restored bucket's
    /// `last_refill_at_ns` is reset to `startup_at_ns` so the downtime
    /// grants no free refill (see module docs — the secure direction).
    /// `micro_tokens` are restored verbatim, preserving each subnet's
    /// depletion level across the restart.
    ///
    /// Buckets that were already GC-eligible at snapshot time (idle >5 min)
    /// are discarded, matching what [`RateLimiter::gc`] would have produced.
    pub fn with_snapshot(snapshot: RateLimitSnapshot, startup_at_ns: u64) -> Self {
        let limiter = Self::new(startup_at_ns);
        limiter.rehydrate(snapshot, startup_at_ns);
        limiter
    }

    /// Create a limiter backed by `sink`, anchored at a fresh
    /// `startup_at_ns`. On construction the sink is consulted for a prior
    /// snapshot and the limiter is rehydrated from it (errors are logged at
    /// `warn` and the limiter starts empty).
    ///
    /// Subsequent calls to [`Self::persist_snapshot`] write through the
    /// same sink.
    pub fn with_snapshot_sink(sink: Arc<dyn RateLimitSnapshotSink>, startup_at_ns: u64) -> Self {
        let limiter = Self {
            buckets: DashMap::with_hasher(SubnetHasher::default()),
            startup_at_ns,
            snapshot_sink: Some(sink.clone()),
        };
        match sink.load() {
            Ok(Some(snap)) => limiter.rehydrate(snap, startup_at_ns),
            Ok(None) => {}
            Err(e) => {
                log::warn!("rate-limit snapshot load failed; starting empty: {e}");
            }
        }
        limiter
    }

    /// Number of distinct subnets currently tracked.
    pub fn tracked_subnets(&self) -> usize {
        self.buckets.len()
    }

    /// Capture a point-in-time copy of every bucket for the snapshot path.
    /// Holds map shards only long enough to clone `(key, value)` pairs; no
    /// locks are held across an `.await` (this is a synchronous function).
    pub fn snapshot(&self) -> RateLimitSnapshot {
        let captured_at_ns = crate::common::time::UnixNanos::now().as_u64();
        let mut buckets = Vec::with_capacity(self.buckets.len());
        for entry in self.buckets.iter() {
            buckets.push((*entry.key(), *entry.value()));
        }
        RateLimitSnapshot {
            buckets,
            captured_at_ns,
        }
    }

    /// Persist the current limiter via the installed
    /// [`RateLimitSnapshotSink`]. Returns `Ok(false)` when no sink is
    /// installed (in-memory-only mode); `Ok(true)` after a successful
    /// write. Errors are propagated so the caller can log and rate-limit.
    pub fn persist_snapshot(&self) -> Result<bool, RateLimitSnapshotError> {
        let Some(sink) = self.snapshot_sink.as_ref() else {
            return Ok(false);
        };
        let snap = self.snapshot();
        sink.save(&snap)?;
        Ok(true)
    }

    /// Load buckets from `snapshot`, discarding idle ones and resetting
    /// every surviving bucket's `last_refill_at_ns` to `boot_at_ns` so the
    /// downtime grants no free refill. Idempotent; existing in-memory state
    /// is REPLACED (this is only called from constructors).
    fn rehydrate(&self, snapshot: RateLimitSnapshot, boot_at_ns: u64) {
        // Use the snapshot's own clock for the GC freshness decision so the
        // result matches what `gc` would have produced at capture time.
        let cutoff = snapshot.captured_at_ns.saturating_sub(5 * ns::MINUTE);
        self.buckets.clear();
        for (subnet, mut bucket) in snapshot.buckets {
            if bucket.last_refill_at_ns < cutoff {
                continue; // would already have been GC'd at capture time
            }
            // Conservative restore: keep the depleted token level but
            // re-anchor refill to the fresh boot instant (no free refill
            // across downtime — see module docs).
            bucket.last_refill_at_ns = boot_at_ns;
            self.buckets.insert(subnet, bucket);
        }
    }

    /// Effective rate at `now_ns`: full rate normally, `/WARMUP_DIVISOR`
    /// during the warmup window.
    pub fn effective_rate_per_sec(&self, now_ns: u64) -> u32 {
        if now_ns < self.startup_at_ns.saturating_add(WARMUP_WINDOW_NS) {
            (RATE_LIMIT_AUTH_INIT_PER_SECOND / WARMUP_DIVISOR).max(1)
        } else {
            RATE_LIMIT_AUTH_INIT_PER_SECOND
        }
    }
}

impl RateLimiter for InMemoryRateLimiter {
    fn check(&self, subnet: Subnet, now_ns: u64) -> RateDecision {
        let rate = self.effective_rate_per_sec(now_ns);
        let capacity = BucketState::capacity_at_rate(rate);

        let mut decision = RateDecision::Allowed;
        self.buckets
            .entry(subnet)
            .and_modify(|b| {
                // Refill: tokens += elapsed_ns × rate / 1e9 (in scaled units).
                let elapsed = now_ns.saturating_sub(b.last_refill_at_ns);
                let refill = elapsed.saturating_mul(rate as u64);
                b.micro_tokens = b.micro_tokens.saturating_add(refill).min(capacity);
                b.last_refill_at_ns = now_ns;

                let cost = 1_000_000_000u64;
                if b.micro_tokens >= cost {
                    b.micro_tokens -= cost;
                    decision = RateDecision::Allowed;
                } else {
                    let deficit = cost - b.micro_tokens;
                    let secs_to_wait = (deficit / (rate as u64)) / 1_000_000_000;
                    let secs_u32 = (secs_to_wait as u32).max(1);
                    decision = RateDecision::RateLimited {
                        retry_after_secs: secs_u32,
                    };
                }
            })
            .or_insert_with(|| {
                // First request from this subnet: bucket starts FULL.
                let cost = 1_000_000_000u64;
                BucketState {
                    micro_tokens: capacity - cost,
                    last_refill_at_ns: now_ns,
                }
            });

        decision
    }

    fn gc(&self, now_ns: u64) {
        // Drop buckets idle for >5 minutes.
        let cutoff = now_ns.saturating_sub(5 * ns::MINUTE);
        self.buckets.retain(|_, b| b.last_refill_at_ns >= cutoff);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(b: u8) -> Subnet {
        Subnet::V4([10, 0, b])
    }

    #[test]
    fn first_request_allowed() {
        let r = InMemoryRateLimiter::new(0);
        let now = WARMUP_WINDOW_NS + 1; // past warmup
        assert_eq!(r.check(s(1), now), RateDecision::Allowed);
    }

    #[test]
    fn ten_requests_per_second_allowed_then_throttled() {
        let r = InMemoryRateLimiter::new(0);
        let now = WARMUP_WINDOW_NS + 1; // past warmup → 10/sec rate

        // First 10 in the same instant: all allowed (bucket starts full).
        for _ in 0..10 {
            assert_eq!(r.check(s(1), now), RateDecision::Allowed);
        }
        // 11th in the same instant: throttled.
        assert!(matches!(
            r.check(s(1), now),
            RateDecision::RateLimited { .. }
        ));
    }

    #[test]
    fn bucket_refills_after_one_second() {
        let r = InMemoryRateLimiter::new(0);
        let now = WARMUP_WINDOW_NS + 1;

        for _ in 0..10 {
            assert_eq!(r.check(s(1), now), RateDecision::Allowed);
        }
        assert!(matches!(
            r.check(s(1), now),
            RateDecision::RateLimited { .. }
        ));

        // 1 second later → bucket fully refilled.
        let later = now + ns::SECOND;
        for _ in 0..10 {
            assert_eq!(r.check(s(1), later), RateDecision::Allowed);
        }
    }

    #[test]
    fn warmup_window_quarters_the_rate() {
        let r = InMemoryRateLimiter::new(0);
        let now = 1_000_000; // well within warmup window

        assert_eq!(r.effective_rate_per_sec(now), 10 / WARMUP_DIVISOR);

        // First 2-3 requests allowed (rate /4 = 2.5/sec, capacity ~2 tokens).
        let mut allowed_in_burst = 0;
        for _ in 0..10 {
            if r.check(s(1), now) == RateDecision::Allowed {
                allowed_in_burst += 1;
            }
        }
        // Within warmup we should get ~2 (rounded down from 2.5).
        assert!(
            allowed_in_burst <= 3,
            "warmup must throttle bursts; got {} allowed",
            allowed_in_burst
        );
    }

    #[test]
    fn separate_subnets_have_independent_buckets() {
        let r = InMemoryRateLimiter::new(0);
        let now = WARMUP_WINDOW_NS + 1;

        for _ in 0..10 {
            assert_eq!(r.check(s(1), now), RateDecision::Allowed);
        }
        // Subnet 1 throttled, but subnet 2 fresh.
        assert!(matches!(
            r.check(s(1), now),
            RateDecision::RateLimited { .. }
        ));
        assert_eq!(r.check(s(2), now), RateDecision::Allowed);
    }

    #[test]
    fn gc_drops_idle_buckets() {
        let r = InMemoryRateLimiter::new(0);
        let now = WARMUP_WINDOW_NS + 1;
        r.check(s(1), now);
        assert_eq!(r.tracked_subnets(), 1);

        r.gc(now + 6 * ns::MINUTE);
        assert_eq!(r.tracked_subnets(), 0);
    }

    #[test]
    fn rate_limited_returns_positive_retry_after() {
        let r = InMemoryRateLimiter::new(0);
        let now = WARMUP_WINDOW_NS + 1;
        for _ in 0..10 {
            r.check(s(1), now);
        }
        match r.check(s(1), now) {
            RateDecision::RateLimited { retry_after_secs } => {
                assert!(retry_after_secs >= 1, "retry_after must be >= 1s");
            }
            RateDecision::Allowed => panic!("should have been throttled"),
        }
    }

    #[test]
    fn warmup_ends_at_60s() {
        let r = InMemoryRateLimiter::new(0);
        // At exactly the boundary: not warmup anymore.
        assert_eq!(r.effective_rate_per_sec(WARMUP_WINDOW_NS), 10);
        assert_eq!(
            r.effective_rate_per_sec(WARMUP_WINDOW_NS - 1),
            10 / WARMUP_DIVISOR
        );
    }

    // -------------------------------------------------------------------
    // Snapshot persistence tests (mirror the lockout subsystem).
    // -------------------------------------------------------------------

    /// In-memory sink for snapshot round-trip tests.
    struct MemSink(std::sync::Mutex<Option<RateLimitSnapshot>>);
    impl MemSink {
        fn new() -> Arc<Self> {
            Arc::new(Self(std::sync::Mutex::new(None)))
        }
    }
    impl RateLimitSnapshotSink for MemSink {
        fn save(&self, snapshot: &RateLimitSnapshot) -> Result<(), RateLimitSnapshotError> {
            *self.0.lock().unwrap() = Some(snapshot.clone());
            Ok(())
        }
        fn load(&self) -> Result<Option<RateLimitSnapshot>, RateLimitSnapshotError> {
            Ok(self.0.lock().unwrap().clone())
        }
    }

    #[test]
    fn ratelimit_snapshot_round_trips() {
        // Drain a subnet's bucket to empty (throttled), snapshot, serialize
        // through msgpack, restore into a NEW limiter at a fresh boot time,
        // and verify: (a) bucket state survives, (b) the restart grants no
        // free tokens for the downtime gap.
        // Anchor at real wall-clock: `snapshot()` stamps `captured_at_ns`
        // with `UnixNanos::now()`, so the bucket timestamps must be on the
        // same clock for the idle-GC check in `rehydrate` to behave as it
        // does in production (where capture time and bucket refill time
        // share one clock).
        let boot = crate::common::time::UnixNanos::now().as_u64();
        let r = InMemoryRateLimiter::new(boot);
        // Past warmup → 10/sec, full bucket of 10 tokens.
        let now = boot + WARMUP_WINDOW_NS + 1;
        for _ in 0..10 {
            assert_eq!(r.check(s(1), now), RateDecision::Allowed);
        }
        // 11th → throttled (bucket drained).
        assert!(matches!(
            r.check(s(1), now),
            RateDecision::RateLimited { .. }
        ));

        let snap = r.snapshot();
        assert_eq!(snap.buckets.len(), 1);

        // Round-trip through msgpack (the durable encoding used by the
        // redb-backed sink in shamir-server).
        let bytes = rmp_serde::to_vec_named(&snap).expect("encode");
        let restored: RateLimitSnapshot = rmp_serde::from_slice(&bytes).expect("decode");

        // Restart far in the future (simulating a long downtime).
        let boot_at = now + 3600 * ns::SECOND;
        let r2 = InMemoryRateLimiter::with_snapshot(restored, boot_at);
        assert_eq!(r2.tracked_subnets(), 1, "bucket must survive restore");

        // The drained subnet must STILL be throttled at the boot instant:
        // the hour of downtime granted NO free refill. `rehydrate`
        // re-anchored `last_refill_at_ns` to `boot_at`, so `elapsed` at the
        // boot instant is 0 — the only thing that survived is the (empty)
        // token level. This is the secure direction: an attacker who forces
        // a restart cannot wash away a depleted bucket.
        assert!(
            matches!(r2.check(s(1), boot_at), RateDecision::RateLimited { .. }),
            "restored drained bucket must not be refilled across downtime"
        );

        // Legitimate refill resumes from boot: 1s of UPTIME later the
        // bucket has earned tokens again (full warmup-rate burst).
        let one_sec_uptime = boot_at + ns::SECOND;
        assert_eq!(r2.check(s(1), one_sec_uptime), RateDecision::Allowed);
    }

    #[test]
    fn with_snapshot_sink_rehydrates_and_persists() {
        let sink = MemSink::new();
        let boot = crate::common::time::UnixNanos::now().as_u64();
        let now = boot + WARMUP_WINDOW_NS + 1;
        {
            let r = InMemoryRateLimiter::with_snapshot_sink(sink.clone(), boot);
            for _ in 0..10 {
                r.check(s(1), now);
            }
            let wrote = r.persist_snapshot().expect("persist must succeed");
            assert!(wrote, "sink installed → persist returns true");
        }

        // A fresh limiter from the same sink mirrors the drained bucket and
        // stays throttled at the boot instant (no free refill across the
        // simulated downtime).
        let boot_at = now + 3600 * ns::SECOND;
        let r2 = InMemoryRateLimiter::with_snapshot_sink(sink, boot_at);
        assert_eq!(r2.tracked_subnets(), 1);
        assert!(matches!(
            r2.check(s(1), boot_at),
            RateDecision::RateLimited { .. }
        ));
    }

    #[test]
    fn persist_snapshot_without_sink_is_noop() {
        let r = InMemoryRateLimiter::new(0);
        r.check(s(1), WARMUP_WINDOW_NS + 1);
        let wrote = r.persist_snapshot().expect("noop must succeed");
        assert!(!wrote, "no sink → persist returns false");
    }

    #[test]
    fn rehydrate_drops_idle_buckets() {
        // A bucket idle >5 min at capture time is dropped on restore,
        // matching what `gc` would have produced.
        let captured_at_ns = 10 * ns::HOUR;
        let snap = RateLimitSnapshot {
            buckets: vec![
                // Idle: last refill >5 min before capture.
                (
                    s(1),
                    BucketState {
                        micro_tokens: 0,
                        last_refill_at_ns: captured_at_ns - 6 * ns::MINUTE,
                    },
                ),
                // Fresh: within the idle window.
                (
                    s(2),
                    BucketState {
                        micro_tokens: 0,
                        last_refill_at_ns: captured_at_ns - ns::MINUTE,
                    },
                ),
            ],
            captured_at_ns,
        };
        let r = InMemoryRateLimiter::with_snapshot(snap, captured_at_ns);
        assert_eq!(r.tracked_subnets(), 1, "idle bucket must be dropped");
    }

    #[test]
    fn snapshot_json_roundtrip() {
        // Belt-and-suspenders: verify against a second codec (JSON) so a
        // codec-specific quirk in rmp can't mask a missing derive.
        let r = InMemoryRateLimiter::new(0);
        r.check(s(1), WARMUP_WINDOW_NS + 1);
        let snap = r.snapshot();
        let json = serde_json::to_vec(&snap).expect("json encode");
        let restored: RateLimitSnapshot = serde_json::from_slice(&json).expect("json decode");
        assert_eq!(restored.buckets.len(), snap.buckets.len());
    }
}
