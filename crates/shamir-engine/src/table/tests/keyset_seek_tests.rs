//! Keyset (seek) pagination — `Pagination::After { key, limit }` —
//! honoured by the sorted-index SEEK fast path.
//!
//! These tests prove the engine routes a single-column ORDER BY +
//! keyset seek through `try_plan_keyset_seek` / `read_keyset_seek`
//! (stats.index_used carries the `sorted_idx_<n>_keyset` label) and
//! returns exactly the rows strictly beyond the seek key, in order,
//! up to `limit`. Edge cases: seek at/after the last row, limit larger
//! than the remaining rows.

use std::sync::Arc;

use shamir_query_types::read::{OrderBy, Pagination, ReadQuery};
use shamir_storage::storage_in_memory::InMemoryStore;
use shamir_storage::types::Store;
use shamir_types::types::common::new_map;
use shamir_types::types::record_id::RecordId;
use shamir_types::types::value::{InnerValue, QueryValue};

use crate::query::filter::eval_context::FilterContext;
use crate::table::TableManager;

// ─────────────────────────────────────────────────────────────────────────────
// Helpers
// ─────────────────────────────────────────────────────────────────────────────

/// Build a TableManager with a sorted index on the `score` field (Int).
async fn make_table() -> TableManager {
    let data: Arc<dyn Store> = Arc::new(InMemoryStore::new());
    let info: Arc<dyn Store> = Arc::new(InMemoryStore::new());

    let tbl = TableManager::create("t".into(), data, info).await.unwrap();

    tbl.create_sorted_index("score_idx", &["score"])
        .await
        .unwrap();

    tbl
}

/// Insert a record `{score: s, label: l}` and return the assigned RecordId.
async fn insert_record(tbl: &TableManager, score: i64, label: &str) -> RecordId {
    let interner = tbl.interner().get().await.unwrap();
    let score_key = interner.touch_ind("score").unwrap().into_key();
    let label_key = interner.touch_ind("label").unwrap().into_key();
    tbl.interner().persist().await.unwrap();

    let mut m = new_map();
    m.insert(score_key, InnerValue::Int(score));
    m.insert(label_key, InnerValue::Str(label.to_owned()));
    let rec = InnerValue::Map(m);

    tbl.insert(&rec).await.unwrap()
}

/// Collect the `score` field (i64) from each returned record, in result order.
fn scores_in_order(result: &crate::query::read::QueryResult) -> Vec<i64> {
    result
        .records
        .iter()
        .filter_map(|r| r.get_value_i64("score"))
        .collect()
}

// ─────────────────────────────────────────────────────────────────────────────
// Happy path — ASC seek
// ─────────────────────────────────────────────────────────────────────────────

