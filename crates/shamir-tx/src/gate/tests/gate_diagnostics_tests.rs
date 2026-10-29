//! REPO-DIAGNOSTICS contracts: these diagnostics and warning seam are absent
//! before this chunk. Counts must exclude completed slots above a blocked W.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll, Wake, Waker};
use std::time::Duration;

use shamir_wal::{WalEntryV2, WalTicket, WalTicketState};

use crate::repo_tx_gate::VisibleOutcome;
use crate::{CompletionState, GateBlockerClass, GateDiagnostics, RepoTxGate};

struct Probe;

impl Wake for Probe {
    fn wake(self: Arc<Self>) {}
}

fn poll_once<F: Future>(future: Pin<&mut F>) -> Poll<F::Output> {
    let waker = Waker::from(Arc::new(Probe));
    future.poll(&mut Context::from_waker(&waker))
}

fn entry(version: u64) -> Arc<WalEntryV2> {
    Arc::new(WalEntryV2 {
        txn_id: version,
        repo_id_interned: 7,
        started_at_ns: 0,
        commit_version: version,
        ops: Vec::new(),
        interner_delta: Vec::new(),
        event: None,
    })
}

#[test]
fn idle_diagnostics_are_healthy_and_zero() {
    assert_eq!(
        RepoTxGate::fresh().diagnostics(),
        GateDiagnostics::default()
    );
    let seeded = RepoTxGate::new(42, 1).diagnostics();
    assert_eq!(seeded.allocation_high_water, 42);
    assert_eq!(seeded.visible_floor, 42);
    assert_eq!(seeded.last_committed, 42);
    assert_eq!(seeded.durable_watermark, 42);
    assert_eq!(seeded.oldest_blocker, None);
    assert_eq!(seeded.unsettled_versions, 0);
}

#[tokio::test(start_paused = true)]
async fn ownership_transfers_keep_lazy_age_and_never_settle() {
    let gate = RepoTxGate::fresh();
    let mut first = gate.assign_next_version_guarded();
    let second = gate.assign_next_version_guarded();
    tokio::time::advance(Duration::from_secs(20)).await;
    let blocker = gate.diagnostics().oldest_blocker.unwrap();
    assert_eq!(blocker.version, 1);
    assert_eq!(blocker.class, GateBlockerClass::PendingUnclassified);
    assert_eq!(blocker.age, Duration::ZERO); // not age since allocation
    first.wal_accepted();
    tokio::time::advance(Duration::from_secs(3)).await;
    let blocker = gate.diagnostics().oldest_blocker.unwrap();
    assert_eq!(blocker.class, GateBlockerClass::ActiveTail);
    assert_eq!(blocker.age, Duration::from_secs(3));
    drop(first); // no drainer exists in this gate-only fixture
    tokio::time::advance(Duration::from_secs(2)).await;
    let blocker = gate.diagnostics().oldest_blocker.unwrap();
    assert_eq!(blocker.class, GateBlockerClass::OrphanAwaitingReplay);
    assert_eq!(blocker.age, Duration::from_secs(5));
    assert_eq!(gate.visible_floor(), 0);
    gate.complete_orphaned_post_wal(1);
    let blocker = gate.diagnostics().oldest_blocker.unwrap();
    assert_eq!(blocker.version, 2);
    assert_eq!(blocker.age, Duration::ZERO);
    second.commit();
    assert_eq!(gate.diagnostics().oldest_blocker, None);
    assert_eq!(gate.diagnostics().unsettled_versions, 0);
}

#[test]
fn pending_and_unknown_tickets_are_unresolved_unknown_keeps_reopen() {
    for state in [WalTicketState::Pending, WalTicketState::Unknown] {
        let gate = RepoTxGate::fresh();
        let mut guard = gate.assign_next_version_guarded();
        guard.wal_queued(Arc::new(WalTicket::detached_for_test(state)), entry(1));
        assert_eq!(
            gate.diagnostics().oldest_blocker.unwrap().class,
            GateBlockerClass::ActiveTail,
        );
        drop(guard);
        let diagnostic = gate.diagnostics();
        assert_eq!(diagnostic.visible_floor, 0);
        assert_eq!(diagnostic.unsettled_versions, 1);
        assert_eq!(
            diagnostic.oldest_blocker.unwrap().class,
            GateBlockerClass::UnresolvedTicket,
        );
        assert_eq!(diagnostic.requires_reopen, state == WalTicketState::Unknown);
        assert!(gate.unresolved_wal(1).is_some()); // read must not take ownership
    }
}

#[test]
fn follower_materialization_without_registry_is_pending_unclassified() {
    let gate = RepoTxGate::fresh();
    let mut guard = gate.assign_next_version_guarded();
    guard.visibility_started_without_wal();
    assert_eq!(
        gate.diagnostics().oldest_blocker.unwrap().class,
        GateBlockerClass::PendingUnclassified,
    );
    drop(guard);
    let diagnostic = gate.diagnostics();
    assert!(diagnostic.requires_reopen);
    assert_eq!(diagnostic.visible_floor, 0);
    assert_eq!(diagnostic.unsettled_versions, 1);
    assert_eq!(
        diagnostic.oldest_blocker.unwrap().class,
        GateBlockerClass::PendingUnclassified,
    );
}

