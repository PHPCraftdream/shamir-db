use std::future::{poll_fn, Future};
use std::pin::Pin;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::task::Poll;

use crate::wal_entry_v2::WalEntryV2;
use crate::wal_group_commit::{WalDurability, WalGroupCommit};
use crate::wal_sink::WalSink;
use crate::wal_ticket::{WalAppendError, WalTicket, WalTicketState};

fn payload(id: u64) -> Vec<u8> {
    WalEntryV2::new(id, 7, Vec::new())
        .with_commit_version(id)
        .encode()
        .unwrap()
}

async fn poll_once<F: Future>(mut future: Pin<&mut F>) -> Poll<F::Output> {
    poll_fn(|cx| Poll::Ready(future.as_mut().poll(cx))).await
}

// Red before the fix: dropping the second leader window strands its follower
// forever with flushing=true. Bounded polls expose the hang without a timeout.
#[tokio::test]
async fn dropped_leader_completes_window_unknown_and_fail_stops() {
    let sink = Arc::new(WalSink::mem());
    let group = WalGroupCommit::new(Arc::clone(&sink));
    sink.arm_pause_next_append();
    let (first, leader) = group.append_ticketed(payload(1), 1, WalDurability::Buffered);
    let mut leader = Box::pin(leader);
    assert!(poll_once(leader.as_mut()).await.is_pending());
    sink.wait_append_paused().await;

    let (follower, completion) = group.append_ticketed(payload(2), 2, WalDurability::Synced);
    let mut completion = Box::pin(completion);
    assert!(poll_once(completion.as_mut()).await.is_pending());
    sink.arm_pause_next_append();
    sink.resume_append();
    assert!(poll_once(leader.as_mut()).await.is_pending());
    sink.wait_append_paused().await;
    assert_eq!(first.state(), WalTicketState::Acked);
    assert_eq!(follower.state(), WalTicketState::Pending);

    let (queued, queued_completion) = group.append_ticketed(payload(3), 3, WalDurability::Buffered);
    let mut queued_completion = Box::pin(queued_completion);
    assert!(poll_once(queued_completion.as_mut()).await.is_pending());
    drop(leader);
    assert_eq!(follower.state(), WalTicketState::Unknown);
    assert_eq!(
        poll_once(completion.as_mut()).await,
        Poll::Ready(Err(WalAppendError::Unknown)),
    );
    assert_eq!(
        poll_once(queued_completion.as_mut()).await,
        Poll::Ready(Err(WalAppendError::Indeterminate)),
    );
    assert_eq!(queued.state(), WalTicketState::Failed);

    let (next, next_completion) = group.append_ticketed(payload(4), 4, WalDurability::Buffered);
    tokio::pin!(next_completion);
    assert_eq!(
        poll_once(next_completion.as_mut()).await,
        Poll::Ready(Err(WalAppendError::Indeterminate)),
    );
    assert_eq!(next.state(), WalTicketState::Failed);
    assert!(group
        .append_many(Vec::new(), WalDurability::Buffered)
        .await
        .is_err(),);
    assert_eq!(sink.replay().await.unwrap()[0].txn_id, 1);
    assert_eq!(sink.replay().await.unwrap().len(), 1);
}

// Counterfactual: acknowledging only the leader or forgetting follower completion
// leaves either bounded completion poll Pending; skipping writes breaks replay.
#[tokio::test]
async fn normal_followers_ack_and_replay_in_order() {
    let sink = Arc::new(WalSink::mem());
    let group = WalGroupCommit::new(Arc::clone(&sink));
    sink.arm_pause_next_append();
    let (_, leader) = group.append_ticketed(payload(1), 1, WalDurability::Buffered);
    tokio::pin!(leader);
    assert!(poll_once(leader.as_mut()).await.is_pending());
    sink.wait_append_paused().await;
    let (buffered, buffered_done) = group.append_ticketed(payload(2), 2, WalDurability::Buffered);
    let (synced, synced_done) = group.append_ticketed(payload(3), 3, WalDurability::Synced);
    tokio::pin!(buffered_done, synced_done);
    assert!(poll_once(buffered_done.as_mut()).await.is_pending());
    assert!(poll_once(synced_done.as_mut()).await.is_pending());
    sink.resume_append();
    assert_eq!(poll_once(leader.as_mut()).await, Poll::Ready(Ok(())));
    assert_eq!(poll_once(buffered_done.as_mut()).await, Poll::Ready(Ok(())));
    assert_eq!(poll_once(synced_done.as_mut()).await, Poll::Ready(Ok(())));
    assert_eq!(buffered.state(), WalTicketState::Acked);
    assert_eq!(synced.state(), WalTicketState::Acked);
    let ids: Vec<_> = sink
        .replay()
        .await
        .unwrap()
        .iter()
        .map(|e| e.txn_id)
        .collect();
    assert_eq!(ids, vec![1, 2, 3]);
    assert_eq!(group.fsync_count(), 1);
}

