//! Regression tests for `ServerIdentityState` rotation CAS
//! (RV-CONNECT-F8A5E821): overlap checks and ticket-version acceptance use
//! the authoritative ArcSwap snapshot.

use crate::common::crypto::with_entropy_failure;
use crate::server::rotation::ServerIdentityState;
use std::sync::Arc;
use std::sync::Barrier;

/// N concurrent `rotate` calls at the same instant: exactly one may win.
/// The pre-fix code was a load-clone-check-store sequence — two threads that
/// both loaded the pre-rotation snapshot each published their own new
/// snapshot unconditionally, so BOTH reported success and the version
/// counter lost an increment.
#[test]
fn rv_connect_f8a5e821_rotate_wins_exactly_once_under_contention() {
    let state = Arc::new(ServerIdentityState::fresh());
    let n = 8;
    let barrier = Arc::new(Barrier::new(n));

    let mut handles = Vec::new();
    for _ in 0..n {
        let barrier = Arc::clone(&barrier);
        let state = Arc::clone(&state);
        handles.push(std::thread::spawn(move || {
            barrier.wait();
            state.rotate(1_000)
        }));
    }
    let mut wins = 0;
    for h in handles {
        if h.join().expect("rotate thread must not panic").is_ok() {
            wins += 1;
        }
    }
    assert_eq!(wins, 1, "exactly one concurrent rotation may win");
    assert_eq!(
        state.current_version(),
        1,
        "version must advance by exactly 1"
    );
    assert!(
        state.rotate(2_000).is_err(),
        "rotation during overlap is rejected"
    );
}

/// After rotation and finalization, ticket acceptance is derived from the
/// current authoritative snapshot at each point in the transition.
#[test]
fn rv_connect_f8a5e821_finalize_then_rotate_keeps_versions_coherent() {
    let state = ServerIdentityState::fresh();
    let overlap_end = 1_000 + crate::server::rotation::ROTATION_OVERLAP_NS;

    assert!(state.rotate(1_000).is_ok());
    assert_eq!(state.current_version(), 1);
    assert!(state.is_ticket_version_acceptable(1));
    assert!(!state.is_ticket_version_acceptable(0));
    assert!(
        state.try_finalize(overlap_end),
        "finalize must clear overlap"
    );
    assert!(
        !state.try_finalize(overlap_end),
        "second finalize is a no-op"
    );

    let outcome = state
        .rotate(overlap_end + 1)
        .expect("rotation after finalize");
    assert_eq!(outcome.new_version, 2);
    assert_eq!(state.current_version(), 2);
    assert!(!state.is_ticket_version_acceptable(1));
    assert!(state.is_ticket_version_acceptable(2));
}

/// Repeated snapshot reads must never accept an older version after a
/// successful rotation; acceptance follows the committed snapshot directly.
#[test]
fn rv_connect_f8a5e821_ticket_acceptance_tracks_authoritative_snapshot() {
    let state = ServerIdentityState::fresh();
    let overlap_end = 1_000 + crate::server::rotation::ROTATION_OVERLAP_NS;

    let first = state.rotate(1_000).expect("first rotation");
    assert_eq!(first.new_version, 1);
    assert!(state.is_ticket_version_acceptable(1));
    assert!(!state.is_ticket_version_acceptable(0));
    assert!(
        state.try_finalize(overlap_end),
        "finalize must clear overlap"
    );
    let second = state
        .rotate(overlap_end + 1)
        .expect("second rotation after finalize");
    assert_eq!(second.new_version, 2);
    assert!(state.is_ticket_version_acceptable(2));
    assert!(!state.is_ticket_version_acceptable(1));
    assert_eq!(state.current_version(), 2);
}

/// Observable requirement for the rejected-flag fix (RV-CONNECT-F8A5E821):
/// after a rotation is rejected during overlap, a rotation after the overlap
/// window must succeed, publish version 2, and ticket acceptance follows the
/// authoritative snapshot.
#[test]
fn rv_connect_f8a5e821_rejection_then_successful_rotation_returns_ok() {
    let state = ServerIdentityState::fresh();
    let overlap_end = 1_000 + crate::server::rotation::ROTATION_OVERLAP_NS;

    assert!(state.rotate(1_000).is_ok());
    let err = state
        .rotate(1_500)
        .expect_err("rotation during overlap must be rejected");
    assert!(matches!(err, crate::common::error::Error::InvalidInput(_)));

    assert!(state.try_finalize(overlap_end), "finalize clears overlap");
    let outcome = state
        .rotate(overlap_end + 1)
        .expect("rotation after rejection + finalize must succeed");
    assert_eq!(outcome.new_version, 2);
    assert_eq!(state.current_version(), 2);
    assert!(!state.is_ticket_version_acceptable(1));
    assert!(state.is_ticket_version_acceptable(2));
}

/// RV-CONNECT-387EAA05 (P2-10 second tranche): under a failing entropy
/// source `rotate` must return `Error::Environment` and leave keys,
/// `rotation_until_ns` and the version counter unchanged — no zero-entropy
/// keypair, no partial publication. On the pre-fix code the test fails:
/// `Ed25519Keypair::generate` panicked on the OS RNG failure.
#[test]
fn rv_connect_387eaa05_rotate_entropy_failure_returns_environment_and_keeps_identity() {
    let state = ServerIdentityState::fresh();
    let pub_before = state.current_pub();
    let version_before = state.current_version();
    let rotation_until_before = state.rotation_until_ns();

    let result = with_entropy_failure(|| state.rotate(1_000));
    assert!(
        matches!(result, Err(crate::common::error::Error::Environment(_))),
        "entropy failure must surface as Error::Environment, got {result:?}"
    );

    assert_eq!(state.current_pub(), pub_before, "current key unchanged");
    assert_eq!(state.previous_pub(), None, "no previous key published");
    assert_eq!(state.current_version(), version_before, "version unchanged");
    assert_eq!(
        state.rotation_until_ns(),
        rotation_until_before,
        "rotation_until unchanged"
    );

    // A later rotation with working entropy succeeds normally.
    let outcome = state
        .rotate(2_000)
        .expect("rotation after entropy recovers");
    assert_eq!(outcome.new_version, 1);
    assert_eq!(state.current_version(), 1);
}
