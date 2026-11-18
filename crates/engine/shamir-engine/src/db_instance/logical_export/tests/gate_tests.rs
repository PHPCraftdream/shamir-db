use super::super::MaintenanceGate;
use futures::FutureExt;
use shamir_storage::error::{DbError, DbResult};
use std::sync::Arc;
use tokio::sync::Notify;
use tokio::time::{Duration, Instant};

#[tokio::test]
async fn drains_and_releases_on_success() {
    let gate = MaintenanceGate::default();
    let writer = gate.enter_mutation().unwrap();
    let entered = Arc::new(Notify::new());
    let worker = {
        let gate = gate.clone();
        let entered = Arc::clone(&entered);
        tokio::spawn(async move {
            gate.with_freeze(|_| {
                async move {
                    entered.notify_one();
                    Ok(())
                }
                .boxed()
            })
            .await
        })
    };
    // Bounded spin: freeze intent is raised (with_freeze is draining) exactly
    // when enter_mutation starts refusing; the deadline makes the bound a
    // hang detector, not the synchronization means.
    let intent_deadline = Instant::now() + BOUND;
    while gate.enter_mutation().is_ok() {
        assert!(
            Instant::now() < intent_deadline,
            "export never raised freeze intent"
        );
        tokio::task::yield_now().await;
    }
    assert!(!worker.is_finished());
    assert!(gate
        .with_freeze(|_| async { Ok(()) }.boxed())
        .await
        .is_err());
    drop(writer);
    entered.notified().await;
    worker.await.unwrap().unwrap();
    assert!(gate.enter_mutation().is_ok());
}

#[tokio::test]
async fn releases_on_export_error() {
    let gate = MaintenanceGate::default();
    let result: Result<(), _> = gate
        .with_freeze(|_| async { Err(DbError::Validation("fixture failure".into())) }.boxed())
        .await;
    assert!(result.is_err());
    assert!(gate.enter_mutation().is_ok());
}

#[tokio::test]
async fn releases_when_cancelled_during_drain() {
    let gate = MaintenanceGate::default();
    let writer = gate.enter_mutation().unwrap();
    let task = {
        let gate = gate.clone();
        tokio::spawn(async move { gate.with_freeze(|_| async { Ok(()) }.boxed()).await })
    };
    while gate.enter_mutation().is_ok() {
        tokio::task::yield_now().await;
    }
    task.abort();
    assert!(task.await.unwrap_err().is_cancelled());
    assert!(gate.enter_mutation().is_ok());
    drop(writer);
}

#[tokio::test]
async fn releases_when_cancelled_inside_export() {
    let gate = MaintenanceGate::default();
    let entered = Arc::new(Notify::new());
    let task = {
        let gate = gate.clone();
        let entered = Arc::clone(&entered);
        tokio::spawn(async move {
            gate.with_freeze(|_| {
                async move {
                    entered.notify_one();
                    futures::future::pending::<Result<(), DbError>>().await
                }
                .boxed()
            })
            .await
        })
    };
    entered.notified().await;
    assert!(gate.enter_mutation().is_err());
    task.abort();
    assert!(task.await.unwrap_err().is_cancelled());
    assert!(gate.enter_mutation().is_ok());
}

#[tokio::test(start_paused = true)]
async fn deadline_includes_drain_and_export() {
    let gate = MaintenanceGate::default();
    let writer = gate.enter_mutation().unwrap();
    let deadline = Instant::now() + Duration::from_secs(1);
    let mut drain = Box::pin(gate.with_deadline(deadline, |_| async { Ok(()) }.boxed()));
    assert!(futures::poll!(drain.as_mut()).is_pending());
    // drain() yields while writers are active, so explicitly advance the
    // paused clock rather than depending on Tokio's idle auto-advance.
    tokio::time::advance(Duration::from_secs(2)).await;
    assert!(drain.await.is_err());
    assert!(gate.enter_mutation().is_ok());
    drop(writer);
    let result = gate
        .with_deadline(Instant::now() + Duration::from_secs(1), |_| {
            futures::future::pending::<Result<(), DbError>>().boxed()
        })
        .await;
    assert!(result.is_err());
    assert!(gate.enter_mutation().is_ok());
}

// Deterministic (poll/handshake, never sleep-based) tests for the async
// wait-for-open admission path and its release on every guard exit. Each
// await is bounded by a generous 120 s timeout per the test policy; the
// handshakes make the bound a hang detector, not the synchronization means.
const BOUND: Duration = Duration::from_secs(120);

