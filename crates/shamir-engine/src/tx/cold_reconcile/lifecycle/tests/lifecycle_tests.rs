//! Deterministic tests for lifecycle admission, worker completion, and shutdown waiters.
use crate::tx::cold_reconcile::lifecycle::{
    spawn_worker, InstallError, JobSubmitError, RepoUnitLifecycle, RepoWorkerLifecycle,
    ShutdownError, WorkerLease, WorkerTerminal,
};
use futures::FutureExt;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::Notify;
struct NoopLease;
impl WorkerLease for NoopLease {}

#[tokio::test(start_paused = true)]
async fn submit_without_worker_is_not_initialized() {
    let owner = RepoUnitLifecycle::new();
    assert!(owner.admission_open());
    assert!(matches!(
        owner.submit(()),
        Err(JobSubmitError::NotInitialized)
    ));
    owner.close();
    assert!(!owner.admission_open());
    assert!(matches!(owner.submit(()), Err(JobSubmitError::Closing)));
}

#[tokio::test(start_paused = true)]
async fn no_worker_shutdown_completes_immediately() {
    let owner = RepoUnitLifecycle::new();
    assert!(matches!(
        owner.shutdown(Duration::ZERO).await,
        Ok(WorkerTerminal::Completed)
    ));
    assert!(owner.supervisor_joined());
}

#[tokio::test(start_paused = true)]
async fn echo_worker_drains_and_repeated_shutdown_uses_terminal() {
    let owner = Arc::new(RepoUnitLifecycle::new());
    spawn_worker(&owner, NoopLease, |mut s| async move {
        while let Some(job) = s.next_job().await {
            job.reply(());
        }
    })
    .unwrap();
    owner.submit(()).unwrap().await.unwrap();
    assert!(matches!(
        owner.shutdown(Duration::from_secs(3600)).await,
        Ok(WorkerTerminal::Completed)
    ));
    assert!(matches!(
        owner.shutdown(Duration::from_secs(3600)).await,
        Ok(WorkerTerminal::Completed)
    ));
    assert!(owner.supervisor_joined());
}

#[tokio::test(start_paused = true)]
async fn install_rejection_distinguishes_installed_and_closed() {
    let owner = Arc::new(RepoUnitLifecycle::new());
    spawn_worker(&owner, NoopLease, |_s| async {}).unwrap();
    assert!(matches!(
        spawn_worker(&owner, NoopLease, |_s| async {}),
        Err(InstallError::AlreadyInstalled)
    ));
    let closed = Arc::new(RepoUnitLifecycle::new());
    closed.close();
    assert!(matches!(
        spawn_worker(&closed, NoopLease, |_s| async {}),
        Err(InstallError::ClosedOrForbidden)
    ));
    assert!(matches!(
        owner.shutdown(Duration::from_secs(3600)).await,
        Ok(WorkerTerminal::Completed)
    ));
}

#[tokio::test(start_paused = true)]
async fn panic_terminal_follows_quarantine_and_supervisor_join() {
    let owner = Arc::new(RepoUnitLifecycle::new());
    spawn_worker(&owner, NoopLease, |_s| async {
        panic!("boom");
    })
    .unwrap();
    assert!(matches!(
        owner.shutdown(Duration::from_secs(3600)).await,
        Ok(WorkerTerminal::Failed(_))
    ));
    assert!(owner.quarantined() && owner.reopen_required() && owner.supervisor_joined());
    assert!(owner.interlock().is_terminated());
}

#[tokio::test(start_paused = true)]
async fn deadline_expiry_quarantines_and_retains_supervisor_until_join() {
    let entered = Arc::new(Notify::new());
    let release = Arc::new(Notify::new());
    let (worker_entered, worker_release) = (Arc::clone(&entered), Arc::clone(&release));
    let owner = Arc::new(RepoUnitLifecycle::new());
    spawn_worker(&owner, NoopLease, move |mut s| async move {
        let _job = s.next_job().await.unwrap();
        worker_entered.notify_one();
        worker_release.notified().await;
    })
    .unwrap();
    let _rx = owner.submit(()).unwrap();
    entered.notified().await;
    assert!(matches!(
        owner.shutdown(Duration::from_secs(3600)).await,
        Err(ShutdownError::TimedOut)
    ));
    assert!(owner.quarantined() && owner.reopen_required());
    assert!(owner.interlock().publish(|| ()).is_err());
    release.notify_one();
    assert!(matches!(
        owner.shutdown(Duration::from_secs(3600)).await,
        Ok(WorkerTerminal::Completed)
    ));
    assert!(owner.supervisor_joined());
}

