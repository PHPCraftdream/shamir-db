//! Tests for slice A3: covering index-only read path.
//!
//! Validates two behaviours:
//!   1. A covering range query (SELECT email WHERE score BETWEEN lo AND hi)
//!      is served entirely from the index posting — no full record fetch —
//!      and `stats.index_used` contains "covering".
//!   2. After a deliberate stale-posting scenario (posting at v1 but the
//!      record's hwm has advanced), the index-only path falls back to
//!      `get_many`; when `get_many` returns `None` (deleted record), the
//!      row is silently skipped, preventing phantom reads.

use std::sync::Arc;

use shamir_query_types::filter::{FieldPath, Filter, FilterValue};
use shamir_query_types::read::select::Select;
use shamir_query_types::read::OrderBy;
use shamir_query_types::read::ReadQuery;
use shamir_storage::storage_in_memory::InMemoryStore;
use shamir_storage::types::RecordKey;
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

/// Build an MVCC-backed TableManager with a covering sorted index:
///   - sort key:    `score`  (Int)
///   - included:    `email`  (Str)
async fn make_mvcc_table() -> (TableManager, Arc<MvccStore>) {
    let data: Arc<dyn Store> = Arc::new(InMemoryStore::new());
    let info: Arc<dyn Store> = Arc::new(InMemoryStore::new());
    let history: Arc<dyn Store> = Arc::new(InMemoryStore::new());

    let base = TableManager::create("t".into(), Arc::clone(&data), Arc::clone(&info))
        .await
        .unwrap();

    let gate = Arc::new(RepoTxGate::fresh());
    let mvcc = Arc::new(MvccStore::new(history, Arc::clone(&gate)));

    let tbl = base.with_mvcc_store(Arc::clone(&mvcc));

    // Covering sorted index: sort on `score`, include `email`.
    tbl.create_sorted_index_with_include("score_idx", &["score"], vec![vec!["email".to_string()]])
        .await
        .unwrap();

    (tbl, mvcc)
}

/// Insert a record `{score: s, email: e}` through an MVCC-backed table.
/// Returns the assigned `RecordId`.
async fn insert_record(tbl: &TableManager, score: i64, email: &str) -> RecordId {
    let interner = tbl.interner().get().await.unwrap();
    let score_key = interner.touch_ind("score").unwrap().into_key();
    let email_key = interner.touch_ind("email").unwrap().into_key();

    // Save any newly-minted keys before we write.
    // (touch_ind on a fresh interner may create new entries; persist them.)
    tbl.interner().persist().await.unwrap();

    let mut m = new_map();
    m.insert(score_key, InnerValue::Int(score));
    m.insert(email_key, InnerValue::Str(email.to_owned()));
    let rec = InnerValue::Map(m);

    tbl.insert(&rec).await.unwrap()
}

/// Build a `ReadQuery` that will trigger the sorted-index scan:
///   SELECT email FROM t WHERE score BETWEEN lo AND hi
fn range_query(lo: i64, hi: i64) -> ReadQuery {
    ReadQuery::new("t")
        .select(Select::fields(["email"]))
        .filter(Filter::Between {
            field: FieldPath::from(vec!["score".to_string()]),
            from: FilterValue::Int(lo),
            to: FilterValue::Int(hi),
        })
}

// ─────────────────────────────────────────────────────────────────────────────
// Test 1 — covering index-only matches full fetch
// ─────────────────────────────────────────────────────────────────────────────

