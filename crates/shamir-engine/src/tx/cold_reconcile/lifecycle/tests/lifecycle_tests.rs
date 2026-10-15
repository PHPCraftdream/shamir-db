//! Behavioural tests for the repository-owned worker lifecycle owner.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::task::Poll;
use std::time::Duration;

use futures::FutureExt;
use tokio::sync::Notify;

use crate::tx::cold_reconcile::lifecycle::{
    spawn_worker, InstallError, JobSubmitError, RepoUnitLifecycle, RepoWorkerLifecycle,
    ShutdownError, WorkerLease, WorkerTerminal,
};

/// No-op lease used by the tests.
struct NoopLease;

impl WorkerLease for NoopLease {}

/// 6. No work is accepted before a worker is installed (`NotInitialized`,
/// the chosen policy) and nothing is accepted after `close` (`Closing`).
#[tokio::test(start_paused = true)]
async fn submit_without_worker_is_not_initialized() {
    let owner = RepoUnitLifecycle::default();
    assert!(owner.admission_open());
    assert!(matches!(
        owner.submit(()),
        Err(JobSubmitError::NotInitialized)
    ));
    owner.close();
    assert!(!owner.admission_open());
    assert!(matches!(owner.submit(()), Err(JobSubmitError::Closing)));
}

/// 7. An echo worker drains submitted jobs, then exits on channel end;
/// `shutdown` returns `Completed` and a SECOND shutdown returns the same
/// terminal from the cache — the join happens exactly once, never repolled.
#[tokio::test(start_paused = true)]
async fn echo_worker_drains_then_shuts_down_completed() {
    let owner = Arc::new(RepoUnitLifecycle::new());
    spawn_worker(&owner, NoopLease, |mut state| async move {
        while let Some(job) = state.next_job().await {
            job.reply(());
        }
    })
    .expect("install succeeds while open");

    let rx = owner.submit(()).expect("admission open");
    assert!(rx.await.is_ok(), "worker echoed the reply");

    let terminal = owner
        .shutdown(Duration::from_secs(3600))
        .await
        .expect("join within deadline");
    assert!(matches!(terminal, WorkerTerminal::Completed));
    assert!(!owner.quarantined());
    assert!(!owner.reopen_required());
    // Late submission is rejected with Closing.
    assert!(matches!(owner.submit(()), Err(JobSubmitError::Closing)));
    // Repeated shutdown: the cached terminal, no repoll of the worker.
    assert!(matches!(
        owner.shutdown(Duration::from_secs(3600)).await,
        Ok(WorkerTerminal::Completed)
    ));
    assert!(owner.supervisor_joined());
}

/// 8. A second `install_worker` is rejected (`AlreadyInstalled`), and
/// after `close` any later install is permanently forbidden
/// (`ClosedOrForbidden`).
#[tokio::test(start_paused = true)]
async fn install_rejects_second_worker_and_close_forbids_later_install() {
    let owner = Arc::new(RepoUnitLifecycle::new());
    spawn_worker(&owner, NoopLease, |_state| async {}).expect("first install");
    assert!(matches!(
        spawn_worker(&owner, NoopLease, |_state| async {}),
        Err(InstallError::AlreadyInstalled)
    ));

    owner.close();
    // A separately closed epoch: install is rejected WITHOUT spawning.
    let closed = Arc::new(RepoUnitLifecycle::new());
    closed.close();
    assert!(matches!(
        spawn_worker(&closed, NoopLease, |_state| async {}),
        Err(InstallError::ClosedOrForbidden)
    ));
    assert!(matches!(closed.submit(()), Err(JobSubmitError::Closing)));
}

