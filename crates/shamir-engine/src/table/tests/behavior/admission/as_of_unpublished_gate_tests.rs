//! S5c-X PR6b-2 round 5 — REAL AsOf gate regression (review P2 follow-up):
//! the round-4 coverage only drove `MvccStore::get_at` and the raw ts index;
//! it never drove `TableManager`'s actual AsOf read (`read_temporal.rs`).
//!
//! This file pins the gate bound on the REAL path: a row that is durably
//! MATERIALIZED but not yet PUBLISHED (floor below its version, guard still
//! armed — the follower receipt's materialize→publish window) must NEVER be
//! returned by an AsOf read, on either the version route (`At::Version`) or
//! the timestamp route (`At::Timestamp`, which resolves via the
//! publication-unaware ts index to the unpublished version and is then
//! bounded by the same gate check).
//!
//! Trace of what the code actually does (`read_temporal.rs:64-128`):
//! the resolved `version` is compared against `gate.visible_floor()`;
//! `2 > 1` → `allocated = version_allocation_high_water_mark()` = 2 (the
//! guard's `assign_next_version` fetch_add), so `2 > allocated` is FALSE —
//! NOT `AsOfFutureVersion`. The read then parks in
//! `observe_gate_with_budget`; with a bounded budget it ends in
//! `VisibilityState::Pending` (timeout) → `DbError::AsOfVisibilityTimeout`
//! with `visible_through = 1`, `blocking_version = 2`.

use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use futures::Future;
use shamir_query_builder::Query;
use shamir_query_types::read::ReadQuery;
use shamir_storage::error::{DbError, DbResult};
use shamir_storage::storage_in_memory::InMemoryStore;
use shamir_storage::types::{KvOp, RecordKey, Store};
use shamir_tx::{MvccStore, RepoTxGate};
use shamir_types::types::common::new_map;
use shamir_types::types::record_id::RecordId;
use shamir_types::types::value::InnerValue;
use tokio::sync::oneshot;

use crate::query::filter::FilterContext;
use crate::query::read::QueryResult;
use crate::table::TableManager;

/// Two rows on DIFFERENT keys: `row(score=10)` on key 1, `row(score=20)` on
/// key 2 — so the assertions can distinguish which row a read served.
async fn fixture() -> (TableManager, Arc<MvccStore>, Arc<RepoTxGate>, KvOp, KvOp) {
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
    let op1 = KvOp::Set(
        RecordKey::from(RecordId([1; 16]).to_bytes()),
        Bytes::from(rmp_serde::to_vec(&InnerValue::Map(row)).unwrap()),
    );

    let mut row = new_map();
    row.insert(
        interner.touch_ind("score").unwrap().into_key(),
        InnerValue::Int(20),
    );
    let op2 = KvOp::Set(
        RecordKey::from(RecordId([2; 16]).to_bytes()),
        Bytes::from(rmp_serde::to_vec(&InnerValue::Map(row)).unwrap()),
    );

    (table, mvcc, gate, op1, op2)
}

async fn read_version(table: &TableManager, version: u64) -> DbResult<QueryResult> {
    let query = Query::from("t")
        .select(["score"])
        .as_of_version(version)
        .build();
    let interner = table.interner().get().await.unwrap();
    let refs = new_map();
    let ctx = FilterContext::new(interner, &refs);
    table.read(&query, &ctx).await
}

async fn read_timestamp(table: &TableManager, ts: u64) -> DbResult<QueryResult> {
    let query = Query::from("t")
        .select(["score"])
        .as_of_timestamp(ts)
        .build();
    let interner = table.interner().get().await.unwrap();
    let refs = new_map();
    let ctx = FilterContext::new(interner, &refs);
    table.read(&query, &ctx).await
}

/// Bounded-visibility read context (17 ms — parked-time slices, fast under
/// `start_paused`).
fn budget_read(
    table: &TableManager,
    query: ReadQuery,
) -> std::pin::Pin<Box<impl std::future::Future<Output = DbResult<QueryResult>> + '_>> {
    let interner = table.interner();
    Box::pin(async move {
        let interner = interner.get().await.unwrap();
        let refs = new_map();
        let ctx = FilterContext::new(interner, &refs)
            .with_read_visibility_budget(Duration::from_millis(17));
        table.read(&query, &ctx).await
    })
}