// Red before the fix: a sync failure collapses into the same generic write error.
// A paused sync also catches moving WRITTEN publication until after fsync.
#[tokio::test]
async fn sync_failure_preserves_written_and_replay() {
    let sink = Arc::new(WalSink::mem());
    let group = WalGroupCommit::new(Arc::clone(&sink));
    sink.arm_pause_next_sync();
    sink.arm_fail_next_sync();
    let (ticket, completion) = group.append_ticketed(payload(1), 1, WalDurability::Synced);
    tokio::pin!(completion);
    assert!(poll_once(completion.as_mut()).await.is_pending());
    sink.wait_sync_paused().await;
    assert_eq!(ticket.state(), WalTicketState::Written);
    assert_eq!(sink.replay().await.unwrap()[0].txn_id, 1);
    sink.resume_sync();
    assert_eq!(
        poll_once(completion.as_mut()).await,
        Poll::Ready(Err(WalAppendError::WrittenNotDurable)),
    );
    assert_eq!(ticket.state(), WalTicketState::Written);
    assert!(group.is_dirty());
    assert_eq!(group.fsync_count(), 1);
}

// Counterfactual: treating every error as UNKNOWN/WRITTEN or failing to perform
// the sink write makes this state/error/replay triple disagree with the brief.
#[tokio::test]
async fn failed_write_is_not_written_and_next_append_recovers() {
    let sink = Arc::new(WalSink::mem());
    let group = WalGroupCommit::new(Arc::clone(&sink));
    sink.arm_fail_next_append_for_test();
    let (ticket, completion) = group.append_ticketed(payload(1), 1, WalDurability::Synced);
    assert_eq!(completion.await, Err(WalAppendError::NotWritten));
    assert_eq!(ticket.state(), WalTicketState::Failed);
    assert!(sink.replay().await.unwrap().is_empty());
    assert_eq!(group.fsync_count(), 0);
    let (next, completion) = group.append_ticketed(payload(2), 2, WalDurability::Buffered);
    assert_eq!(completion.await, Ok(()));
    assert_eq!(next.state(), WalTicketState::Acked);
    assert_eq!(sink.replay().await.unwrap()[0].txn_id, 2);
}

// Counterfactual: a guard that covers writes but not fsync leaves WRITTEN
// indefinitely and permits another writer after losing the durability ack.
#[tokio::test]
async fn dropped_leader_during_sync_is_unknown() {
    let sink = Arc::new(WalSink::mem());
    let group = WalGroupCommit::new(Arc::clone(&sink));
    sink.arm_pause_next_sync();
    let (ticket, completion) = group.append_ticketed(payload(1), 1, WalDurability::Synced);
    let mut completion = Box::pin(completion);
    assert!(poll_once(completion.as_mut()).await.is_pending());
    sink.wait_sync_paused().await;
    assert_eq!(ticket.state(), WalTicketState::Written);
    drop(completion);
    assert_eq!(ticket.state(), WalTicketState::Unknown);
    assert_eq!(sink.replay().await.unwrap()[0].txn_id, 1);
    let (_, completion) = group.append_ticketed(payload(2), 2, WalDurability::Buffered);
    tokio::pin!(completion);
    assert_eq!(
        poll_once(completion.as_mut()).await,
        Poll::Ready(Err(WalAppendError::Indeterminate)),
    );
}

