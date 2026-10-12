//! SNAP-4: drive the real AsOf read, observing its first poll rather than time.

use std::future::{poll_fn, Future};
use std::sync::Arc;

use bytes::Bytes;
use shamir_query_builder::Query;
use shamir_storage::error::{DbError, DbResult};
use shamir_storage::storage_in_memory::InMemoryStore;
use shamir_storage::types::{KvOp, RecordKey, Store};
use shamir_tx::{MvccStore, RepoTxGate};
use shamir_types::types::common::new_map;
use shamir_types::types::record_id::RecordId;
use shamir_types::types::value::InnerValue;
use tokio::sync::oneshot;
use tokio::task::JoinHandle;

use crate::query::filter::FilterContext;
use crate::query::read::QueryResult;
use crate::table::TableManager;

async fn fixture() -> (TableManager, Arc<MvccStore>, Arc<RepoTxGate>, KvOp) {
    let data: Arc<dyn Store> = Arc::new(InMemoryStore::new());
    let info: Arc<dyn Store> = Arc::new(InMemoryStore::new());
    let history: Arc<dyn Store> = Arc::new(InMemoryStore::new());
    let gate = Arc::new(RepoTxGate::fresh());
    let mvcc = Arc::new(MvccStore::new(history, Arc::clone(&gate)));
    let table = TableManager::create("t".into(), data, info)
        .await
        .unwrap()
        .with_mvcc_store(Arc::clone(&mvcc))
        .with_changefeed(Arc::clone(&gate));
    let interner = table.interner().get().await.unwrap();
    let mut row = new_map();
    row.insert(
        interner.touch_ind("score").unwrap().into_key(),
        InnerValue::Int(10),
    );
    let bytes = Bytes::from(rmp_serde::to_vec(&InnerValue::Map(row)).unwrap());
    let key = RecordKey::from(RecordId([1; 16]).to_bytes());
    (table, mvcc, gate, KvOp::Set(key, bytes))
}

async fn read(table: &TableManager, version: u64) -> DbResult<QueryResult> {
    let query = Query::from("t")
        .select(["score"])
        .as_of_version(version)
        .build();
    let interner = table.interner().get().await?;
    let refs = new_map();
    let ctx = FilterContext::new(interner, &refs);
    table.read(&query, &ctx).await
}

// The channel reports the actual read poll's result, even on the broken code
// where the read returns Ready immediately. No scheduler/timing assumptions.
async fn spawn_read(
    table: TableManager,
    version: u64,
) -> (bool, JoinHandle<DbResult<QueryResult>>) {
    let (sender, receiver) = oneshot::channel();
    let task = tokio::spawn(async move {
        let read = read(&table, version);
        tokio::pin!(read);
        let mut sender = Some(sender);
        poll_fn(|cx| {
            let result = read.as_mut().poll(cx);
            if let Some(sender) = sender.take() {
                sender.send(result.is_pending()).unwrap();
            }
            result
        })
        .await
    });
    (receiver.await.unwrap(), task)
}

async fn waits_for_lower_version(abort: bool) {
    let (table, mvcc, gate, op) = fixture().await;
    let lower = gate.assign_next_version_guarded();
    let higher = gate.assign_next_version_guarded();
    let version = higher.version();
    higher.commit();
    gate.raise_published(version);
    let (pending, task) = spawn_read(table.clone(), version).await;
    // Settle before asserting so a red assertion cannot strand the read task.
    if abort {
        drop(lower);
    } else {
        mvcc.apply_committed_visible(&[op], lower.version());
        lower.commit();
    }
    let result = task.await.unwrap().unwrap();
    assert!(
        pending,
        "AsOf above W must wait, not return an unstable scan"
    );
    assert_eq!(gate.visible_floor(), version);
    if abort {
        assert!(result.records.is_empty());
    } else {
        assert_eq!(result.records.len(), 1);
        assert_eq!(result.records[0].get_value_i64("score"), Some(10));
    }
    let again = read(&table, version).await.unwrap();
    assert_eq!(again.records.len(), result.records.len());
}

#[tokio::test]
async fn as_of_waits_until_lower_version_materializes() {
    waits_for_lower_version(false).await;
}

#[tokio::test]
async fn as_of_waits_until_lower_version_aborts() {
    waits_for_lower_version(true).await;
}

#[tokio::test]
async fn as_of_unallocated_version_is_typed_error() {
    let (table, _mvcc, gate, _op) = fixture().await;
    let requested = gate.version_allocation_high_water_mark() + 1;
    let error = read(&table, requested).await.unwrap_err();
    assert_eq!(error.code(), Some("as_of_future_version"));
    assert!(matches!(
        error,
        DbError::AsOfFutureVersion {
            requested: 1,
            allocated: 0,
        },
    ));
}

#[tokio::test]
async fn as_of_at_watermark_keeps_rows_and_does_not_wait_on_higher_gap() {
    let (table, mvcc, gate, op) = fixture().await;
    let settled = gate.assign_next_version_guarded();
    let version = settled.version();
    mvcc.apply_committed_visible(&[op], version);
    settled.commit();
    let lower = gate.assign_next_version_guarded();
    let higher = gate.assign_next_version_guarded();
    gate.raise_published(higher.version());
    higher.commit();
    gate.set_requires_reopen();
    let result = read(&table, version).await.unwrap();
    assert_eq!(result.records.len(), 1);
    assert_eq!(result.records[0].get_value_i64("score"), Some(10));
    let empty = read(&table, 0).await.unwrap();
    assert!(empty.records.is_empty());
    drop(lower);
}

#[tokio::test(start_paused = true)]
async fn as_of_wait_exits_if_reopen_becomes_required() {
    let (table, _mvcc, gate, _op) = fixture().await;
    let lower = gate.assign_next_version_guarded();
    let (pending, task) = spawn_read(table, lower.version()).await;
    gate.set_requires_reopen();
    // Paused Tokio time auto-advances the gate's reopen-check park slice.
    let error = task.await.unwrap().unwrap_err();
    assert!(pending, "allocated AsOf must first park below W");
    assert_eq!(error.code(), Some("as_of_reopen_required"));
    assert!(matches!(
        error,
        DbError::AsOfReopenRequired { requested: 1 }
    ));
    drop(lower);
}

#[tokio::test]
async fn as_of_reopen_required_is_typed_error() {
    let (table, _mvcc, gate, _op) = fixture().await;
    let lower = gate.assign_next_version_guarded();
    gate.set_requires_reopen();
    let error = read(&table, lower.version()).await.unwrap_err();
    assert_eq!(error.code(), Some("as_of_reopen_required"));
    assert!(matches!(
        error,
        DbError::AsOfReopenRequired { requested: 1 }
    ));
    drop(lower);
}
