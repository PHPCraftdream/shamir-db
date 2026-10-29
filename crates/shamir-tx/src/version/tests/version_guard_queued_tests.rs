use std::sync::Arc;

use shamir_wal::{
    WalDurability, WalEntryV2, WalGroupCommit, WalOpV2, WalSink, WalTicket, WalTicketState,
};

use crate::{RepoTxGate, RepoWalManager, TxMetrics};

fn entry(version: u64) -> Arc<WalEntryV2> {
    Arc::new(WalEntryV2 {
        txn_id: version + 100,
        repo_id_interned: 7,
        started_at_ns: 0,
        commit_version: version,
        ops: vec![WalOpV2::CounterDelta {
            table_id_interned: 9,
            delta: 3,
        }],
        interner_delta: vec![(9, "field".to_owned(), 11)],
        event: None,
    })
}

fn ticket(state: WalTicketState) -> Arc<WalTicket> {
    Arc::new(WalTicket::detached_for_test(state))
}

fn assert_watermarks(gate: &RepoTxGate, visible: u64, durable: u64) {
    assert_eq!(gate.completion().watermark(), visible);
    assert_eq!(gate.last_committed(), visible);
    assert_eq!(gate.durable_watermark(), durable);
}

// Counterfactual: blind pre-WAL abort advances both trackers and loses the
// entry; treating WRITTEN as failure also fails this replay/ownership oracle.
#[test]
fn written_and_acked_drops_handover_entry_without_completing_either_tracker() {
    for state in [WalTicketState::Written, WalTicketState::Acked] {
        let gate = RepoTxGate::fresh();
        let metrics = Arc::new(TxMetrics::new());
        let mut guard = gate.assign_next_version_guarded();
        guard.attach_metrics(Arc::clone(&metrics));
        let version = guard.version();
        let replay = entry(version);
        guard.wal_queued(ticket(state), Arc::clone(&replay));
        assert!(gate.is_active_post_wal(version));
        assert!(!gate.is_orphaned_post_wal(version));
        assert_eq!(gate.orphan_max(), 0);
        assert!(gate.unresolved_wal_versions().is_empty());
        assert!(gate.orphan_entry(version).is_none());
        assert_eq!(gate.version_allocation_high_water_mark(), version);
        assert_watermarks(&gate, 0, 0);
        assert_eq!(metrics.snapshot().post_wal_orphans_total, 0);
        drop(guard);
        assert_eq!(metrics.snapshot().post_wal_orphans_total, 1);

        assert!(!gate.is_active_post_wal(version));
        assert!(gate.is_orphaned_post_wal(version));
        assert_eq!(gate.orphan_max(), version);
        assert!(Arc::ptr_eq(&gate.orphan_entry(version).unwrap(), &replay));
        assert!(gate.unresolved_wal_versions().is_empty());
        assert!(gate.take_unresolved(version).is_none());
        assert!(!gate.requires_reopen());
        assert_watermarks(&gate, 0, 0);

        let next = gate.assign_next_version_guarded();
        let next_version = next.version();
        next.commit();
        gate.mark_durable(next_version);
        assert_watermarks(&gate, 0, 0);
        gate.complete_orphaned_post_wal(version);
        assert_watermarks(&gate, next_version, 0);
        gate.mark_durable(version);
        gate.forget_orphaned_post_wal(version);
        assert_watermarks(&gate, next_version, next_version);
        assert!(!gate.is_orphaned_post_wal(version));
        assert!(gate.orphan_entry(version).is_none());
        assert_eq!(gate.orphan_max(), version);
        assert_eq!(metrics.snapshot().post_wal_orphans_total, 1);
    }
}

// Only an armed written Drop counts; queued ambiguity, absence and commit do not.
#[test]
fn queued_non_orphan_transitions_do_not_increment_attached_metric() {
    for state in [
        WalTicketState::Failed,
        WalTicketState::Pending,
        WalTicketState::Unknown,
        WalTicketState::Written,
        WalTicketState::Acked,
    ] {
        let gate = RepoTxGate::fresh();
        let metrics = Arc::new(TxMetrics::new());
        let mut guard = gate.assign_next_version_guarded();
        guard.attach_metrics(Arc::clone(&metrics));
        let version = guard.version();
        guard.wal_queued(ticket(state), entry(version));
        if matches!(state, WalTicketState::Written | WalTicketState::Acked) {
            guard.commit();
            assert_watermarks(&gate, version, 0);
        } else {
            drop(guard);
        }
        assert!(!gate.is_orphaned_post_wal(version));
        assert_eq!(metrics.snapshot().post_wal_orphans_total, 0);
    }
}

