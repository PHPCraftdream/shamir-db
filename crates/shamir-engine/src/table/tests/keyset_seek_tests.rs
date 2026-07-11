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

/// Extract each returned row's `_id` (task #537 tie-breaker channel) as a
/// parsed `RecordId`, simulating the real client flow: the server serializes
/// the row (injecting `_id` via the `InsertedRecord` wire pattern), the client
/// deserializes it and reads the `_id` string back out.
fn ids_in_order(result: &crate::query::read::QueryResult) -> Vec<RecordId> {
    result
        .records
        .iter()
        .filter_map(|r| {
            // Round-trip through msgpack so `_id` lands in the row map exactly
            // as a real client would receive it, then read + parse it.
            let bytes = rmp_serde::to_vec_named(r).ok()?;
            let back: crate::query::read::QueryRecord = rmp_serde::from_slice(&bytes).ok()?;
            let id_str = back.get_value_str("_id")?.to_owned();
            id_str.parse::<RecordId>().ok()
        })
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

// ─────────────────────────────────────────────────────────────────────────────
// Task #537 — record-id tie-breaker for ties on the ORDER BY value
// ─────────────────────────────────────────────────────────────────────────────

/// REGRESSION (#537): documents the CURRENT data-loss bug for a client that
/// does NOT send the tie-breaker. Rows tied on the ORDER BY value that
/// straddle a page boundary are silently and permanently dropped.
///
/// Rows (score ASC): 10, 20a, 20b, 20c, 30. Page 1 = `after([10], limit=2)`
/// returns [20a, 20b]. A naive next page seeks past the bare value 20 —
/// `after([20], limit=2)` — which skips ALL THREE 20s, so 20c is never
/// returned on ANY page: the result jumps straight to [30]. 20c is lost.
#[tokio::test]
async fn keyset_seek_ties_lost_without_tiebreaker_regression() {
    let tbl = make_table().await;
    insert_record(&tbl, 10, "a").await;
    insert_record(&tbl, 20, "b1").await;
    insert_record(&tbl, 20, "b2").await;
    insert_record(&tbl, 20, "b3").await;
    insert_record(&tbl, 30, "c").await;

    let interner = tbl.interner().get().await.unwrap();
    let refs = new_map();
    let ctx = FilterContext::new(interner, &refs);

    // Page 1: two of the three tied-at-20 rows.
    let page1 = tbl
        .read(
            &ReadQuery::new("t")
                .order_by(OrderBy::asc("score"))
                .pagination(Pagination::after(vec![QueryValue::Int(10)], Some(2))),
            &ctx,
        )
        .await
        .unwrap();
    assert_eq!(scores_in_order(&page1), vec![20, 20]);

    // Page 2 the OLD way (bare value seek, no tie-breaker): the third 20 is
    // gone — result jumps to [30]. This is the bug #537 fixes; the client
    // that opts in (next test) recovers it.
    let page2_buggy = tbl
        .read(
            &ReadQuery::new("t")
                .order_by(OrderBy::asc("score"))
                .pagination(Pagination::after(vec![QueryValue::Int(20)], Some(2))),
            &ctx,
        )
        .await
        .unwrap();
    assert_eq!(
        scores_in_order(&page2_buggy),
        vec![30],
        "documents the pre-#537 data-loss: the third tied 20 is dropped"
    );
}

/// A client that echoes back the previous page's last `_id` as the tie-breaker
/// gets EVERY tied row across pages — the third 20 is recovered, nothing lost.
///
/// Page 1 (`after([10], limit=2)`) → [20a, 20b]; client remembers 20b's `_id`.
/// Page 2 (`after_with_id([20], limit=2, Some(id_of_20b))`) → [20c, 30].
#[tokio::test]
async fn keyset_seek_tiebreaker_recovers_all_ties_across_pages() {
    let tbl = make_table().await;
    insert_record(&tbl, 10, "a").await;
    insert_record(&tbl, 20, "b1").await;
    insert_record(&tbl, 20, "b2").await;
    insert_record(&tbl, 20, "b3").await;
    insert_record(&tbl, 30, "c").await;

    let interner = tbl.interner().get().await.unwrap();
    let refs = new_map();
    let ctx = FilterContext::new(interner, &refs);

    // Page 1: two of the three tied-at-20 rows.
    let page1 = tbl
        .read(
            &ReadQuery::new("t")
                .order_by(OrderBy::asc("score"))
                .pagination(Pagination::after(vec![QueryValue::Int(10)], Some(2))),
            &ctx,
        )
        .await
        .unwrap();
    assert_eq!(scores_in_order(&page1), vec![20, 20]);

    // The client echoes the LAST row's id (the 2nd of the tied 20s).
    let page1_ids = ids_in_order(&page1);
    assert_eq!(page1_ids.len(), 2, "each row must surface its _id");
    let last_id = *page1_ids.last().unwrap();

    // Page 2 WITH the tie-breaker: the third 20 is recovered, then 30.
    let page2 = tbl
        .read(
            &ReadQuery::new("t").order_by(OrderBy::asc("score")).pagination(
                Pagination::after_with_id(vec![QueryValue::Int(20)], Some(2), Some(last_id)),
            ),
            &ctx,
        )
        .await
        .unwrap();
    assert_eq!(
        scores_in_order(&page2),
        vec![20, 30],
        "tie-breaker recovers the third tied 20 (lost without it), then 30"
    );

    // Full scroll must yield each distinct row exactly once — no loss, no dup.
    let page2_ids = ids_in_order(&page2);
    let mut all: Vec<RecordId> = Vec::new();
    all.extend(page1_ids);
    all.extend(page2_ids);
    all.sort();
    all.dedup();
    assert_eq!(
        all.len(),
        4,
        "the two 20a/20b + the recovered 20c + 30 = 4 distinct rows, no dups"
    );
}

/// DESC mirror: tie-breaker recovers tied rows walking high→low.
///
/// Rows: 30, 20a, 20b, 20c, 10. Page 1 (`DESC after([30], limit=2)`) →
/// [20a', 20b'] (the two largest-id 20s, since DESC orders record_id
/// descending within the value). Page 2 with the tie-breaker → [20c', 10].
#[tokio::test]
async fn keyset_seek_desc_tiebreaker_recovers_all_ties_across_pages() {
    let tbl = make_table().await;
    insert_record(&tbl, 10, "a").await;
    insert_record(&tbl, 20, "b1").await;
    insert_record(&tbl, 20, "b2").await;
    insert_record(&tbl, 20, "b3").await;
    insert_record(&tbl, 30, "c").await;

    let interner = tbl.interner().get().await.unwrap();
    let refs = new_map();
    let ctx = FilterContext::new(interner, &refs);

    let page1 = tbl
        .read(
            &ReadQuery::new("t")
                .order_by(OrderBy::desc("score"))
                .pagination(Pagination::after(vec![QueryValue::Int(30)], Some(2))),
            &ctx,
        )
        .await
        .unwrap();
    assert_eq!(scores_in_order(&page1), vec![20, 20]);
    let last_id = *ids_in_order(&page1).last().unwrap();

    let page2 = tbl
        .read(
            &ReadQuery::new("t")
                .order_by(OrderBy::desc("score"))
                .pagination(Pagination::after_with_id(
                    vec![QueryValue::Int(20)],
                    Some(2),
                    Some(last_id),
                )),
            &ctx,
        )
        .await
        .unwrap();
    assert_eq!(
        scores_in_order(&page2),
        vec![20, 10],
        "DESC tie-breaker recovers the third tied 20, then 10"
    );
}

/// Old-client behavior is byte-identical: `after(..)` with no tie-breaker
/// behaves EXACTLY as it did before #537 — all rows sharing the seek value
/// are excluded (the pre-existing skip-all-ties limitation, unchanged).
/// This guards against a regression that would silently change results for
/// callers who never opt in.
#[tokio::test]
async fn keyset_seek_without_tiebreaker_unchanged_skip_all_ties() {
    let tbl = make_table().await;
    insert_record(&tbl, 10, "a").await;
    insert_record(&tbl, 20, "b1").await;
    insert_record(&tbl, 20, "b2").await;
    insert_record(&tbl, 20, "b3").await;
    insert_record(&tbl, 30, "c").await;

    let interner = tbl.interner().get().await.unwrap();
    let refs = new_map();
    let ctx = FilterContext::new(interner, &refs);

    // No tie-breaker: seek past bare value 20 → all three 20s skipped, [30].
    let result = tbl
        .read(
            &ReadQuery::new("t")
                .order_by(OrderBy::asc("score"))
                .pagination(Pagination::after(vec![QueryValue::Int(20)], Some(10))),
            &ctx,
        )
        .await
        .unwrap();
    assert_eq!(
        scores_in_order(&result),
        vec![30],
        "old-client (no after_id) behavior must be unchanged: skip all ties"
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// Audit 1.2 — keyset seek must NOT fetch/decode the whole remaining table
// ─────────────────────────────────────────────────────────────────────────────

/// Over a large table with a small `limit`, the keyset-seek path must scan
/// (fetch + decode + project) only ~`limit` records — NOT the entire
/// remaining half-plane. Before the ordered early-stop fix, seeking near
/// the start of a 500-row table with `limit = 5` fetched and decoded all
/// ~495 rows past the seek key, then sorted and truncated — O(N) per page.
/// We assert `records_scanned` is bounded by `limit`, proving the walk
/// stops early instead of materialising the whole tail.
#[tokio::test]
async fn keyset_seek_scans_only_limit_not_whole_tail() {
    let tbl = make_table().await;
    let n: i64 = 500;
    for s in 0..n {
        insert_record(&tbl, s, &format!("r{s}")).await;
    }

    let interner = tbl.interner().get().await.unwrap();
    let refs = new_map();
    let ctx = FilterContext::new(interner, &refs);

    // Seek at 10 → 489 rows remain in the half-plane (11..=499), but with
    // limit = 5 we must return exactly [11,12,13,14,15] and touch ~5 rows.
    let limit = 5u64;
    let query = ReadQuery::new("t")
        .order_by(OrderBy::asc("score"))
        .pagination(Pagination::after(vec![QueryValue::Int(10)], Some(limit)));

    let result = tbl.read(&query, &ctx).await.unwrap();
    assert_eq!(scores_in_order(&result), vec![11, 12, 13, 14, 15]);

    let scanned = result
        .stats
        .as_ref()
        .map(|s| s.records_scanned)
        .unwrap_or(u64::MAX);
    assert!(
        scanned <= limit,
        "keyset seek scanned {scanned} records for limit {limit} — expected \
         a bounded ~limit walk, not the whole remaining tail"
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// G2 (#526) — short page on STALE sorted-index entries
// ─────────────────────────────────────────────────────────────────────────────

/// Plant a STALE sorted-index entry: a posting for `score` → `fake_id`
/// where `fake_id` has NO record body in the data store. `get_many_bytes`
/// will return `None` for it, so it must be treated as a dead entry that
/// the keyset walk skips over while still continuing further into the range.
async fn plant_stale_sorted_entry(tbl: &TableManager, score: i64) -> RecordId {
    let interner = tbl.interner().get().await.unwrap();
    let score_key = interner.touch_ind("score").unwrap().into_key();
    tbl.interner().persist().await.unwrap();

    let mut m = new_map();
    m.insert(score_key, InnerValue::Int(score));
    let fake_rec = InnerValue::Map(m);

    let fake_id = RecordId::new();
    // Writes a sorted-index posting (score → fake_id) but no record body,
    // so the read path's get_many_bytes yields None for this id.
    tbl.sorted_indexes()
        .on_record_created(&fake_id, &fake_rec, 1)
        .await
        .unwrap();
    fake_id
}

/// REGRESSION (#526): the first `limit` PHYSICAL index entries beyond the
/// seek boundary include stale postings (record bodies gone). Naively
/// collecting the first-`limit`-physical-entries and dropping the dead ones
/// under-fills the page. The fix must keep advancing through the range until
/// `limit` LIVE rows are collected.
///
/// Layout (score ASC): live 11, STALE 12, live 13, STALE 14, live 15,
/// live 16, live 17, … Seek at 10, limit 3.
/// Pre-fix: physical first-3 = [11(live), 12(stale), 13(live)] → after
/// dropping the stale 12, only [11, 13] survive → SHORT page of 2.
/// Post-fix: [11, 13, 15] — three LIVE rows, in order.
#[tokio::test]
async fn keyset_seek_asc_stale_entries_still_fill_full_page() {
    let tbl = make_table().await;
    // Live rows.
    for s in [11, 13, 15, 16, 17] {
        insert_record(&tbl, s, &format!("r{s}")).await;
    }
    // Interleaved stale postings that under-fill the naive first-k page.
    plant_stale_sorted_entry(&tbl, 12).await;
    plant_stale_sorted_entry(&tbl, 14).await;

    let interner = tbl.interner().get().await.unwrap();
    let refs = new_map();
    let ctx = FilterContext::new(interner, &refs);

    let query = ReadQuery::new("t")
        .order_by(OrderBy::asc("score"))
        .pagination(Pagination::after(vec![QueryValue::Int(10)], Some(3)));

    let result = tbl.read(&query, &ctx).await.unwrap();

    // Full page of 3 LIVE rows, in ascending order — the stale 12/14 are
    // skipped and the walk continued to 15 to fill the page.
    assert_eq!(
        scores_in_order(&result),
        vec![11, 13, 15],
        "stale index entries must not shorten the page when live rows remain"
    );

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

/// DESC mirror of the above: seek high, walk down, stale entries interleaved.
///
/// Layout (score DESC from seek 100): live 50, STALE 49, live 48, STALE 47,
/// live 46, live 45. Seek at 100, limit 3 → [50, 48, 46].
#[tokio::test]
async fn keyset_seek_desc_stale_entries_still_fill_full_page() {
    let tbl = make_table().await;
    for s in [50, 48, 46, 45] {
        insert_record(&tbl, s, &format!("r{s}")).await;
    }
    plant_stale_sorted_entry(&tbl, 49).await;
    plant_stale_sorted_entry(&tbl, 47).await;

    let interner = tbl.interner().get().await.unwrap();
    let refs = new_map();
    let ctx = FilterContext::new(interner, &refs);

    let query = ReadQuery::new("t")
        .order_by(OrderBy::desc("score"))
        .pagination(Pagination::after(vec![QueryValue::Int(100)], Some(3)));

    let result = tbl.read(&query, &ctx).await.unwrap();

    assert_eq!(
        scores_in_order(&result),
        vec![50, 48, 46],
        "DESC: stale entries must not shorten the page when live rows remain"
    );
}

/// A GENUINE last-page short return: the range really IS exhausted with
/// fewer than `limit` live rows (some of them stale). The read path must
/// return exactly the live rows that exist and MUST NOT loop forever
/// re-seeking an exhausted range.
#[tokio::test]
async fn keyset_seek_asc_genuine_short_last_page_terminates() {
    let tbl = make_table().await;
    // Only two live rows beyond the seek, plus a couple of stale postings.
    insert_record(&tbl, 11, "a").await;
    insert_record(&tbl, 13, "b").await;
    plant_stale_sorted_entry(&tbl, 12).await;
    plant_stale_sorted_entry(&tbl, 14).await; // trailing stale — nothing live after.

    let interner = tbl.interner().get().await.unwrap();
    let refs = new_map();
    let ctx = FilterContext::new(interner, &refs);

    // limit 5 but only 2 live rows exist beyond seek 10 → return [11, 13],
    // and the loop terminates (range exhausted) rather than spinning.
    let query = ReadQuery::new("t")
        .order_by(OrderBy::asc("score"))
        .pagination(Pagination::after(vec![QueryValue::Int(10)], Some(5)));

    // If the fix regressed into an unbounded re-seek loop this test would
    // hang; nextest's per-test timeout turns that into a visible failure.
    let result = tbl.read(&query, &ctx).await.unwrap();

    assert_eq!(
        scores_in_order(&result),
        vec![11, 13],
        "genuine last page must return exactly the live rows and stop"
    );
}

/// REGRESSION (#537, found by adversarial review of the tie-breaker's first
/// implementation): the tie-breaker must survive the internal stale-posting
/// retry loop WITHIN A SINGLE request, not just across separate client
/// requests. `read_keyset_seek`'s internal loop (G2/#526) resumes multiple
/// times in one call whenever a stale posting forces another physical page;
/// an earlier version of the fix dropped `after_id` to `None` on every
/// iteration after the first, reopening the exact permanent-data-loss bug
/// #537 exists to close for any tied row sitting behind a stale posting.
///
/// Layout (score ASC): 10 (anchor), then at value 20: live b1, a STALE
/// posting, live b2, live b3 (record-ids increase with insertion order since
/// `RecordId::new()` is timestamp-prefixed). Client already has b1 and sends
/// `after_id = b1`, `limit = 2`. The internal walk's first physical page
/// (need=2) collects [stale, b2] — `get_many_bytes` drops the stale entry,
/// leaving only 1 live row, so a second internal page is needed. Pre-fix,
/// that second page dropped the tie-breaker and skipped ALL boundary-value
/// rows, permanently losing b3. Post-fix: [b2, b3], both live, both
/// distinct from the stale entry and from b1.
#[tokio::test]
async fn keyset_seek_tiebreaker_survives_stale_posting_in_same_request() {
    let tbl = make_table().await;
    insert_record(&tbl, 10, "anchor").await;
    let b1 = insert_record(&tbl, 20, "b1").await;
    plant_stale_sorted_entry(&tbl, 20).await;
    let b2 = insert_record(&tbl, 20, "b2").await;
    let b3 = insert_record(&tbl, 20, "b3").await;

    let interner = tbl.interner().get().await.unwrap();
    let refs = new_map();
    let ctx = FilterContext::new(interner, &refs);

    // Single request: client already has b1, wants the next 2 tied rows.
    let query = ReadQuery::new("t").order_by(OrderBy::asc("score")).pagination(
        Pagination::after_with_id(vec![QueryValue::Int(20)], Some(2), Some(b1)),
    );
    let result = tbl.read(&query, &ctx).await.unwrap();

    assert_eq!(
        scores_in_order(&result),
        vec![20, 20],
        "both remaining tied rows must be returned in one request, even \
         though a stale posting forced an internal retry page"
    );
    assert_eq!(
        ids_in_order(&result),
        vec![b2, b3],
        "must be exactly b2 then b3 — not the stale entry, not b1 again, \
         and b3 must NOT be silently dropped by a re-seek that forgot the \
         tie-breaker on the internal retry"
    );
}
