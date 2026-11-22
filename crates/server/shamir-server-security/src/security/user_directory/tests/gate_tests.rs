//! Internal unit tests for [`ScramDerivationGate`]
//! (`crate::user_directory::scram_gate`).
//!
//! These need the `pub(super)` visibility into `ScramDerivationGate::run`
//! (the generic closure seam that lets a test inject a derivation error
//! or a panic and prove RAII permit release). Coverage:
//!   - capacity-1 contention: a held derivation causes a second call to
//!     get the typed `Busy` refusal (counter +1, closure never entered),
//!     and a later call succeeds after release;
//!   - the KDF outcome is passed through untouched by `run` (including
//!     an injected error — the gate wraps it, it does not swallow it);
//!   - permit release after a panicking closure (RAII drop on unwind),
//!     with no extra busy count;
//!   - end-to-end `derive_scram_record_gated` success with a small real
//!     KDF, and the fixed safe `Busy` Display text.
//!
//! Every await is wrapped in the 120 s watchdog timeout and every
//! channel receive is bounded — a hang fails the test loudly instead of
//! wedging the suite (layout rules: ≥120 s, Windows runner up to 10x
//! slower; never sleep-based synchronization).
//!
//! The semaphore's own clock-failure → `None` → busy mapping is proven
//! in `shamir_connect::server::tests::security::argon2_semaphore_tests`;
//! the gate maps every `None` to `Busy`, so no duplicate is needed here.

use std::sync::mpsc;
use std::sync::Arc;
use std::time::Duration;

use shamir_connect::common::crypto::StoredKey;
use shamir_connect::common::kdf_params::KdfParams;
use shamir_connect::server::argon2_semaphore::Argon2Semaphore;
use shamir_connect::server::user_record::UserRecord;
use zeroize::Zeroizing;

use crate::user_directory::{derive_scram_record_gated, ScramDerivationError, ScramDerivationGate};

/// Generous per-await watchdog: a hang detector, never a sleep-based
/// synchronization.
const WATCHDOG: Duration = Duration::from_secs(120);

/// Minimum legal KDF params (OWASP floor) — small but real for the
/// end-to-end derivation test.
fn small_kdf() -> KdfParams {
    KdfParams {
        memory_kb: 19_456,
        time: 2,
        parallelism: 1,
        argon2_version: 0x13,
    }
}

/// Arbitrary non-secret record payload for gate-closure injections.
fn dummy_record() -> UserRecord {
    UserRecord {
        salt: [0u8; 16],
        stored_key: StoredKey([0u8; 32]),
        server_key: Zeroizing::new([0u8; 32]),
        kdf_params: small_kdf(),
        tickets_invalid_before_ns: 0,
    }
}

