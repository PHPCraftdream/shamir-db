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

use crate::common::time::ns;
use crate::server::lockout::Subnet;
use dashmap::DashMap;

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

/// In-memory token-bucket rate limiter.
///
/// State per subnet: `(tokens_remaining, last_refill_at_ns)`. Refill rate
/// is `RATE_LIMIT_AUTH_INIT_PER_SECOND` (or `/4` during warmup).
/// `FxHasher` for small fixed-size Subnet keys ([u8;3] for IPv4 /24,
/// [u8;8] for IPv6 /64). DoS resistance for this map is moot — the
/// limiter itself is what protects against DoS.
type SubnetHasher = std::hash::BuildHasherDefault<fxhash::FxHasher>;

/// Token-bucket rate limiter keyed by client subnet.
pub struct InMemoryRateLimiter {
    buckets: DashMap<Subnet, BucketState, SubnetHasher>,
    /// Wall-clock at server-process start. Used to detect warmup window.
    startup_at_ns: u64,
}

#[derive(Debug, Clone, Copy)]
struct BucketState {
    /// Tokens currently in the bucket (fixed-point: tokens × 1e9).
    /// Stored scaled so we can do sub-token refill without floats.
    micro_tokens: u64,
    /// Last refill timestamp.
    last_refill_at_ns: u64,
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
        }
    }

    /// Number of distinct subnets currently tracked.
    pub fn tracked_subnets(&self) -> usize {
        self.buckets.len()
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
}
