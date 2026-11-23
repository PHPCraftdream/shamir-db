//! Internal tests for [`ScramDerivationGate`] and bounded SCRAM admission.
//!
//! Every await and channel receive is bounded by the watchdog; synchronization
//! uses handshakes, never sleeps. One test relies on Tokio's blocking pool
//! popping queued jobs in FIFO order (as of 1.51.6 it uses
//! `VecDeque::pop_front`): that is NOT a documented contract. If the order
//! ever changes, the post-cancel sentinel can complete before the earlier
//! queued gated job, so the closing assertions stop witnessing that job —
//! a possible FALSE PASS, never a false red.

use std::future::Future;
use std::pin::Pin;
use std::sync::mpsc;
use std::sync::Arc;
use std::task::{Context, Poll, Waker};
use std::time::{Duration, Instant};

use shamir_connect::common::crypto::StoredKey;
use shamir_connect::common::kdf_params::KdfParams;
use shamir_connect::common::time::{ns, UnixNanos};
use shamir_connect::server::argon2_semaphore::{Argon2PermitBudget, Argon2Semaphore};
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

/// Budget for a job admitted four seconds ago on the monotonic clock whose
/// wall deadline is still three seconds ahead.
///
/// The wall half alone would accept it (the deadline has not passed), and the
/// wall budget is the full three seconds; only the monotonic remainder
/// measured from the stale anchor exceeds that budget. This is the budget a
/// job that sat in the blocking queue past its admission deadline carries, and
/// it costs the test no real waiting.
fn budget_with_stale_monotonic_anchor() -> Argon2PermitBudget {
    let admitted_ns = UnixNanos::try_now().expect("wall clock").as_u64();
    Argon2PermitBudget::at_admission_with_anchor(
        admitted_ns + 3 * ns::SECOND,
        admitted_ns,
        Instant::now() - Duration::from_secs(4),
    )
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

        // The job is queued behind the blocker with a budget whose monotonic
        // anchor predates admission by four seconds while its wall deadline is
        // still in the future: only the monotonic remainder can refuse it, and
        // nothing in this test waits for that remainder to be spent.
        let stale_budget = budget_with_stale_monotonic_anchor();
        let ticket = gate.admit().expect("queued job admission");
        let (job_outcome_tx, job_outcome_rx) = mpsc::sync_channel(1);
        let job_gate = Arc::clone(&gate);
        let job = tokio::task::spawn_blocking(move || {
            // The ticket belongs to the blocking job, exactly like the
            // production closure's ticket, so the caller cannot release it.
            let _ticket = ticket;
            let outcome = job_gate.run_with_budget(stale_budget, || ());
            // The cancelled caller is gone; the typed outcome travels to the
            // test on a channel so the refusal REASON stays observable.
            let _ = job_outcome_tx.send(outcome);
        });
        // Cancellation: the caller-side handle is dropped and never polled
        // again. `spawn_blocking` work is not cancelled by that, so the job
        // still owns its ticket and its already-spent budget.
        drop(job);

        // The queued job still owns its ticket after the caller is cancelled.
        let other_ticket = gate.admit().expect("second admission slot remains");
        let before_overflow = gate.busy_refusals();
        assert!(matches!(gate.admit(), Err(ScramDerivationError::Busy)));
        assert_eq!(gate.busy_refusals(), before_overflow + 1);
        let before_job_refusal = gate.busy_refusals();
        let before_job_kdf_runs = gate.kdf_runs();
        let before_available = gate.semaphore().available();

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

        // The queued job's outcome is fixed before it runs: its budget's
        // monotonic remainder was already spent at admission, so it is refused
        // as busy exactly once, never reaches the derivation closure and never
        // takes the free permit.
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
            before_available,
            "the expired job must not consume the free permit"
        );
        assert!(
            matches!(
                job_outcome_rx.recv_timeout(WATCHDOG),
                Ok(Err(ScramDerivationError::Busy))
            ),
            "the refusal reason must be the typed busy outcome"
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
/// the derivation and without consuming the free permit.
///
/// This test drives the gate's BLOCKING CORE (`run_with_budget`) with a
/// caller-supplied stale-anchor budget; no public future is involved. The
/// budget anchors the monotonic clock four seconds in the past while its wall
/// deadline is still in the future, so the refusal can only come from the
/// monotonic remainder measured from admission. No wall time has to pass: a
/// check that compares only the wall clock to the deadline would accept this
/// budget and run the derivation.
///
/// The PUBLIC future's budget reuse — that `derive_scram_record_gated` carries
/// the budget fixed at `gate.admit()` into the blocking job instead of
/// recomputing one there — is covered by
/// `public_future_reuses_its_admission_budget_across_a_spent_queue_wait` below.
#[test]
fn queued_job_resuming_past_its_admission_budget_is_refused() {
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

        // Admission handshake, not a yield count: when the job is submitted it
        // already owns its ticket and is queued behind the blocker.
        let ticket = gate.admit().expect("queued job admission");
        let stale_budget = budget_with_stale_monotonic_anchor();
        let job_gate = Arc::clone(&gate);
        let job = tokio::task::spawn_blocking(move || {
            let _ticket = ticket;
            job_gate.run_with_budget(stale_budget, || ())
        });

        // Observed, not hoped for: the queued job holds one of the two
        // admission slots of a capacity-one gate.
        let probe = gate
            .admit()
            .expect("the queued job holds one admission slot");
        assert!(matches!(gate.admit(), Err(ScramDerivationError::Busy)));
        drop(probe);
        let before_refusals = gate.busy_refusals();
        let before_kdf_runs = gate.kdf_runs();
        let before_available = gate.semaphore().available();

        // Releasing the blocker lets the queued job run. Its budget is already
        // spent on the monotonic clock, so it must refuse before reaching the
        // derivation even though its wall deadline has not passed.
        release_tx.send(()).expect("blocking worker still waiting");
        tokio::time::timeout(WATCHDOG, blocker)
            .await
            .expect("blocker watchdog")
            .expect("blocker join");
        let outcome = tokio::time::timeout(WATCHDOG, job)
            .await
            .expect("queued job watchdog")
            .expect("queued job join");

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
        assert_eq!(
            gate.semaphore().available(),
            before_available,
            "no permit may be consumed"
        );
        // The ticket is released: both admission slots are free again.
        let first = gate.admit().expect("queued job released its ticket");
        let second = gate.admit().expect("queued job released its ticket");
        drop(first);
        drop(second);
    });
}

