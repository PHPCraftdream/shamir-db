//! Tests for `Session::check_post_auth_rate_limit` (task #608).
//!
//! The pre-auth `InMemoryRateLimiter` (`rate_limit.rs`) only guards
//! `auth_init`; once a session exists, nothing previously bounded request
//! FREQUENCY (only concurrency, via `CONN_MAX_IN_FLIGHT`). These tests pin
//! down the new per-session token-bucket gate: a burst up to the configured
//! rate is allowed, the very next request in the same instant is rejected,
//! and a 1-second refill restores exactly one token's worth of headroom.

use crate::common::types::{BindingMode, TransportKind};
use crate::server::session::{Session, SessionPermissions};
use shamir_tunables::instance_defaults::POST_AUTH_RATE_LIMIT_PER_SEC;

fn fresh_session(now_ns: u64) -> Session {
    Session::new(
        [0u8; 16],
        "alice".into(),
        SessionPermissions::from_roles(vec!["read_write".into()]),
        TransportKind::Tcp,
        BindingMode::None,
        [0u8; 32],
        now_ns,
    )
}

#[test]
fn burst_up_to_configured_rate_is_allowed_then_next_is_rejected() {
    let now_ns = 1_000_000_000u64;
    let session = fresh_session(now_ns);

    // Freshly-created session starts with a full bucket: exactly `rate`
    // requests at the SAME instant must all be allowed.
    for i in 0..POST_AUTH_RATE_LIMIT_PER_SEC {
        let decision = session.check_post_auth_rate_limit(now_ns);
        assert!(
            decision.is_none(),
            "request #{i} within burst budget should be allowed, got {decision:?}"
        );
    }

    // The very next request (burst + 1) at the same instant must be
    // rejected — this is the assertion that would NOT have existed (and
    // would trivially pass as "allowed" pre-fix, since there was no gate
    // at all before task #608).
    let rejected = session.check_post_auth_rate_limit(now_ns);
    assert!(
        rejected.is_some(),
        "request beyond burst budget at the same instant must be rate-limited"
    );
    assert!(rejected.unwrap() >= 1, "retry_after_secs must be >= 1");
}

#[test]
fn refill_after_one_second_allows_further_requests() {
    let now_ns = 1_000_000_000u64;
    let session = fresh_session(now_ns);

    // Drain the full burst budget.
    for _ in 0..POST_AUTH_RATE_LIMIT_PER_SEC {
        assert!(session.check_post_auth_rate_limit(now_ns).is_none());
    }
    assert!(session.check_post_auth_rate_limit(now_ns).is_some());

    // Advance the clock by exactly 1 second: the bucket refills by
    // `rate` tokens, so at least one more request must be allowed.
    let one_sec_later = now_ns + 1_000_000_000;
    assert!(
        session.check_post_auth_rate_limit(one_sec_later).is_none(),
        "request 1s after full drain should be allowed by refill"
    );
}

#[test]
fn single_request_on_fresh_session_is_always_allowed() {
    let now_ns = 42;
    let session = fresh_session(now_ns);
    assert!(session.check_post_auth_rate_limit(now_ns).is_none());
}