// Counterfactual: classifying every queued drop as an orphan or unresolved
// wedges both trackers at the failed version and leaves replay membership.
#[test]
fn failed_ticket_aborts_both_trackers_even_after_wal_accepted() {
    let gate = RepoTxGate::fresh();
    let mut guard = gate.assign_next_version_guarded();
    let version = guard.version();
    guard.wal_queued(ticket(WalTicketState::Failed), entry(version));
    guard.wal_accepted();
    guard.wal_accepted();
    assert!(gate.is_active_post_wal(version));
    drop(guard);
    assert_watermarks(&gate, version, version);
    assert!(!gate.is_active_post_wal(version));
    assert!(!gate.is_orphaned_post_wal(version));
    assert!(gate.orphan_entry(version).is_none());
    assert!(gate.unresolved_wal_versions().is_empty());
    assert!(gate.take_unresolved(version).is_none());
    assert_eq!(gate.orphan_max(), 0);
    assert_eq!(gate.version_allocation_high_water_mark(), version);
    assert!(!gate.requires_reopen());
}

// Counterfactual: blind abort unblocks visibility/durability; blind orphaning
// raises orphan_max. Losing ticket/entry, sorting, or destructive take fails.
#[test]
fn pending_and_unknown_remain_unresolved_until_owned_resolution() {
    let gate = RepoTxGate::fresh();
    let mut first = gate.assign_next_version_guarded();
    let mut second = gate.assign_next_version_guarded();
    let v1 = first.version();
    let v2 = second.version();
    let t1 = ticket(WalTicketState::Pending);
    let t2 = ticket(WalTicketState::Unknown);
    let e1 = entry(v1);
    let e2 = entry(v2);
    first.wal_queued(Arc::clone(&t1), Arc::clone(&e1));
    second.wal_queued(Arc::clone(&t2), Arc::clone(&e2));
    assert!(gate.is_active_post_wal(v1));
    assert!(gate.is_active_post_wal(v2));
    // Reverse insertion order exercises the ascending snapshot contract.
    drop(second);
    assert!(gate.requires_reopen());
    drop(first);
    assert_eq!(gate.unresolved_wal_versions(), vec![v1, v2]);
    for version in [v1, v2] {
        assert!(!gate.is_active_post_wal(version));
        assert!(!gate.is_orphaned_post_wal(version));
        assert!(gate.orphan_entry(version).is_none());
    }
    assert_eq!(gate.orphan_max(), 0);
    assert_eq!(gate.version_allocation_high_water_mark(), v2);
    assert_watermarks(&gate, 0, 0);

    let (taken_ticket, taken_entry) = gate.take_unresolved(v2).unwrap();
    assert!(Arc::ptr_eq(&taken_ticket, &t2));
    assert!(Arc::ptr_eq(&taken_entry, &e2));
    assert_eq!(taken_ticket.state(), WalTicketState::Unknown);
    assert_eq!(gate.unresolved_wal_versions(), vec![v1]);
    assert!(gate.take_unresolved(v2).is_none());
    assert_watermarks(&gate, 0, 0);
    // Simulate a later resolver proving absence during reopen. Merely taking
    // UNKNOWN must not settle it; abort_unwritten requires that external proof.
    gate.abort_unwritten(v2);
    assert_watermarks(&gate, 0, 0);
    let (taken_ticket, taken_entry) = gate.take_unresolved(v1).unwrap();
    assert!(Arc::ptr_eq(&taken_ticket, &t1));
    assert!(Arc::ptr_eq(&taken_entry, &e1));
    assert_eq!(taken_ticket.state(), WalTicketState::Pending);
    assert!(gate.unresolved_wal_versions().is_empty());
    assert_watermarks(&gate, 0, 0);
    gate.abort_unwritten(v1);
    gate.abort_unwritten(v1);
    assert_watermarks(&gate, v2, v2);
    assert!(gate.requires_reopen());
    assert_eq!(gate.orphan_max(), 0);
    assert!(!gate.is_active_post_wal(v1));
    assert!(!gate.is_orphaned_post_wal(v1));
}

