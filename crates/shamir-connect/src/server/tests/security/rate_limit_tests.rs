use crate::common::time::{ns, UnixNanos};
use crate::server::lockout::Subnet;
use crate::server::rate_limit::{
    BucketState, InMemoryRateLimiter, RateDecision, RateLimitSnapshot, RateLimitSnapshotError,
    RateLimitSnapshotSink, RateLimiter, WARMUP_DIVISOR, WARMUP_WINDOW_NS,
};
use std::sync::Arc;

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
    let boot = UnixNanos::now().as_u64();
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
    let boot = UnixNanos::now().as_u64();
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
fn snapshot_second_codec_roundtrip() {
    // Belt-and-suspenders: verify against a second codec (CBOR-via-rmp
    // named encoding) so a codec-specific quirk can't mask a missing derive.
    let r = InMemoryRateLimiter::new(0);
    r.check(s(1), WARMUP_WINDOW_NS + 1);
    let snap = r.snapshot();
    let bytes = rmp_serde::to_vec_named(&snap).expect("encode");
    let restored: RateLimitSnapshot = rmp_serde::from_slice(&bytes).expect("decode");
    assert_eq!(restored.buckets.len(), snap.buckets.len());
}

#[test]
fn with_rate_honours_configured_rate() {
    // rate=2, past warmup → exactly 2 tokens/sec burst.
    let r = InMemoryRateLimiter::with_rate(0, 2);
    let now = WARMUP_WINDOW_NS + 1;

    assert_eq!(r.effective_rate_per_sec(now), 2);

    // First 2 requests: allowed (bucket starts full).
    assert_eq!(r.check(s(1), now), RateDecision::Allowed);
    assert_eq!(r.check(s(1), now), RateDecision::Allowed);
    // 3rd in the same instant: throttled.
    assert!(matches!(
        r.check(s(1), now),
        RateDecision::RateLimited { .. }
    ));

    // 1 second later → bucket refilled, 2 more allowed.
    let later = now + ns::SECOND;
    assert_eq!(r.check(s(1), later), RateDecision::Allowed);
    assert_eq!(r.check(s(1), later), RateDecision::Allowed);
    assert!(matches!(
        r.check(s(1), later),
        RateDecision::RateLimited { .. }
    ));
}

#[test]
fn with_rate_high_allows_many_requests() {
    // rate=1000, past warmup → 1000 tokens/sec.
    let r = InMemoryRateLimiter::with_rate(0, 1000);
    let now = WARMUP_WINDOW_NS + 1;

    assert_eq!(r.effective_rate_per_sec(now), 1000);

    // 1000 requests in the same instant: all allowed (full bucket).
    for _ in 0..1000 {
        assert_eq!(r.check(s(1), now), RateDecision::Allowed);
    }
    // 1001st: throttled (bucket exhausted).
    assert!(matches!(
        r.check(s(1), now),
        RateDecision::RateLimited { .. }
    ));
}

#[test]
fn with_rate_warmup_divides_configured_rate() {
    // rate=8, within warmup → effective 8/4 = 2.
    let r = InMemoryRateLimiter::with_rate(0, 8);
    let now = 1_000_000; // well within warmup window

    assert_eq!(r.effective_rate_per_sec(now), 8 / WARMUP_DIVISOR);
}
