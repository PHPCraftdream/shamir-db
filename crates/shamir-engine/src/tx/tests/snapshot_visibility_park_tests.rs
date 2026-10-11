use std::sync::atomic::Ordering;

use shamir_query_builder::{select, Query};
use shamir_storage::error::{DbError, DbResult};
use shamir_tx::{IsolationLevel, TxContext};
use shamir_types::types::common::new_map;
use shamir_types::types::record_id::RecordId;
use shamir_types::types::value::InnerValue;

use crate::query::filter::FilterContext;
use crate::query::read::{QueryResult, ReadQuery};
use crate::table::TableManager;
use crate::tx::materialize::TEST_PRE_MATERIALIZE_REPO;
use crate::tx::tests::post_wal_orphan_ordering_tests::fixture;
use crate::tx::MaterializationState;

async fn point(table: &TableManager, id: RecordId, tx: &TxContext) -> DbResult<Option<InnerValue>> {
    match table.read_one_tx(id, Some(tx)).await {
        Ok(value) => Ok(Some(value)),
        Err(DbError::NotFound(_)) => Ok(None),
        Err(error) => Err(error),
    }
}

async fn read(table: &TableManager, query: &ReadQuery, tx: &TxContext) -> DbResult<QueryResult> {
    let interner = table.interner().get().await?;
    let refs = new_map();
    let ctx = FilterContext::new(interner, &refs);
    table.read_tx(query, &ctx, Some(tx)).await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn snapshot_reads_stay_identical_after_earlier_parked_commit_materializes() {
    let (repo, hook) = fixture("snapshot_visibility_park").await;
    TEST_PRE_MATERIALIZE_REPO
        .set(repo.name().to_owned())
        .unwrap();
    let a = repo.get_table("a").await.unwrap();
    let b = repo.get_table("b").await.unwrap();
    let gate = repo.tx_gate().await.unwrap();
    let interner = a.interner().get().await.unwrap();
    let mut row = new_map();
    row.insert(
        interner.touch_ind("score").unwrap().into_key(),
        InnerValue::Int(10),
    );
    a.interner().persist().await.unwrap();
    let a_value = InnerValue::Map(row);
    let scan_query = Query::from("a").select(["score"]).build();
    let count_query = Query::from("a")
        .select([select::count_all("answer")])
        .build();

    let (mut tx_a, guard_a) = repo.begin_tx(IsolationLevel::Snapshot).await.unwrap();
    let a_id = a.insert_tx(&a_value, Some(&mut tx_a)).await.unwrap();
    let completion_before = gate.completion().watermark();
    hook.armed.store(true, Ordering::Release);
    let owned = repo.clone();
    let task_a = tokio::spawn(async move { owned.commit_tx(tx_a).await });
    hook.reached.notified().await;
    let a_version = *repo.drainer().window_keys().last().unwrap();
    assert!(gate.is_active_post_wal(a_version));
    assert!(completion_before < a_version);
    assert_eq!(gate.completion().watermark(), completion_before);
    assert_eq!(a.mvcc_store_ref().unwrap().version_of(a_id.as_bytes()), 0);

    let (mut tx_b, guard_b) = repo.begin_tx(IsolationLevel::Snapshot).await.unwrap();
    let b_value = InnerValue::Str("later-b".into());
    let b_id = b.insert_tx(&b_value, Some(&mut tx_b)).await.unwrap();
    let outcome_b = repo.commit_tx(tx_b).await.unwrap();
    assert_eq!(outcome_b.materialization, MaterializationState::Complete);
    assert!(outcome_b.background.is_none());
    assert_eq!(outcome_b.commit_version, a_version + 1);
    assert_eq!(
        b.mvcc_store_ref().unwrap().version_of(b_id.as_bytes()),
        outcome_b.commit_version,
    );
    assert_eq!(b.get(b_id).await.unwrap(), b_value);
    assert!(!gate.is_active_post_wal(outcome_b.commit_version));
    assert_eq!(gate.last_committed(), outcome_b.commit_version);
    assert_eq!(gate.completion().watermark(), completion_before);
    assert!(gate.is_active_post_wal(a_version));

    let (tx_c, guard_c) = repo.begin_tx(IsolationLevel::Snapshot).await.unwrap();
    let c_version = tx_c.snapshot_version;
    assert_eq!(c_version, outcome_b.commit_version);
    assert_eq!(guard_c.version(), c_version);
    assert!(a_version < c_version);
    assert!(gate.completion().watermark() < a_version);
    let point_before = point(&a, a_id, &tx_c).await;
    let scan_before = read(&a, &scan_query, &tx_c).await;
    let count_before = read(&a, &count_query, &tx_c).await;

    hook.resume.notify_one();
    let outcome_a = task_a.await.unwrap().unwrap();
    assert_eq!(outcome_a.commit_version, a_version);
    assert_eq!(outcome_a.materialization, MaterializationState::Complete);
    assert!(outcome_a.background.is_none());
    assert!(!gate.is_active_post_wal(a_version));
    assert_eq!(gate.completion().watermark(), outcome_b.commit_version);
    assert_eq!(gate.last_committed(), outcome_b.commit_version);
    assert_eq!(
        a.mvcc_store_ref().unwrap().version_of(a_id.as_bytes()),
        a_version,
    );
    assert_eq!(tx_c.snapshot_version, c_version);
    assert_eq!(guard_c.version(), c_version);
    let point_after = point(&a, a_id, &tx_c).await;
    let scan_after = read(&a, &scan_query, &tx_c).await;
    let count_after = read(&a, &count_query, &tx_c).await;

    let (tx_d, guard_d) = repo.begin_tx(IsolationLevel::Snapshot).await.unwrap();
    let point_d = point(&a, a_id, &tx_d).await;
    let scan_d = read(&a, &scan_query, &tx_d).await;
    let count_d = read(&a, &count_query, &tx_d).await;
    assert_eq!(tx_d.snapshot_version, outcome_b.commit_version);
    assert!(a_version <= tx_d.snapshot_version);
    assert_eq!(point_d.unwrap(), Some(a_value));
    let scan_d = scan_d.unwrap();
    assert_eq!(scan_d.records.len(), 1);
    assert_eq!(scan_d.records[0].get_value_i64("score"), Some(10));
    let count_d = count_d.unwrap();
    assert_eq!(count_d.records.len(), 1);
    assert_eq!(count_d.records[0].get_value_i64("answer"), Some(1));

    let point_before = point_before.unwrap();
    let scan_before = scan_before.unwrap();
    let count_before = count_before.unwrap();
    let point_after = point_after.unwrap();
    let scan_after = scan_after.unwrap();
    let count_after = count_after.unwrap();
    assert_eq!(point_before, None);
    assert!(scan_before.records.is_empty());
    assert_eq!(count_before.records.len(), 1);
    assert_eq!(count_before.records[0].get_value_i64("answer"), Some(0));
    assert_eq!(
        (point_after, scan_after.records, count_after.records),
        (point_before, scan_before.records, count_before.records),
        "Snapshot C point read, full scan and COUNT must remain identical",
    );
    drop((guard_a, guard_b, guard_c, guard_d));
}