#[tokio::test]
async fn capacity_one_contention_refusal_then_recovery() {
    let gate = Arc::new(ScramDerivationGate::new(Arc::new(
        Argon2Semaphore::with_capacity(1),
    )));

    // First call: hold the permit until the test says so.
    let (hold_tx, hold_rx) = mpsc::channel::<()>();
    let (started_tx, started_rx) = mpsc::channel::<()>();
    let g1 = Arc::clone(&gate);
    let holder = tokio::task::spawn_blocking(move || {
        g1.run(|| {
            let _ = started_tx.send(());
            // Deterministically block (bounded channel receive, no
            // sleeps) while holding the permit.
            let _ = hold_rx.recv_timeout(WATCHDOG);
            dummy_record()
        })
    });

    let started = tokio::task::spawn_blocking(move || started_rx.recv_timeout(WATCHDOG));
    tokio::time::timeout(WATCHDOG, started)
        .await
        .expect("holder-start watchdog")
        .expect("watchdog join")
        .expect("holder started");

    // Second call while the permit is held: typed Busy refusal; the KDF
    // closure is provably never entered (unreachable arm).
    let g2 = Arc::clone(&gate);
    let refused_task = tokio::task::spawn_blocking(move || {
        g2.run(|| unreachable!("KDF closure must not run when refused"))
    });
    let refused = tokio::time::timeout(WATCHDOG, refused_task)
        .await
        .expect("refusal watchdog")
        .expect("watchdog join");

    assert!(matches!(refused, Err(ScramDerivationError::Busy)));
    assert_eq!(gate.busy_refusals(), 1);

    // Release the first permit; a third call then succeeds.
    hold_tx.send(()).expect("holder still alive");
    tokio::time::timeout(WATCHDOG, holder)
        .await
        .expect("holder watchdog")
        .expect("watchdog join")
        .expect("holder result");

    let g3 = Arc::clone(&gate);
    let recovered_task = tokio::task::spawn_blocking(move || g3.run(dummy_record));
    let recovered = tokio::time::timeout(WATCHDOG, recovered_task)
        .await
        .expect("recovery watchdog")
        .expect("watchdog join");

    assert!(recovered.is_ok());
    // Capacity fully restored after success; refusal counter unchanged.
    assert_eq!(
        gate.semaphore().available(),
        i64::from(gate.semaphore().capacity())
    );
    assert_eq!(gate.busy_refusals(), 1);
}

#[tokio::test]
async fn run_passes_kdf_outcome_through_untouched() {
    let gate = ScramDerivationGate::new(Arc::new(Argon2Semaphore::with_capacity(2)));

    // The gate is generic over the KDF outcome: an injected error comes
    // back inside `Ok` exactly as the closure produced it.
    let outcome = gate.run(|| Err::<(), String>("kdf boom".to_string()));
    match outcome {
        Ok(Err(msg)) => assert_eq!(msg, "kdf boom"),
        other => panic!("expected Ok(Err(kdf boom)), got {other:?}"),
    }
    assert_eq!(gate.semaphore().available(), 2);
    assert_eq!(gate.busy_refusals(), 0);
    // And a success value is handed back verbatim.
    let success = gate.run(|| 41u8 + 1);
    assert!(matches!(success, Ok(42u8)));
}

#[tokio::test]
async fn permit_released_after_panicking_closure() {
    let gate = Arc::new(ScramDerivationGate::new(Arc::new(
        Argon2Semaphore::with_capacity(1),
    )));

    // The panic propagates out of `run`; the RAII permit guard drops
    // during the unwind and releases the permit — that is the property
    // under test, so the panic itself is caught here.
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let _ = gate.run(|| panic!("kdf panic injection"));
    }));
    assert!(result.is_err(), "closure should have panicked");
    // The unwind is a panic release, not a busy refusal.
    assert_eq!(gate.busy_refusals(), 0);

    // A subsequent successful run proves the permit was released
    // (capacity 1: it could only be acquired if the unwind released it).
    assert!(gate.run(dummy_record).is_ok());
    assert_eq!(gate.semaphore().available(), 1);
    assert_eq!(gate.busy_refusals(), 0);
}

#[tokio::test]
async fn gated_end_to_end_small_kdf_success() {
    let gate = Arc::new(ScramDerivationGate::new(Arc::new(
        Argon2Semaphore::with_capacity(1),
    )));

    let record = tokio::time::timeout(
        WATCHDOG,
        derive_scram_record_gated(
            "correct horse battery staple".to_string(),
            small_kdf(),
            Arc::clone(&gate),
        ),
    )
    .await
    .expect("derivation watchdog")
    .expect("gated derivation succeeds");

    assert_eq!(record.kdf_params, small_kdf());
    assert_eq!(record.salt.len(), 16);
    assert_eq!(gate.semaphore().available(), 1);
    assert_eq!(gate.busy_refusals(), 0);
}

#[test]
fn busy_display_is_fixed_safe_text() {
    let text = ScramDerivationError::Busy.to_string();
    assert_eq!(text, "password derivation is busy; retry later");
}
