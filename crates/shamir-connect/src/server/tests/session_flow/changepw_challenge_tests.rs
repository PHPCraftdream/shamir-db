//! Regression tests for the lock-free `pending_changepw_challenge` slot
//! (`Session::pending_changepw_challenge: ArcSwapOption<PendingChangePwChallenge>`).
//!
//! Invariant under test (spec §12.5 double-submit guard): consume is a single
//! atomic `swap(None)` so exactly one concurrent caller observes a non-empty
//! slot. The prior `parking_lot::Mutex<Option<_>>` enforced this with
//! `lock().take()`; the migration to `ArcSwapOption` MUST preserve it (a naive
//! `load` + `store(None)` would reintroduce a TOCTOU race where two callers
//! each see `Some` before either clears it).

use crate::client::changepw::build_request;
use crate::common::crypto::{Ed25519Keypair, StoredKey};
use crate::common::error::EnvironmentError;
use crate::common::error::Error;
use crate::common::kdf_params::KdfParams;
use crate::common::time::UnixNanos;
use crate::common::types::limits;
use crate::common::types::{BindingMode, TransportKind};
use crate::common::username::NormalizedUsername;
use crate::server::auth::changepw::{
    start_change_password_challenge, verify_change_password_request_with_sid, ChangePwRequest,
};
use crate::server::config::ServerSecrets;
use crate::server::session::{PendingChangePwChallenge, Session, SessionPermissions};
use std::sync::Arc;
use std::time::{Duration, UNIX_EPOCH};
use zeroize::Zeroize;

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
fn bootstrap_entropy_failure_does_not_publish_token() {
    let state = crate::server::bootstrap::BootstrapState::empty();
    let result = crate::common::crypto::with_entropy_failure(|| state.issue_token(100, 10));
    assert!(matches!(
        result,
        Err(Error::Environment(
            crate::common::error::EnvironmentError::Entropy
        ))
    ));
    assert!(state.is_bootstrap_allowed());
    assert_eq!(state.token_expires_at_ns(), None);
    assert!(state.issue_token(100, 10).is_ok());
}

#[test]
fn local_bootstrap_wipes_password_before_hello_and_verification_errors() {
    let key = Ed25519Keypair::from_seed(&[4; 32]);
    for entropy_failure in [true, false] {
        let mut password = b"new even better password".to_vec();
        let mut operation = || {
            crate::client::bootstrap::run_local_bootstrap_with(
                &key,
                &[0; 32],
                TransportKind::Tcp,
                &[0; 32],
                [1; 32],
                NormalizedUsername::from_raw("alice").unwrap(),
                &mut password,
                KdfParams::DEFAULT,
                0,
            )
        };
        let result = if entropy_failure {
            crate::common::crypto::with_entropy_failure(operation)
        } else {
            operation()
        };
        assert!(result.is_err());
        assert!(password.iter().all(|b| *b == 0));
    }
}

#[test]
fn bootstrap_password_wiped_on_request_exits() {
    let kdf = KdfParams {
        memory_kb: 19_456,
        time: 2,
        parallelism: 1,
        argon2_version: 0x13,
    };
    for case in 0..4 {
        let mut password = if case == 0 {
            b"short".to_vec()
        } else {
            b"new even better password".to_vec()
        };
        let mut params = kdf;
        if case == 1 {
            params.memory_kb = 0;
        }
        let mut operation = || {
            crate::client::bootstrap::build_request(
                [1; 32],
                NormalizedUsername::from_raw("alice").unwrap(),
                &mut password,
                params,
            )
        };
        let result = if case == 2 {
            crate::common::crypto::with_entropy_failure(operation)
        } else {
            operation()
        };
        if case == 3 {
            assert!(result.is_ok());
        } else {
            assert!(result.is_err());
        }
        assert!(password.iter().all(|b| *b == 0), "case {case}");
    }
}

#[test]
fn password_change_wipes_both_buffers_on_all_paths() {
    let username = NormalizedUsername::from_raw("alice").unwrap();
    let kdf = KdfParams {
        memory_kb: 19_456,
        time: 2,
        parallelism: 1,
        argon2_version: 0x13,
    };
    for case in 0..5 {
        let mut old = b"old correct password".to_vec();
        let mut new = if case == 0 {
            b"short".to_vec()
        } else {
            b"new even better password".to_vec()
        };
        let mut recommended = kdf;
        if case == 1 {
            recommended.memory_kb = 0;
        }
        let nonce = if case == 2 { [0; 32] } else { [1; 32] };
        let mut operation = || {
            build_request(
                &username,
                &[9; 32],
                &nonce,
                &[2; 32],
                &[3; 16],
                kdf,
                TransportKind::Tcp,
                BindingMode::None,
                &[0; 32],
                &mut old,
                &mut new,
                recommended,
            )
        };
        let result = if case == 3 {
            crate::common::crypto::with_entropy_failure(operation)
        } else {
            operation()
        };
        if case == 4 {
            assert!(result.is_ok());
        } else {
            assert!(result.is_err(), "case {case}");
        }
        assert!(old.iter().all(|b| *b == 0), "old case {case}");
        assert!(new.iter().all(|b| *b == 0), "new case {case}");
    }
}