#[test]
fn sparse_counts_distinguish_pending_missing_and_terminal_above_w() {
    let gate = RepoTxGate::fresh();
    for _ in 0..5 {
        gate.assign_next_version();
    }
    gate.completion().mark(2, CompletionState::Materialized);
    gate.completion().mark(3, CompletionState::Pending);
    gate.completion().mark(4, CompletionState::Aborted);
    gate.raise_published(5);
    let diagnostic = gate.diagnostics();
    assert_eq!(diagnostic.allocation_high_water, 5);
    assert_eq!(diagnostic.visible_floor, 0);
    assert_eq!(diagnostic.last_committed, 5);
    assert_eq!(diagnostic.durable_watermark, 0);
    assert_eq!(diagnostic.unsettled_versions, 3);
    assert_eq!(diagnostic.pending_versions, 1);
    assert_eq!(diagnostic.missing_versions, 2);
    assert_eq!(diagnostic.oldest_blocker.unwrap().version, 1);
    gate.completion().mark(1, CompletionState::Aborted);
    let diagnostic = gate.diagnostics();
    assert_eq!(diagnostic.visible_floor, 2);
    assert_eq!(diagnostic.oldest_blocker.unwrap().version, 3);
    assert_eq!(diagnostic.unsettled_versions, 2);
    assert_eq!(diagnostic.pending_versions, 1);
    assert_eq!(diagnostic.missing_versions, 1);
}

#[tokio::test(start_paused = true)]
async fn diagnostics_waiters_release_cancel_and_fast_path() {
    let gate = RepoTxGate::fresh();
    let guard = gate.assign_next_version_guarded();
    let mut first = Box::pin(gate.await_visible(1));
    let mut second = Box::pin(gate.await_visible(1));
    assert!(poll_once(first.as_mut()).is_pending());
    assert!(poll_once(second.as_mut()).is_pending());
    assert_eq!(gate.diagnostics().visibility_waiters, 2);
    drop(first);
    assert_eq!(gate.diagnostics().visibility_waiters, 1);
    guard.commit();
    assert_eq!(
        poll_once(second.as_mut()),
        Poll::Ready(VisibleOutcome::Visible)
    );
    assert_eq!(gate.diagnostics().visibility_waiters, 0);
    assert_eq!(gate.await_visible(1).await, VisibleOutcome::Visible);
    assert_eq!(gate.completion().waiter_registrations(), 2);
}

#[tokio::test(start_paused = true)]
async fn n_slow_waiters_share_rolling_warning_limiter_but_count_each_wait() {
    let gate = RepoTxGate::fresh();
    let other = RepoTxGate::fresh();
    let guard = gate.assign_next_version_guarded();
    let mut waits: Vec<_> = (0..16).map(|_| Box::pin(gate.await_visible(1))).collect();
    let mut independent = Box::pin(other.await_visible(1));
    for wait in &mut waits {
        assert!(poll_once(wait.as_mut()).is_pending());
    }
    assert!(poll_once(independent.as_mut()).is_pending());
    tokio::time::advance(Duration::from_millis(4_999)).await;
    for wait in &mut waits {
        assert!(poll_once(wait.as_mut()).is_pending());
    }
    assert_eq!(gate.visibility_warning_count(), 0);
    // The wait re-checks only after its 50 ms park slice expires.
    tokio::time::advance(Duration::from_millis(51)).await;
    for wait in &mut waits {
        assert!(poll_once(wait.as_mut()).is_pending());
    }
    assert!(poll_once(independent.as_mut()).is_pending());
    assert_eq!(gate.visibility_warning_count(), 1);
    assert_eq!(other.visibility_warning_count(), 1);
    assert_eq!(gate.completion().visible_wait_slow_count(), 16);
    assert_eq!(gate.diagnostics().visibility_waiters, 16);
    tokio::time::advance(Duration::from_millis(4_999)).await;
    for wait in &mut waits {
        assert!(poll_once(wait.as_mut()).is_pending());
    }
    assert_eq!(gate.visibility_warning_count(), 1);
    // The wait re-checks only after its 50 ms park slice expires.
    tokio::time::advance(Duration::from_millis(51)).await;
    for wait in &mut waits {
        assert!(poll_once(wait.as_mut()).is_pending());
    }
    assert_eq!(gate.visibility_warning_count(), 2);
    assert_eq!(gate.completion().visible_wait_slow_count(), 16);
    guard.commit();
    for wait in &mut waits {
        assert_eq!(
            poll_once(wait.as_mut()),
            Poll::Ready(VisibleOutcome::Visible)
        );
    }
    assert_eq!(gate.diagnostics().visibility_waiters, 0);
    drop(independent);
    assert_eq!(other.diagnostics().visibility_waiters, 0);
}