#[tokio::test(start_paused = true)]
async fn cancelled_reply_receiver_does_not_cancel_processing() {
    let count = Arc::new(AtomicUsize::new(0));
    let worker_count = Arc::clone(&count);
    let owner = Arc::new(RepoWorkerLifecycle::<u64, u64>::new());
    spawn_worker(&owner, NoopLease, move |mut s| async move {
        while let Some(job) = s.next_job().await {
            worker_count.fetch_add(1, Ordering::SeqCst);
            let payload = job.payload;
            job.reply(payload);
        }
    })
    .unwrap();
    drop(owner.submit(7).unwrap());
    assert!(matches!(
        owner.shutdown(Duration::from_secs(3600)).await,
        Ok(WorkerTerminal::Completed)
    ));
    assert_eq!(count.load(Ordering::SeqCst), 1);
}

#[tokio::test(start_paused = true)]
async fn queue_close_race_preserves_execution_fifo() {
    let entered = Arc::new(Notify::new());
    let release = Arc::new(Notify::new());
    let (worker_entered, worker_release) = (Arc::clone(&entered), Arc::clone(&release));
    let order = Arc::new(Mutex::new(Vec::new()));
    let worker_order = Arc::clone(&order);
    let owner = Arc::new(RepoWorkerLifecycle::<u64, u64>::new());
    spawn_worker(&owner, NoopLease, move |mut s| async move {
        while let Some(job) = s.next_job().await {
            worker_order.lock().unwrap().push(job.payload);
            if job.payload == 0 {
                worker_entered.notify_one();
                worker_release.notified().await;
            }
            let p = job.payload;
            job.reply(p);
        }
    })
    .unwrap();
    let rx0 = owner.submit(0).unwrap();
    entered.notified().await;
    let rxs: Vec<_> = (1..5).map(|i| owner.submit(i).unwrap()).collect();
    owner.close();
    assert!(matches!(owner.submit(5), Err(JobSubmitError::Closing)));
    release.notify_one();
    rx0.await.unwrap();
    for (i, rx) in rxs.into_iter().enumerate() {
        assert_eq!(rx.await.unwrap(), (i + 1) as u64);
    }
    assert!(matches!(
        owner.shutdown(Duration::from_secs(3600)).await,
        Ok(WorkerTerminal::Completed)
    ));
    assert_eq!(*order.lock().unwrap(), vec![0, 1, 2, 3, 4]);
}

#[tokio::test(start_paused = true)]
async fn concurrent_shutdown_waiters_are_prepolled_pending_before_release() {
    let entered = Arc::new(Notify::new());
    let release = Arc::new(Notify::new());
    let (worker_entered, worker_release) = (Arc::clone(&entered), Arc::clone(&release));
    let owner = Arc::new(RepoUnitLifecycle::new());
    spawn_worker(&owner, NoopLease, move |mut s| async move {
        let _ = s.next_job().await.unwrap();
        worker_entered.notify_one();
        worker_release.notified().await;
    })
    .unwrap();
    let _rx = owner.submit(()).unwrap();
    entered.notified().await;
    let mut waiters = [0, 1, 2, 3].map(|_| {
        let o = Arc::clone(&owner);
        Box::pin(async move { o.shutdown(Duration::from_secs(3600)).await })
    });
    for waiter in &mut waiters {
        assert!(matches!(waiter.as_mut().now_or_never(), None));
    }
    release.notify_one();
    let [mut a, mut b, mut c, mut d] = waiters;
    let (a, b, c, d) = futures::join!(a.as_mut(), b.as_mut(), c.as_mut(), d.as_mut());
    for result in [a, b, c, d] {
        assert!(matches!(result, Ok(WorkerTerminal::Completed)));
    }
    assert!(owner.supervisor_joined());
}

