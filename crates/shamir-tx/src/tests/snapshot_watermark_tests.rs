use std::sync::Arc;

use futures::poll;
use shamir_wal::{WalEntryV2, WalTicket, WalTicketState};

use crate::completion_tracker::State;
use crate::repo_tx_gate::VisibleOutcome;
use crate::RepoTxGate;

#[tokio::test]
async fn force_publish_settles_pending_missing_and_preserves_terminal_prefix() {
    let gate = RepoTxGate::new(0, 1);
    gate.completion().mark(1, State::Pending);
    gate.completion().mark(3, State::Aborted);
    gate.completion().mark(4, State::Materialized);
    let mut wait = Box::pin(gate.await_visible(4));
    assert!(poll!(&mut wait).is_pending());
    gate.publish_committed_max(4);
    assert_eq!(wait.await, VisibleOutcome::Visible);
    assert_eq!(gate.open_snapshot().await.version(), 4);
    gate.completion().mark(2, State::Pending);
    gate.publish_committed_max(2);
    assert_eq!(gate.visible_floor(), 4);
    assert_eq!(gate.last_committed(), 4);
}

#[tokio::test]
async fn eager_publish_and_watermark_sync_do_not_force_pending_versions() {
    let gate = RepoTxGate::new(0, 1);
    let lower = gate.assign_next_version_guarded();
    let higher = gate.assign_next_version_guarded();
    higher.commit();
    gate.raise_published(2);
    gate.sync_last_committed_from_watermark();
    assert_eq!(gate.open_snapshot().await.version(), 0);
    assert_eq!(gate.last_committed(), 2);
    assert_eq!(gate.visible_floor(), 0);
    drop(lower);
    assert_eq!(gate.open_snapshot().await.version(), 2);
}

#[tokio::test]
async fn aborted_lower_pre_wal_and_failed_ticket_do_not_stall_higher_visibility() {
    let gate = RepoTxGate::new(0, 1);
    let pre_wal = gate.assign_next_version_guarded();
    let mut failed = gate.assign_next_version_guarded();
    let higher = gate.assign_next_version_guarded();
    let higher_version = higher.version();
    higher.commit();
    gate.raise_published(higher_version);
    // State-machine coverage only: physical FAILED injection is crate-private
    // in shamir-wal and unavailable here. Mem sync faults leave WRITTEN, not
    // FAILED; the engine regression separately exercises that real sync fault.
    let ticket = Arc::new(WalTicket::detached_for_test(WalTicketState::Failed));
    let entry = Arc::new(WalEntryV2 {
        txn_id: 2,
        repo_id_interned: 1,
        started_at_ns: 0,
        commit_version: failed.version(),
        ops: vec![],
        interner_delta: vec![],
    });
    failed.wal_queued(Arc::clone(&ticket), entry);
    assert_eq!(ticket.state(), WalTicketState::Failed);
    let mut wait = Box::pin(gate.await_visible(higher_version));
    assert!(poll!(&mut wait).is_pending());
    drop(pre_wal);
    assert_eq!(gate.visible_floor(), 1);
    assert!(poll!(&mut wait).is_pending());
    drop(failed);
    assert_eq!(wait.await, VisibleOutcome::Visible);
    assert_eq!(gate.open_snapshot().await.version(), higher_version);
}
