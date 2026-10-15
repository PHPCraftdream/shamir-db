//! SNAP-5: acknowledgement belongs after the complete engine write sequence.

use std::future::{poll_fn, Future};
use std::sync::Arc;

use futures::FutureExt;
use shamir_storage::storage_in_memory::InMemoryRepo;
use shamir_tx::IsolationLevel;
use shamir_types::types::value::InnerValue;
use tokio::sync::oneshot;

use crate::repo::repo_types::BoxRepo;
use crate::repo::RepoInstance;
use crate::table::TableConfig;

async fn parked_insert(abort_lower: bool) {
    let repo = RepoInstance::new(
        "non_tx_ack".into(),
        BoxRepo::InMemory(Arc::new(InMemoryRepo::new())),
        Vec::new(),
    );
    repo.add_table_for_test(TableConfig::new("t"));
    let table = repo.get_table("t").await.unwrap();
    let gate = repo.tx_gate().await.unwrap();
    let mvcc = table.mvcc_store().unwrap();
    let lower = gate.assign_next_version_guarded();
    let lower_version = lower.version();
    let higher_version = lower_version + 1;
    let (sender, receiver) = oneshot::channel();
    let owned = table.clone();
    let observed = Arc::clone(&mvcc);
    let task = tokio::spawn(async move {
        let value = InnerValue::Str("acknowledged".into());
        let insert = owned.insert(&value);
        tokio::pin!(insert);
        let mut sender = Some(sender);
        poll_fn(|cx| {
            let result = insert.as_mut().poll(cx);
            // The overlay is installed after the history write. Observe the
            // actual insert poll, not merely that the task was scheduled.
            if observed.overlay_len() != 0 {
                if let Some(sender) = sender.take() {
                    sender.send(result.is_pending()).unwrap();
                }
            }
            result
        })
        .await
    });
    // Red before ack relocation: insert's materializing poll returns Ready.
    assert!(receiver.await.unwrap(), "insert acknowledged above W");
    assert_eq!(gate.visible_floor(), lower_version - 1);
    assert!(!task.is_finished());

    // A lower orphan's posting replay uses this exact barrier protocol.
    // Acquiring it while ack is pending proves the writer drain was released.
    let (barrier, lock) = table
        .begin_write_barrier(crate::index::write_barrier_flags::SCHEMA_ACTIVATION)
        .await
        .unwrap();
    drop(lock);
    drop(barrier);
    if abort_lower {
        drop(lower);
    } else {
        lower.commit();
    }
    let id = task.await.unwrap().unwrap();
    assert!(gate.visible_floor() >= higher_version);
    let (reader, pin) = repo.begin_tx(IsolationLevel::Snapshot).await.unwrap();
    assert!(reader.snapshot_version >= higher_version);
    assert_eq!(
        table.read_one_tx(id, Some(&reader)).await.unwrap(),
        InnerValue::Str("acknowledged".into()),
    );
    drop(reader);
    drop(pin);
}

#[tokio::test]
async fn non_tx_insert_waits_for_lower_completion_and_snapshot_sees_row() {
    parked_insert(false).await;
}

#[tokio::test]
async fn non_tx_insert_waits_for_lower_abort_and_snapshot_sees_row() {
    parked_insert(true).await;
}

#[tokio::test]
async fn non_tx_insert_already_visible_does_not_suspend() {
    let repo = RepoInstance::new(
        "non_tx_ack_ready".into(),
        BoxRepo::InMemory(Arc::new(InMemoryRepo::new())),
        Vec::new(),
    );
    repo.add_table_for_test(TableConfig::new("t"));
    let table = repo.get_table("t").await.unwrap();
    let gate = repo.tx_gate().await.unwrap();
    let value = InnerValue::Str("ready".into());
    let id = table.insert(&value).now_or_never().unwrap().unwrap();
    assert_eq!(
        gate.visible_floor(),
        gate.version_allocation_high_water_mark(),
    );
    let (reader, pin) = repo.begin_tx(IsolationLevel::Snapshot).await.unwrap();
    assert_eq!(table.read_one_tx(id, Some(&reader)).await.unwrap(), value);
    drop(reader);
    drop(pin);
}
