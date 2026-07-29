//! F-53b Step 2 (#878) — AsOf-aware cursor index-seek: production-path tests.
//!
//! Ports F-53b Step 1's spike tests (`f53b_cursor_seek_spike_tests.rs`) from
//! the test-local `asof_index_seek_spike` helper to the PRODUCTION `read()`
//! path — `Temporal::AsOf` + `Pagination::After` (the shape
//! `try_plan_keyset_seek` requires, now also accepted by the AsOf arm wired
//! in `read_as_of`). Asserts the `QueryStats::index_used` label switches to
//! the seek path (`sorted_idx_<n>_asof_keyset`) and `records_scanned` drops
//! to `O(page_size)`, not just that the results match.
//!
//! ## What each test proves
//!
//! - `asof_seek_asc_pages_match_baseline` — when NO concurrent write has
//!   touched the index since the pin (the common, quiescent-table cursor
//!   workload), the production seek arm yields BYTE-IDENTICAL page contents
//!   to the full-scan baseline across 3 pages, at `O(page_size)` seek cost
//!   per page instead of `O(N)`. `index_used` is the seek label.
//!
//! - `asof_seek_desc_parity` — DESC direction: same proof as the ASC case
//!   but walking the index in reverse.
//!
//! - `asof_seek_gate_fallback_on_concurrent_insert` — a concurrent INSERT
//!   after the pin advances the high-water; the seek arm DECLINES and the
//!   result equals the full-scan baseline (insert correctly excluded by the
//!   full scan's `get_at == None` check). Proves the gate is bumped on
//!   creates.
//!
//! - `asof_seek_gate_fallback_on_concurrent_update` — a concurrent UPDATE to
//!   the indexed field after the pin advances the high-water; the seek arm
//!   declines and the row appears at its OLD (pinned) position via the full
//!   scan. This is the test that proves the gate prevents the spike's
//!   negative-test failure modes from ever reaching a real cursor.
//!
//! - `asof_seek_gate_fallback_on_concurrent_delete` — same but with DELETE.

use std::sync::Arc;

use shamir_query_types::read::select::Select;
use shamir_query_types::read::{At, OrderBy, Pagination, ReadQuery, Temporal};
use shamir_storage::storage_in_memory::InMemoryStore;
use shamir_storage::types::Store;
use shamir_tx::{MvccStore, RepoTxGate, Retention};
use shamir_types::types::common::new_map;
use shamir_types::types::record_id::RecordId;
use shamir_types::types::value::{InnerValue, QueryValue};

use crate::query::filter::eval_context::FilterContext;
use crate::query::read::{QueryRecord, QueryResult};
use crate::table::TableManager;

// ─────────────────────────────────────────────────────────────────────────────
// Harness — MVCC table with a single-column sorted index on `score` (Int).
// Mirrors the spike's `make_mvcc_score_table` exactly.
// ─────────────────────────────────────────────────────────────────────────────

async fn make_mvcc_score_table() -> (TableManager, Arc<MvccStore>, Arc<RepoTxGate>) {
    let data: Arc<dyn Store> = Arc::new(InMemoryStore::new());
    let info: Arc<dyn Store> = Arc::new(InMemoryStore::new());
    let history: Arc<dyn Store> = Arc::new(InMemoryStore::new());

    let base = TableManager::create("t".into(), Arc::clone(&data), Arc::clone(&info))
        .await
        .unwrap();

    let gate = Arc::new(RepoTxGate::fresh());
    let mvcc = Arc::new(MvccStore::new(history, Arc::clone(&gate)));
    // Keep full history so pinned-version values survive vacuum.
    mvcc.set_retention(Retention::keep_history()).unwrap();

    let tbl = base.with_mvcc_store(Arc::clone(&mvcc));
    // Single-column ORDER BY index on `score` — the target case. NON-covering
    // (no INCLUDES), so postings carry NO covering-projection version stamp.
    // The mechanism is `MvccStore::version_of`, not the envelope stamp.
    tbl.create_sorted_index("score_idx", &["score"])
        .await
        .unwrap();
    (tbl, mvcc, gate)
}

/// Insert `{score, label}` and return the assigned RecordId + commit version.
async fn insert_score(
    tbl: &TableManager,
    mvcc: &MvccStore,
    score: i64,
    label: &str,
) -> (RecordId, u64) {
    let interner = tbl.interner().get().await.unwrap();
    let score_key = interner.touch_ind("score").unwrap().into_key();
    let label_key = interner.touch_ind("label").unwrap().into_key();
    tbl.interner().persist().await.unwrap();

    let mut m = new_map();
    m.insert(score_key, InnerValue::Int(score));
    m.insert(label_key, InnerValue::Str(label.to_owned()));
    let rec = InnerValue::Map(m);

    let id = tbl.insert(&rec).await.unwrap();
    let v = mvcc.version_of(&id.to_bytes());
    (id, v)
}