#[tokio::test(start_paused = true)]
async fn concurrent_failure_and_different_deadlines_are_independent() {
    let entered = Arc::new(Notify::new());
    let release = Arc::new(Notify::new());
    let (worker_entered, worker_release) = (Arc::clone(&entered), Arc::clone(&release));
    let owner = Arc::new(RepoUnitLifecycle::new());
    spawn_worker(&owner, NoopLease, move |mut s| async move {
        let _ = s.next_job().await.unwrap();
        worker_entered.notify_one();
        worker_release.notified().await;
        panic!("failure");
    })
    .unwrap();
    let _rx = owner.submit(()).unwrap();
    entered.notified().await;
    let mut short = Box::pin(owner.shutdown(Duration::from_secs(10)));
    let mut long = Box::pin(owner.shutdown(Duration::from_secs(3600)));
    assert!(matches!(short.as_mut().now_or_never(), None));
    assert!(matches!(long.as_mut().now_or_never(), None));
    tokio::time::advance(Duration::from_secs(11)).await;
    assert!(matches!(
        short.as_mut().now_or_never(),
        Some(Err(ShutdownError::TimedOut))
    ));
    drop(short);
    release.notify_one();
    assert!(matches!(long.await, Ok(WorkerTerminal::Failed(_))));
    assert!(owner.supervisor_joined() && owner.quarantined());
}

#[tokio::test(start_paused = true)]
async fn shutdown_waiter_cancellation_keeps_close_applied() {
    let entered = Arc::new(Notify::new());
    let release = Arc::new(Notify::new());
    let (worker_entered, worker_release) = (Arc::clone(&entered), Arc::clone(&release));
    let owner = Arc::new(RepoUnitLifecycle::new());
    spawn_worker(&owner, NoopLease, move |mut s| async move {
        let _ = s.next_job().await.unwrap();
        worker_entered.notify_one();
        worker_release.notified().await;
    })
    .unwrap();
    let _rx = owner.submit(()).unwrap();
    entered.notified().await;
    let mut waiter = Box::pin(owner.shutdown(Duration::from_secs(3600)));
    assert!(matches!(waiter.as_mut().now_or_never(), None));
    drop(waiter);
    assert!(!owner.admission_open() && !owner.quarantined());
    release.notify_one();
    assert!(matches!(
        owner.shutdown(Duration::from_secs(3600)).await,
        Ok(WorkerTerminal::Completed)
    ));
}

#[tokio::test(start_paused = true)]
async fn phase_matrix_notifies_each_next_phase_before_waiting() {
    for cancel_at in 0..3 {
        let phases = [
            Arc::new(Notify::new()),
            Arc::new(Notify::new()),
            Arc::new(Notify::new()),
        ];
        let gates = [
            Arc::new(Notify::new()),
            Arc::new(Notify::new()),
            Arc::new(Notify::new()),
        ];
        let done = Arc::new(Notify::new());
        let owner = Arc::new(RepoWorkerLifecycle::<(), ()>::new());
        let (p0, p1, p2) = (
            Arc::clone(&phases[0]),
            Arc::clone(&phases[1]),
            Arc::clone(&phases[2]),
        );
        let (g0, g1, g2) = (
            Arc::clone(&gates[0]),
            Arc::clone(&gates[1]),
            Arc::clone(&gates[2]),
        );
        let next0 = Arc::clone(&phases[1]);
        let next1 = Arc::clone(&phases[2]);
        let worker_done = Arc::clone(&done);
        spawn_worker(&owner, NoopLease, move |mut s| async move {
            let job = s.next_job().await.unwrap();
            for (i, (p, g)) in [(p0, g0), (p1, g1), (p2, g2)].into_iter().enumerate() {
                p.notify_one();
                g.notified().await;
                if i == 0 {
                    next0.notify_one();
                } else if i == 1 {
                    next1.notify_one();
                }
            }
            job.reply(());
            worker_done.notify_one();
        })
        .unwrap();
        let rx = owner.submit(()).unwrap();
        let mut phase = 0;
        while phase < 3 {
            phases[phase].notified().await;
            if phase == cancel_at {
                drop(rx);
            }
            gates[phase].notify_one();
            phase += 1;
        }
        done.notified().await;
        assert!(matches!(
            owner.shutdown(Duration::from_secs(3600)).await,
            Ok(WorkerTerminal::Completed)
        ));
    }
}