/// 9. A panicking worker is caught INSIDE the worker boundary: shutdown
/// yields `Failed`, quarantine is sticky, the interlock is terminated,
/// and a second shutdown returns the same `Failed` from the cache.
#[tokio::test(start_paused = true)]
async fn panicking_worker_yields_failed_terminal_and_quarantines() {
    let owner = Arc::new(RepoUnitLifecycle::new());
    spawn_worker(&owner, NoopLease, |state| async move {
        let _state = state;
        panic!("boom");
    })
    .expect("install");

    let terminal = owner
        .shutdown(Duration::from_secs(3600))
        .await
        .expect("joined");
    match terminal {
        WorkerTerminal::Failed(msg) => {
            assert_eq!(msg, "worker panicked", "boundary publishes the cause");
        }
        other => panic!("expected Failed, got {other:?}"),
    }
    assert!(owner.quarantined());
    assert!(owner.reopen_required());
    assert!(owner.interlock().is_terminated());
    assert!(owner.supervisor_joined());
    // Cached: a second shutdown returns the same terminal.
    assert!(matches!(
        owner.shutdown(Duration::from_secs(3600)).await,
        Ok(WorkerTerminal::Failed(_))
    ));
}

/// 10. FIXED ORACLE: a worker parked mid-job is NOT joinable — the
/// shutdown deadline expires, quarantine is applied (publication blocked),
/// and the handle stays retained: after the test releases the worker it
/// drains and the SAME lifecycle still joins it to `Completed`. The old
/// oracle wrongly relied on the quarantine's channel close finishing the
/// job for it.
#[tokio::test(start_paused = true)]
async fn deadline_expiry_quarantines_and_keeps_handle() {
    let started = Arc::new(Notify::new());
    let block = Arc::new(Notify::new());
    let (worker_started, worker_block) = (Arc::clone(&started), Arc::clone(&block));
    let owner = Arc::new(RepoUnitLifecycle::new());
    spawn_worker(&owner, NoopLease, move |mut state| async move {
        let job = state.next_job().await.expect("one job submitted");
        worker_started.notify_one();
        // Park mid-job until the test releases — never during the first
        // shutdown.
        worker_block.notified().await;
        drop(job);
        while let Some(job) = state.next_job().await {
            job.reply(());
        }
    })
    .expect("install");

    let _rx = owner.submit(()).expect("admission open");
    started.notified().await;

    assert!(matches!(
        owner.shutdown(Duration::from_secs(3600)).await,
        Err(ShutdownError::TimedOut)
    ));
    assert!(owner.quarantined());
    assert!(owner.reopen_required());
    assert!(!owner.admission_open());
    assert!(
        !owner.interlock().begin_publication(),
        "quarantine blocks publication"
    );

    // Release the worker: it drains the (now closed) channel and exits;
    // the supervisor still owns the handle and publishes the terminal.
    block.notify_one();
    assert!(matches!(
        owner.shutdown(Duration::from_secs(3600)).await,
        Ok(WorkerTerminal::Completed)
    ));
    assert!(owner.supervisor_joined());
}

/// 11. A caller cancelling (dropping the reply receiver) before the reply
/// does not cancel the job: it is still processed exactly once.
#[tokio::test(start_paused = true)]
async fn caller_cancellation_drops_only_the_reply_receiver() {
    let processed = Arc::new(AtomicUsize::new(0));
    let counter = Arc::clone(&processed);
    let owner = Arc::new(RepoWorkerLifecycle::<u64, u64>::new());
    spawn_worker(&owner, NoopLease, move |mut state| async move {
        while let Some(job) = state.next_job().await {
            counter.fetch_add(1, Ordering::SeqCst);
            let payload = job.payload;
            job.reply(payload);
        }
    })
    .expect("install");

    let rx = owner.submit(7).expect("admission open");
    // Caller cancels BEFORE the worker replies.
    drop(rx);

    let terminal = owner
        .shutdown(Duration::from_secs(3600))
        .await
        .expect("joined");
    assert!(matches!(terminal, WorkerTerminal::Completed));
    assert_eq!(processed.load(Ordering::SeqCst), 1, "job was processed");
}

