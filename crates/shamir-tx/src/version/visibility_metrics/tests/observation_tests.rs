use std::future::{poll_fn, Future};
use std::task::Poll;

use crate::mvcc_store::write_receipt::observe;
use crate::mvcc_store::write_receipt::{VisibilityPendingReason, VisibilityState, VisibilityWait};
use crate::repo_tx_gate::RepoTxGate;
use crate::visibility_metrics::{
    kind_index, mode_index, reason_index, result_index, AckMode, AckResult, PendingKind,
};

async fn pending_poll<F: Future>(future: &mut std::pin::Pin<Box<F>>) {
    poll_fn(|cx| {
        assert!(future.as_mut().poll(cx).is_pending());
        Poll::Ready(())
    })
    .await;
}

// Red before VIS-4c: the direct-write fast path recorded nothing at all.
// A settled version needs no timer even under a Bounded policy.
#[tokio::test(start_paused = true)]
async fn fast_path_visible_records_only_the_counter() {
    let gate = RepoTxGate::fresh();
    let version = gate.assign_next_version_guarded();
    version.commit();
    let target = gate.visible_floor();
    let before = tokio::time::Instant::now();
    let state = observe(&gate, target, VisibilityWait::Strict).await;
    assert_eq!(state, VisibilityState::Visible);
    assert_eq!(tokio::time::Instant::now(), before, "no timer on fast path");
    let snap = gate.visibility().snapshot();
    let (s, v) = (
        mode_index(AckMode::Strict),
        result_index(AckResult::Visible),
    );
    assert_eq!(snap.ack_total[s][v], 1);
    assert_eq!(snap.ack_wait_ms[s][v].iter().sum::<u64>(), 0);
    assert_eq!(snap.pending_total, [[0; 3]; 2]);
}

// Red before VIS-4c: a Bounded{0} observation recorded no wait and no timer.
#[tokio::test(start_paused = true)]
async fn bounded_zero_records_pending_timeout_without_waiting() {
    let gate = RepoTxGate::fresh();
    let blocker = gate.assign_next_version_guarded();
    let target = blocker.version();
    let before = tokio::time::Instant::now();
    let state = observe(&gate, target, VisibilityWait::Bounded { timeout_ms: 0 }).await;
    assert_eq!(
        state,
        VisibilityState::Pending {
            visible_through: 0,
            blocking_version: target,
            reason: VisibilityPendingReason::Timeout,
        }
    );
    assert_eq!(
        tokio::time::Instant::now(),
        before,
        "zero budget never waits"
    );
    let snap = gate.visibility().snapshot();
    let (b, t) = (
        mode_index(AckMode::Bounded),
        result_index(AckResult::PendingTimeout),
    );
    assert_eq!(snap.ack_total[b][t], 1);
    assert_eq!(snap.ack_wait_ms[b][t].iter().sum::<u64>(), 0);
    assert_eq!(
        snap.pending_total[reason_index(VisibilityPendingReason::Timeout)]
            [kind_index(PendingKind::DirectWrite)],
        1
    );
    blocker.commit();
}

// Red before VIS-4c: an expired bounded wait did not observe its duration.
#[tokio::test(start_paused = true)]
async fn bounded_expiry_records_one_histogram_observation() {
    let gate = RepoTxGate::fresh();
    let blocker = gate.assign_next_version_guarded();
    let target = blocker.version();
    let state = observe(&gate, target, VisibilityWait::Bounded { timeout_ms: 17 }).await;
    assert_eq!(
        state,
        VisibilityState::Pending {
            visible_through: 0,
            blocking_version: target,
            reason: VisibilityPendingReason::Timeout,
        }
    );
    let snap = gate.visibility().snapshot();
    let (b, t) = (
        mode_index(AckMode::Bounded),
        result_index(AckResult::PendingTimeout),
    );
    assert_eq!(snap.ack_total[b][t], 1);
    assert_eq!(snap.ack_wait_ms[b][t].iter().sum::<u64>(), 1);
    assert_eq!(snap.ack_total[mode_index(AckMode::Strict)][t], 0);
    blocker.commit();
}

// Red before VIS-4c: reopen-required recorded no reason counter.
#[tokio::test(start_paused = true)]
async fn reopen_required_records_the_reopen_reason() {
    let gate = RepoTxGate::fresh();
    let blocker = gate.assign_next_version_guarded();
    let target = blocker.version();
    gate.set_requires_reopen();
    let state = observe(&gate, target, VisibilityWait::Strict).await;
    assert_eq!(
        state,
        VisibilityState::Pending {
            visible_through: 0,
            blocking_version: target,
            reason: VisibilityPendingReason::ReopenRequired,
        }
    );
    let snap = gate.visibility().snapshot();
    assert_eq!(
        snap.pending_total[reason_index(VisibilityPendingReason::ReopenRequired)]
            [kind_index(PendingKind::DirectWrite)],
        1
    );
    assert_eq!(
        snap.pending_total[reason_index(VisibilityPendingReason::Timeout)],
        [0; 3]
    );
    blocker.commit();
}

// Red before VIS-4c: the waiters gauge was not surfaced through the metrics
// seam (it reuses the completion tracker's RAII registration, so a dropped
// observer — cancellation — must restore it to zero).
#[tokio::test(start_paused = true)]
async fn waiter_gauge_tracks_parked_settled_and_cancelled_observers() {
    let gate = RepoTxGate::fresh();
    let blocker = gate.assign_next_version_guarded();
    let target = blocker.version();

    let mut parked = Box::pin(observe(&gate, target, VisibilityWait::Strict));
    pending_poll(&mut parked).await;
    assert_eq!(gate.completion().waiter_count(), 1);
    // Dropping ONLY the observer cancels only the observation.
    drop(parked);
    assert_eq!(gate.completion().waiter_count(), 0);

    let mut parked = Box::pin(observe(&gate, target, VisibilityWait::Strict));
    pending_poll(&mut parked).await;
    assert_eq!(gate.completion().waiter_count(), 1);
    blocker.commit();
    assert_eq!(parked.await, VisibilityState::Visible);
    assert_eq!(gate.completion().waiter_count(), 0);
    let snap = gate.visibility().snapshot();
    // The settle-at-expiry win records visible, not pending.
    assert_eq!(
        snap.ack_total[mode_index(AckMode::Strict)][result_index(AckResult::Visible)],
        1
    );
}
