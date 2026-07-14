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