// A ticket must outlive its follower future; removing the queued frame on
// cancellation would change recovery and leave this ticket forever Pending.
#[tokio::test]
async fn cancelled_follower_ticket_still_observes_ack() {
    let sink = Arc::new(WalSink::mem());
    let group = WalGroupCommit::new(Arc::clone(&sink));
    sink.arm_pause_next_append();
    let (_, leader) = group.append_ticketed(payload(1), 1, WalDurability::Buffered);
    tokio::pin!(leader);
    assert!(poll_once(leader.as_mut()).await.is_pending());
    sink.wait_append_paused().await;
    let (ticket, completion) = group.append_ticketed(payload(2), 2, WalDurability::Buffered);
    let mut completion = Box::pin(completion);
    assert!(poll_once(completion.as_mut()).await.is_pending());
    drop(completion);
    sink.resume_append();
    assert_eq!(poll_once(leader.as_mut()).await, Poll::Ready(Ok(())));
    assert_eq!(ticket.state(), WalTicketState::Acked);
    assert_eq!(sink.replay().await.unwrap()[1].txn_id, 2);
}

// The Mem fast path must still finish in one poll: a spawn/channel hop would
// break current-thread commit atomicity even if eventual replay were correct.
#[tokio::test]
async fn uncontended_mem_append_finishes_in_one_poll() {
    let sink = Arc::new(WalSink::mem());
    let group = WalGroupCommit::new(Arc::clone(&sink));
    for tier in [WalDurability::Buffered, WalDurability::Synced] {
        let (ticket, completion) = group.append_ticketed(payload(1), 1, tier);
        tokio::pin!(completion);
        assert_eq!(poll_once(completion.as_mut()).await, Poll::Ready(Ok(())));
        assert_eq!(ticket.state(), WalTicketState::Acked);
    }
    assert_eq!(sink.replay().await.unwrap().len(), 2);
}

// Red before the fix: `complete()` used `notify_one`, so of N awaiters parked
// on ONE shared ticket exactly one was woken and the other N-1 stayed parked
// forever on a deadline-less select — a lost wakeup on the batched append
// path (`append_many` shares one ticket across all frames' awaiters). With
// `notify_waiters` every awaiter registered BEFORE completion is woken, and
// each re-checks `done` (enable-before-check), so late registrants observe
// the terminal state instead of waiting. A registered-waiter counter makes
// all three registrations deterministic; no sleeps; every task is joined.
#[tokio::test]
async fn completion_wakes_every_awaiter_of_a_shared_ticket() {
    let ticket = Arc::new(WalTicket::detached_for_test(WalTicketState::Pending));

    // Mirrors `drive_and_wait`'s park loop: enable-before-check, no deadline.
    async fn await_terminal(
        ticket: Arc<WalTicket>,
        registered: Arc<AtomicUsize>,
    ) -> WalAppendError {
        let mut announced = false;
        loop {
            let notified = ticket.notify.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            if !announced {
                registered.fetch_add(1, Ordering::SeqCst);
                announced = true;
            }
            if let Some(outcome) = ticket.outcome() {
                return outcome.expect_err("fixture completes as Failed, not Ok");
            }
            notified.await;
        }
    }

    let registered = Arc::new(AtomicUsize::new(0));
    let mut waiters = Vec::new();
    for _ in 0..3 {
        let ticket = Arc::clone(&ticket);
        let registered = Arc::clone(&registered);
        waiters.push(tokio::spawn(async move {
            await_terminal(ticket, registered).await
        }));
    }
    // Wait for the CONDITION: every waiter has registered its `Notified`
    // BEFORE completion fires (a missed wakeup would otherwise be untestable).
    while registered.load(Ordering::SeqCst) < 3 {
        tokio::task::yield_now().await;
    }

    ticket.complete_detached_for_test(WalTicketState::Failed);

    for waiter in waiters {
        let error = waiter.await.unwrap();
        assert_eq!(error, WalAppendError::NotWritten);
    }
    assert_eq!(ticket.state(), WalTicketState::Failed);
}