/// The PUBLIC `derive_scram_record_gated` future must carry the permit budget
/// it fixed at admission into its blocking job, not recompute one there.
///
/// Mutant killed: `derive_scram_record.rs:80` (`let budget = ticket.budget;`)
/// replaced by a budget recomputed at blocking-job start. A recomputing job
/// holds a FRESH three-second budget, so it takes the free permit, runs the
/// derivation and SUCCEEDS after the queue wait; reusing the admission budget
/// makes the same job refuse with `Busy` and enter the derivation zero times.
///
/// Why a real queue wait is the oracle here: `ScramDerivationGate` holds no
/// clock — `admit()` hardcodes `UnixNanos::try_now()` (scram_gate.rs:107) and
/// `ScramDerivationGate::new` takes only the semaphore — so the
/// `admit_with_clock` seam cannot reach the public future's admission, and no
/// already-spent budget can be handed to it. The two behaviours are separated
/// by elapsed time instead, with the ORDER guaranteed by a handshake: the sole
/// blocking worker is busy, so the submitted job provably cannot run until
/// this test releases it, and the test releases it only after more than the
/// production three-second budget has really elapsed. The wait below
/// synchronizes nothing and asserts nothing (the channel handshake does both);
/// it only spends the production budget, which makes the refusal certain
/// rather than racy — the job starts at least four seconds after its admission
/// on the monotonic clock, and past its wall deadline too.
///
/// Parent commit: this test PASSES on it (it already reuses `ticket.budget`)
/// and fails only on the mutation above; its value is that no earlier test
/// drove the public future through a spent queue wait, which is exactly why
/// the recompute mutant survived.
#[test]
fn public_future_reuses_its_admission_budget_across_a_spent_queue_wait() {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .max_blocking_threads(1)
        .enable_all()
        .build()
        .expect("build single-blocking-worker runtime");

    runtime.block_on(async {
        let gate = Arc::new(ScramDerivationGate::new(Arc::new(
            Argon2Semaphore::with_capacity(1),
        )));

        // Occupy the only blocking worker so the public future's job stays
        // queued behind it.
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

        // One manual poll is the admission handshake: Pending means the public
        // future has already admitted itself (fixing its budget at that very
        // moment) and submitted its blocking job.
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

        // Observed, not hoped for: the submitted job holds one of the two
        // admission slots of a capacity-one gate.
        let probe = gate
            .admit()
            .expect("the queued job holds one admission slot");
        assert!(matches!(gate.admit(), Err(ScramDerivationError::Busy)));
        drop(probe);
        let before_refusals = gate.busy_refusals();
        let before_kdf_runs = gate.kdf_runs();
        let before_available = gate.semaphore().available();

        // Cancellation: the caller-side future is dropped and never polled
        // again, so the queued job runs on its own — exactly like a cancelled
        // request. Its admission ticket, and the budget inside it, travel with
        // it.
        drop(job);

        // Spend the production three-second budget. This is not synchronization
        // — the handshake above and below already orders the job after this
        // point — it is the physical quantity the budget measures, so the job
        // must observe a spent admission budget.
        tokio::time::sleep(Duration::from_secs(4)).await;

        // FIFO blocking-pool submission makes this sentinel's channel message
        // independent evidence that the earlier cancelled job has finished.
        let (job_done_tx, job_done_rx) = mpsc::sync_channel(1);
        let sentinel = tokio::task::spawn_blocking(move || {
            job_done_tx
                .send(())
                .expect("test receives completion witness");
        });
        release_tx
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
            .expect("sentinel confirms the queued public job completed first");

        // Reuse: the admission budget is spent, so the job refuses exactly once
        // and never reaches the derivation. Recompute: the job would take the
        // free permit and enter the KDF, moving `kdf_runs`.
        assert_eq!(
            gate.kdf_runs(),
            before_kdf_runs,
            "the public future must carry its admission budget, not a fresh one: \
             a recomputed budget would have run the derivation"
        );
        assert_eq!(
            gate.busy_refusals(),
            before_refusals + 1,
            "the queued public job reports its spent admission budget as exactly one counted refusal"
        );
        assert_eq!(
            gate.semaphore().available(),
            before_available,
            "an expired admission budget must not consume the free permit"
        );
        // The ticket is released: both admission slots are free again.
        let first = gate
            .admit()
            .expect("the queued public job released its ticket");
        let second = gate
            .admit()
            .expect("the queued public job released its ticket");
        drop(first);
        drop(second);
    });
}
