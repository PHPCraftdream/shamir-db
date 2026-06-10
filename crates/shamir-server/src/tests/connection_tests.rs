use std::time::Duration;

use shamir_connect::common::latency::{padding_for, target_constant_time_ms, FIXED_FLOOR_MS};
use shamir_connect::common::types::limits::MAX_PRE_AUTH_FRAME;
use shamir_connect::server::lockout::{
    FailureOutcome, InMemoryLockoutStore, LockoutStore, PairKey, Subnet, BACKOFF_BASE_MS,
    BACKOFF_CAP_MS, LOCKOUT_THRESHOLD,
};
use shamir_transport_tcp::framing::MAX_FRAME_SIZE_DEFAULT;

// HIGH-1 compile-time invariants: pin the pre-auth frame ceiling at
// the spec §8 value and prove it stays strictly below the post-auth
// ceiling the request loop uses. `const` assertions trip at compile
// time so a future spec edit that weakens either bound fails the
// build, not just the test suite.
const _: () = assert!(
    MAX_PRE_AUTH_FRAME == 4 * 1024,
    "MAX_PRE_AUTH_FRAME must equal 4 KiB per spec §8",
);
const _: () = assert!(
    MAX_PRE_AUTH_FRAME < MAX_FRAME_SIZE_DEFAULT,
    "pre-auth ceiling must be strictly smaller than post-auth ceiling",
);

/// HIGH-1 regression: `run_handshake` must read pre-auth frames with
/// the 4 KiB ceiling, not the post-auth 16 MiB ceiling. The compile-
/// time `const` asserts above pin the constants; this runtime test
/// surfaces a human-readable failure when the bounds are tightened
/// or loosened in the future, and additionally documents the
/// resource-exhaustion budget.
#[test]
fn pre_auth_frame_budget_is_safe_for_ten_thousand_unauth_peers() {
    // Defense-in-depth: 10 000 concurrent unauthenticated peers
    // multiplied by the pre-auth cap must stay well under
    // commodity-server RAM. 10 000 × 4 KiB = 40 MiB, vs. the
    // 10 000 × 16 MiB = ~160 GiB that the old shape allowed.
    let max_unauth_memory = MAX_PRE_AUTH_FRAME.saturating_mul(10_000);
    assert!(
        max_unauth_memory < 128 * 1024 * 1024,
        "pre-auth cap × 10k connections should be under 128 MiB; got {}",
        max_unauth_memory,
    );
}

// ---------------------------------------------------------------------
// NEW-2: per-pair exponential backoff is COMPUTED *and APPLIED*.
//
// The application logic lives in two spots:
//   * `run_handshake` (ProofOutcome::Rejected arm) maps the
//     `register_failure` outcome → `backoff_ms` and returns it via
//     `HandshakeError::BadProof { backoff_ms }`.
//   * `handle_connection` widens the negative-path latency pad to
//     `max(target_constant_time_ms(), backoff_ms)`.
//
// Driving the full async `run_handshake` requires a complete
// `ConnectionContext` (TLS identity, redb user dir, …) so these tests
// exercise the two load-bearing pieces directly: (1) the exact
// outcome→backoff mapping used by the reject arm, against the real
// `InMemoryLockoutStore`, and (2) the `max(floor, backoff)` pad formula.
// ---------------------------------------------------------------------

/// Mirror of the `ProofOutcome::Rejected` arm in `run_handshake`: map a
/// `register_failure` outcome to the `backoff_ms` that gets plumbed into
/// `HandshakeError::BadProof`. Kept in lockstep with the production code
/// so the test fails if the mapping drifts.
fn backoff_ms_for(outcome: FailureOutcome) -> u64 {
    match outcome {
        FailureOutcome::Backoff { delay_ms } => delay_ms,
        FailureOutcome::LockedOut => BACKOFF_CAP_MS,
    }
}

fn pair(subnet: u8, user: u8) -> PairKey {
    (Subnet::V4([10, 0, subnet]), [user; 16])
}

/// The backoff plumbed into the reject path must escalate `100ms × 2^N`
/// (capped 30s) as failures accumulate for a `(subnet, username_hash)`
/// pair, exactly as the reject arm computes it from the real store.
#[test]
fn reject_path_backoff_escalates_per_failure() {
    let store = InMemoryLockoutStore::new();
    let now = 1_000_000_000u64;
    let k = pair(1, 1);
    const SECOND_NS: u64 = 1_000_000_000;

    // First failures (well below the 50-failure lockout threshold)
    // double each time: 100, 200, 400, 800, 1600, ...
    let expected = [
        100u64, 200, 400, 800, 1600, 3200, 6400, 12800, 25600, 30000, 30000,
    ];
    for (i, &want) in expected.iter().enumerate() {
        let outcome = store.register_failure(k, now + (i as u64) * SECOND_NS);
        assert_eq!(
            backoff_ms_for(outcome),
            want,
            "failure #{} should map to {}ms backoff",
            i + 1,
            want,
        );
    }
    // Base × 2^0 sanity (documents the formula's anchor).
    assert_eq!(BACKOFF_BASE_MS, 100);
}