#[tokio::test]
async fn waiter_queues_while_frozen_and_enters_after_thaw() {
    let gate = MaintenanceGate::default();
    let writer = gate.enter_mutation().unwrap();
    let (reached_tx, mut reached_rx) = tokio::sync::oneshot::channel();
    let worker = {
        let gate = gate.clone();
        tokio::spawn(async move {
            gate.with_freeze(|_| {
                let reached_tx = reached_tx;
                async move {
                    let _ = reached_tx.send(());
                    Ok(())
                }
                .boxed()
            })
            .await
        })
    };
    // Freeze intent is raised (with_freeze is draining) exactly when
    // enter_mutation starts refusing — no sleep involved. Bounded: the
    // deadline makes the spin a hang detector, not the synchronization means.
    let intent_deadline = Instant::now() + BOUND;
    while gate.enter_mutation().is_ok() {
        assert!(
            Instant::now() < intent_deadline,
            "export never raised freeze intent"
        );
        tokio::task::yield_now().await;
    }
    let mut waiter = Box::pin(gate.wait_for_mutation());
    // Pending proves the waiter parked; the callback has not run because the
    // drain is still blocked on `writer`.
    assert!(futures::poll!(waiter.as_mut()).is_pending());
    assert!(reached_rx.try_recv().is_err());
    // Guard exit must wake the queued waiter; the bounded await below hangs
    // if the release is lost.
    drop(writer);
    let guard = tokio::time::timeout(BOUND, waiter.as_mut()).await.unwrap();
    drop(guard);
    let joined = tokio::time::timeout(BOUND, worker)
        .await
        .expect("export task did not finish within the hang-detector bound");
    let export = joined.expect("export task must not be cancelled");
    assert!(
        matches!(export, Ok(())),
        "export failed after thaw: {export:?}"
    );
    assert!(reached_rx.try_recv().is_ok());
    assert!(gate.enter_mutation().is_ok());
}

#[tokio::test]
async fn simultaneous_waiters_both_enter_after_thaw() {
    let gate = MaintenanceGate::default();
    let writer = gate.enter_mutation().unwrap();
    // Deterministic freeze: pin the freeze future borrowed from this task and
    // poll it to Pending instead of racing a spawned worker. Pending plus the
    // refusing enter_mutation proves the drain is live under the initial
    // writer — no pre-release oracle, no sleep.
    let mut freeze = Box::pin(gate.with_freeze(|_| async { Ok(()) }.boxed()));
    assert!(futures::poll!(freeze.as_mut()).is_pending());
    assert!(gate.enter_mutation().is_err());
    let mut first = Box::pin(gate.wait_for_mutation());
    let mut second = Box::pin(gate.wait_for_mutation());
    // Both must park under one freeze: notify_waiters plus enable-before-check
    // is the only protocol that wakes two concurrent waiters; a permit-based
    // notify would lose one of them.
    assert!(futures::poll!(first.as_mut()).is_pending());
    assert!(futures::poll!(second.as_mut()).is_pending());
    drop(writer);
    // The single thaw must complete the freeze AND admit both waiters. A
    // waiter that still counts itself across suspension stalls the drain and
    // times the freeze out here; a notify_one that wakes only the first
    // waiter leaves the second suspended past the join bound.
    // Double unwrap: the timeout asserts completion, the inner unwrap asserts
    // the freeze itself succeeded (a spurious error must fail the test).
    tokio::time::timeout(BOUND, freeze.as_mut())
        .await
        .unwrap()
        .unwrap();
    let both = futures::future::join(first.as_mut(), second.as_mut());
    let (a, b) = tokio::time::timeout(BOUND, both).await.unwrap();
    drop(a);
    drop(b);
    assert!(gate.enter_mutation().is_ok());
}

#[tokio::test]
async fn panic_during_export_releases_and_wakes_waiter() {
    // Tail-diverging async block: output type comes from the signature, no
    // unreachable code.
    fn panic_export() -> futures::future::BoxFuture<'static, DbResult<()>> {
        async { panic!("fixture panic") }.boxed()
    }
    let gate = MaintenanceGate::default();
    let writer = gate.enter_mutation().unwrap();
    let worker = {
        let gate = gate.clone();
        tokio::spawn(async move { gate.with_freeze::<(), _>(|_| panic_export()).await })
    };
    let intent_deadline = Instant::now() + BOUND;
    while gate.enter_mutation().is_ok() {
        assert!(
            Instant::now() < intent_deadline,
            "export never raised freeze intent"
        );
        tokio::task::yield_now().await;
    }
    let mut waiter = Box::pin(gate.wait_for_mutation());
    assert!(futures::poll!(waiter.as_mut()).is_pending());
    // Release the initial writer BEFORE awaiting the worker: the export
    // cannot run (and panic) while the drain is still blocked on it.
    drop(writer);
    let outcome = tokio::time::timeout(BOUND, worker).await.unwrap();
    // JoinError::panic, not cancellation: the unwind ran, so the guard's Drop
    // released the gate on the panic path itself.
    assert!(outcome.unwrap_err().is_panic());
    let guard = tokio::time::timeout(BOUND, waiter.as_mut()).await.unwrap();
    drop(guard);
    assert!(gate.enter_mutation().is_ok());
}