#[test]
fn pending_alone_does_not_require_reopen_or_allow_watermark_progress() {
    let gate = RepoTxGate::fresh();
    let mut guard = gate.assign_next_version_guarded();
    let version = guard.version();
    guard.wal_queued(ticket(WalTicketState::Pending), entry(version));
    drop(guard);
    let next = gate.assign_next_version_guarded();
    let next_version = next.version();
    next.commit();
    gate.mark_durable(next_version);
    assert_watermarks(&gate, 0, 0);
    assert_eq!(gate.unresolved_wal_versions(), vec![version]);
    assert!(!gate.requires_reopen());
    assert!(!gate.is_active_post_wal(version));
    assert!(!gate.is_orphaned_post_wal(version));
    assert_eq!(gate.orphan_max(), 0);
    gate.set_requires_reopen();
    assert!(gate.requires_reopen());
    // Flag exposure deliberately does not change allocation in this stage.
    drop(gate.assign_next_version_guarded());
    assert_eq!(gate.version_allocation_high_water_mark(), next_version + 1);
    assert_watermarks(&gate, 0, 0);
}

// Real frame fate rather than a detached fixture: the committer disappears
// after append, without wal_accepted. Old pre-WAL Drop aborts this WAL frame.
#[tokio::test]
async fn real_mem_acked_drop_hands_over_the_recoverable_frame() {
    let gate = RepoTxGate::fresh();
    let manager = RepoWalManager::new(1, Arc::new(WalGroupCommit::new(Arc::new(WalSink::mem()))));
    let mut guard = gate.assign_next_version_guarded();
    let version = guard.version();
    let replay = entry(version);
    let (ticket, completion) = manager
        .begin_grouped_ticketed(&replay, WalDurability::Synced)
        .unwrap();
    guard.wal_queued(Arc::clone(&ticket), Arc::clone(&replay));
    completion.await.unwrap();
    assert_eq!(ticket.state(), WalTicketState::Acked);
    drop(guard);
    assert_watermarks(&gate, 0, 0);
    assert!(!gate.is_active_post_wal(version));
    assert!(gate.is_orphaned_post_wal(version));
    assert_eq!(gate.orphan_max(), version);
    assert!(gate.unresolved_wal_versions().is_empty());
    assert!(!gate.requires_reopen());
    let handed_over = gate.orphan_entry(version).unwrap();
    assert!(Arc::ptr_eq(&handed_over, &replay));
    assert_eq!(
        manager.recover().await.unwrap(),
        vec![(*handed_over).clone()]
    );
}

// Real Mem append covers manager encoding, lazy ticket creation, recovery,
// and normal commit. Retaining stale membership or aborting on commit fails.
#[tokio::test]
async fn real_mem_ticketed_commit_is_unaffected_and_legacy_begin_still_replays() {
    let gate = RepoTxGate::fresh();
    let group = Arc::new(WalGroupCommit::new(Arc::new(WalSink::mem())));
    let manager = RepoWalManager::new(1, group);
    let mut guard = gate.assign_next_version_guarded();
    let version = guard.version();
    let replay = entry(version);
    let (ticket, completion) = manager
        .begin_grouped_ticketed(&replay, WalDurability::Buffered)
        .unwrap();
    assert_eq!(ticket.state(), WalTicketState::Pending);
    assert!(manager.recover().await.unwrap().is_empty());
    guard.wal_queued(Arc::clone(&ticket), Arc::clone(&replay));
    assert!(gate.is_active_post_wal(version));
    completion.await.unwrap();
    assert_eq!(ticket.state(), WalTicketState::Acked);
    guard.wal_accepted();
    guard.wal_accepted();
    guard.commit();
    assert_watermarks(&gate, version, 0);
    assert!(!gate.is_active_post_wal(version));
    assert!(!gate.is_orphaned_post_wal(version));
    assert!(gate.orphan_entry(version).is_none());
    assert!(gate.unresolved_wal_versions().is_empty());
    assert_eq!(gate.orphan_max(), 0);
    assert!(!gate.requires_reopen());
    gate.mark_durable(version);
    assert_watermarks(&gate, version, version);
    let legacy = entry(version + 1);
    manager
        .begin_grouped(&legacy, WalDurability::Synced)
        .await
        .unwrap();
    assert_eq!(
        manager.recover().await.unwrap(),
        vec![(*replay).clone(), (*legacy).clone()],
    );
}
