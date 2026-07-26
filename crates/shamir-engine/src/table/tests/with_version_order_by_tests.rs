//! F-7 (#797): `with_version` + reorder/collapse read paths.
//!
//! Covers the three behaviour changes introduced by F-7:
//!   1. Plain `ORDER BY` (no GROUP BY / aggregate / DISTINCT) + `with_version`
//!      now actually WORKS — `QueryResult::versions` is `Some(...)` and is
//!      index-aligned with the REORDERED records (each row's `RecordId` is
//!      threaded through the sort, and the version array is rebuilt from the
//!      repositioned ids). Includes a pagination sub-case proving the paged
//!      `versions` slice lines up with the paged `records` slice.
//!   2. `GROUP BY` / aggregates / `DISTINCT` + `with_version` are now a HARD
//!      `DbError::Validation` at request time (no single version applies to a
//!      collapsed row) instead of silently returning `versions: None`.
//!   3. `ORDER BY` + `with_version` on a non-MVCC table keeps `versions: None`
//!      (mirrors the documented FG-2 non-MVCC exception — with_version is
//!      opt-in assistance, never a correctness contract).
//!
//! Wire-layer note: a read-path `DbError::Validation` surfaces to clients as a
//! `BatchError::QueryError { code: None, message: "<thiserror Display>" }`
//! (the read branch's `map_err` in `query_runner.rs` does not propagate
//! `DbError::code()`, and `Validation`'s `code()` is `None` anyway). The
//! `thiserror` Display is `"Validation error: <msg>"`, so the inner `msg`
//! asserted here is exactly what rides the wire inside that wrapper.

use std::sync::Arc;

use shamir_query_types::read::select::Select;
use shamir_query_types::read::{AggFunc, AggregateField, GroupBy, OrderBy, ReadQuery, SelectItem};
use shamir_storage::error::DbError;
use shamir_storage::storage_in_memory::InMemoryStore;
use shamir_storage::types::Store;
use shamir_tx::{MvccStore, RepoTxGate};
use shamir_types::types::common::new_map;
use shamir_types::types::record_id::RecordId;
use shamir_types::types::value::InnerValue;

use crate::query::filter::eval_context::FilterContext;
use crate::table::TableManager;

// ─────────────────────────────────────────────────────────────────────────────
// Helpers
// ─────────────────────────────────────────────────────────────────────────────

/// A plain (NO sorted index) MVCC-backed table. An `ORDER BY` query against it
/// is forced through `read_collecting`'s in-memory sort (not an index-ordered
/// scan), which is exactly the path F-7's id-threading modifies. Returns the
/// table and its `MvccStore` so tests can read ground-truth versions via
/// `version_of`.
async fn make_plain_mvcc_table() -> (TableManager, Arc<MvccStore>) {
    let data: Arc<dyn Store> = Arc::new(InMemoryStore::new());
    let info: Arc<dyn Store> = Arc::new(InMemoryStore::new());
    let history: Arc<dyn Store> = Arc::new(InMemoryStore::new());
    let base = TableManager::create("t".into(), Arc::clone(&data), Arc::clone(&info))
        .await
        .unwrap();
    let gate = Arc::new(RepoTxGate::fresh());
    let mvcc = Arc::new(MvccStore::new(history, Arc::clone(&gate)));
    (base.with_mvcc_store(Arc::clone(&mvcc)), mvcc)
}

/// A plain (no index, no MVCC) table — `mvcc_store_ref()` is `None`, so
/// `with_version` must yield `versions: None` (not an error).
async fn make_plain_non_mvcc_table() -> TableManager {
    let data: Arc<dyn Store> = Arc::new(InMemoryStore::new());
    let info: Arc<dyn Store> = Arc::new(InMemoryStore::new());
    TableManager::create("t".into(), data, info).await.unwrap()
}

