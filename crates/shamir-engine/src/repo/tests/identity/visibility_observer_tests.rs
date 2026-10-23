use std::future::{poll_fn, Future};
use std::task::Poll;
use std::time::Duration;

use shamir_query_types::batch::{VisibilityPendingReason, VisibilityState, VisibilityWait};
use shamir_storage::error::DbError;
use shamir_tx::RepoTxGate;

use crate::repo::visibility_observer::observe_gate_visibility;

const STATUS: VisibilityWait = VisibilityWait::Bounded { timeout_ms: 0 };

#[tokio::test(start_paused = true)]
async fn visibility_zero_future_reopen_and_visible_precedence() {
    let gate = RepoTxGate::fresh();
    let lower = gate.assign_next_version_guarded();
    assert_eq!(
        observe_gate_visibility(&gate, 1, STATUS).await.unwrap(),
        VisibilityState::Pending {
            visible_through: 0,
            blocking_version: 1,
            reason: VisibilityPendingReason::Timeout,
        },
    );
    assert!(matches!(
        observe_gate_visibility(&gate, 2, STATUS).await,
        Err(DbError::VisibilityFutureVersion {
            requested: 2,
            allocated: 1,
        }),
    ));
    gate.set_requires_reopen();
    assert_eq!(
        observe_gate_visibility(&gate, 1, VisibilityWait::Strict)
            .await
            .unwrap(),
        VisibilityState::Pending {
            visible_through: 0,
            blocking_version: 1,
            reason: VisibilityPendingReason::ReopenRequired,
        },
    );
    drop(lower); // Aborted positions also establish settlement, not commit proof.
    assert_eq!(
        observe_gate_visibility(&gate, 1, STATUS).await.unwrap(),
        VisibilityState::Visible,
    );
}

#[tokio::test(start_paused = true)]
async fn visibility_timeout_and_strict_settlement_are_deterministic() {
    let gate = RepoTxGate::fresh();
    let lower = gate.assign_next_version_guarded();
    let mut bounded = Box::pin(observe_gate_visibility(
        &gate,
        1,
        VisibilityWait::Bounded { timeout_ms: 17 },
    ));
    poll_fn(|cx| {
        assert!(bounded.as_mut().poll(cx).is_pending());
        Poll::Ready(())
    })
    .await;
    tokio::time::advance(Duration::from_millis(17)).await;
    assert_eq!(
        bounded.await.unwrap(),
        VisibilityState::Pending {
            visible_through: 0,
            blocking_version: 1,
            reason: VisibilityPendingReason::Timeout,
        },
    );
    let mut strict = Box::pin(observe_gate_visibility(&gate, 1, VisibilityWait::Strict));
    poll_fn(|cx| {
        assert!(strict.as_mut().poll(cx).is_pending());
        Poll::Ready(())
    })
    .await;
    tokio::time::advance(Duration::from_secs(60)).await;
    poll_fn(|cx| {
        assert!(strict.as_mut().poll(cx).is_pending());
        Poll::Ready(())
    })
    .await;
    lower.commit();
    assert_eq!(strict.await.unwrap(), VisibilityState::Visible);
}

#[tokio::test(start_paused = true)]
async fn visibility_expiry_settlement_wins_and_cancel_does_not_settle() {
    let gate = RepoTxGate::fresh();
    let lower = gate.assign_next_version_guarded();
    let mut observer = Box::pin(observe_gate_visibility(&gate, 1, VisibilityWait::Strict));
    poll_fn(|cx| {
        assert!(observer.as_mut().poll(cx).is_pending());
        Poll::Ready(())
    })
    .await;
    drop(observer);
    assert_eq!(gate.visible_floor(), 0);
    assert_eq!(gate.version_allocation_high_water_mark(), 1);
    let mut bounded = Box::pin(observe_gate_visibility(
        &gate,
        1,
        VisibilityWait::Bounded { timeout_ms: 17 },
    ));
    poll_fn(|cx| {
        assert!(bounded.as_mut().poll(cx).is_pending());
        Poll::Ready(())
    })
    .await;
    tokio::time::advance(Duration::from_millis(17)).await;
    lower.commit();
    gate.set_requires_reopen();
    assert_eq!(bounded.await.unwrap(), VisibilityState::Visible);
}