/// 12. FIXED ORACLE: the close/drain race preserves EXECUTION FIFO, not
/// just reply order — the worker records each payload when it STARTS the
/// job, so the recorded order is the true execution order [0,1,2,3,4].
/// Admission stays closed mid-drain (5th submit rejected).
#[tokio::test(start_paused = true)]
async fn queue_close_race_four_queued_plus_active() {
    let started = Arc::new(Notify::new());
    let go = Arc::new(Notify::new());
    let order: Arc<Mutex<Vec<u64>>> = Arc::new(Mutex::new(Vec::new()));
    let (worker_started, worker_go, worker_order) =
        (Arc::clone(&started), Arc::clone(&go), Arc::clone(&order));
    let owner = Arc::new(RepoWorkerLifecycle::<u64, u64>::new());
    spawn_worker(&owner, NoopLease, move |mut state| async move {
        while let Some(job) = state.next_job().await {
            // Record at EXECUTION start, before any blocking.
            worker_order.lock().unwrap().push(job.payload);
            if job.payload == 0 {
                worker_started.notify_one();
                worker_go.notified().await;
            }
            let payload = job.payload;
            job.reply(payload);
        }
    })
    .expect("install");

    let rx0 = owner.submit(0).expect("admission open");
    started.notified().await;
    let rxs: Vec<_> = (1..5)
        .map(|i| owner.submit(i).expect("queued while active"))
        .collect();
    owner.close();
    assert!(matches!(owner.submit(5), Err(JobSubmitError::Closing)));

    go.notify_one();
    assert_eq!(rx0.await.expect("reply"), 0);
    for (i, rx) in rxs.into_iter().enumerate() {
        assert_eq!(rx.await.expect("reply"), (i + 1) as u64);
    }

    let terminal = owner
        .shutdown(Duration::from_secs(3600))
        .await
        .expect("joined");
    assert!(matches!(terminal, WorkerTerminal::Completed));
    assert!(!owner.admission_open());
    assert_eq!(
        *order.lock().unwrap(),
        vec![0, 1, 2, 3, 4],
        "execution order is FIFO"
    );
}

/// 13. Concurrent shutdown callers all observe the ONE terminal: four
/// waiters park on the watch channel under paused time and every one is
/// woken by the supervisor's single publish.
#[tokio::test(start_paused = true)]
async fn concurrent_shutdown_all_observe_one_terminal() {
    let started = Arc::new(Notify::new());
    let block = Arc::new(Notify::new());
    let (worker_started, worker_block) = (Arc::clone(&started), Arc::clone(&block));
    let owner = Arc::new(RepoUnitLifecycle::new());
    spawn_worker(&owner, NoopLease, move |mut state| async move {
        let _job = state.next_job().await.expect("one job submitted");
        worker_started.notify_one();
        worker_block.notified().await;
    })
    .expect("install");
    let _rx = owner.submit(()).expect("admission open");
    started.notified().await;

    let waiters: Vec<_> = (0..4)
        .map(|_| {
            let owner = Arc::clone(&owner);
            tokio::spawn(async move { owner.shutdown(Duration::from_secs(3600)).await })
        })
        .collect();
    // All four waiters are parked on the terminal watch now.

    block.notify_one();
    for waiter in waiters {
        assert!(
            matches!(
                waiter.await.expect("waiter task joined"),
                Ok(WorkerTerminal::Completed)
            ),
            "every concurrent caller observes Completed"
        );
    }
    assert!(owner.supervisor_joined());
}

/// 14. A shutdown waiter that is cancelled (its future dropped mid-wait)
/// does not cancel the close: admission stays closed, no timeout fired,
/// and a fresh shutdown still joins the worker to `Completed`.
#[tokio::test(start_paused = true)]
async fn shutdown_waiter_cancellation_does_not_cancel_close() {
    let started = Arc::new(Notify::new());
    let block = Arc::new(Notify::new());
    let (worker_started, worker_block) = (Arc::clone(&started), Arc::clone(&block));
    let owner = Arc::new(RepoUnitLifecycle::new());
    spawn_worker(&owner, NoopLease, move |mut state| async move {
        let _job = state.next_job().await.expect("one job submitted");
        worker_started.notify_one();
        worker_block.notified().await;
    })
    .expect("install");
    let _rx = owner.submit(()).expect("admission open");
    started.notified().await;

    // Poll the shutdown future exactly once (applies close()), then cancel
    // it by dropping the pinned future while it is Pending.
    let shutdown_fut = Box::pin(owner.shutdown(Duration::from_secs(3600)));
    assert!(
        matches!(shutdown_fut.as_mut().now_or_never(), Some(Poll::Pending)),
        "worker is still parked: shutdown is Pending"
    );
    drop(shutdown_fut);
    assert!(!owner.admission_open(), "close survived the cancellation");
    assert!(!owner.quarantined(), "no timeout happened");

    block.notify_one();
    assert!(matches!(
        owner.shutdown(Duration::from_secs(3600)).await,
        Ok(WorkerTerminal::Completed)
    ));
}

