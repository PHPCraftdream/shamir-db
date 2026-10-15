//! Tests for the owner drop contract: dropping the last owner closes
//! admission and signals close — no abort, no panic, no owner cycle.

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::Notify;

use crate::tx::cold_reconcile::lifecycle::{
    spawn_worker, InstallError, RepoUnitLifecycle, RepoWorkerLifecycle, WorkerExecutionState,
    WorkerLease, WorkerTerminal,
};

/// No-op lease used by the tests.
struct NoopLease;

impl WorkerLease for NoopLease {}

#[tokio::test(start_paused = true)]
async fn drop_last_owner_closes_admission_signals_close() {
    let exited = Arc::new(AtomicBool::new(false));
    let exit_signal = Arc::new(Notify::new());
    let (worker_exited, worker_signal) = (Arc::clone(&exited), Arc::clone(&exit_signal));
    let owner = Arc::new(RepoUnitLifecycle::new());
    spawn_worker(
        &owner,
        NoopLease,
        move |mut state: WorkerExecutionState<(), ()>| async move {
            while let Some(job) = state.next_job().await {
                job.reply(());
            }
            worker_exited.store(true, Ordering::SeqCst);
            worker_signal.notify_one();
        },
    )
    .expect("install");

    // Dropping the last owner Arc closes the channel; the parked worker
    // observes `None` from `next_job` and exits.
    drop(owner);
    // `Notify` stores the permit, so the wait below cannot miss the
    // signal even if the worker already exited.
    tokio::time::timeout(Duration::from_secs(1), exit_signal.wait())
        .await
        .expect("worker exited after owner drop");
    assert!(exited.load(Ordering::SeqCst));
}

#[tokio::test(start_paused = true)]
async fn drop_while_queued_and_active_supervisor_observes_exit_and_release() {
    let held = Arc::new(AtomicUsize::new(1));
    struct LeaseGuard {
        held: Arc<AtomicUsize>,
    }
    impl WorkerLease for LeaseGuard {
        fn on_quarantine(&self) {
            self.held.fetch_sub(1, Ordering::SeqCst);
        }
    }

    let started = Arc::new(Notify::new());
    let go = Arc::new(Notify::new());
    let exited = Arc::new(Notify::new());
    let (worker_started, worker_go, worker_exited) =
        (Arc::clone(&started), Arc::clone(&go), Arc::clone(&exited));
    let owner = Arc::new(RepoWorkerLifecycle::<u64, u64>::new());
    // The `Weak` proves the handler captured NO owner Arc: the worker
    // only ever receives `WorkerExecutionState` (no sender, no owner).
    let weak = Arc::downgrade(&owner);
    spawn_worker(
        &owner,
        LeaseGuard {
            held: Arc::clone(&held),
        },
        move |mut state: WorkerExecutionState<u64, u64>| async move {
            while let Some(job) = state.next_job().await {
                if job.payload == 0 {
                    worker_started.notify_one();
                    worker_go.wait().await;
                }
                let payload = job.payload;
                job.reply(payload);
            }
            state.quarantine_cleanup();
            worker_exited.notify_one();
        },
    )
    .expect("install");

    let rx0 = owner.submit(0).expect("admission open");
    started.wait().await;
    let rx1 = owner.submit(1).expect("queued while active");

    // Release the go-signal from a clone taken BEFORE the drop, then drop
    // the last owner Arc while one job is queued and one is active.
    go.notify_one();
    drop(owner);
    assert!(weak.upgrade().is_none(), "no owner cycle in the worker");

    // The worker finishes the active job (whose caller rx0 was also
    // dropped mid-flight), then drains the queued job FIFO.
    drop(rx0);
    assert_eq!(rx1.await.expect("queued job still drained"), 1);

    tokio::time::timeout(Duration::from_secs(1), exited.wait())
        .await
        .expect("worker exited after draining");
    // Quarantine cleanup ran on the worker's exit path: lease released.
    assert_eq!(held.load(Ordering::SeqCst), 0);
}

#[tokio::test(start_paused = true)]
async fn close_racing_first_install_never_spawns() {
    let owner = Arc::new(RepoUnitLifecycle::new());
    owner.close();
    assert!(matches!(
        spawn_worker(&owner, NoopLease, |_state| async {}),
        Err(InstallError::ClosedOrForbidden)
    ));

    // KEY: no task was spawned, so shutdown must return WITHOUT waiting —
    // with paused time an actually-spawned parked task would make this
    // deadline fire (the negative behaviour this test catches).
    assert!(matches!(
        owner.shutdown(Duration::from_secs(1)).await,
        Ok(WorkerTerminal::Completed)
    ));

    // After shutdown the spawn stays forbidden (sticky).
    assert!(matches!(
        spawn_worker(&owner, NoopLease, |_state| async {}),
        Err(InstallError::ClosedOrForbidden)
    ));
}
