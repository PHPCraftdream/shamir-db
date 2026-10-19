//! Regression tests for the lock-free `pending_changepw_challenge` slot
//! (`Session::pending_changepw_challenge: ArcSwapOption<PendingChangePwChallenge>`).
//!
//! Invariant under test (spec §12.5 double-submit guard): consume is a single
//! atomic `swap(None)` so exactly one concurrent caller observes a non-empty
//! slot. The prior `parking_lot::Mutex<Option<_>>` enforced this with
//! `lock().take()`; the migration to `ArcSwapOption` MUST preserve it (a naive
//! `load` + `store(None)` would reintroduce a TOCTOU race where two callers
//! each see `Some` before either clears it).

use crate::common::types::{BindingMode, TransportKind};
use crate::server::session::{PendingChangePwChallenge, Session, SessionPermissions};
use std::sync::Arc;

fn fresh_session() -> Session {
    Session::new(
        [0u8; 16],
        "alice".into(),
        SessionPermissions::from_roles(vec!["read_write".into()]),
        TransportKind::Tcp,
        BindingMode::None,
        [0u8; 32],
        1_000,
    )
}

fn pending() -> PendingChangePwChallenge {
    PendingChangePwChallenge {
        server_nonce_cp: [0xAA; 32],
        client_nonce_cp: [0xBB; 32],
        issued_at_ns: 2_000,
    }
}

#[test]
fn consume_after_issue_yields_challenge_then_none() {
    // Sequential baseline: single-in-flight semantics.
    let s = fresh_session();
    assert!(
        s.pending_changepw_challenge.load().is_none(),
        "freshly-constructed session must have an empty challenge slot"
    );

    s.pending_changepw_challenge
        .store(Some(Arc::new(pending())));

    let first = s.pending_changepw_challenge.swap(None);
    assert!(
        first.as_deref().is_some(),
        "first consume after issue MUST observe the challenge"
    );

    let second = s.pending_changepw_challenge.swap(None);
    assert!(
        second.as_deref().is_none(),
        "second consume MUST be empty (single-use, §12.5)"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_consume_yields_exactly_one_winner() {
    // The §12.5 double-submit guard: two consumers race on the same issued
    // challenge; exactly one gets `Some`, the other `None`. Repeated many
    // times to surface a flaky TOCTOU (a load-then-store regression would
    // fail here with non-zero probability under contention). `spawn_blocking`
    // mirrors the concurrent session-dispatch shape (SCRAM proof runs on the
    // blocking pool for CPU-bound work).
    for _ in 0..200 {
        let s = Arc::new(fresh_session());
        s.pending_changepw_challenge
            .store(Some(Arc::new(pending())));

        let s1 = Arc::clone(&s);
        let s2 = Arc::clone(&s);
        let (a, b) = tokio::join!(
            tokio::task::spawn_blocking(move || {
                s1.pending_changepw_challenge
                    .swap(None)
                    .as_deref()
                    .is_some()
            }),
            tokio::task::spawn_blocking(move || {
                s2.pending_changepw_challenge
                    .swap(None)
                    .as_deref()
                    .is_some()
            })
        );
        let (a, b) = (a.unwrap(), b.unwrap());

        assert!(
            a ^ b,
            "exactly one of two concurrent consumes MUST win (got a={a}, b={b}); \
             a non-atomic load+store would let both observe Some"
        );
    }
}

#[test]
fn reissue_overwrites_prior_challenge() {
    // Multi-tab semantics (§12.5): a second `changePasswordChallenge`
    // overwrites the previous pending state. The old Arc is simply dropped
    // (refcount → 0) — no leak, no double-consume window.
    let s = fresh_session();
    s.pending_changepw_challenge
        .store(Some(Arc::new(pending())));
    s.pending_changepw_challenge
        .store(Some(Arc::new(pending())));

    let taken = s.pending_changepw_challenge.swap(None);
    assert!(
        taken.as_deref().is_some(),
        "reissue keeps the slot occupied"
    );
    // Still single-use after reissue.
    let second = s.pending_changepw_challenge.swap(None);
    assert!(second.as_deref().is_none());
}
