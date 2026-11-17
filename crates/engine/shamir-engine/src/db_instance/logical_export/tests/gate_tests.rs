use super::super::MaintenanceGate;
use futures::FutureExt;
use shamir_storage::error::DbError;
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
    // Polling the export until it raises intent proves it is waiting for the
    // admitted writer, rather than relying on an arbitrary sleep.
    while gate.enter_mutation().is_ok() {
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