#[test]
fn checked_clock_rejects_pre_epoch_and_overflow() {
    assert!(matches!(
        UnixNanos::try_from_system_time(UNIX_EPOCH - Duration::from_nanos(1)),
        Err(EnvironmentError::ClockBeforeEpoch)
    ));
    assert!(matches!(
        UnixNanos::try_from_system_time(
            UNIX_EPOCH + Duration::from_secs(u64::MAX / 1_000_000_000 + 1)
        ),
        Err(EnvironmentError::ClockOverflow)
    ));
}

#[test]
fn server_secrets_clone_redaction_and_zeroize() {
    let mut secrets = ServerSecrets {
        server_secret: [17; 32],
        lockout_secret: [29; 32],
    };
    let copy = secrets.clone();
    let debug = format!("{secrets:?}");
    assert!(debug.contains("REDACTED"));
    assert!(!debug.contains("17"));
    assert!(!debug.contains("29"));
    secrets.zeroize();
    assert_eq!(secrets.server_secret, [0; 32]);
    assert_eq!(secrets.lockout_secret, [0; 32]);
    assert_eq!(copy.server_secret, [17; 32]);
    assert_eq!(copy.lockout_secret, [29; 32]);
}

#[test]
fn entropy_failure_preserves_existing_challenge() {
    let session = fresh_session();
    session
        .pending_changepw_challenge
        .store(Some(Arc::new(pending())));
    let result = crate::common::crypto::with_entropy_failure(|| {
        start_change_password_challenge(&session, [1; 16], KdfParams::DEFAULT, [2; 32], 2001)
    });
    assert!(matches!(
        result,
        Err(Error::Environment(
            crate::common::error::EnvironmentError::Entropy
        ))
    ));
    assert_eq!(
        session
            .pending_changepw_challenge
            .load()
            .as_ref()
            .unwrap()
            .server_nonce_cp,
        [0xAA; 32]
    );
}

#[test]
fn zero_nonce_preserves_existing_challenge() {
    let session = fresh_session();
    let first =
        start_change_password_challenge(&session, [1; 16], KdfParams::DEFAULT, [2; 32], 2000)
            .unwrap();
    assert!(matches!(
        start_change_password_challenge(&session, [1; 16], KdfParams::DEFAULT, [0; 32], 2001),
        Err(Error::InvalidInput("client_nonce_cp all-zero"))
    ));
    assert_eq!(
        session
            .pending_changepw_challenge
            .load()
            .as_ref()
            .unwrap()
            .server_nonce_cp,
        first.server_nonce_cp
    );
    let next =
        start_change_password_challenge(&session, [1; 16], KdfParams::DEFAULT, [3; 32], 2002)
            .unwrap();
    assert_eq!(
        session
            .pending_changepw_challenge
            .load()
            .as_ref()
            .unwrap()
            .server_nonce_cp,
        next.server_nonce_cp
    );
}

#[test]
fn mismatched_session_id_consumes_challenge() {
    let session = fresh_session();
    session
        .pending_changepw_challenge
        .store(Some(Arc::new(pending())));
    let result = verify_change_password_request_with_sid(
        &session,
        &[1; 32],
        [1; 16],
        &StoredKey([2; 32]),
        KdfParams::DEFAULT,
        &ChangePwRequest {
            client_proof_old: [0; 32],
            new_salt: [0; 16],
            new_stored_key: [0; 32],
            new_server_key: [0; 32],
        },
        KdfParams::DEFAULT,
        2000,
    );
    assert!(matches!(result, Err(Error::AuthFailed)));
    assert!(session.pending_changepw_challenge.load().is_none());
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

/// RV-CONNECT-C9066D95: a backwards clock (`now_ns < issued_at_ns`) must be
/// rejected as `AuthFailed`, never wrap through the unsigned TTL subtraction
/// into a huge "elapsed" value (a debug-build overflow panic pre-fix).
#[test]
fn rv_connect_c9066d95_backwards_clock_rejected_without_underflow() {
    let s = fresh_session();
    let pending = PendingChangePwChallenge {
        server_nonce_cp: [0xAA; 32],
        client_nonce_cp: [0xBB; 32],
        issued_at_ns: 2_000, // issued "in the future" relative to now
    };
    s.pending_changepw_challenge.store(Some(Arc::new(pending)));

    let err = verify_change_password_request_with_sid(
        &s,
        &[0u8; limits::SESSION_ID_BYTES],
        [0u8; limits::SALT_BYTES],
        &StoredKey([0u8; 32]),
        KdfParams::DEFAULT,
        &ChangePwRequest {
            client_proof_old: [0u8; 32],
            new_salt: [0u8; limits::SALT_BYTES],
            new_stored_key: [0u8; 32],
            new_server_key: [0u8; 32],
        },
        KdfParams::DEFAULT,
        1_000, // now BEFORE issued_at
    );
    assert!(
        matches!(err, Err(Error::AuthFailed)),
        "reversed timestamps must be rejected without underflow"
    );
}