/// #1090: `PostAuthBucket` migrated from `std::sync::Mutex` to two
/// independent atomics (`micro_tokens` via a `fetch_update` CAS retry loop,
/// `last_refill_at_ns` via `fetch_max`). The property that actually matters
/// for security — no concurrent caller can double-spend a token — must hold
/// even when many threads hammer the SAME session at the SAME instant. A
/// burst-sized bucket under `N > burst` concurrent callers at one instant
/// must admit EXACTLY `burst` of them, never more (a naive non-atomic
/// refactor of the old lock-guarded read-modify-write would admit more
/// under a lost-update race).
///
/// Uses a `Barrier` (found necessary by @oh review, 2026-08-11: bare
/// `thread::spawn` in a loop lets spawn overhead dominate the sub-microsecond
/// checked body, so threads mostly serialize and the assertion can pass
/// vacuously even against a genuinely broken, non-atomic implementation) so
/// every thread reaches `check_post_auth_rate_limit` as close to
/// simultaneously as the OS scheduler allows.
#[test]
fn concurrent_callers_never_admit_more_than_the_burst_budget() {
    use std::sync::{Arc, Barrier};
    use std::thread;

    let now_ns = 1_000_000_000u64;
    let session = Arc::new(fresh_session(now_ns));
    let burst = POST_AUTH_RATE_LIMIT_PER_SEC as usize;
    // Deliberately over-subscribe: 4x the burst budget, all racing at the
    // exact same `now_ns` so none of them get a refill edge to exploit.
    let concurrent_callers = burst * 4;
    let barrier = Arc::new(Barrier::new(concurrent_callers));

    let handles: Vec<_> = (0..concurrent_callers)
        .map(|_| {
            let session = Arc::clone(&session);
            let barrier = Arc::clone(&barrier);
            thread::spawn(move || {
                barrier.wait();
                session.check_post_auth_rate_limit(now_ns).is_none()
            })
        })
        .collect();

    let admitted = handles
        .into_iter()
        .map(|h| h.join().expect("thread must not panic"))
        .filter(|&was_admitted| was_admitted)
        .count();

    assert_eq!(
        admitted, burst,
        "exactly `burst` concurrent callers must be admitted, never more (double-spend) \
         or fewer (a lost update that drops an already-earned admission)"
    );
}

/// #1090 regression (found by @oh review, 2026-08-11): an earlier version of
/// the lock-free migration used a plain `swap` on `last_refill_at_ns`, which
/// lets the stored watermark REGRESS when an "older" `now_ns` call is
/// processed after a "newer" one (e.g. a thread preempted between reading
/// the wall clock and reaching the atomic op) — the regressed watermark then
/// lets a LATER call re-credit an already-credited wall-clock interval, an
/// unbounded over-refill with no aggregate cap. `fetch_max` closes this: the
/// stored watermark never regresses, so a stale/out-of-order `now_ns` credits
/// ZERO tokens (its `elapsed` computes as `now_ns.saturating_sub(newer value)
/// == 0`) instead of re-crediting.
#[test]
fn out_of_order_now_ns_credits_no_extra_tokens() {
    let t0 = 1_000_000_000u64;
    let session = fresh_session(t0);
    let burst = POST_AUTH_RATE_LIMIT_PER_SEC as usize;

    // Fully drain the burst budget at `t0`.
    for _ in 0..burst {
        assert!(session.check_post_auth_rate_limit(t0).is_none());
    }
    assert!(session.check_post_auth_rate_limit(t0).is_some());

    // A "fast" concurrent caller reaches the atomic op first with a MUCH
    // later timestamp (simulating a large, genuine wall-clock gap) --
    // refills the bucket far past capacity (clamped), then drains the full
    // burst right back down to empty again.
    let t_later = t0 + 10_000_000_000; // +10s
    for _ in 0..burst {
        assert!(session.check_post_auth_rate_limit(t_later).is_none());
    }
    assert!(session.check_post_auth_rate_limit(t_later).is_some());

    // A "slow" concurrent caller's `now_ns` (`t0`, OLDER than the watermark
    // `fetch_max` already advanced to `t_later`) arrives out of order. With
    // `fetch_max`, the stored watermark never regresses, so `elapsed =
    // t0.saturating_sub(t_later) == 0` for this stale call -- ZERO refill,
    // and the bucket is still empty from the drain above, so it must be
    // rejected. A `swap`-based implementation would instead let this call
    // overwrite the watermark back down to `t0`, and the run wouldn't even
    // need to complete for the hazard to be real: ANY subsequent call
    // computing `elapsed` against a regressed `t0` re-credits the interval
    // the fast caller's drain already consumed.
    assert!(
        session.check_post_auth_rate_limit(t0).is_some(),
        "an out-of-order (stale) now_ns must not re-credit any tokens once the \
         watermark has already advanced past it"
    );
}
