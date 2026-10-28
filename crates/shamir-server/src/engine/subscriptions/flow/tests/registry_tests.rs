//! Registry teardown-race regression tests (RV-SERVER-6CA31DF8).

use std::time::Duration;

use tokio::sync::oneshot;

use crate::engine::subscriptions::flow::registry::SubscriptionRegistry;

struct DropSignal(Option<oneshot::Sender<()>>);

impl Drop for DropSignal {
    fn drop(&mut self) {
        if let Some(tx) = self.0.take() {
            let _ = tx.send(());
        }
    }
}

/// A late handle attachment to a closed registry aborts the spawned task.
#[tokio::test(start_paused = true)]
async fn rv_server_6ca31df8_late_attach_aborts_task_after_close_all() {
    let registry = SubscriptionRegistry::with_cap(4);
    registry.try_reserve().expect("slot reserved");
    assert!(registry.reserve_pending(1));

    let (dropped_tx, dropped_rx) = oneshot::channel();
    let signal = DropSignal(Some(dropped_tx));
    let handle = tokio::spawn(async move {
        let _signal = signal;
        std::future::pending::<()>().await;
    });

    registry.close_all();
    registry.attach_handle(1, handle);
    tokio::time::timeout(Duration::from_secs(5), dropped_rx)
        .await
        .expect("task drop signal timed out")
        .expect("aborted task drops its signal");
    assert_eq!(registry.count(), 0);
    assert!(registry.try_reserve().is_err());
}

/// close_all removes entries but retains uninserted reservations until cleanup.
#[tokio::test(start_paused = true)]
async fn rv_server_6ca31df8_close_all_preserves_outstanding_reservations() {
    let registry = SubscriptionRegistry::with_cap(4);
    registry.try_reserve().expect("entry reservation");
    assert!(registry.reserve_pending(1));
    registry.try_reserve().expect("outstanding reservation");

    registry.close_all();
    assert_eq!(registry.count(), 1);
    assert!(registry.try_reserve().is_err());
    assert!(!registry.reserve_pending(2));
    assert_eq!(registry.count(), 0);
}

/// Admission is rejected once teardown closes the registry.
#[tokio::test(start_paused = true)]
async fn rv_server_6ca31df8_admission_rejected_after_close_all() {
    let registry = SubscriptionRegistry::with_cap(1);
    registry.close_all();
    assert!(registry.try_reserve().is_err());
    assert_eq!(registry.count(), 0);
}
