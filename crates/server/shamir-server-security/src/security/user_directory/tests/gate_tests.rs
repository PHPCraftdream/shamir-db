//! Internal tests for [`ScramDerivationGate`] and bounded SCRAM admission.
//!
//! Every await and channel receive is bounded by the watchdog; synchronization
//! uses handshakes, never sleeps. Tokio 1.51.6's blocking pool pops queued jobs
//! with `VecDeque::pop_front`, which makes the post-cancel sentinel a completion
//! witness for the earlier queued gated job.

use std::future::Future;
use std::pin::Pin;
use std::sync::mpsc;
use std::sync::Arc;
use std::task::{Context, Poll, Waker};
use std::time::Duration;

use shamir_connect::common::crypto::StoredKey;
use shamir_connect::common::kdf_params::KdfParams;
use shamir_connect::server::argon2_semaphore::Argon2Semaphore;
use shamir_connect::server::user_record::UserRecord;
use zeroize::Zeroizing;

use crate::user_directory::{derive_scram_record_gated, ScramDerivationError, ScramDerivationGate};

/// Generous hang detector; it is not used as synchronization.
const WATCHDOG: Duration = Duration::from_secs(120);

/// Minimum legal KDF params — small but real for end-to-end derivation tests.
fn small_kdf() -> KdfParams {
    KdfParams {
        memory_kb: 19_456,
        time: 2,
        parallelism: 1,
        argon2_version: 0x13,
    }
}

/// Arbitrary non-secret record payload for injected gate closures.
fn dummy_record() -> UserRecord {
    UserRecord {
        salt: [0u8; 16],
        stored_key: StoredKey([0u8; 32]),
        server_key: Zeroizing::new([0u8; 32]),
        kdf_params: small_kdf(),
        tickets_invalid_before_ns: 0,
    }
}

#[test]
fn bounded_admission_refuses_before_blocking_submission_and_recovers() {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .max_blocking_threads(1)
        .enable_all()
        .build()
        .expect("build single-blocking-worker runtime");

    runtime.block_on(async {
        let gate = Arc::new(ScramDerivationGate::new(Arc::new(
            Argon2Semaphore::with_capacity(1),
        )));
        let held_permit = gate
            .semaphore()
            .try_acquire()
            .expect("hold the sole semaphore permit");

        // Keep the only blocking worker occupied: the public future must reject
        // synchronously at full admission rather than submit another job.
        let (blocker_started_tx, blocker_started_rx) = mpsc::sync_channel(1);
        let (release_blocker_tx, release_blocker_rx) = mpsc::sync_channel(1);
        let blocker = tokio::task::spawn_blocking(move || {
            blocker_started_tx
                .send(())
                .expect("test still observes blocker start");
            release_blocker_rx
                .recv_timeout(WATCHDOG)
                .expect("test releases blocking worker");
        });
        blocker_started_rx
            .recv_timeout(WATCHDOG)
            .expect("blocking worker reserved by handshake");

        // With capacity one, two admissions (2 × capacity) are allowed even
        // while the permit is held. The third is refused exactly once.
        let first = gate.admit().expect("first bounded admission");
        let second = gate.admit().expect("second bounded admission");
        let before_refusal = gate.busy_refusals();
        assert!(matches!(gate.admit(), Err(ScramDerivationError::Busy)));
        assert_eq!(gate.busy_refusals(), before_refusal + 1);

        // Releasing one ticket restores one admission slot.
        drop(first);
        let replacement = gate.admit().expect("ticket drop reopens admission");
        assert_eq!(gate.busy_refusals(), before_refusal + 1);

        // The gate is full again. Poll the actual public future exactly once:
        // it must already be Ready(Busy), not queued behind the held worker.
        let before_public_refusal = gate.busy_refusals();
        let mut future = Box::pin(derive_scram_record_gated(
            "correct horse battery staple".to_string(),
            small_kdf(),
            Arc::clone(&gate),
        ));
        let waker = Waker::noop();
        let mut context = Context::from_waker(waker);
        assert!(matches!(
            Future::poll(Pin::as_mut(&mut future), &mut context),
            Poll::Ready(Err(ScramDerivationError::Busy))
        ));
        assert_eq!(
            gate.busy_refusals(),
            before_public_refusal + 1,
            "one rejected request increments the refusal counter once"
        );
        drop(future);

        drop(replacement);
        drop(second);
        release_blocker_tx
            .send(())
            .expect("blocking worker still waiting");
        tokio::time::timeout(WATCHDOG, blocker)
            .await
            .expect("blocker watchdog")
            .expect("blocker join");
        drop(held_permit);

        let recovered = gate
            .admit()
            .expect("admission available after ticket drops");
        drop(recovered);
        assert_eq!(gate.semaphore().available(), 1);
    });
}