/// The cursor's pinned snapshot version = the MVCC high-water mark after the
/// initial inserts.
fn pinned_at(gate: &RepoTxGate) -> u64 {
    gate.last_committed()
}

/// `score` (i64) for each row of a `QueryResult`, in result order.
fn baseline_scores(result: &QueryResult) -> Vec<i64> {
    result
        .records
        .iter()
        .filter_map(|r| r.get_value_i64("score"))
        .collect()
}

/// Extract the `_id` (RecordId) from a `QueryRecord::Inserted` row — the
/// seek arm's wire shape (mirrors `read_keyset_seek`). Used to thread the
/// `after_id` tie-breaker across pages.
fn row_id(record: &QueryRecord) -> Option<RecordId> {
    match record {
        QueryRecord::Inserted(rec) => rec.id,
        _ => None,
    }
}

/// Build a seek AsOf query (ASC): ORDER BY + `Pagination::After` + at the
/// pinned version. This is the shape that triggers the new seek arm.
fn seek_query_asc(seek: i64, limit: u64, pinned: u64, after_id: Option<RecordId>) -> ReadQuery {
    let mut q = ReadQuery::new("t")
        .select(Select::fields(["score"]))
        .order_by(OrderBy::asc("score"))
        .pagination(Pagination::after_with_id(
            vec![QueryValue::Int(seek)],
            Some(limit),
            after_id,
        ));
    q.temporal = Temporal::AsOf {
        at: At::Version(pinned),
    };
    q
}

/// Build a seek AsOf query for DESC direction.
fn seek_query_desc(seek: i64, limit: u64, pinned: u64, after_id: Option<RecordId>) -> ReadQuery {
    let mut q = ReadQuery::new("t")
        .select(Select::fields(["score"]))
        .order_by(OrderBy::desc("score"))
        .pagination(Pagination::after_with_id(
            vec![QueryValue::Int(seek)],
            Some(limit),
            after_id,
        ));
    q.temporal = Temporal::AsOf {
        at: At::Version(pinned),
    };
    q
}

/// Assert the result used the AsOf seek arm (`_asof_keyset` label).
fn assert_seek_path(result: &QueryResult) {
    let label = result
        .stats
        .as_ref()
        .and_then(|s| s.index_used.as_deref())
        .unwrap_or("<none>");
    assert!(
        label.ends_with("_asof_keyset"),
        "expected the AsOf keyset seek path, got index_used = {label:?}"
    );
}

