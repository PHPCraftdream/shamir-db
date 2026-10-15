use std::future::{poll_fn, Future};
use std::sync::Arc;
use std::time::Duration;

use futures::{FutureExt, StreamExt};
use shamir_query_types::batch::{VisibilityPendingReason, VisibilityState, VisibilityWait};
use shamir_storage::error::DbError;
use shamir_storage::storage_in_memory::InMemoryRepo;
use shamir_types::types::record_id::RecordId;
use shamir_types::types::value::InnerValue;
use tokio::sync::oneshot;

use crate::repo::repo_types::BoxRepo;
use crate::repo::RepoInstance;
use crate::table::{TableConfig, TableManager, WriteOutcome};

const STATUS: VisibilityWait = VisibilityWait::Bounded { timeout_ms: 0 };
const WRITES: [Write; 4] = [Write::Insert, Write::Many, Write::Delete, Write::Set];

#[derive(Clone, Copy, Debug)]
enum Write {
    Insert,
    Many,
    Delete,
    Set,
}

async fn fixture() -> (RepoInstance, TableManager, RecordId) {
    let repo = RepoInstance::new(
        "receipts".into(),
        BoxRepo::InMemory(Arc::new(InMemoryRepo::new())),
        Vec::new(),
    )
    .with_commit_visibility_wait(STATUS);
    repo.add_table(TableConfig::new("t"));
    let table = repo.get_table("t").await.unwrap();
    let id = table.insert(&InnerValue::Int(1)).await.unwrap();
    (repo, table, id)
}

async fn apply(
    table: &TableManager,
    write: Write,
    id: RecordId,
    wait: VisibilityWait,
) -> WriteOutcome<Vec<RecordId>> {
    let value = InnerValue::Int(2);
    match write {
        Write::Insert => {
            let r = table.insert_with_receipt(&value, wait).await.unwrap();
            WriteOutcome {
                value: vec![r.value],
                commit_version: r.commit_version,
                visibility: r.visibility,
            }
        }
        Write::Many => table
            .insert_many_with_receipt(&[value.clone(), value], wait)
            .await
            .unwrap(),
        Write::Delete => {
            let r = table.delete_with_receipt(id, wait).await.unwrap();
            assert!(r.value);
            WriteOutcome {
                value: vec![id],
                commit_version: r.commit_version,
                visibility: r.visibility,
            }
        }
        Write::Set => {
            let r = table.set_with_receipt(id, &value, wait).await.unwrap();
            assert!(!r.value, "set replacement preserves the real bool contract");
            WriteOutcome {
                value: vec![id],
                commit_version: r.commit_version,
                visibility: r.visibility,
            }
        }
    }
}

fn width(write: Write) -> u64 {
    if matches!(write, Write::Many) {
        2
    } else {
        1
    }
}

async fn assert_rows(table: &TableManager, write: Write, ids: &[RecordId]) {
    assert_eq!(u64::try_from(ids.len()).unwrap(), width(write));
    for &id in ids {
        if matches!(write, Write::Delete) {
            assert!(matches!(table.get(id).await, Err(DbError::NotFound(_))));
        } else {
            assert_eq!(table.get(id).await.unwrap(), InnerValue::Int(2));
        }
    }
}

// Pending writes are committed but not yet visible at W, so a Latest read cannot
// see them; the oracle reads the explicit committed version instead.
async fn assert_committed_at(table: &TableManager, write: Write, ids: &[RecordId], target: u64) {
    let mvcc = table.mvcc_store().unwrap();
    let rows = mvcc.snapshot_stream(16, target);
    tokio::pin!(rows);
    let mut found = Vec::new();
    while let Some(batch) = rows.next().await {
        for (key, bytes) in batch.unwrap() {
            found.push((
                RecordId::try_from_bytes(&key).unwrap(),
                InnerValue::from_bytes(bytes).unwrap(),
            ));
        }
    }
    for &id in ids {
        let row = found.iter().find(|(found_id, _)| *found_id == id);
        if matches!(write, Write::Delete) {
            assert!(row.is_none());
        } else {
            assert_eq!(row.map(|(_, value)| value), Some(&InnerValue::Int(2)));
        }
    }
}