#[test]
fn queued_cancel_retains_ticket_until_blocking_job_finishes() {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .max_blocking_threads(1)
        .enable_all()
        .build()
        .expect("build single-blocking-worker runtime");

    runtime.block_on(async {
        let gate = Arc::new(ScramDerivationGate::new(Arc::new(
            Argon2Semaphore::with_capacity(1),
        )));
        let (blocker_started_tx, blocker_started_rx) = mpsc::sync_channel(1);
        let (release_blocker_tx, release_blocker_rx) = mpsc::sync_channel(1);
        let blocker = tokio::task::spawn_blocking(move || {
            blocker_started_tx
                .send(())
                .expect("test still observes blocker start");
            release_blocker_rx
                .recv_timeout(WATCHDOG)
                .expect("test releases blocking worker");
        });
        blocker_started_rx
            .recv_timeout(WATCHDOG)
            .expect("blocking worker reserved by handshake");

        // One manual poll admits and submits the real gated derivation, then
        // returns Pending because its sole blocking worker remains occupied.
        let mut future = Box::pin(derive_scram_record_gated(
            "queued cancellation password".to_string(),
            small_kdf(),
            Arc::clone(&gate),
        ));
        let waker = Waker::noop();
        let mut context = Context::from_waker(waker);
        assert!(matches!(
            Future::poll(Pin::as_mut(&mut future), &mut context),
            Poll::Pending
        ));
        drop(future);

        // The queued job still owns its ticket after the caller is cancelled.
        let other_ticket = gate.admit().expect("second admission slot remains");
        let before_overflow = gate.busy_refusals();
        assert!(matches!(gate.admit(), Err(ScramDerivationError::Busy)));
        assert_eq!(gate.busy_refusals(), before_overflow + 1);

        // FIFO blocking-pool submission makes this sentinel's channel message
        // independent evidence that the earlier cancelled request's job ended.
        let (job_done_tx, job_done_rx) = mpsc::sync_channel(1);
        let sentinel = tokio::task::spawn_blocking(move || {
            job_done_tx
                .send(())
                .expect("test receives completion witness");
        });
        release_blocker_tx
            .send(())
            .expect("blocking worker still waiting");
        tokio::time::timeout(WATCHDOG, blocker)
            .await
            .expect("blocker watchdog")
            .expect("blocker join");
        tokio::time::timeout(WATCHDOG, sentinel)
            .await
            .expect("sentinel watchdog")
            .expect("sentinel join");
        job_done_rx
            .recv_timeout(WATCHDOG)
            .expect("sentinel confirms queued job completed first");

        // The queued job has dropped its ticket; with the other ticket still
        // live, exactly one slot has reopened.
        let recovered = gate.admit().expect("queued job completion released ticket");
        drop(recovered);
        drop(other_ticket);
    });
}

#[test]
fn admission_deadline_is_passed_unchanged_and_expiry_skips_kdf() {
    let gate = Arc::new(ScramDerivationGate::new(Arc::new(
        Argon2Semaphore::with_capacity(1),
    )));

    // Inject a deterministic already-expired absolute deadline at admission;
    // the permit is free, so only recomputing/replacing that deadline could
    // enter the KDF closure.
    let ticket = gate
        .admit_with_clock(|| Some(0))
        .expect("admission clock seam supplies deadline");
    let deadline_ns = ticket.deadline_ns;
    let kdf_called = std::sync::atomic::AtomicBool::new(false);
    let result = gate.run(deadline_ns, || {
        kdf_called.store(true, std::sync::atomic::Ordering::Relaxed);
    });

    assert!(matches!(result, Err(ScramDerivationError::Busy)));
    assert!(!kdf_called.load(std::sync::atomic::Ordering::Relaxed));
    assert_eq!(gate.semaphore().available(), 1);
    assert_eq!(gate.busy_refusals(), 1);
}

#[test]
fn run_passes_kdf_outcome_through_untouched() {
    let gate = Arc::new(ScramDerivationGate::new(Arc::new(
        Argon2Semaphore::with_capacity(2),
    )));

    // The gate is generic over KDF outcome: injected errors come back inside
    // `Ok` exactly as the closure produced them.
    let ticket = gate.admit().expect("admission");
    let outcome = gate.run(ticket.deadline_ns, || {
        Err::<(), String>("kdf boom".to_string())
    });
    match outcome {
        Ok(Err(msg)) => assert_eq!(msg, "kdf boom"),
        other => panic!("expected Ok(Err(kdf boom)), got {other:?}"),
    }
    drop(ticket);
    assert_eq!(gate.semaphore().available(), 2);
    assert_eq!(gate.busy_refusals(), 0);

    let ticket = gate.admit().expect("admission");
    let success = gate.run(ticket.deadline_ns, || 41u8 + 1);
    assert!(matches!(success, Ok(42u8)));
}

#[test]
fn permit_released_after_panicking_closure() {
    let gate = Arc::new(ScramDerivationGate::new(Arc::new(
        Argon2Semaphore::with_capacity(1),
    )));

    // Panic propagates out of run; both permit and admission ticket release on
    // unwind. Catch it here to prove the subsequent run can acquire capacity.
    let ticket = gate.admit().expect("admission");
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let _ = gate.run(ticket.deadline_ns, || panic!("kdf panic injection"));
    }));
    assert!(result.is_err(), "closure should have panicked");
    drop(ticket);
    assert_eq!(gate.busy_refusals(), 0);

    let ticket = gate.admit().expect("admission after unwind");
    assert!(gate.run(ticket.deadline_ns, dummy_record).is_ok());
    drop(ticket);
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
    assert_eq!(
        ScramDerivationError::Busy.to_string(),
        "password derivation is busy; retry later"
    );
}