/// Assert the result used the full-scan AsOf path (`temporal_asof` label).
fn assert_fullscan_path(result: &QueryResult) {
    let label = result
        .stats
        .as_ref()
        .and_then(|s| s.index_used.as_deref())
        .unwrap_or("<none>");
    assert!(
        label == "temporal_asof",
        "expected the full-scan temporal_asof path (gate fallback), got index_used = {label:?}"
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// GREEN — production seek arm: ASC, 3 pages, no concurrent writes.
//
// The gate only passes when `last_mutation_version <= pinned` — i.e. NO
// write has touched the index since the pin. This is the dominant cursor
// workload (paging a large, stable result set), and the case where the seek
// arm delivers the O(page_size) asymptotic win. A concurrent write trips the
// gate (one-way ratchet) and later pages fall back to the full scan — see
// the gate-fallback tests below.
// ─────────────────────────────────────────────────────────────────────────────

/// When no concurrent write has touched the index since the pin, the
/// production seek arm yields BYTE-IDENTICAL page contents to the full-scan
/// baseline across 3 pages, at `O(page_size)` seek cost per page instead of
/// `O(N)`. `index_used` is the seek label on every page.
#[tokio::test]
async fn asof_seek_asc_pages_match_baseline() {
    let (tbl, mvcc, gate) = make_mvcc_score_table().await;
    // 30 rows: scores 0,10,...,290 ⇒ pages [0-90],[100-190],[200-290].
    for s in (0..30).map(|i| i * 10) {
        let _ = insert_score(&tbl, &mvcc, s, &format!("r{s}")).await;
    }
    let pinned = pinned_at(&gate);

    let interner = tbl.interner().get().await.unwrap();
    let refs = new_map();
    let ctx = FilterContext::new(interner, &refs);

    let page_size = 10u64;
    let mut all_seek_scores: Vec<i64> = Vec::new();
    let mut all_seek_scanned: u64 = 0;
    let mut all_baseline_scanned: u64 = 0;

    let mut seek_val = i64::MIN;
    let mut after_id = None;

    for page_num in 1..=3u64 {
        let q = seek_query_asc(seek_val, page_size, pinned, after_id);
        let p = tbl.read(&q, &ctx).await.unwrap();
        assert_seek_path(&p);
        let scanned = p.stats.as_ref().unwrap().records_scanned;
        assert!(
            scanned <= page_size,
            "page {page_num} records_scanned must be ~page_size; got {scanned}"
        );
        all_seek_scanned += scanned;
        all_seek_scores.extend(baseline_scores(&p));

        // Baseline comparison: full-scan query at the pinned version.
        // Use the same LIMIT to compare this page's slice.
        let bq = baseline_asc_page(seek_val, page_size, pinned);
        let b = tbl.read(&bq, &ctx).await.unwrap();
        all_baseline_scanned += b.stats.as_ref().unwrap().records_scanned;
        assert_eq!(
            baseline_scores(&p),
            baseline_scores(&b),
            "page {page_num}: seek must match baseline"
        );

        // Thread the tie-breaker for the next page.
        if let Some(last) = p.records.last() {
            seek_val = last.get_value_i64("score").unwrap();
            after_id = row_id(last);
        }
    }

    // Every original row appears exactly once across the 3 pages, in order.
    assert_eq!(
        all_seek_scores,
        (0..30).map(|i| i * 10).collect::<Vec<_>>(),
        "all 30 rows in order"
    );
    // The asymptotic win: seek cumulative cost is O(pages × page_size), the
    // baseline is O(pages × N).
    assert!(
        all_seek_scanned < all_baseline_scanned,
        "cumulative seek cost ({all_seek_scanned}) must beat baseline ({all_baseline_scanned})"
    );

    // Page 4 must be EMPTY.
    let q4 = seek_query_asc(seek_val, page_size, pinned, after_id);
    let p4 = tbl.read(&q4, &ctx).await.unwrap();
    assert!(
        p4.records.is_empty(),
        "page 4 must be empty; got {} rows",
        p4.records.len()
    );
}

/// Baseline full-scan ASC page with inclusive boundary + OFFSET 1 (mirrors
/// the cursor's boundary_filter + tie_skip so the baseline matches the
/// seek's "strictly after" semantics).
fn baseline_asc_page(seek_score: i64, limit: u64, pinned: u64) -> ReadQuery {
    use shamir_query_types::filter::{FieldPath, Filter, FilterValue};
    let offset = if seek_score == i64::MIN { 0 } else { 1 };
    let mut q = ReadQuery::new("t")
        .select(Select::fields(["score"]))
        .order_by(OrderBy::asc("score"))
        .limit(limit)
        .offset(offset);
    q.temporal = Temporal::AsOf {
        at: At::Version(pinned),
    };
    if seek_score != i64::MIN {
        q.r#where = Some(Filter::Gte {
            field: FieldPath::from(vec!["score".to_string()]),
            value: FilterValue::Int(seek_score),
        });
    }
    q
}

// ─────────────────────────────────────────────────────────────────────────────
// DESC parity — the seek arm walks the index in reverse.
// ─────────────────────────────────────────────────────────────────────────────

