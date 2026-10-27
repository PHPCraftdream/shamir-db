//! Regression tests for `ServerIdentityState` rotation CAS
//! (RV-CONNECT-F8A5E821): the overlap check and the snapshot publish must be
//! one atomic transition on the `ArcSwap`, and the version mirror must stay
//! in sync with the published snapshot.

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

/// After the overlap window expires, `try_finalize` clears the overlap
/// atomically and rotation becomes possible again; versions advance 0 -> 1
/// -> 2 with no lost updates relative to the mirror.
#[test]
fn rv_connect_f8a5e821_finalize_then_rotate_keeps_versions_coherent() {
    let state = ServerIdentityState::fresh();
    let overlap_end = 1_000 + crate::server::rotation::ROTATION_OVERLAP_NS;

    assert!(state.rotate(1_000).is_ok());
    assert_eq!(state.current_version(), 1);
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
    assert_eq!(state.current_version(), 2, "mirror must match the snapshot");
}

/// Version mirror must never regress (RV-CONNECT-F8A5E821): an older
/// rotation finishing last (stale publication) must not overwrite a newer
/// version in the lock-free mirror. Calls the (crate-private)
/// `publish_version` helper with the OLDER outcome's version and asserts the
/// mirror still reports the snapshot's version. With the pre-fix `store`
/// implementation this test fails (mirror regresses to 1); with
/// `fetch_max` it passes.
#[test]
fn rv_connect_f8a5e821_version_mirror_never_regresses() {
    let state = ServerIdentityState::fresh();
    let overlap_end = 1_000 + crate::server::rotation::ROTATION_OVERLAP_NS;

    let first = state.rotate(1_000).expect("first rotation");
    assert_eq!(first.new_version, 1);
    assert!(
        state.try_finalize(overlap_end),
        "finalize must clear overlap"
    );
    let second = state
        .rotate(overlap_end + 1)
        .expect("second rotation after finalize");
    assert_eq!(second.new_version, 2);
    assert_eq!(state.current_version(), 2);

    // Simulate the stale publication: the older rotation's publish lands
    // after the newer one. The mirror must stay at the snapshot's version.
    state.publish_version(first.new_version);
    assert_eq!(
        state.current_version(),
        second.new_version,
        "stale publication must not regress the version mirror"
    );
}

/// Observable requirement for the rejected-flag fix (RV-CONNECT-F8A5E821):
/// after a rotation is rejected during overlap, a rotation after the overlap
/// window must succeed, publish version 2, and keep the mirror in sync. The
/// underlying property — the per-attempt `rejected` flag being reset on a
/// retried winning attempt — only manifests under a concurrent retry
/// schedule inside `rcu` and is not deterministically testable here; this
/// test pins the externally observable behavior.
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
    assert_eq!(state.current_version(), 2, "mirror must equal 2");
}