/// A covered range query (SELECT email WHERE score BETWEEN lo AND hi) with an
/// MVCC-backed table must:
///   a) return exactly the emails whose scores fall in the range,
///   b) report `stats.index_used` containing "covering" (proving the
///      index-only code path was taken, not the full-fetch path).
#[tokio::test]
async fn covering_index_only_matches_full_fetch() {
    let (tbl, _mvcc) = make_mvcc_table().await;

    // Insert five records with distinct scores.
    insert_record(&tbl, 10, "a@example.com").await;
    insert_record(&tbl, 20, "b@example.com").await;
    insert_record(&tbl, 30, "c@example.com").await;
    insert_record(&tbl, 40, "d@example.com").await;
    insert_record(&tbl, 50, "e@example.com").await;

    let interner = tbl.interner().get().await.unwrap();
    let refs = new_map();
    let ctx = FilterContext::new(interner, &refs);

    // Query: SELECT email WHERE score BETWEEN 20 AND 40 → b, c, d.
    let query = range_query(20, 40);
    let result = tbl.read(&query, &ctx).await.unwrap();

    // Collect returned emails.
    let mut emails: Vec<String> = result
        .records
        .iter()
        .filter_map(|r| r.get_value_str("email").map(str::to_owned))
        .collect();
    emails.sort();

    assert_eq!(
        emails,
        vec!["b@example.com", "c@example.com", "d@example.com"],
        "covering query must return exactly the three in-range emails"
    );

    // The index-only path stamps "covering" into index_used.
    let index_used = result
        .stats
        .as_ref()
        .expect("stats must be present")
        .index_used
        .as_deref()
        .unwrap_or("");
    assert!(
        index_used.contains("covering"),
        "index_used must contain 'covering'; got {:?}",
        index_used
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// Test 2 — stale posting → no phantom
// ─────────────────────────────────────────────────────────────────────────────

/// Phantom-prevention invariant:
///
/// Insert record R with `{score: 30, email: "r@example.com"}`. The sorted
/// index gets a posting at version V1 with the email projection baked in.
///
/// Then call `mvcc.delete_versioned(R_id.to_bytes())` DIRECTLY. This:
///   - removes the record from `main` (the backing data store), and
///   - advances the hwm for that key to V2 ≠ V1.
///   - BUT the sorted posting (at version V1) is NOT cleaned up — it is
///     deliberately left as a stale posting.
///
/// A subsequent covering range query that would match R (score BETWEEN 20 AND 40)
/// MUST NOT return R:
///   - The eligibility gate is satisfied (mvcc attached, no residual, etc.).
///   - `decode_covering_projection` decodes the V1 projection successfully.
///   - `mvcc.live_version(R_id)` returns Some(V2) ≠ V1 → version mismatch.
///   - The code falls back to `get_many([R_id])`.
///   - `get_many` returns `None` because the record was deleted from `main`.
///   - The `None` is silently skipped — no phantom row is returned.
///
/// Without the freshness validation the query WOULD return the phantom row
/// (it would trust the stale V1 projection directly).
#[tokio::test]
async fn covering_index_only_rejects_stale_posting_no_phantom() {
    let (tbl, mvcc) = make_mvcc_table().await;

    // Insert an out-of-range record first so the range scan still has
    // candidates and we can verify the count, not just an empty result.
    insert_record(&tbl, 10, "safe@example.com").await;

    // Insert the record that will become a stale posting.
    let stale_id = insert_record(&tbl, 30, "r@example.com").await;

    // Verify it is there before deletion.
    let interner = tbl.interner().get().await.unwrap();
    let refs = new_map();
    let ctx = FilterContext::new(interner, &refs);
    let pre = tbl.read(&range_query(20, 40), &ctx).await.unwrap();
    let pre_emails: Vec<String> = pre
        .records
        .iter()
        .filter_map(|r| r.get_value_str("email").map(str::to_owned))
        .collect();
    assert!(
        pre_emails.iter().any(|e| e == "r@example.com"),
        "record must appear before deletion; got {:?}",
        pre_emails
    );

    // Delete the record directly through the MvccStore.  This advances the
    // hwm to V2 and removes the record from `main`, but does NOT remove the
    // sorted-index posting (V1 projection is still there — the stale window).
    mvcc.delete_versioned(RecordKey::from_slice(stale_id.as_bytes()))
        .await
        .unwrap();

    // Run the same covered range query.
    let interner = tbl.interner().get().await.unwrap();
    let refs = new_map();
    let ctx = FilterContext::new(interner, &refs);
    let result = tbl.read(&range_query(20, 40), &ctx).await.unwrap();

    let emails: Vec<String> = result
        .records
        .iter()
        .filter_map(|r| r.get_value_str("email").map(str::to_owned))
        .collect();

    // r@example.com must NOT appear — that would be a phantom read.
    assert!(
        !emails.iter().any(|e| e == "r@example.com"),
        "stale posting must NOT produce a phantom row; got {:?}",
        emails
    );

    // The index_used label still says "covering" — the covering path ran,
    // detected the stale posting, fell back for that id, got None, and skipped.
    let index_used = result
        .stats
        .as_ref()
        .expect("stats must be present")
        .index_used
        .as_deref()
        .unwrap_or("");
    assert!(
        index_used.contains("covering"),
        "index_used must still be 'covering'; got {:?}",
        index_used
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// #128 regression — top-K ORDER BY + LIMIT must still emit pagination metadata
// ─────────────────────────────────────────────────────────────────────────────

/// A plain MVCC table with NO sorted index — so an `ORDER BY` query is
/// forced through the in-memory top-K heap path (`use_topk` in
/// `read_exec.rs`), not an index-ordered scan.
async fn make_plain_mvcc_table() -> TableManager {
    let data: Arc<dyn Store> = Arc::new(InMemoryStore::new());
    let info: Arc<dyn Store> = Arc::new(InMemoryStore::new());
    let history: Arc<dyn Store> = Arc::new(InMemoryStore::new());
    let base = TableManager::create("t".into(), Arc::clone(&data), Arc::clone(&info))
        .await
        .unwrap();
    let gate = Arc::new(RepoTxGate::fresh());
    let mvcc = Arc::new(MvccStore::new(history, Arc::clone(&gate)));
    base.with_mvcc_store(mvcc)
}

/// The top-K read path (`use_topk` in `read_exec.rs`: ORDER BY + finite
/// LIMIT, no distinct/group_by/agg) once hardcoded `pagination: None`,
/// silently dropping pagination metadata for every ordered+limited query —
/// while the sibling `apply_pagination` branch returned `Some(..)`. This is
/// the engine-level lock for that wire-contract regression (#128): an
/// ordered+limited read served by the heap must return `Some(PaginationInfo)`,
/// matching the non-top-K branch, AND the heap must still return the correct
/// top rows. A plain (no-index) table guarantees the top-K path is taken
/// rather than an index-ordered scan.
#[tokio::test]
async fn topk_order_by_limit_emits_pagination() {
    let tbl = make_plain_mvcc_table().await;

    insert_record(&tbl, 10, "a@example.com").await;
    insert_record(&tbl, 20, "b@example.com").await;
    insert_record(&tbl, 30, "c@example.com").await;
    insert_record(&tbl, 40, "d@example.com").await;
    insert_record(&tbl, 50, "e@example.com").await;

    let interner = tbl.interner().get().await.unwrap();
    let refs = new_map();
    let ctx = FilterContext::new(interner, &refs);

    // ORDER BY score DESC LIMIT 2 → triggers the top-K heap path.
    let query = ReadQuery::new("t")
        .order_by(OrderBy::desc("score"))
        .limit(2);
    let result = tbl.read(&query, &ctx).await.unwrap();

    assert!(
        result.pagination.is_some(),
        "#128 regression: top-K ORDER BY + LIMIT must emit pagination \
         metadata (got None — the heap path dropped it)"
    );
    assert_eq!(result.records.len(), 2, "LIMIT 2 yields exactly two rows");

    // Heap correctness: top two scores are 50 then 40.
    let scores: Vec<i64> = result
        .records
        .iter()
        .filter_map(|r| r.get_value_i64("score"))
        .collect();
    assert_eq!(
        scores,
        vec![50, 40],
        "top-K returns the highest scores in order"
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// Pagination contract — completeness critic across the LIMIT fast paths
// ─────────────────────────────────────────────────────────────────────────────

/// Run a query through `tbl.read` and return whether the wire result
/// carried pagination metadata.
async fn read_has_pagination(tbl: &TableManager, query: &ReadQuery) -> bool {
    let interner = tbl.interner().get().await.unwrap();
    let refs = new_map();
    let ctx = FilterContext::new(interner, &refs);
    tbl.read(query, &ctx).await.unwrap().pagination.is_some()
}

/// The pagination wire-contract must hold across EVERY read path that a
/// `LIMIT` query can take, not just the one that happened to regress.
/// #128 introduced two independent `LIMIT` fast paths that each forgot to
/// emit pagination: the in-memory top-K heap (`read_collecting`) and the
/// sorted-index walk (`read_order_limit_fast`). This test enumerates the
/// fast-path surface so a future optimization cannot silently drop the
/// metadata again:
///   - ORDER BY indexed-field ASC + LIMIT  → `read_order_limit_fast` (first_k)
///   - ORDER BY indexed-field DESC + LIMIT → `read_order_limit_fast` (last_k)
///   - plain SELECT + LIMIT (no order)     → `read_streaming`
/// The no-index top-K heap path is covered by
/// `topk_order_by_limit_emits_pagination`.
#[tokio::test]
async fn limit_queries_all_emit_pagination_contract() {
    // Indexed table — ORDER BY score uses the sorted-index fast path.
    let (tbl, _mvcc) = make_mvcc_table().await;
    insert_record(&tbl, 10, "a@example.com").await;
    insert_record(&tbl, 20, "b@example.com").await;
    insert_record(&tbl, 30, "c@example.com").await;
    insert_record(&tbl, 40, "d@example.com").await;
    insert_record(&tbl, 50, "e@example.com").await;

    // (1) sorted-index ORDER BY ASC LIMIT → read_order_limit_fast (first_k).
    let asc = ReadQuery::new("t").order_by(OrderBy::asc("score")).limit(2);
    assert!(
        read_has_pagination(&tbl, &asc).await,
        "sorted-index ORDER BY ASC + LIMIT must emit pagination"
    );

    // (2) sorted-index ORDER BY DESC LIMIT → read_order_limit_fast (last_k).
    let desc = ReadQuery::new("t")
        .order_by(OrderBy::desc("score"))
        .limit(2);
    assert!(
        read_has_pagination(&tbl, &desc).await,
        "sorted-index ORDER BY DESC + LIMIT must emit pagination"
    );

    // (3) plain SELECT + LIMIT (no order) → read_streaming path.
    let plain = ReadQuery::new("t").limit(3);
    assert!(
        read_has_pagination(&tbl, &plain).await,
        "plain SELECT + LIMIT must emit pagination"
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// F-6 / #796 regression — count_total=true must exclude the top-K LIMIT fast path
// ─────────────────────────────────────────────────────────────────────────────

/// The top-K read path (`use_topk` in `read_exec.rs`) is a bounded heap that
/// only ever sees K rows, so it cannot supply a true total. Before the fix the
/// `use_topk` gate did not check `query.count_total`, so an
/// `ORDER BY + LIMIT + count_total=true` query was silently served by the heap
/// and got `total_count: None` back via `fast_path_pagination` — ignoring the
/// flag. The gate now adds `&& !query.count_total`, which routes such queries
/// to the full-sort branch (`apply_order_by_qv` + `apply_pagination`) that
/// correctly computes `Some(records.len())` before slicing.
///
/// This test uses a plain (no-index) MVCC table to force the in-memory top-K
/// heap path when `count_total` is absent, and the full-sort path when it is
/// present. It asserts:
///   1. `count_total=true`  → `total_count == Some(<true row count>)`, AND the
///      returned page is still the correct top-K (sort order preserved).
///   2. `count_total=false` → `total_count == None` (the intentional top-K
///      fast-path behavior is unchanged when the flag is not set, guarding
///      against accidentally always taking the slow path now).
#[tokio::test]
async fn count_total_true_excludes_topk_fast_path() {
    let tbl = make_plain_mvcc_table().await;
    insert_record(&tbl, 10, "a@example.com").await;
    insert_record(&tbl, 20, "b@example.com").await;
    insert_record(&tbl, 30, "c@example.com").await;
    insert_record(&tbl, 40, "d@example.com").await;
    insert_record(&tbl, 50, "e@example.com").await;
    const TOTAL_ROWS: u64 = 5;

    let interner = tbl.interner().get().await.unwrap();
    let refs = new_map();
    let ctx = FilterContext::new(interner, &refs);

    // (1) ORDER BY score DESC LIMIT 2 + count_total=true → must NOT take the
    //     top-K heap; the full-sort path must report the TRUE total.
    let with_total = ReadQuery::new("t")
        .order_by(OrderBy::desc("score"))
        .limit(2)
        .count_total(true);
    let result = tbl.read(&with_total, &ctx).await.unwrap();

    assert_eq!(
        result.records.len(),
        2,
        "LIMIT 2 still yields exactly two rows even with count_total"
    );
    // Sort order is preserved by the full-sort fallback.
    let scores: Vec<i64> = result
        .records
        .iter()
        .filter_map(|r| r.get_value_i64("score"))
        .collect();
    assert_eq!(
        scores,
        vec![50, 40],
        "full-sort fallback must still return the highest scores in order"
    );

    let pagination = result
        .pagination
        .as_ref()
        .expect("count_total=true must emit pagination metadata");
    assert_eq!(
        pagination.total_count,
        Some(TOTAL_ROWS),
        "F-6 regression: count_total=true on ORDER BY + LIMIT must report \
         the TRUE total row count, not None (top-K heap was wrongly taken)"
    );

    // (2) Same query WITHOUT count_total → the top-K fast path is still taken
    //     and total_count is None (the intentional fast-path behavior is
    //     unchanged; this guards against always routing to the slow path).
    let no_total = ReadQuery::new("t")
        .order_by(OrderBy::desc("score"))
        .limit(2);
    let result = tbl.read(&no_total, &ctx).await.unwrap();
    assert_eq!(
        result.records.len(),
        2,
        "LIMIT 2 yields exactly two rows on the top-K fast path"
    );
    let pagination = result
        .pagination
        .as_ref()
        .expect("top-K fast path must still emit pagination metadata (#128)");
    assert_eq!(
        pagination.total_count,
        Option::None,
        "count_total omitted → top-K fast path is taken and total_count is \
         None (do not always take the slow path)"
    );
}