#[tokio::test(start_paused = true)]
async fn as_of_real_read_bounds_materialized_unpublished_version() {
    let (table, mvcc, gate, op1, op2) = fixture().await;

    // v1: materialize AND publish (visible) → floor 1.
    let guard1 = gate.assign_next_version_guarded();
    assert_eq!(guard1.version(), 1);
    mvcc.apply_committed_visible(std::slice::from_ref(&op1), 1);
    guard1.commit();
    assert_eq!(gate.visible_floor(), 1);

    // v2: MATERIALIZED but never published — the receipt's
    // materialize→publish window. The guard stays armed (uncommitted), so
    // the completion tracker keeps v2 pending and the floor stays at 1;
    // the allocation high-water mark is 2.
    let guard2 = gate.assign_next_version_guarded();
    assert_eq!(guard2.version(), 2);
    mvcc.materialize_committed_to_history(std::slice::from_ref(&op2), 2)
        .await
        .unwrap();
    assert_eq!(gate.visible_floor(), 1, "publish must not have happened");
    assert_eq!(gate.version_allocation_high_water_mark(), 2);

    // The ts index is publication-unaware: it DOES resolve to the
    // unpublished v2 — this is exactly what the timestamp route feeds the
    // gate check below.
    assert_eq!(
        mvcc.version_at_or_before_ts(u64::MAX).await.unwrap(),
        Some(2)
    );

    // ── Published version: the real AsOf read returns ONLY the v1 row. ──
    let result = read_version(&table, 1).await.unwrap();
    assert_eq!(result.records.len(), 1, "only the published row is visible");
    assert_eq!(result.records[0].get_value_i64("score"), Some(10));

    // ── Unpublished version (At::Version): parks, then times out. Never
    //    returns the v2 row. ──
    let query = Query::from("t").select(["score"]).as_of_version(2).build();
    let mut read = budget_read(&table, query);
    {
        // First poll must PARK (version above the floor), not serve data.
        let (sender, receiver) = oneshot::channel();
        let mut sender = Some(sender);
        std::future::poll_fn(|cx| {
            if let Some(sender) = sender.take() {
                sender.send(read.as_mut().poll(cx).is_pending()).unwrap();
            }
            std::task::Poll::Ready(())
        })
        .await;
        assert!(receiver.await.unwrap(), "AsOf above the floor must park");
    }
    tokio::time::advance(Duration::from_millis(17)).await;
    let error = read.await.unwrap_err();
    assert_eq!(error.code(), Some("as_of_visibility_timeout"));
    assert!(
        matches!(
            error,
            DbError::AsOfVisibilityTimeout {
                requested: 2,
                visible_through: 1,
                blocking_version: 2,
            },
        ),
        "unexpected error: {error:?}"
    );

    // ── Timestamp route: ts index resolves to the UNPUBLISHED v2, and the
    //    same gate bound applies — timeout, never the v2 row. ──
    let query = Query::from("t")
        .select(["score"])
        .as_of_timestamp(u64::MAX)
        .build();
    let mut read = budget_read(&table, query);
    // Register the timer (first poll parks in the gate observation) BEFORE
    // advancing paused time, mirroring the budget-timeout test above.
    std::future::poll_fn(|cx| {
        assert!(read.as_mut().poll(cx).is_pending());
        std::task::Poll::Ready(())
    })
    .await;
    tokio::time::advance(Duration::from_millis(17)).await;
    let error = read.await.unwrap_err();
    assert_eq!(error.code(), Some("as_of_visibility_timeout"));
    assert!(matches!(
        error,
        DbError::AsOfVisibilityTimeout {
            requested: 2,
            visible_through: 1,
            blocking_version: 2,
        },
    ));

    // Publish v2 → both routes return BOTH rows (v1 published, v2 published).
    mvcc.publish_materialized(&[op2], 2);
    guard2.commit();
    let result = read_version(&table, 2).await.unwrap();
    assert_eq!(result.records.len(), 2);
    let result = read_timestamp(&table, u64::MAX).await.unwrap();
    assert_eq!(result.records.len(), 2);
}
