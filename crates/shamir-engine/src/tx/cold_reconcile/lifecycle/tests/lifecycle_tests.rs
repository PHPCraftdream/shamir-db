//! Behavioural tests for the repository-owned worker lifecycle owner.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::Notify;

use crate::tx::cold_reconcile::lifecycle::{
    spawn_worker, InstallError, JobSubmitError, RepoUnitLifecycle, RepoWorkerLifecycle,
    ShutdownError, WorkerExecutionState, WorkerLease, WorkerTerminal,
};

/// No-op lease used by the tests.
struct NoopLease;

impl WorkerLease for NoopLease {}

#[tokio::test]
async fn submit_without_worker_is_not_initialized() {
    let owner = RepoUnitLifecycle::default();
    assert!(owner.admission_open());
    assert!(matches!(
        owner.submit(()),
        Err(JobSubmitError::NotInitialized)
    ));
    // Close with no worker: shutdown joins nothing and completes.
    assert!(matches!(
        owner.shutdown(Duration::from_secs(1)).await,
        Ok(WorkerTerminal::Completed)
    ));
    assert!(!owner.admission_open());
}

#[tokio::test]
async fn echo_worker_drains_then_shuts_down_completed() {
    let owner = std::sync::Arc::new(RepoUnitLifecycle::new());
    spawn_worker(
        &owner,
        NoopLease,
        |mut state: WorkerExecutionState<(), ()>| async move {
            while let Some(job) = state.next_job().await {
                job.reply(());
            }
        },
    )
    .expect("install succeeds while open");

    let rx = owner.submit(()).expect("admission open");
    assert!(rx.await.is_ok(), "worker echoed the reply");

    let terminal = owner
        .shutdown(Duration::from_secs(5))
        .await
        .expect("join within deadline");
    assert!(matches!(terminal, WorkerTerminal::Completed));
    assert!(!owner.quarantined());
    assert!(!owner.reopen_required());
    // Late submission is rejected with Closing.
    assert!(matches!(owner.submit(()), Err(JobSubmitError::Closing)));
}

#[tokio::test]
async fn install_rejects_second_worker_and_close_forbids_later_install() {
    let owner = std::sync::Arc::new(RepoUnitLifecycle::new());
    spawn_worker(&owner, NoopLease, |_state| async {}).expect("first install");
    assert!(matches!(
        spawn_worker(&owner, NoopLease, |_state| async {}),
        Err(InstallError::AlreadyInstalled)
    ));

    owner.close();
    // Close-before-initialize on ANOTHER lifecycle epoch object: spawn is
    // permanently forbidden and NO task is left spawned-but-untracked.
    let closed = RepoUnitLifecycle::new();
    closed.close();
    assert!(matches!(
        spawn_worker(&closed, NoopLease, |_state| async {}),
        Err(InstallError::ClosedOrForbidden)
    ));
    assert!(matches!(closed.submit(()), Err(JobSubmitError::Closing)));
}

#[tokio::test]
async fn panicking_worker_yields_failed_terminal_and_quarantines() {
    let owner = std::sync::Arc::new(RepoUnitLifecycle::new());
    spawn_worker(
        &owner,
        NoopLease,
        |state: WorkerExecutionState<(), ()>| async move {
            let _ = state;
            panic!("boom");
        },
    )
    .expect("install");

    let terminal = owner
        .shutdown(Duration::from_secs(5))
        .await
        .expect("joined");
    match terminal {
        WorkerTerminal::Failed(msg) => assert!(msg.contains("boom"), "{msg}"),
        other => panic!("expected Failed, got {other:?}"),
    }
    assert!(owner.quarantined());
    assert!(owner.reopen_required());
    assert!(owner.interlock().is_terminated());
    // Cached: a second shutdown returns the same terminal.
    assert!(matches!(
        owner.shutdown(Duration::from_secs(5)).await,
        Ok(WorkerTerminal::Failed(_))
    ));
}

#[tokio::test(start_paused = true)]
async fn deadline_expiry_quarantines_and_keeps_handle() {
    // Long-lived worker that only ends when the channel drains.
    struct BusyLease;
    impl WorkerLease for BusyLease {
        fn on_quarantine(&self) {}
    }

    let owner = std::sync::Arc::new(RepoUnitLifecycle::new());
    spawn_worker(
        &owner,
        BusyLease,
        |mut state: WorkerExecutionState<(), ()>| async move {
            // Drain queued jobs, then park waiting for more.
            while let Some(job) = state.next_job().await {
                job.reply(());
            }
        },
    )
    .expect("install");
    owner.submit(()).expect("admission open");

    assert!(matches!(
        owner.shutdown(Duration::from_secs(3600)).await,
        Err(ShutdownError::TimedOut)
    ));
    assert!(owner.quarantined());
    assert!(owner.reopen_required());
    // The handle stays stored: a generous later call can still join. The
    // quarantine's close() dropped the sender, so the parked worker now
    // observes channel end and exits cleanly.
    assert!(matches!(
        owner.shutdown(Duration::from_secs(5)).await,
        Ok(WorkerTerminal::Completed)
    ));
}