// Explicit-version storage inspection captures generated IDs before ack returns.
// This bypasses W only for the test's committed-data oracle, not a Latest read
// or a visibility claim. No production hook or mutation is introduced.
async fn capture_written_ids(
    table: &TableManager,
    write: Write,
    id: RecordId,
    target: u64,
) -> Vec<RecordId> {
    if matches!(write, Write::Delete | Write::Set) {
        return vec![id];
    }
    let mvcc = table.mvcc_store().unwrap();
    let rows = mvcc.snapshot_stream(16, target);
    tokio::pin!(rows);
    let mut ids = Vec::new();
    while let Some(batch) = rows.next().await {
        for (key, bytes) in batch.unwrap() {
            if InnerValue::from_bytes(bytes).unwrap() == InnerValue::Int(2) {
                ids.push(RecordId::try_from_bytes(&key).unwrap());
            }
        }
    }
    ids
}

// Counterfactual: discarding observation or waiting Strict ignores the zero
// budget and returns Visible incorrectly or blocks behind the lower guard.
#[tokio::test]
async fn bounded_crud_preserves_positions_values_and_pending_prefix() {
    for write in WRITES {
        for slow in [false, true] {
            let (repo, table, id) = fixture().await;
            let gate = repo.tx_gate().await.unwrap();
            let lower = gate.assign_next_version_guarded();
            let barrier = if slow {
                let (barrier, lock) = table
                    .begin_write_barrier(crate::index::write_barrier_flags::SCHEMA_ACTIVATION)
                    .await
                    .unwrap();
                drop(lock);
                Some(barrier)
            } else {
                None
            };
            let floor = gate.visible_floor();
            let owned = table.clone();
            let task = tokio::spawn(async move { apply(&owned, write, id, STATUS).await });
            let receipt = task.await.unwrap();
            let target = receipt.commit_version.unwrap();
            assert_eq!(target, lower.version() + width(write));
            assert_eq!(
                receipt.visibility,
                VisibilityState::Pending {
                    visible_through: floor,
                    blocking_version: lower.version(),
                    reason: VisibilityPendingReason::Timeout,
                },
            );
            assert_eq!(gate.completion().waiter_count(), 0);
            drop(barrier);
            let (replay, lock) = table
                .begin_write_barrier(crate::index::write_barrier_flags::SCHEMA_ACTIVATION)
                .await
                .unwrap();
            drop(lock);
            drop(replay);
            lower.commit();
            assert_eq!(
                repo.observe_visibility(target, STATUS).await.unwrap(),
                VisibilityState::Visible,
            );
            assert_rows(&table, write, &receipt.value).await;
        }
    }
}

