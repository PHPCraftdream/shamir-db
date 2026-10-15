//! Tests for the owner drop contract: dropping the last owner closes
//! admission and signals close — no abort, no panic, no owner cycle.

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::Notify;

use crate::tx::cold_reconcile::lifecycle::{
    spawn_worker, InstallError, JobSubmitError, RepoUnitLifecycle, RepoWorkerLifecycle,
    WorkerLease, WorkerTerminal,
};

/// No-op lease used by the tests.
struct NoopLease;

impl WorkerLease for NoopLease {}

/// Lease accounting the physical resource via REAL destruction: `Drop`
/// releases the held count — no handler hook involved.
struct LeaseGuard {
    held: Arc<AtomicUsize>,
}

impl Drop for LeaseGuard {
    fn drop(&mut self) {
        self.held.fetch_sub(1, Ordering::SeqCst);
    }
}

impl WorkerLease for LeaseGuard {}

/// 16. Dropping the last owner `Arc` closes the job channel; the parked
/// worker observes end-of-channel, exits, and signals — no detach, no
/// abort.
#[tokio::test(start_paused = true)]
async fn drop_last_owner_closes_admission_signals_close() {
    let exited = Arc::new(AtomicBool::new(false));
    let exit_signal = Arc::new(Notify::new());
    let (worker_exited, worker_signal) = (Arc::clone(&exited), Arc::clone(&exit_signal));
    let owner = Arc::new(RepoUnitLifecycle::new());
    spawn_worker(&owner, NoopLease, move |mut state| async move {
        while let Some(job) = state.next_job().await {
            job.reply(());
        }
        worker_exited.store(true, Ordering::SeqCst);
        worker_signal.notify_one();
    })
    .expect("install");

    drop(owner);
    // `notify_one` stores one permit, so the wait cannot miss.
    tokio::time::timeout(Duration::from_secs(1), exit_signal.notified())
        .await
        .expect("worker exited after owner drop");
    assert!(exited.load(Ordering::SeqCst));
}

/// 17. Dropping the last owner while one job is active and one is queued:
/// the queued job is still drained and replied, the worker exit is
/// observed, and the lease is released by REAL destruction (`Drop` on the
/// last `Arc<L>`, held by the supervisor strictly until after join).
#[tokio::test(start_paused = true)]
async fn drop_while_queued_and_active() {
    let held = Arc::new(AtomicUsize::new(1));
    let started = Arc::new(Notify::new());
    let go = Arc::new(Notify::new());
    let exited = Arc::new(Notify::new());
    let (worker_started, worker_go, worker_exited) =
        (Arc::clone(&started), Arc::clone(&go), Arc::clone(&exited));
    let owner = Arc::new(RepoWorkerLifecycle::<u64, u64>::new());
    // `Weak` proves the handler captured NO owner `Arc`.
    let weak = Arc::downgrade(&owner);
    spawn_worker(
        &owner,
        LeaseGuard {
            held: Arc::clone(&held),
        },
        move |mut state| async move {
            while let Some(job) = state.next_job().await {
                if job.payload == 0 {
                    worker_started.notify_one();
                    worker_go.notified().await;
                }
                let payload = job.payload;
                job.reply(payload);
            }
            worker_exited.notify_one();
        },
    )
    .expect("install");

    let rx0 = owner.submit(0).expect("admission open");
    started.notified().await;
    let rx1 = owner.submit(1).expect("queued while active");

    // Release the parked job, then drop the last owner Arc while one job
    // is queued and one is active.
    go.notify_one();
    drop(owner);
    assert!(weak.upgrade().is_none(), "no owner cycle in the worker");

    // The active job's caller also cancels mid-flight; the queued job must
    // still drain FIFO.
    drop(rx0);
    assert_eq!(rx1.await.expect("queued job still drained"), 1);

    tokio::time::timeout(Duration::from_secs(1), exited.notified())
        .await
        .expect("worker exited after draining");
    // The supervisor dropped the last lease Arc: real destruction released
    // the resource.
    assert_eq!(held.load(Ordering::SeqCst), 0, "lease dropped after join");
}

/// 18. A close racing the FIRST install can never spawn:
/// (a) close -> install is rejected and NO task is spawned (the handler
///     would notify `spawned` on entry; the timeout must elapse, which is
///     deterministic under paused time);
/// (b) install -> close: the installed parked worker drains the closed
///     channel, exits, and shutdown completes.
#[tokio::test(start_paused = true)]
async fn close_racing_first_install_never_spawns() {
    // (a) close BEFORE any install.
    let owner = Arc::new(RepoUnitLifecycle::new());
    owner.close();
    let spawned = Arc::new(Notify::new());
    let handler_spawned = Arc::clone(&spawned);
    assert!(matches!(
        spawn_worker(&owner, NoopLease, move |_state| async move {
            handler_spawned.notify_one();
        }),
        Err(InstallError::ClosedOrForbidden)
    ));
    // Nothing was spawned: the notify can never fire, so the bounded wait
    // must elapse (auto-advancing paused time makes this deterministic).
    assert!(
        tokio::time::timeout(Duration::from_secs(1), spawned.notified())
            .await
            .is_err(),
        "no task may spawn on a closed epoch"
    );
    assert!(!owner.admission_open());
    assert!(matches!(owner.submit(()), Err(JobSubmitError::Closing)));

    // (b) install BEFORE close: the parked worker exits on channel end.
    let owner2 = Arc::new(RepoUnitLifecycle::new());
    spawn_worker(&owner2, NoopLease, |mut state| async move {
        while let Some(job) = state.next_job().await {
            job.reply(());
        }
    })
    .expect("install while open");
    owner2.close();
    assert!(matches!(
        owner2.shutdown(Duration::from_secs(3600)).await,
        Ok(WorkerTerminal::Completed)
    ));
    assert!(matches!(
        spawn_worker(&owner2, NoopLease, |_state| async {}),
        Err(InstallError::ClosedOrForbidden),
        "post-shutdown install stays forbidden"
    ));
}