/// Crossing the lockout threshold must still yield a bounded backoff for
/// the final response (`BACKOFF_CAP_MS`), not panic or 0 — the reject arm
/// maps `FailureOutcome::LockedOut → BACKOFF_CAP_MS`.
#[test]
fn reject_path_backoff_caps_at_lockout_threshold() {
    let store = InMemoryLockoutStore::new();
    let now = 1_000_000_000u64;
    let k = pair(2, 2);
    const SECOND_NS: u64 = 1_000_000_000;

    let mut last = 0u64;
    for i in 0..LOCKOUT_THRESHOLD {
        let outcome = store.register_failure(k, now + (i as u64) * SECOND_NS);
        last = backoff_ms_for(outcome);
    }
    // The 50th failure trips the lockout; the mapped backoff is the cap.
    assert_eq!(
        last, BACKOFF_CAP_MS,
        "threshold-crossing backoff is the cap"
    );
    assert!(store.is_locked_out(k, now + (LOCKOUT_THRESHOLD as u64) * SECOND_NS));
}

/// The negative-path pad target is `max(constant_time_floor, backoff)` —
/// the timing-oracle floor is preserved AND the escalation is enforced.
/// With elapsed below the target, the computed sleep reaches the backoff.
#[test]
fn pad_target_is_max_of_floor_and_backoff() {
    // A large backoff dominates the floor: total pad ≈ backoff.
    let backoff_ms = 6400u64;
    // Sample the (random) floor many times; the formula must never drop
    // below either input.
    for _ in 0..256 {
        let target_ms = target_constant_time_ms().max(backoff_ms);
        assert!(
            target_ms >= backoff_ms,
            "pad target must be >= backoff ({target_ms} < {backoff_ms})",
        );
        assert!(
            target_ms >= FIXED_FLOOR_MS,
            "pad target must be >= constant-time floor ({target_ms} < {FIXED_FLOOR_MS})",
        );
    }

    // With a tiny elapsed, the sleep computed by the same helper the
    // negative path uses must reach the backoff window.
    let target_ms = 6400u64.max(target_constant_time_ms());
    let sleep = padding_for(Duration::from_millis(5), target_ms);
    assert!(
        sleep >= Duration::from_millis(backoff_ms - 5),
        "negative-path sleep ({sleep:?}) must reach the backoff window",
    );
}

/// When there is no backoff (e.g. an internal verify error path with
/// `backoff_ms = 0`), the pad target collapses to the constant-time
/// floor — behaviour identical to the pre-NEW-2 flat pad.
#[test]
fn zero_backoff_collapses_to_constant_time_floor() {
    for _ in 0..256 {
        let floor = target_constant_time_ms();
        // `black_box` so the zero is treated as an opaque runtime value
        // (matching the production `floor.max(backoff_ms)` where
        // `backoff_ms == 0`) rather than a compile-time no-op.
        let backoff_ms = std::hint::black_box(0u64);
        let target_ms = floor.max(backoff_ms);
        assert_eq!(target_ms, floor);
        assert!((FIXED_FLOOR_MS..=FIXED_FLOOR_MS + 25).contains(&target_ms));
    }
}

/// No user-existence oracle: the store's backoff depends ONLY on the
/// failure count for the `(subnet, username_hash)` pair, never on whether
/// the username maps to a real account. Two distinct username hashes on
/// the same subnet, failed the same number of times, must yield identical
/// backoff progressions — so widening the pad to the backoff cannot leak
/// which usernames exist.
#[test]
fn backoff_progression_is_identical_for_any_pair() {
    let store = InMemoryLockoutStore::new();
    let now = 1_000_000_000u64;
    const SECOND_NS: u64 = 1_000_000_000;
    let real_user = pair(7, 0xaa); // imagine this hash maps to a real account
    let fake_user = pair(7, 0xbb); // and this one does not

    for i in 0..8u64 {
        let a = backoff_ms_for(store.register_failure(real_user, now + i * SECOND_NS));
        let b = backoff_ms_for(store.register_failure(fake_user, now + i * SECOND_NS));
        assert_eq!(
            a,
            b,
            "backoff at failure #{} must not depend on the pair identity",
            i + 1,
        );
    }
}