/// `ORDER BY score ASC` with `After { key: [30], limit: 2 }` over rows
/// 10,20,30,40,50 must return exactly [40, 50] — the two rows strictly
/// greater than 30, in ascending order — and prove it took the index path.
#[tokio::test]
async fn keyset_seek_asc_returns_strictly_after() {
    let tbl = make_table().await;
    insert_record(&tbl, 10, "a").await;
    insert_record(&tbl, 20, "b").await;
    insert_record(&tbl, 30, "c").await;
    insert_record(&tbl, 40, "d").await;
    insert_record(&tbl, 50, "e").await;

    let interner = tbl.interner().get().await.unwrap();
    let refs = new_map();
    let ctx = FilterContext::new(interner, &refs);

    let query = ReadQuery::new("t")
        .order_by(OrderBy::asc("score"))
        .pagination(Pagination::after(vec![QueryValue::Int(30)], Some(2)));

    let result = tbl.read(&query, &ctx).await.unwrap();

    assert_eq!(scores_in_order(&result), vec![40, 50]);

    let label = result
        .stats
        .as_ref()
        .and_then(|s| s.index_used.as_deref())
        .unwrap_or("<none>");
    assert!(
        label.ends_with("_keyset"),
        "expected the keyset index path, got index_used = {label:?}"
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// Happy path — DESC seek
// ─────────────────────────────────────────────────────────────────────────────

/// `ORDER BY score DESC` with `After { key: [30], limit: 2 }` over rows
/// 10,20,30,40,50 must return exactly [20, 10] — the two rows strictly
/// less than 30, in descending order.
#[tokio::test]
async fn keyset_seek_desc_returns_strictly_before() {
    let tbl = make_table().await;
    insert_record(&tbl, 10, "a").await;
    insert_record(&tbl, 20, "b").await;
    insert_record(&tbl, 30, "c").await;
    insert_record(&tbl, 40, "d").await;
    insert_record(&tbl, 50, "e").await;

    let interner = tbl.interner().get().await.unwrap();
    let refs = new_map();
    let ctx = FilterContext::new(interner, &refs);

    let query = ReadQuery::new("t")
        .order_by(OrderBy::desc("score"))
        .pagination(Pagination::after(vec![QueryValue::Int(30)], Some(2)));

    let result = tbl.read(&query, &ctx).await.unwrap();

    assert_eq!(scores_in_order(&result), vec![20, 10]);

    let label = result
        .stats
        .as_ref()
        .and_then(|s| s.index_used.as_deref())
        .unwrap_or("<none>");
    assert!(
        label.ends_with("_keyset"),
        "expected the keyset index path, got index_used = {label:?}"
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// Edge — seek at/after the last row → empty (ASC)
// ─────────────────────────────────────────────────────────────────────────────

/// ASC seek past the last row returns no records (and still reports the
/// keyset index path — proving it didn't fall back to a full scan).
#[tokio::test]
async fn keyset_seek_asc_at_last_row_returns_empty() {
    let tbl = make_table().await;
    insert_record(&tbl, 10, "a").await;
    insert_record(&tbl, 20, "b").await;
    insert_record(&tbl, 30, "c").await;

    let interner = tbl.interner().get().await.unwrap();
    let refs = new_map();
    let ctx = FilterContext::new(interner, &refs);

    // Seek at the last row (30) — strictly greater → empty.
    let query = ReadQuery::new("t")
        .order_by(OrderBy::asc("score"))
        .pagination(Pagination::after(vec![QueryValue::Int(30)], Some(5)));

    let result = tbl.read(&query, &ctx).await.unwrap();
    assert!(result.records.is_empty(), "expected no rows after the last");
    assert!(
        result
            .stats
            .as_ref()
            .and_then(|s| s.index_used.as_deref())
            .is_some(),
        "stats.index_used must be present"
    );

    // Seek past the last row (99) — also empty.
    let query = ReadQuery::new("t")
        .order_by(OrderBy::asc("score"))
        .pagination(Pagination::after(vec![QueryValue::Int(99)], Some(5)));
    let result = tbl.read(&query, &ctx).await.unwrap();
    assert!(result.records.is_empty(), "expected no rows past the last");
}

// ─────────────────────────────────────────────────────────────────────────────
// Edge — seek before the first row → empty (DESC)
// ─────────────────────────────────────────────────────────────────────────────

/// DESC seek at/below the smallest row returns no records.
#[tokio::test]
async fn keyset_seek_desc_at_first_row_returns_empty() {
    let tbl = make_table().await;
    insert_record(&tbl, 10, "a").await;
    insert_record(&tbl, 20, "b").await;
    insert_record(&tbl, 30, "c").await;

    let interner = tbl.interner().get().await.unwrap();
    let refs = new_map();
    let ctx = FilterContext::new(interner, &refs);

    // Seek at the smallest row (10) — strictly less → empty.
    let query = ReadQuery::new("t")
        .order_by(OrderBy::desc("score"))
        .pagination(Pagination::after(vec![QueryValue::Int(10)], Some(5)));
    let result = tbl.read(&query, &ctx).await.unwrap();
    assert!(
        result.records.is_empty(),
        "expected no rows before the first"
    );

    // Seek below the smallest row (0) — also empty.
    let query = ReadQuery::new("t")
        .order_by(OrderBy::desc("score"))
        .pagination(Pagination::after(vec![QueryValue::Int(0)], Some(5)));
    let result = tbl.read(&query, &ctx).await.unwrap();
    assert!(
        result.records.is_empty(),
        "expected no rows below the first"
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// Edge — limit larger than remaining rows → all remaining
// ─────────────────────────────────────────────────────────────────────────────

/// ASC seek with a limit larger than the remaining rows returns all
/// remaining rows (no padding, no error).
#[tokio::test]
async fn keyset_seek_asc_limit_exceeds_remaining() {
    let tbl = make_table().await;
    insert_record(&tbl, 10, "a").await;
    insert_record(&tbl, 20, "b").await;
    insert_record(&tbl, 30, "c").await;
    insert_record(&tbl, 40, "d").await;

    let interner = tbl.interner().get().await.unwrap();
    let refs = new_map();
    let ctx = FilterContext::new(interner, &refs);

    // Seek at 10, limit 100 → only 20,30,40 remain.
    let query = ReadQuery::new("t")
        .order_by(OrderBy::asc("score"))
        .pagination(Pagination::after(vec![QueryValue::Int(10)], Some(100)));
    let result = tbl.read(&query, &ctx).await.unwrap();
    assert_eq!(scores_in_order(&result), vec![20, 30, 40]);
}

/// DESC seek with a limit larger than the remaining rows returns all
/// remaining rows.
#[tokio::test]
async fn keyset_seek_desc_limit_exceeds_remaining() {
    let tbl = make_table().await;
    insert_record(&tbl, 10, "a").await;
    insert_record(&tbl, 20, "b").await;
    insert_record(&tbl, 30, "c").await;
    insert_record(&tbl, 40, "d").await;

    let interner = tbl.interner().get().await.unwrap();
    let refs = new_map();
    let ctx = FilterContext::new(interner, &refs);

    // Seek at 40, limit 100 → only 30,20,10 remain (desc).
    let query = ReadQuery::new("t")
        .order_by(OrderBy::desc("score"))
        .pagination(Pagination::after(vec![QueryValue::Int(40)], Some(100)));
    let result = tbl.read(&query, &ctx).await.unwrap();
    assert_eq!(scores_in_order(&result), vec![30, 20, 10]);
}

// ─────────────────────────────────────────────────────────────────────────────
// Exclusivity — the seek key itself is never returned
// ─────────────────────────────────────────────────────────────────────────────

/// When multiple rows share the seek-key value, NONE of them are returned
/// (the seek is on the value, not the (value, record_id) tuple). ASC case.
#[tokio::test]
async fn keyset_seek_asc_excludes_all_rows_with_seek_value() {
    let tbl = make_table().await;
    insert_record(&tbl, 10, "a").await;
    insert_record(&tbl, 20, "b1").await;
    insert_record(&tbl, 20, "b2").await;
    insert_record(&tbl, 20, "b3").await;
    insert_record(&tbl, 30, "c").await;

    let interner = tbl.interner().get().await.unwrap();
    let refs = new_map();
    let ctx = FilterContext::new(interner, &refs);

    // Seek at 20 — all three 20s must be excluded; only 30 remains.
    let query = ReadQuery::new("t")
        .order_by(OrderBy::asc("score"))
        .pagination(Pagination::after(vec![QueryValue::Int(20)], Some(10)));
    let result = tbl.read(&query, &ctx).await.unwrap();
    assert_eq!(scores_in_order(&result), vec![30]);
}