#[tokio::test(start_paused = true)]
async fn caller_cancellation_drops_only_the_reply_receiver() {
    let processed = Arc::new(AtomicUsize::new(0));
    let counter = Arc::clone(&processed);
    let owner = Arc::new(RepoWorkerLifecycle::<u64, u64>::new());
    spawn_worker(
        &owner,
        NoopLease,
        move |mut state: WorkerExecutionState<u64, u64>| async move {
            while let Some(job) = state.next_job().await {
                counter.fetch_add(1, Ordering::SeqCst);
                let payload = job.payload;
                job.reply(payload);
            }
        },
    )
    .expect("install");

    let rx = owner.submit(7).expect("admission open");
    // Caller cancels BEFORE the worker replies: only the receiver half is
    // dropped — the job itself must still be processed.
    drop(rx);

    let terminal = owner
        .shutdown(Duration::from_secs(5))
        .await
        .expect("joined");
    assert!(matches!(terminal, WorkerTerminal::Completed));
    assert_eq!(processed.load(Ordering::SeqCst), 1, "job was processed");
}

#[tokio::test(start_paused = true)]
async fn queue_close_race_four_queued_plus_active() {
    let started = Arc::new(Notify::new());
    let go = Arc::new(Notify::new());
    let (worker_started, worker_go) = (Arc::clone(&started), Arc::clone(&go));
    let owner = Arc::new(RepoWorkerLifecycle::<u64, u64>::new());
    spawn_worker(
        &owner,
        NoopLease,
        move |mut state: WorkerExecutionState<u64, u64>| async move {
            while let Some(job) = state.next_job().await {
                if job.payload == 0 {
                    worker_started.notify_one();
                    worker_go.wait().await;
                }
                let payload = job.payload;
                job.reply(payload);
            }
        },
    )
    .expect("install");

    let rx0 = owner.submit(0).expect("admission open");
    started.wait().await;
    let rxs: Vec<_> = (1..5)
        .map(|i| owner.submit(i).expect("queued while active"))
        .collect();
    // Closing admission mid-drain: the 5th submission is rejected, the
    // four accepted jobs must still drain FIFO.
    owner.close();
    assert!(matches!(owner.submit(5), Err(JobSubmitError::Closing)));

    go.notify_one();
    assert_eq!(rx0.await.expect("reply"), 0);
    for (i, rx) in rxs.into_iter().enumerate() {
        assert_eq!(rx.await.expect("reply"), (i + 1) as u64, "FIFO order");
    }

    let terminal = owner
        .shutdown(Duration::from_secs(5))
        .await
        .expect("joined");
    assert!(matches!(terminal, WorkerTerminal::Completed));
    assert!(!owner.admission_open());
}

/// Lease counting live holders of the repository resource: `held` starts
/// at 1 and `on_quarantine` releases it.
struct LeaseGuard {
    held: Arc<AtomicUsize>,
}

impl WorkerLease for LeaseGuard {
    fn on_quarantine(&self) {
        self.held.fetch_sub(1, Ordering::SeqCst);
    }
}

#[tokio::test(start_paused = true)]
async fn timeout_with_locally_held_guard_quarantines_blocks_publication_and_retains_lease() {
    let held = Arc::new(AtomicUsize::new(1));
    let started = Arc::new(Notify::new());
    let exit_wait = Arc::new(Notify::new());
    let (worker_started, worker_exit) = (Arc::clone(&started), Arc::clone(&exit_wait));
    let owner = Arc::new(RepoWorkerLifecycle::<u64, u64>::new());
    spawn_worker(
        &owner,
        LeaseGuard {
            held: Arc::clone(&held),
        },
        move |mut state: WorkerExecutionState<u64, u64>| async move {
            let job = state.next_job().await.expect("one job submitted");
            worker_started.notify_one();
            // Simulate work that never finishes on its own: parked on a
            // Notify until the test releases it.
            worker_exit.wait().await;
            state.quarantine_cleanup();
            drop(job);
            while let Some(job) = state.next_job().await {
                drop(job);
            }
        },
    )
    .expect("install");

    owner.submit(1).expect("admission open");
    started.wait().await;

    // The worker never exits on its own: the shutdown deadline expires.
    assert!(matches!(
        owner.shutdown(Duration::from_secs(3600)).await,
        Err(ShutdownError::TimedOut)
    ));
    assert!(owner.quarantined());
    assert!(owner.reopen_required());
    assert!(!owner.admission_open());
    // Quarantine terminates the interlock: publication is blocked.
    assert!(!owner.interlock().begin_publication());
    // The lease is STILL held — resources are retained until join.
    assert_eq!(held.load(Ordering::SeqCst), 1);

    // Let the worker exit: the notification is buffered by `Notify`, so
    // the worker wakes even though the test calls this before the second
    // shutdown parks on `join`.
    exit_wait.notify_one();
    let terminal = owner
        .shutdown(Duration::from_secs(3600))
        .await
        .expect("joined after release");
    assert!(matches!(terminal, WorkerTerminal::Completed));
    // Quarantine cleanup released the guard on the worker's exit path.
    assert_eq!(held.load(Ordering::SeqCst), 0);
}