/// 15. Phase-hook cancellation matrix: cancelling the reply receiver at
/// ANY of the three publication phase hooks (Prepared, Materialized,
/// PrePublication) never cancels the job — the worker finishes it
/// internally, allocation is identical for every cancellation point, and
/// the lifecycle still shuts down cleanly.
#[tokio::test(start_paused = true)]
async fn phase_hook_cancellation_matrix() {
    async fn run_case(cancel_at: usize) -> (usize, usize, bool) {
        const SLOTS_PER_JOB: usize = 8;
        let phases: [Arc<Notify>; 3] = [
            Arc::new(Notify::new()),
            Arc::new(Notify::new()),
            Arc::new(Notify::new()),
        ];
        let gates: [Arc<Notify>; 3] = [
            Arc::new(Notify::new()),
            Arc::new(Notify::new()),
            Arc::new(Notify::new()),
        ];
        let done = Arc::new(Notify::new());
        let processed = Arc::new(AtomicUsize::new(0));
        let allocated = Arc::new(AtomicUsize::new(0));
        let job_completed = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let owner = Arc::new(RepoWorkerLifecycle::<u64, ()>::new());
        let (wp0, wp1, wp2) = (
            Arc::clone(&phases[0]),
            Arc::clone(&phases[1]),
            Arc::clone(&phases[2]),
        );
        let (wg0, wg1, wg2) = (
            Arc::clone(&gates[0]),
            Arc::clone(&gates[1]),
            Arc::clone(&gates[2]),
        );
        spawn_worker(&owner, NoopLease, move |mut state| async move {
            let job = state.next_job().await.expect("one job submitted");
            for (phase, gate) in [(wp0, wg0), (wp1, wg1), (wp2, wg2)] {
                phase.notify_one();
                gate.notified().await;
            }
            processed.fetch_add(1, Ordering::SeqCst);
            allocated.fetch_add(SLOTS_PER_JOB, Ordering::SeqCst);
            job.reply(());
            job_completed.store(true, Ordering::SeqCst);
            done.notify_one();
            while let Some(job) = state.next_job().await {
                job.reply(());
            }
        })
        .expect("install");

        let rx = owner.submit(1).expect("admission open");
        // Caller reaches the cancellation hook k = `cancel_at` and drops
        // the reply receiver exactly there.
        phases[cancel_at].notified().await;
        drop(rx);
        // Release every remaining phase gate; the worker finishes the job
        // internally.
        for gate in &gates[cancel_at..] {
            gate.notify_one();
        }
        done.notified().await;

        assert!(matches!(
            owner.shutdown(Duration::from_secs(3600)).await,
            Ok(WorkerTerminal::Completed)
        ));
        (
            processed.load(Ordering::SeqCst),
            allocated.load(Ordering::SeqCst),
            job_completed.load(Ordering::SeqCst),
        )
    }

    let mut results: Vec<(usize, usize, bool)> = Vec::new();
    for k in 0..3 {
        results.push(run_case(k).await);
    }
    for (k, (processed, allocated, completed)) in results.iter().enumerate() {
        assert_eq!(*processed, 1, "case {k}: job processed exactly once");
        assert!(*completed, "case {k}: job completed internally");
        assert_eq!(*allocated, 8, "case {k}: fixed allocation per job");
    }
    assert_eq!(
        results[0], results[1],
        "cancellation at hook 0 vs 1: identical allocation"
    );
    assert_eq!(
        results[1], results[2],
        "cancellation at hook 1 vs 2: identical allocation"
    );
}