/// DESC direction: the production seek arm walks the index high→low and
/// produces pages in score-DESC order, with the same `O(page_size)` cost.
#[tokio::test]
async fn asof_seek_desc_parity() {
    let (tbl, mvcc, gate) = make_mvcc_score_table().await;
    for s in (0..30).map(|i| i * 10) {
        let _ = insert_score(&tbl, &mvcc, s, &format!("r{s}")).await;
    }
    let pinned = pinned_at(&gate);

    let interner = tbl.interner().get().await.unwrap();
    let refs = new_map();
    let ctx = FilterContext::new(interner, &refs);

    let page_size = 10u64;

    // DESC first page: seek from i64::MAX so every real score is strictly
    // before it (DESC walks high→low).
    let q1 = seek_query_desc(i64::MAX, page_size, pinned, None);
    let p1 = tbl.read(&q1, &ctx).await.unwrap();
    assert_seek_path(&p1);
    // DESC order: 290, 280, ..., 200.
    assert_eq!(
        baseline_scores(&p1),
        (20..30).rev().map(|i| i * 10).collect::<Vec<_>>(),
        "page 1 DESC scores"
    );
    let p1_scanned = p1.stats.as_ref().unwrap().records_scanned;
    assert!(
        p1_scanned <= page_size,
        "page 1 DESC records_scanned must be ~page_size; got {p1_scanned}"
    );

    // Thread the tie-breaker.
    let last_id_1 = row_id(p1.records.last().unwrap()).unwrap();
    let last_score_1 = p1.records.last().unwrap().get_value_i64("score").unwrap();

    // Page 2 DESC: 190, 180, ..., 100.
    let q2 = seek_query_desc(last_score_1, page_size, pinned, Some(last_id_1));
    let p2 = tbl.read(&q2, &ctx).await.unwrap();
    assert_seek_path(&p2);
    assert_eq!(
        baseline_scores(&p2),
        (10..20).rev().map(|i| i * 10).collect::<Vec<_>>(),
        "page 2 DESC scores"
    );

    // Page 3 DESC: 90, 80, ..., 0.
    let last_id_2 = row_id(p2.records.last().unwrap()).unwrap();
    let last_score_2 = p2.records.last().unwrap().get_value_i64("score").unwrap();
    let q3 = seek_query_desc(last_score_2, page_size, pinned, Some(last_id_2));
    let p3 = tbl.read(&q3, &ctx).await.unwrap();
    assert_seek_path(&p3);
    assert_eq!(
        baseline_scores(&p3),
        (0..10).rev().map(|i| i * 10).collect::<Vec<_>>(),
        "page 3 DESC scores"
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// GATE FALLBACK — concurrent INSERT / UPDATE / DELETE trips the gate → full scan.
//
// The gate is bumped on EVERY mutation (create / update / delete). A
// concurrent INSERT trips it just like an UPDATE or DELETE — a one-way
// ratchet that disables the fast path for the cursor's remaining lifetime.
// This is the accepted tradeoff: the gate cannot distinguish a safe
// INSERT-after-pin (the classifier would exclude it) from an unsafe
// UPDATE/DELETE-after-pin (the classifier cannot place the row), so it
// conservatively declines on ANY mutation. The full-scan fallback is always
// correct.
// ─────────────────────────────────────────────────────────────────────────────

/// A concurrent INSERT after the pin advances the per-index mutation
/// high-water; the seek arm DECLINES and the result equals the full-scan
/// baseline (insert excluded by `get_at == None`). Proves the gate is bumped
/// on creates.
#[tokio::test]
async fn asof_seek_gate_fallback_on_concurrent_insert() {
    let (tbl, mvcc, gate) = make_mvcc_score_table().await;
    for s in (0..30).map(|i| i * 10) {
        let _ = insert_score(&tbl, &mvcc, s, &format!("r{s}")).await;
    }
    let pinned = pinned_at(&gate);
    assert!(
        tbl.sorted_indexes().last_mutation_version() <= pinned,
        "pre-condition: gate must pass before the concurrent write"
    );

    // INSERT after the pin.
    let (_ins_id, ins_v) = insert_score(&tbl, &mvcc, 125, "late125").await;
    assert!(ins_v > pinned);
    assert!(
        tbl.sorted_indexes().last_mutation_version() > pinned,
        "post-insert: gate must have advanced past the pin"
    );

    let interner = tbl.interner().get().await.unwrap();
    let refs = new_map();
    let ctx = FilterContext::new(interner, &refs);

    // The gate has advanced past the pin: the seek arm must DECLINE.
    let q = seek_query_asc(i64::MIN, 100, pinned, None);
    let result = tbl.read(&q, &ctx).await.unwrap();
    assert_fullscan_path(&result);
    // The late insert must NOT appear.
    assert!(
        !baseline_scores(&result).contains(&125),
        "gate fallback (full scan) must exclude the post-pin insert"
    );
    assert_eq!(
        baseline_scores(&result).len(),
        30,
        "exactly 30 pinned-version rows"
    );
}

/// A concurrent UPDATE to the indexed field after the pin advances the
/// per-index mutation high-water past the pin. The seek arm DECLINES (gate
/// check fails in `read_as_of`) and the result equals the full-scan baseline
/// — the row appears at its OLD (pinned) position. This is the test that
/// proves the gate actually prevents the spike's negative-test failure modes
/// from ever reaching a real cursor.
#[tokio::test]
async fn asof_seek_gate_fallback_on_concurrent_update() {
    let (tbl, mvcc, gate) = make_mvcc_score_table().await;
    // 5 rows: scores 10,20,30,40,50.
    let mut ids = Vec::new();
    for s in [10, 20, 30, 40, 50] {
        ids.push(insert_score(&tbl, &mvcc, s, &format!("r{s}")).await.0);
    }
    let pinned = pinned_at(&gate);
    let score_id = {
        let interner = tbl.interner().get().await.unwrap();
        interner.touch_ind("score").unwrap().key().id()
    };

    // UPDATE the score-30 row to score-35 AFTER the pin (moves its posting
    // from the 30 position to the 35 position).
    let row30 = find_row_with_score(&mvcc, &ids, 30, pinned, score_id).await;
    let mut m = new_map();
    m.insert(
        shamir_types::core::interner::InternerKey::new(score_id),
        InnerValue::Int(35),
    );
    let label_id = {
        let interner = tbl.interner().get().await.unwrap();
        interner.touch_ind("label").unwrap().key().id()
    };
    m.insert(
        shamir_types::core::interner::InternerKey::new(label_id),
        InnerValue::Str("r30→35".to_owned()),
    );
    tbl.set(row30, &InnerValue::Map(m)).await.unwrap();
    assert!(
        tbl.sorted_indexes().last_mutation_version() > pinned,
        "post-update: gate must have advanced past the pin"
    );

    let interner = tbl.interner().get().await.unwrap();
    let refs = new_map();
    let ctx = FilterContext::new(interner, &refs);

    // The gate has advanced past the pin: the seek arm must DECLINE.
    let q = seek_query_asc(i64::MIN, 100, pinned, None);
    let result = tbl.read(&q, &ctx).await.unwrap();
    assert_fullscan_path(&result);
    // Baseline: the row is STILL at score 30 (its pinned value).
    assert_eq!(
        baseline_scores(&result),
        vec![10, 20, 30, 40, 50],
        "gate fallback must return the row at its OLD (pinned) position"
    );
}

/// A concurrent DELETE after the pin advances the high-water; the seek arm
/// declines and the full scan returns the deleted-at-pin row via
/// tombstone-inclusive enumeration.
#[tokio::test]
async fn asof_seek_gate_fallback_on_concurrent_delete() {
    let (tbl, mvcc, gate) = make_mvcc_score_table().await;
    let mut ids = Vec::new();
    for s in [10, 20, 30, 40, 50] {
        ids.push(insert_score(&tbl, &mvcc, s, &format!("r{s}")).await.0);
    }
    let pinned = pinned_at(&gate);
    let score_id = {
        let interner = tbl.interner().get().await.unwrap();
        interner.touch_ind("score").unwrap().key().id()
    };

    // DELETE the score-30 row AFTER the pin.
    let row30 = find_row_with_score(&mvcc, &ids, 30, pinned, score_id).await;
    tbl.delete(row30).await.unwrap();
    assert!(
        tbl.sorted_indexes().last_mutation_version() > pinned,
        "post-delete: gate must have advanced past the pin"
    );

    let interner = tbl.interner().get().await.unwrap();
    let refs = new_map();
    let ctx = FilterContext::new(interner, &refs);

    // The gate has advanced: the seek arm must DECLINE.
    let q = seek_query_asc(i64::MIN, 100, pinned, None);
    let result = tbl.read(&q, &ctx).await.unwrap();
    assert_fullscan_path(&result);
    // Baseline: the row STILL existed at pin (tombstone is post-pin).
    assert_eq!(
        baseline_scores(&result),
        vec![10, 20, 30, 40, 50],
        "gate fallback must return the deleted-after-pin row at its pinned value"
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// Helpers
// ─────────────────────────────────────────────────────────────────────────────

/// Find the record id whose AS-OF value at `pinned` has `score == target`.
async fn find_row_with_score(
    mvcc: &MvccStore,
    ids: &[RecordId],
    target: i64,
    pinned: u64,
    score_id: u64,
) -> RecordId {
    for id in ids {
        if let Some(bytes) = mvcc.get_at(&id.to_bytes(), pinned).await.unwrap() {
            if score_from_bytes(&bytes, score_id) == Some(target) {
                return *id;
            }
        }
    }
    panic!("no row with score {target}");
}

/// Extract the `score` (i64) from a materialised AS-OF record byte buffer.
fn score_from_bytes(bytes: &bytes::Bytes, score_id: u64) -> Option<i64> {
    let view = shamir_types::record_view::RecordView::new(bytes).ok()?;
    match view.get_by_id(score_id)? {
        shamir_types::record_view::RecordValue::Int(i) => Some(i),
        _ => None,
    }
}