// Counterfactual: retaining either fast drain or slow UWL while observing
// prevents lower replay from acquiring its barrier. Observer cancellation must
// preserve committed rows AND the lower owner's active/replay obligation.
// Direct writes have no background tail; this test makes no tail-lifetime claim.
#[tokio::test(start_paused = true)]
async fn strict_observer_releases_all_crud_guards_and_preserves_recovery_on_cancel() {
    for write in WRITES {
        for slow in [false, true] {
            let (repo, table, id) = fixture().await;
            let gate = repo.tx_gate().await.unwrap();
            let mut lower = gate.assign_next_version_guarded();
            lower.wal_accepted();
            let lower_version = lower.version();
            let target = lower_version + width(write);
            let barrier = if slow {
                let (barrier, lock) = table
                    .begin_write_barrier(crate::index::write_barrier_flags::SCHEMA_ACTIVATION)
                    .await
                    .unwrap();
                drop(lock);
                Some(barrier)
            } else {
                None
            };
            let (sender, receiver) = oneshot::channel();
            let owned = table.clone();
            let observed = Arc::clone(&gate);
            let task = tokio::spawn(async move {
                let operation = apply(&owned, write, id, VisibilityWait::Strict);
                tokio::pin!(operation);
                let mut sender = Some(sender);
                poll_fn(|cx| {
                    let result = operation.as_mut().poll(cx);
                    if observed.completion().waiter_count() != 0 || result.is_ready() {
                        if let Some(sender) = sender.take() {
                            sender.send(result.is_pending()).unwrap();
                        }
                    }
                    result
                })
                .await
            });
            assert!(receiver.await.unwrap());
            assert_eq!(gate.version_allocation_high_water_mark(), target);
            assert_eq!(gate.completion().waiter_count(), 1);
            assert!(gate.is_active_post_wal(lower_version));
            let ids = capture_written_ids(&table, write, id, target).await;
            assert_committed_at(&table, write, &ids, target).await;
            drop(barrier);
            // Exact lower-replay protocol, while the Strict observer is parked.
            let (replay, lock) = table
                .begin_write_barrier(crate::index::write_barrier_flags::SCHEMA_ACTIVATION)
                .await
                .unwrap();
            drop(lock);
            drop(replay);
            task.abort();
            assert!(task.await.unwrap_err().is_cancelled());
            assert_eq!(gate.completion().waiter_count(), 0);
            assert_eq!(gate.visible_floor(), lower_version - 1);
            assert!(gate.is_active_post_wal(lower_version));
            assert_committed_at(&table, write, &ids, target).await;
            // Transfer the still-owned blocker to the existing legacy orphan
            // protocol. Cancellation must not have discharged this obligation.
            drop(lower);
            assert!(!gate.is_active_post_wal(lower_version));
            assert!(gate.is_orphaned_post_wal(lower_version));
            assert_eq!(gate.visible_floor(), lower_version - 1);
            gate.complete_orphaned_post_wal(lower_version);
            assert_eq!(gate.visible_floor(), target);
            gate.mark_durable(lower_version);
            gate.forget_orphaned_post_wal(lower_version);
            assert!(!gate.is_orphaned_post_wal(lower_version));
            assert_eq!(
                repo.observe_visibility(target, STATUS).await.unwrap(),
                VisibilityState::Visible,
            );
            assert_rows(&table, write, &ids).await;
        }
    }
}

// Counterfactual: omitting a CRUD tail's policy propagation makes its positive
// budget wait indefinitely or return Visible while the lower version is held.
#[tokio::test(start_paused = true)]
async fn bounded_crud_timers_expire_with_values_then_reconcile_visible() {
    for write in WRITES {
        let (repo, table, id) = fixture().await;
        let gate = repo.tx_gate().await.unwrap();
        let lower = gate.assign_next_version_guarded();
        let target = lower.version() + width(write);
        let owned = table.clone();
        let (sender, receiver) = oneshot::channel();
        let observed = Arc::clone(&gate);
        let task = tokio::spawn(async move {
            let operation = apply(
                &owned,
                write,
                id,
                VisibilityWait::Bounded { timeout_ms: 100 },
            );
            tokio::pin!(operation);
            let mut sender = Some(sender);
            poll_fn(|cx| {
                let result = operation.as_mut().poll(cx);
                if observed.completion().waiter_count() != 0 || result.is_ready() {
                    if let Some(sender) = sender.take() {
                        sender.send(result.is_pending()).unwrap();
                    }
                }
                result
            })
            .await
        });
        assert!(receiver.await.unwrap());
        assert_eq!(gate.completion().waiter_count(), 1);
        assert_eq!(gate.version_allocation_high_water_mark(), target);
        tokio::time::advance(Duration::from_millis(100)).await;
        let receipt = task.await.unwrap();
        assert_eq!(receipt.commit_version, Some(target));
        assert_eq!(
            receipt.visibility,
            VisibilityState::Pending {
                visible_through: lower.version() - 1,
                blocking_version: lower.version(),
                reason: VisibilityPendingReason::Timeout,
            },
        );
        assert_eq!(gate.completion().waiter_count(), 0);
        assert_committed_at(&table, write, &receipt.value, target).await;
        lower.commit();
        assert_eq!(
            repo.observe_visibility(target, STATUS).await.unwrap(),
            VisibilityState::Visible,
        );
        assert_rows(&table, write, &receipt.value).await;
    }
}

