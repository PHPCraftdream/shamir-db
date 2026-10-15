//! Tests for the supervisor-owned-join-handle design: the lease is
//! dropped strictly after the join and before the terminal is published,
//! and the panic boundary quarantines before destroying the worker's
//! guards.

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::sync::Notify;

use crate::tx::cold_reconcile::lifecycle::{
    spawn_worker, RepoUnitLifecycle, WorkerLease, WorkerTerminal,
};

/// No-op lease used by the tests.
struct NoopLease;

impl WorkerLease for NoopLease {}

/// Lease whose physical resource is released by REAL destruction: the
/// `Drop` impl (not any handler hook) decrements the held count and flags
/// the release.
struct LeaseGuard {
    /// 1 while the lease struct is alive; the ONLY strong `Arc` is owned
    /// by the lifecycle (supervisor + worker state).
    held: Arc<AtomicUsize>,
    released: Arc<AtomicBool>,
}

impl Drop for LeaseGuard {
    fn drop(&mut self) {
        self.held.fetch_sub(1, Ordering::SeqCst);
        self.released.store(true, Ordering::SeqCst);
    }
}

impl WorkerLease for LeaseGuard {}

/// 19. The lease is destroyed ONLY after the supervisor joins the worker
/// and strictly BEFORE the terminal wakes `shutdown`: the test holds a
/// `Weak` to the lease's counter (no strong of its own), so
/// `upgrade().is_none()` after `Completed` proves the lease struct was
/// dropped by the supervisor — no detach.
#[tokio::test(start_paused = true)]
async fn lease_dropped_only_after_join_and_before_terminal() {
    let held = Arc::new(AtomicUsize::new(1));
    let released = Arc::new(AtomicBool::new(false));
    // Weak is taken from the SAME Arc the lease owns; the test keeps NO
    // strong clone, so upgrade() observes the lease's own destruction.
    let weak = Arc::downgrade(&held);

    let block = Arc::new(Notify::new());
    let worker_block = Arc::clone(&block);
    let owner = Arc::new(RepoUnitLifecycle::new());
    spawn_worker(
        &owner,
        LeaseGuard {
            held,
            released: Arc::clone(&released),
        },
        move |mut state| async move {
            let _job = state.next_job().await.expect("one job submitted");
            worker_block.notified().await;
        },
    )
    .expect("install");
    let _rx = owner.submit(()).expect("admission open");

    block.notify_one();
    assert!(matches!(
        owner.shutdown(Duration::from_secs(3600)).await,
        Ok(WorkerTerminal::Completed)
    ));

    // ORDERED observations, all deterministic because the supervisor drops
    // the lease and sets `supervisor_joined` BEFORE publishing the
    // terminal that wakes `shutdown`:
    assert!(owner.supervisor_joined(), "supervisor joined first");
    assert!(weak.upgrade().is_none(), "lease struct destroyed");
    assert!(
        released.load(Ordering::SeqCst),
        "Drop released the resource"
    );
}

/// Lease recording quarantine order for the panic-boundary control.
struct PanicLease {
    order: Arc<Mutex<Vec<u8>>>,
}

impl WorkerLease for PanicLease {
    fn on_quarantine(&self) {
        self.order.lock().unwrap().push(0);
    }
}

/// 20. NEGATIVE CONTROL for the catch_unwind boundary: the handler's own
/// guard lives INSIDE the worker future, so the boundary must quarantine
/// (push 0) and run `lease.on_quarantine` BEFORE the future (and its
/// guard, push 1) is dropped. Under the old spawn-directly design this
/// ordering was unobservable.
#[tokio::test(start_paused = true)]
async fn panic_quarantines_before_guard_destruction() {
    let order: Arc<Mutex<Vec<u8>>> = Arc::new(Mutex::new(Vec::new()));
    let lease_order = Arc::clone(&order);
    let guard_order = Arc::clone(&order);
    let owner = Arc::new(RepoUnitLifecycle::new());
    spawn_worker(
        &owner,
        PanicLease { order: lease_order },
        move |state| async move {
            struct Guard {
                order: Arc<Mutex<Vec<u8>>>,
            }
            impl Drop for Guard {
                fn drop(&mut self) {
                    self.order.lock().unwrap().push(1);
                }
            }
            let _state = state;
            let _guard = Guard { order: guard_order };
            panic!("boom");
        },
    )
    .expect("install");

    assert!(matches!(
        owner.shutdown(Duration::from_secs(3600)).await,
        Ok(WorkerTerminal::Failed(_))
    ));
    assert!(owner.quarantined());
    assert_eq!(
        *order.lock().unwrap(),
        vec![0, 1],
        "quarantine STRICTLY before guard destruction"
    );
}

/// 21. After a clean echo shutdown the supervisor has joined and set the
/// sticky `supervisor_joined` flag.
#[tokio::test(start_paused = true)]
async fn supervisor_joined_after_terminal_observed() {
    let owner = Arc::new(RepoUnitLifecycle::new());
    spawn_worker(&owner, NoopLease, |mut state| async move {
        while let Some(job) = state.next_job().await {
            job.reply(());
        }
    })
    .expect("install");
    let rx = owner.submit(()).expect("admission open");
    assert!(rx.await.is_ok());

    assert!(matches!(
        owner.shutdown(Duration::from_secs(3600)).await,
        Ok(WorkerTerminal::Completed)
    ));
    assert!(owner.supervisor_joined());
}
