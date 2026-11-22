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
        // The cancelled request's job is still queued and its admission
        // budget is spent while the sole blocking worker is occupied, so the
        // expiry outcome it reaches is fixed before it runs.
        let before_job_refusal = gate.busy_refusals();
        let before_job_kdf_runs = gate.kdf_runs();
        tokio::time::sleep(Duration::from_secs(4)).await;

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

        // The queued job's outcome is deterministic: its permit budget was
        // anchored at admission and is spent by the time it runs, so it is
        // refused as busy exactly once, never reaches the derivation closure
        // and never takes the free permit.
        assert_eq!(
            gate.busy_refusals(),
            before_job_refusal + 1,
            "the queued job reports its expiry as exactly one counted refusal"
        );
        assert_eq!(
            gate.kdf_runs(),
            before_job_kdf_runs,
            "an expired permit must never reach the derivation closure"
        );
        assert_eq!(
            gate.semaphore().available(),
            1,
            "the expired job must not consume the free permit"
        );

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

    // Inject a deterministic admission wall-clock reading of zero, so the
    // three-second budget expired in 1970 while the permit stays free: only
    // recomputing or replacing that budget could enter the KDF closure.
    let ticket = gate
        .admit_with_clock(|| Some(0))
        .expect("admission clock seam supplies the wall reading");
    let budget = ticket.budget;
    let kdf_called = std::sync::atomic::AtomicBool::new(false);
    let result = gate.run(budget, || {
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
    let outcome = gate.run(ticket.budget, || Err::<(), String>("kdf boom".to_string()));
    match outcome {
        Ok(Err(msg)) => assert_eq!(msg, "kdf boom"),
        other => panic!("expected Ok(Err(kdf boom)), got {other:?}"),
    }
    drop(ticket);
    assert_eq!(gate.semaphore().available(), 2);
    assert_eq!(gate.busy_refusals(), 0);

    let ticket = gate.admit().expect("admission");
    let success = gate.run(ticket.budget, || 41u8 + 1);
    assert!(matches!(success, Ok(42u8)));
}

#[test]
fn permit_released_after_panicking_closure() {
    let gate = Arc::new(ScramDerivationGate::new(Arc::new(
        Argon2Semaphore::with_capacity(1),
    )));

    // Panic propagates out of run; both permit and admission ticket release on
    // unwind. The ticket is owned by the panicking job, not by this test frame,
    // so the assertions below observe a release that only unwind can perform.
    let ticket = gate.admit().expect("admission");
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let job_ticket = ticket;
        let _ = gate.run(job_ticket.budget, || panic!("kdf panic injection"));
    }));
    assert!(result.is_err(), "closure should have panicked");
    assert_eq!(gate.busy_refusals(), 0);

    // Both admission slots of the capacity-one gate are free again, which only
    // holds if the unwind released the ticket.
    let first = gate.admit().expect("admission slot released on unwind");
    let second = gate
        .admit()
        .expect("second admission slot released on unwind");
    let before_refusal = gate.busy_refusals();
    assert!(matches!(gate.admit(), Err(ScramDerivationError::Busy)));
    assert_eq!(gate.busy_refusals(), before_refusal + 1);
    drop(first);
    drop(second);
    assert_eq!(gate.semaphore().available(), 1);

    let ticket = gate.admit().expect("admission after unwind");
    assert!(gate.run(ticket.budget, dummy_record).is_ok());
    drop(ticket);
    assert_eq!(gate.semaphore().available(), 1);
    assert_eq!(gate.busy_refusals(), before_refusal + 1);
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

/// A job that only reaches the semaphore after its admission budget is spent
/// must be refused with the typed busy outcome, counted once, without running
/// the KDF and without consuming the free permit. This drives the PUBLIC
/// `derive_scram_record_gated` path, so it fails if that job recomputes a
/// fresh budget instead of reusing the one fixed at admission.
#[test]
fn queued_public_job_resuming_past_its_admission_budget_is_refused() {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .max_blocking_threads(1)
        .enable_all()
        .build()
        .expect("build single-blocking-worker runtime");

    runtime.block_on(async {
        let gate = Arc::new(ScramDerivationGate::new(Arc::new(
            Argon2Semaphore::with_capacity(1),
        )));

        // Occupy the only blocking worker so the gated job stays queued.
        let (started_tx, started_rx) = mpsc::sync_channel(1);
        let (release_tx, release_rx) = mpsc::sync_channel(1);
        let blocker = tokio::task::spawn_blocking(move || {
            started_tx
                .send(())
                .expect("test observes the blocker start");
            release_rx
                .recv_timeout(WATCHDOG)
                .expect("test releases the blocking worker");
        });
        started_rx
            .recv_timeout(WATCHDOG)
            .expect("blocking worker reserved by handshake");

        // One manual poll is the admission handshake: when it returns Pending
        // the job has already admitted itself, fixed its budget and been
        // submitted to the blocking queue. No yield count is involved.
        let mut job = Box::pin(derive_scram_record_gated(
            "queued past its admission budget".to_string(),
            small_kdf(),
            Arc::clone(&gate),
        ));
        let waker = Waker::noop();
        let mut context = Context::from_waker(waker);
        assert!(matches!(
            Future::poll(Pin::as_mut(&mut job), &mut context),
            Poll::Pending
        ));

        // Observed, not hoped for: the queued job holds one of the two
        // admission slots of a capacity-one gate.
        let probe = gate
            .admit()
            .expect("the queued job holds one admission slot");
        assert!(matches!(gate.admit(), Err(ScramDerivationError::Busy)));
        drop(probe);
        let before_refusals = gate.busy_refusals();
        let before_kdf_runs = gate.kdf_runs();

        // Stay queued past the fixed three-second admission budget. The
        // oracle below does not depend on the wall clock: the monotonic
        // remainder measured from admission is already spent, so the correct
        // implementation refuses even if no wall time appears to pass.
        tokio::time::sleep(Duration::from_secs(4)).await;

        release_tx.send(()).expect("blocking worker still waiting");
        tokio::time::timeout(WATCHDOG, blocker)
            .await
            .expect("blocker watchdog")
            .expect("blocker join");
        // Resuming the future registers the real waker; a job whose blocking
        // closure already finished returns at once.
        let outcome = tokio::time::timeout(WATCHDOG, job)
            .await
            .expect("queued job watchdog");

        assert!(
            matches!(outcome, Err(ScramDerivationError::Busy)),
            "a job resuming after its admission budget must be refused as busy"
        );
        assert_eq!(
            gate.kdf_runs(),
            before_kdf_runs,
            "an expired permit must never reach the derivation closure"
        );
        assert_eq!(
            gate.busy_refusals(),
            before_refusals + 1,
            "the queued refusal is counted exactly once"
        );
        assert_eq!(gate.semaphore().available(), 1, "no permit may be consumed");
        // The ticket is released: both admission slots are free again.
        let first = gate.admit().expect("queued job released its ticket");
        let second = gate.admit().expect("queued job released its ticket");
        drop(first);
        drop(second);
    });
}
