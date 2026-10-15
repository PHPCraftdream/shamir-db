//! Supervisor join, lease release, and fail-closed panic tests.
use crate::tx::cold_reconcile::lifecycle::{
    spawn_worker, ForegroundLifecycle, RepoUnitLifecycle, WorkerLease, WorkerTerminal,
};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Notify;

struct NoopLease;
impl WorkerLease for NoopLease {}
struct DropSignal(Arc<Notify>);
impl Drop for DropSignal {
    fn drop(&mut self) {
        self.0.notify_one();
    }
}
struct LeaseGuard {
    released: Arc<AtomicBool>,
    _signal: DropSignal,
    _resource: Arc<()>,
}
impl Drop for LeaseGuard {
    fn drop(&mut self) {
        self.released.store(true, Ordering::SeqCst);
    }
}
impl WorkerLease for LeaseGuard {}

#[tokio::test(start_paused = true)]
async fn lease_is_dropped_after_join_before_terminal() {
    let dropped = Arc::new(Notify::new());
    let resource = Arc::new(());
    let weak = Arc::downgrade(&resource);
    let released = Arc::new(AtomicBool::new(false));
    let owner = Arc::new(RepoUnitLifecycle::new());
    let gate = Arc::new(Notify::new());
    let worker_gate = Arc::clone(&gate);
    spawn_worker(
        &owner,
        LeaseGuard {
            released: Arc::clone(&released),
            _signal: DropSignal(Arc::clone(&dropped)),
            _resource: resource,
        },
        move |mut state| async move {
            let _job = state.next_job().await.expect("submitted job");
            worker_gate.notified().await;
        },
    )
    .unwrap();
    let _rx = owner.submit(()).unwrap();
    gate.notify_one();
    assert!(matches!(
        owner.shutdown(Duration::from_secs(3600)).await,
        Ok(WorkerTerminal::Completed)
    ));
    assert!(owner.supervisor_joined());
    assert!(weak.upgrade().is_none());
    dropped.notified().await;
    assert!(released.load(Ordering::SeqCst));
}

struct PanicLease {
    _status: crate::tx::cold_reconcile::lifecycle::LifecycleStatus,
}
impl WorkerLease for PanicLease {}
struct ProtectedResource {
    status: crate::tx::cold_reconcile::lifecycle::LifecycleStatus,
    observed: Arc<AtomicBool>,
}
impl Drop for ProtectedResource {
    fn drop(&mut self) {
        self.observed
            .store(self.status.quarantined(), Ordering::SeqCst);
    }
}

#[tokio::test(start_paused = true)]
async fn protected_resource_drop_observes_actual_quarantine_during_unwind() {
    let owner = Arc::new(RepoUnitLifecycle::new());
    let owner_status = owner.status();
    let observed = Arc::new(AtomicBool::new(false));
    let drop_observed = Arc::clone(&observed);
    spawn_worker(
        &owner,
        PanicLease {
            _status: owner_status.clone(),
        },
        move |state| async move {
            let _guard = state.protect_guard(ProtectedResource {
                status: state.status(),
                observed: drop_observed,
            });
            panic!("intentional worker panic");
        },
    )
    .unwrap();
    assert!(matches!(
        owner.shutdown(Duration::from_secs(3600)).await,
        Ok(WorkerTerminal::Failed(_))
    ));
    assert!(owner.supervisor_joined());
    assert!(owner.quarantined());
    assert!(
        observed.load(Ordering::SeqCst),
        "protected resource Drop observed quarantine"
    );
    drop(owner_status);
}

#[tokio::test(start_paused = true)]
async fn protected_local_resource_survives_timeout_until_worker_join() {
    let owner = Arc::new(RepoUnitLifecycle::new());
    let entered = Arc::new(Notify::new());
    let release = Arc::new(Notify::new());
    let (worker_entered, worker_release) = (Arc::clone(&entered), Arc::clone(&release));
    let resource = Arc::new(());
    let weak_resource = Arc::downgrade(&resource);
    let worker_resource = Arc::clone(&resource);
    spawn_worker(&owner, NoopLease, move |state| async move {
        let _protected = state.protect_guard(worker_resource);
        worker_entered.notify_one();
        worker_release.notified().await;
    })
    .unwrap();
    drop(resource);
    entered.notified().await;
    assert!(matches!(
        owner.shutdown(Duration::ZERO).await,
        Err(crate::tx::cold_reconcile::lifecycle::ShutdownError::TimedOut)
    ));
    assert!(owner.quarantined() && owner.reopen_required());
    assert!(owner.interlock().publish(|| ()).is_err());
    assert!(weak_resource.upgrade().is_some());
    release.notify_one();
    assert!(matches!(
        owner.shutdown(Duration::from_secs(3600)).await,
        Ok(WorkerTerminal::Completed)
    ));
    assert!(weak_resource.upgrade().is_none());
}

#[tokio::test(start_paused = true)]
async fn final_foreground_token_closes_shared_lifecycle_without_raw_arc_cycle() {
    let lifecycle = Arc::new(RepoUnitLifecycle::new());
    let token = Arc::new(ForegroundLifecycle::from_lifecycle(Arc::clone(&lifecycle)));
    let retained_token = Arc::clone(&token);
    drop(token);
    assert!(lifecycle.admission_open());
    drop(retained_token);
    assert!(!lifecycle.admission_open());
    assert!(matches!(
        lifecycle.shutdown(Duration::ZERO).await,
        Ok(WorkerTerminal::Completed)
    ));
}

#[tokio::test(start_paused = true)]
async fn supervisor_joined_after_clean_terminal() {
    let owner = Arc::new(RepoUnitLifecycle::new());
    spawn_worker(&owner, NoopLease, |mut state| async move {
        while let Some(job) = state.next_job().await {
            job.reply(());
        }
    })
    .unwrap();
    let rx = owner.submit(()).unwrap();
    rx.await.unwrap();
    assert!(matches!(
        owner.shutdown(Duration::from_secs(3600)).await,
        Ok(WorkerTerminal::Completed)
    ));
    assert!(owner.supervisor_joined());
}