#[tokio::test(start_paused = true)]
async fn strict_adapter_returns_plain_value_after_settlement_despite_bounded_default() {
    let (repo, table, id) = fixture().await;
    let gate = repo.tx_gate().await.unwrap();
    let lower = gate.assign_next_version_guarded();
    let (sender, receiver) = oneshot::channel();
    let owned = table.clone();
    let observed = Arc::clone(&gate);
    let task = tokio::spawn(async move {
        let value = InnerValue::Int(5);
        let operation = owned.set(id, &value);
        tokio::pin!(operation);
        let mut sender = Some(sender);
        poll_fn(|cx| {
            let result = operation.as_mut().poll(cx);
            if observed.completion().waiter_count() != 0 || result.is_ready() {
                if let Some(sender) = sender.take() {
                    sender.send(result.is_pending()).unwrap();
                }
            }
            result
        })
        .await
    });
    assert!(
        receiver.await.unwrap(),
        "Strict adapter must wait despite bounded repository default",
    );
    assert_eq!(gate.completion().waiter_count(), 1);
    lower.commit();
    assert!(!task.await.unwrap().unwrap());
    assert_eq!(table.get(id).await.unwrap(), InnerValue::Int(5));
}

#[tokio::test]
async fn reopen_receipt_is_pending_and_strict_error_retains_committed_position() {
    let (repo, table, id) = fixture().await;
    let gate = repo.tx_gate().await.unwrap();
    let lower = gate.assign_next_version_guarded();
    gate.set_requires_reopen();
    let receipt = table
        .set_with_receipt(id, &InnerValue::Int(6), STATUS)
        .await
        .unwrap();
    assert!(!receipt.value);
    assert_eq!(receipt.commit_version, Some(lower.version() + 1));
    assert_eq!(
        receipt.visibility,
        VisibilityState::Pending {
            visible_through: lower.version() - 1,
            blocking_version: lower.version(),
            reason: VisibilityPendingReason::ReopenRequired,
        },
    );
    let error = table.set(id, &InnerValue::Int(7)).await.unwrap_err();
    assert_eq!(error.code(), Some("write_committed_visibility_pending"));
    assert!(matches!(
        error,
        DbError::WriteCommittedVisibilityPending {
            commit_version,
            visible_through,
            reason: "reopen_required",
        } if commit_version == lower.version() + 2 && visible_through == lower.version() - 1
    ));
    let lower_version = lower.version();
    lower.commit();
    assert_eq!(
        repo.observe_visibility(receipt.commit_version.unwrap(), STATUS)
            .await
            .unwrap(),
        VisibilityState::Visible,
    );
    assert_eq!(
        repo.observe_visibility(lower_version + 2, STATUS)
            .await
            .unwrap(),
        VisibilityState::Visible,
    );
    assert_eq!(table.get(id).await.unwrap(), InnerValue::Int(7));
}

// Counterfactual: minting a tombstone on a missing CRUD delete or reporting
// position zero exposes a fake reconciliation position; registering/timing a
// no-op introduces a needless wait behind lower W.
#[tokio::test(start_paused = true)]
async fn noops_and_visible_fast_path_do_not_register_or_suspend() {
    let (repo, table, _) = fixture().await;
    let gate = repo.tx_gate().await.unwrap();
    let lower = gate.assign_next_version_guarded();
    let allocated = gate.version_allocation_high_water_mark();
    let waiters = gate.completion().waiter_count();
    let missing = table
        .delete_with_receipt(RecordId::new(), VisibilityWait::Strict)
        .now_or_never()
        .unwrap()
        .unwrap();
    assert!(!missing.value);
    assert_eq!(missing.commit_version, None);
    assert_eq!(missing.visibility, VisibilityState::Visible);
    let empty = table
        .insert_many_with_receipt(
            &[],
            VisibilityWait::Bounded {
                timeout_ms: u64::MAX,
            },
        )
        .now_or_never()
        .unwrap()
        .unwrap();
    assert!(empty.value.is_empty());
    assert_eq!(empty.commit_version, None);
    assert_eq!(gate.version_allocation_high_water_mark(), allocated);
    assert_eq!(gate.completion().waiter_count(), waiters);
    lower.commit();
    let visible = table
        .insert_with_receipt(&InnerValue::Int(8), VisibilityWait::Strict)
        .now_or_never()
        .unwrap()
        .unwrap();
    assert_eq!(visible.visibility, VisibilityState::Visible);
    assert!(visible.commit_version.is_some());
    assert_eq!(gate.completion().waiter_count(), waiters);
}