/// Insert `{score: s}` and return the assigned `RecordId`.
async fn insert_scored(tbl: &TableManager, score: i64) -> RecordId {
    let interner = tbl.interner().get().await.unwrap();
    let score_key = interner.touch_ind("score").unwrap().into_key();
    tbl.interner().persist().await.unwrap();
    let mut m = new_map();
    m.insert(score_key, InnerValue::Int(score));
    tbl.insert(&InnerValue::Map(m)).await.unwrap()
}

/// Assert `err` is a `DbError::Validation` whose message mentions `needle`.
fn assert_with_version_validation(err: DbError, needle: &str) {
    match err {
        DbError::Validation(msg) => assert!(
            msg.contains(needle),
            "expected Validation error mentioning '{needle}', got: {msg}"
        ),
        other => panic!("expected DbError::Validation, got: {other:?}"),
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Test 1 — plain ORDER BY + with_version threads RecordId through the sort
// ─────────────────────────────────────────────────────────────────────────────

/// Plain `ORDER BY score` (no group_by / agg / distinct) + `with_version` must:
///   - return `versions: Some(...)`,
///   - keep `versions[i]` aligned with the REORDERED `records[i]` (not scan
///     order) — i.e. the version is the canonical version of whichever source
///     row landed at position `i` after the sort,
///   - hold for both ASC and DESC, and
///   - hold across a pagination slice (`LIMIT`/`OFFSET`): the paged `versions`
///     line up with the paged `records`.
#[tokio::test]
async fn plain_order_by_with_version_threads_ids_through_sort() {
    let (tbl, mvcc) = make_plain_mvcc_table().await;
    // Insert in deliberately NON-sorted order so ORDER BY truly reorders.
    let rids = [
        insert_scored(&tbl, 10).await,
        insert_scored(&tbl, 50).await,
        insert_scored(&tbl, 20).await,
        insert_scored(&tbl, 40).await,
        insert_scored(&tbl, 30).await,
    ];
    let inserted_scores = [10i64, 50, 20, 40, 30];

    // Ground truth: each (score, version) straight from the MVCC store.
    let truth: Vec<(i64, u64)> = inserted_scores
        .iter()
        .zip(rids.iter())
        .map(|(&s, rid)| (s, mvcc.version_of(rid.as_bytes())))
        .collect();
    let mut truth_asc = truth.clone();
    truth_asc.sort_by_key(|(s, _)| *s);
    let mut truth_desc = truth.clone();
    truth_desc.sort_by_key(|(s, _)| std::cmp::Reverse(*s));

    let interner = tbl.interner().get().await.unwrap();
    let refs = new_map();
    let ctx = FilterContext::new(interner, &refs);

    // ── ASC ──────────────────────────────────────────────────────────────
    let mut q_asc = ReadQuery::new("t").order_by(OrderBy::asc("score"));
    q_asc.with_version = true;
    let res_asc = tbl.read(&q_asc, &ctx).await.unwrap();
    let versions_asc = res_asc
        .versions
        .as_ref()
        .expect("plain ORDER BY + with_version must populate versions");
    assert_eq!(
        versions_asc.len(),
        res_asc.records.len(),
        "versions must be index-aligned with records"
    );
    let scores_asc: Vec<i64> = res_asc
        .records
        .iter()
        .filter_map(|r| r.get_value_i64("score"))
        .collect();
    assert_eq!(
        scores_asc,
        vec![10, 20, 30, 40, 50],
        "records sorted ascending by score"
    );
    assert_eq!(
        versions_asc,
        &truth_asc
            .iter()
            .map(|(_, v)| *v)
            .collect::<Vec<_>>(),
        "versions[i] must be the version of the record NOW at position i (post-sort), not scan order"
    );

    // ── DESC (reordering must be a true inverse, not identity) ───────────
    let mut q_desc = ReadQuery::new("t").order_by(OrderBy::desc("score"));
    q_desc.with_version = true;
    let res_desc = tbl.read(&q_desc, &ctx).await.unwrap();
    let versions_desc = res_desc
        .versions
        .as_ref()
        .expect("plain ORDER BY DESC + with_version must populate versions");
    let scores_desc: Vec<i64> = res_desc
        .records
        .iter()
        .filter_map(|r| r.get_value_i64("score"))
        .collect();
    assert_eq!(scores_desc, vec![50, 40, 30, 20, 10], "records sorted desc");
    assert_eq!(
        versions_desc,
        &truth_desc.iter().map(|(_, v)| *v).collect::<Vec<_>>(),
        "DESC versions must follow the DESC-reordered records"
    );

    // ── Pagination slice: ORDER BY score ASC LIMIT 2 OFFSET 1 → 20, 30 ────
    // Forces the full-sort path (with_version excludes the top-K heap), so the
    // id-threading + apply_pagination lockstep slicing is exercised.
    let mut q_page = ReadQuery::new("t")
        .order_by(OrderBy::asc("score"))
        .limit(2)
        .offset(1);
    q_page.with_version = true;
    let res_page = tbl.read(&q_page, &ctx).await.unwrap();
    let versions_page = res_page
        .versions
        .as_ref()
        .expect("paginated ORDER BY + with_version must populate versions");
    let scores_page: Vec<i64> = res_page
        .records
        .iter()
        .filter_map(|r| r.get_value_i64("score"))
        .collect();
    assert_eq!(
        scores_page,
        vec![20, 30],
        "paginated slice (skip 1, take 2)"
    );
    assert_eq!(versions_page.len(), scores_page.len());
    for (i, &score) in scores_page.iter().enumerate() {
        let expected_v = truth_asc
            .iter()
            .find(|(s, _)| *s == score)
            .map(|(_, v)| *v)
            .unwrap();
        assert_eq!(
            versions_page[i], expected_v,
            "paginated versions[{i}] must match the record at that page position (score {score})"
        );
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Test 2 — GROUP BY + with_version → hard error
// ─────────────────────────────────────────────────────────────────────────────

/// `GROUP BY city` + `count(*)` + `with_version` must be rejected: a group
/// collapses many source rows into one output row, so no single version
/// applies. The error is raised at request time, before any scan.
#[tokio::test]
async fn group_by_with_version_is_rejected() {
    let (tbl, _mvcc) = make_plain_mvcc_table().await;
    insert_scored(&tbl, 10).await;
    insert_scored(&tbl, 20).await;

    let interner = tbl.interner().get().await.unwrap();
    let refs = new_map();
    let ctx = FilterContext::new(interner, &refs);

    let mut q = ReadQuery::new("t").select(Select {
        items: vec![
            SelectItem::Field {
                path: vec!["score".into()],
                alias: None,
            },
            SelectItem::Aggregate {
                func: AggFunc::Count,
                field: AggregateField::All,
                alias: Some("cnt".into()),
                distinct: false,
            },
        ],
        distinct: false,
    });
    q.group_by = Some(GroupBy::new(["score"]));
    q.with_version = true;

    let err = tbl.read(&q, &ctx).await.unwrap_err();
    assert_with_version_validation(err, "with_version");
}

// ─────────────────────────────────────────────────────────────────────────────
// Test 3 — aggregate SELECT + with_version → hard error
// ─────────────────────────────────────────────────────────────────────────────

/// Two aggregate shapes are rejected, including `count(*)` (no WHERE) which
/// would otherwise be served by the O(1) record-counter shortcut and silently
/// return `versions: None` — the exact silent gap F-7 closes by checking the
/// combination up-front in `read_impl`, before that shortcut.
#[tokio::test]
async fn aggregate_with_version_is_rejected() {
    let (tbl, _mvcc) = make_plain_mvcc_table().await;
    insert_scored(&tbl, 10).await;
    insert_scored(&tbl, 20).await;

    let interner = tbl.interner().get().await.unwrap();
    let refs = new_map();
    let ctx = FilterContext::new(interner, &refs);

    // (a) SELECT count(*) FROM t  — the shortcut-eligible shape.
    let mut q_count = ReadQuery::new("t").select(Select {
        items: vec![SelectItem::CountAll { alias: None }],
        distinct: false,
    });
    q_count.with_version = true;
    let err = tbl.read(&q_count, &ctx).await.unwrap_err();
    assert_with_version_validation(err, "with_version");

    // (b) SELECT sum(score) FROM t  — full-scan aggregate.
    let mut q_sum = ReadQuery::new("t").select(Select {
        items: vec![SelectItem::Aggregate {
            func: AggFunc::Sum,
            field: AggregateField::Field(vec!["score".into()]),
            alias: Some("total".into()),
            distinct: false,
        }],
        distinct: false,
    });
    q_sum.with_version = true;
    let err = tbl.read(&q_sum, &ctx).await.unwrap_err();
    assert_with_version_validation(err, "with_version");
}

// ─────────────────────────────────────────────────────────────────────────────
// Test 4 — DISTINCT + with_version → hard error
// ─────────────────────────────────────────────────────────────────────────────

/// `SELECT DISTINCT score` + `with_version` must be rejected: DISTINCT
/// collapses duplicate-duplicate rows into one output row, so (like GROUP BY)
/// which of several possible source versions `versions[i]` would mean is
/// ill-defined.
#[tokio::test]
async fn distinct_with_version_is_rejected() {
    let (tbl, _mvcc) = make_plain_mvcc_table().await;
    insert_scored(&tbl, 10).await;
    insert_scored(&tbl, 10).await; // deliberate duplicate
    insert_scored(&tbl, 20).await;

    let interner = tbl.interner().get().await.unwrap();
    let refs = new_map();
    let ctx = FilterContext::new(interner, &refs);

    let mut q = ReadQuery::new("t").select(Select {
        items: vec![SelectItem::Field {
            path: vec!["score".into()],
            alias: None,
        }],
        distinct: true,
    });
    q.with_version = true;

    let err = tbl.read(&q, &ctx).await.unwrap_err();
    assert_with_version_validation(err, "with_version");
}

// ─────────────────────────────────────────────────────────────────────────────
// Test 5 — ORDER BY + with_version on a NON-MVCC table → versions stays None
// ─────────────────────────────────────────────────────────────────────────────

/// On a table with no MVCC backing store, `with_version` yields `versions:
/// None` (not an error) — mirroring the documented FG-2 non-MVCC exception.
/// `with_version` is opt-in assistance, never a correctness contract, and the
/// plain-ORDER BY path honours that by leaving `versions` unset when there is
/// no version authority.
#[tokio::test]
async fn order_by_with_version_non_mvcc_keeps_versions_none() {
    let tbl = make_plain_non_mvcc_table().await;
    insert_scored(&tbl, 30).await;
    insert_scored(&tbl, 10).await;
    insert_scored(&tbl, 20).await;

    let interner = tbl.interner().get().await.unwrap();
    let refs = new_map();
    let ctx = FilterContext::new(interner, &refs);

    let mut q = ReadQuery::new("t").order_by(OrderBy::asc("score"));
    q.with_version = true;
    let res = tbl.read(&q, &ctx).await.unwrap();

    // The read still succeeds and returns sorted records...
    let scores: Vec<i64> = res
        .records
        .iter()
        .filter_map(|r| r.get_value_i64("score"))
        .collect();
    assert_eq!(
        scores,
        vec![10, 20, 30],
        "ORDER BY still sorts on non-MVCC table"
    );
    // ...but versions is None — no MVCC authority on this table.
    assert!(
        res.versions.is_none(),
        "versions must stay None for a non-MVCC table even with with_version"
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// Test 6 — ORDER BY + LIMIT + with_version + count_total together (F-23/#816)
// ─────────────────────────────────────────────────────────────────────────────

/// F-23 (#816): pin the F-6∩F-7 interaction the post-wave `/crush` review
/// (NF-5, `docs/dev-artifacts/research/2026-07-26-wave-f-post-review-crush/
/// REPORT.md`) flagged as untested — `ORDER BY` + `LIMIT` + `with_version: true`
/// + `count_total: true` ALL AT ONCE. Both F-6 (#796) and F-7 (#797)
/// independently exclude the top-K heap fast path (the `use_topk` gate),
/// routing the query onto the SHARED full-sort code path where
/// `apply_order_by_qv_with_ids` threads `RecordId`s through the sort (for
/// `with_version`) AND `apply_pagination` computes `total_count` (for
/// `count_total`). This test asserts the two correctness concerns do not
/// interfere on that shared path, in a single response:
///   1. `records` are correctly sorted and paginated (LIMIT 2 of 5),
///   2. `versions` is `Some(...)` and index-aligned with the paginated page,
///   3. `pagination.total_count` is `Some(5)` — the TRUE total row count, not
///      the page size — proving `count_total` and `with_version` compose.
#[tokio::test]
async fn order_by_limit_with_version_and_count_total_compose() {
    let (tbl, mvcc) = make_plain_mvcc_table().await;
    // Insert in deliberately NON-sorted order so ORDER BY truly reorders.
    let rids = [
        insert_scored(&tbl, 10).await,
        insert_scored(&tbl, 50).await,
        insert_scored(&tbl, 20).await,
        insert_scored(&tbl, 40).await,
        insert_scored(&tbl, 30).await,
    ];
    let inserted_scores = [10i64, 50, 20, 40, 30];

    // Ground truth: each (score, version) straight from the MVCC store.
    let mut truth_asc: Vec<(i64, u64)> = inserted_scores
        .iter()
        .zip(rids.iter())
        .map(|(&s, rid)| (s, mvcc.version_of(rid.as_bytes())))
        .collect();
    truth_asc.sort_by_key(|(s, _)| *s);

    let interner = tbl.interner().get().await.unwrap();
    let refs = new_map();
    let ctx = FilterContext::new(interner, &refs);

    // ORDER BY score ASC LIMIT 2 OFFSET 1 → page is [20, 30] of the 5-row total,
    // with BOTH with_version AND count_total demanded on the same query. Both
    // flags exclude the top-K heap, so the shared full-sort path must serve
    // id-threading AND total_count together.
    let mut q = ReadQuery::new("t")
        .order_by(OrderBy::asc("score"))
        .limit(2)
        .offset(1);
    q.with_version = true;
    q.count_total = true;
    let res = tbl.read(&q, &ctx).await.unwrap();

    // (1) records: correctly sorted ASC and paginated to the LIMIT/OFFSET slice.
    let scores: Vec<i64> = res
        .records
        .iter()
        .filter_map(|r| r.get_value_i64("score"))
        .collect();
    assert_eq!(scores, vec![20, 30], "paginated slice (skip 1, take 2)");

    // (2) versions: Some(...), index-aligned with the paginated records, and
    //     each entry is the canonical version of the row NOW at that position.
    let versions = res
        .versions
        .as_ref()
        .expect("ORDER BY + LIMIT + with_version + count_total must populate versions");
    assert_eq!(
        versions.len(),
        scores.len(),
        "versions must be index-aligned with the paginated records"
    );
    for (i, &score) in scores.iter().enumerate() {
        let expected_v = truth_asc
            .iter()
            .find(|(s, _)| *s == score)
            .map(|(_, v)| *v)
            .unwrap();
        assert_eq!(
            versions[i], expected_v,
            "paginated versions[{i}] must match the record at that page position (score {score})"
        );
    }

    // (3) pagination.total_count: the TRUE total row count (5), NOT the page
    //     size — proving count_total and with_version did not interfere on the
    //     shared full-sort path.
    let pagination = res
        .pagination
        .as_ref()
        .expect("count_total=true must emit pagination metadata");
    assert_eq!(
        pagination.total_count,
        Some(5),
        "count_total=true with with_version must report the TRUE total row count"
    );
}
