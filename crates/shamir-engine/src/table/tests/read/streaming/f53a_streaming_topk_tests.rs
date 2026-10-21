//!
//! F-53a (#874) — streaming top-K: bound memory during the scan, not after.
//!
//! `read_collecting` (and `read_as_of`) now merge WHERE-filter → projection →
//! sort-key extraction directly into a `k = skip + take` bounded max-heap
//! DURING the scan, instead of materialising every matched+projected row into
//! a `Vec` before the ORDER BY + LIMIT top-K trim ever runs. The pre-F-53a
//! code comment claiming "O(K) memory" was true only for the heap's own
//! internals — the `rec_acc` / `matched` Vec feeding it was still O(N).
//!
//! These tests pin the three things F-53a had to preserve while moving the
//! bounding into the scan loop:
//!   1. ORDER BY output is byte-identical to the old full-sort path for every
//!      ordering variant the comparator handles (single-key asc/desc, multi-key
//!      mixed direction, NULLS FIRST/LAST). The shared `TopKHeap` comparator is
//!      unit-fuzzed in `qv_postprocess_tests::apply_order_by_topk_byte_identical`;
//!      these cover the end-to-end engine path.
//!   2. `count_total` now composes with the bounded heap via an independent
//!      running counter (it used to force the O(N) full-sort fallback).
//!   3. `with_version` now composes with the bounded heap by carrying each
//!      heap item's `RecordId` (it used to force the O(N) full-sort fallback).
//!
//! All tests use a PLAIN MVCC table with NO sorted index, so an ORDER BY query
//! is forced through `read_collecting`'s in-memory heap path (`use_topk`),
//! never an index-ordered scan.

use std::sync::Arc;

use shamir_query_types::read::{OrderBy, OrderByItem, ReadQuery};
use shamir_storage::storage_in_memory::InMemoryStore;
use shamir_storage::types::Store;
use shamir_tx::{MvccStore, RepoTxGate};
use shamir_types::core::interner::InternerKey;
use shamir_types::types::common::new_map;
use shamir_types::types::record_id::RecordId;
use shamir_types::types::value::InnerValue;

use crate::query::filter::eval_context::FilterContext;
use crate::table::TableManager;

// ─────────────────────────────────────────────────────────────────────────────
// Helpers
// ─────────────────────────────────────────────────────────────────────────────

/// A plain (NO sorted index) MVCC-backed table. An ORDER BY query against it is
/// forced through `read_collecting`'s in-memory heap path (F-53a's subject),
/// not an index-ordered scan.
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

/// Intern `field` and persist, returning the key. Centralises the
/// touch_ind + persist dance every insert helper needs.
async fn intern_key(tbl: &TableManager, field: &str) -> InternerKey {
    let interner = tbl.interner().get().await.unwrap();
    let key = interner.touch_ind(field).unwrap().into_key();
    tbl.interner().persist().await.unwrap();
    key
}

/// Insert `{score: s}` and return the assigned RecordId.
async fn insert_scored(tbl: &TableManager, score: i64) -> RecordId {
    let score_key = intern_key(tbl, "score").await;
    let mut m = new_map();
    m.insert(score_key, InnerValue::Int(score));
    tbl.insert(&InnerValue::Map(m)).await.unwrap()
}

/// Insert `{active: bool, score: i64}` and return the assigned RecordId.
async fn insert_active_scored(tbl: &TableManager, active: bool, score: i64) -> RecordId {
    let active_key = intern_key(tbl, "active").await;
    let score_key = intern_key(tbl, "score").await;
    let mut m = new_map();
    m.insert(active_key, InnerValue::Bool(active));
    m.insert(score_key, InnerValue::Int(score));
    tbl.insert(&InnerValue::Map(m)).await.unwrap()
}

/// Insert `{score: i64, tag: str}` and return the assigned RecordId.
async fn insert_scored_tagged(tbl: &TableManager, score: i64, tag: &str) -> RecordId {
    let score_key = intern_key(tbl, "score").await;
    let tag_key = intern_key(tbl, "tag").await;
    let mut m = new_map();
    m.insert(score_key, InnerValue::Int(score));
    m.insert(tag_key, InnerValue::Str(tag.to_owned()));
    tbl.insert(&InnerValue::Map(m)).await.unwrap()
}

/// Insert `{score: i64}` (no `tag`) — used to produce NULL sort keys for the
/// NULL-ordering test (a missing field projects to a NULL ORDER BY key).
async fn insert_scored_untagged(tbl: &TableManager, score: i64) -> RecordId {
    insert_scored(tbl, score).await
}

// ─────────────────────────────────────────────────────────────────────────────
// Test 1 — single-key ORDER BY ASC/DESC + LIMIT is byte-identical to full sort
// ─────────────────────────────────────────────────────────────────────────────

/// The inline bounded heap must return exactly the rows a full sort + truncate
/// would, in the same order, for both ASC and DESC. Deliberately non-monotonic
/// insertion order so the heap truly reorders.
#[tokio::test]
async fn streaming_topk_single_key_asc_desc_matches_expected() {
    let (tbl, _mvcc) = make_plain_mvcc_table().await;
    // Insert in NON-sorted order: 30, 10, 50, 20, 40.
    for &s in &[30i64, 10, 50, 20, 40] {
        insert_scored(&tbl, s).await;
    }
    let interner = tbl.interner().get().await.unwrap();
    let refs = new_map();
    let ctx = FilterContext::new(interner, &refs);

    // ASC LIMIT 3 → [10, 20, 30].
    let q_asc = ReadQuery::new("t").order_by(OrderBy::asc("score")).limit(3);
    let res_asc = tbl.read(&q_asc, &ctx).await.unwrap();
    let asc: Vec<i64> = res_asc
        .records
        .iter()
        .filter_map(|r| r.get_value_i64("score"))
        .collect();
    assert_eq!(asc, vec![10, 20, 30], "ASC LIMIT 3 must be the 3 smallest");

    // DESC LIMIT 3 → [50, 40, 30].
    let q_desc = ReadQuery::new("t")
        .order_by(OrderBy::desc("score"))
        .limit(3);
    let res_desc = tbl.read(&q_desc, &ctx).await.unwrap();
    let desc: Vec<i64> = res_desc
        .records
        .iter()
        .filter_map(|r| r.get_value_i64("score"))
        .collect();
    assert_eq!(desc, vec![50, 40, 30], "DESC LIMIT 3 must be the 3 largest");
}

// ─────────────────────────────────────────────────────────────────────────────
// Test 2 — OFFSET (skip) window: the heap holds k = skip+take, returns take
// ─────────────────────────────────────────────────────────────────────────────

/// `into_sorted` applies the skip/take window over the heap's k survivors.
/// ORDER BY score ASC LIMIT 2 OFFSET 2 of [10,20,30,40,50] → [30, 40].
#[tokio::test]
async fn streaming_topk_offset_window_correct() {
    let (tbl, _mvcc) = make_plain_mvcc_table().await;
    for &s in &[30i64, 10, 50, 20, 40] {
        insert_scored(&tbl, s).await;
    }
    let interner = tbl.interner().get().await.unwrap();
    let refs = new_map();
    let ctx = FilterContext::new(interner, &refs);

    let q = ReadQuery::new("t")
        .order_by(OrderBy::asc("score"))
        .limit(2)
        .offset(2);
    let res = tbl.read(&q, &ctx).await.unwrap();
    let scores: Vec<i64> = res
        .records
        .iter()
        .filter_map(|r| r.get_value_i64("score"))
        .collect();
    assert_eq!(
        scores,
        vec![30, 40],
        "OFFSET 2 LIMIT 2 over [10,20,30,40,50]"
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// Test 3 — multi-key ORDER BY with mixed ASC/DESC direction
// ─────────────────────────────────────────────────────────────────────────────

/// Multi-key `ORDER BY active ASC, score DESC` with LIMIT must respect BOTH
/// keys and BOTH directions. Exercises the pre-resolved multi-key comparator
/// path (SmallVec<[QvSortKey; 4]>) shared with the full-sort path.
#[tokio::test]
async fn streaming_topk_multi_key_mixed_direction() {
    let (tbl, _mvcc) = make_plain_mvcc_table().await;
    // active=false: scores 30, 10, 50  → DESC order 50, 30, 10
    // active=true:  scores 20, 40, 60  → DESC order 60, 40, 20
    // ORDER BY active ASC, score DESC:
    //   [false/50, false/30, false/10, true/60, true/40, true/20]
    insert_active_scored(&tbl, false, 30).await;
    insert_active_scored(&tbl, false, 10).await;
    insert_active_scored(&tbl, false, 50).await;
    insert_active_scored(&tbl, true, 20).await;
    insert_active_scored(&tbl, true, 40).await;
    insert_active_scored(&tbl, true, 60).await;

    let interner = tbl.interner().get().await.unwrap();
    let refs = new_map();
    let ctx = FilterContext::new(interner, &refs);

    let order = OrderBy::new(vec![OrderByItem::asc("active"), OrderByItem::desc("score")]);
    let q = ReadQuery::new("t").order_by(order).limit(4);
    let res = tbl.read(&q, &ctx).await.unwrap();

    let got: Vec<(bool, i64)> = res
        .records
        .iter()
        .filter_map(|r| {
            let a = r.get_value_bool("active")?;
            let s = r.get_value_i64("score")?;
            Some((a, s))
        })
        .collect();
    assert_eq!(
        got,
        vec![(false, 50), (false, 30), (false, 10), (true, 60),],
        "multi-key (active ASC, score DESC) LIMIT 4"
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// Test 4 — NULLS FIRST / NULLS LAST ordering preserved
// ─────────────────────────────────────────────────────────────────────────────

/// Rows missing the ORDER BY field project to a NULL sort key. NULLS FIRST must
/// surface them before the valued rows; NULLS LAST after — in both cases the
/// heap's eviction honours the nulls-ordering exactly as the full sort would.
#[tokio::test]
async fn streaming_topk_nulls_first_and_last() {
    let (tbl, _mvcc) = make_plain_mvcc_table().await;
    // Three tagged rows (tags "b", "a", "c") and two untagged (NULL tag).
    insert_scored_tagged(&tbl, 1, "b").await;
    insert_scored_untagged(&tbl, 2).await;
    insert_scored_tagged(&tbl, 3, "a").await;
    insert_scored_untagged(&tbl, 4).await;
    insert_scored_tagged(&tbl, 5, "c").await;

    let interner = tbl.interner().get().await.unwrap();
    let refs = new_map();
    let ctx = FilterContext::new(interner, &refs);

    // NULLS FIRST ASC: [null, null, "a", "b", "c"], LIMIT 4 → [null, null, "a", "b"]
    let q_first = ReadQuery::new("t")
        .order_by(OrderBy::new(vec![OrderByItem::asc("tag").nulls_first()]))
        .limit(4);
    let res_first = tbl.read(&q_first, &ctx).await.unwrap();
    let first_tags: Vec<Option<String>> = res_first
        .records
        .iter()
        .map(|r| r.get_value_str("tag").map(str::to_owned))
        .collect();
    assert_eq!(
        first_tags,
        vec![None, None, Some("a".into()), Some("b".into())],
        "NULLS FIRST ASC LIMIT 4"
    );

    // NULLS LAST ASC: ["a", "b", "c", null, null], LIMIT 4 → ["a", "b", "c", null]
    let q_last = ReadQuery::new("t")
        .order_by(OrderBy::new(vec![OrderByItem::asc("tag").nulls_last()]))
        .limit(4);
    let res_last = tbl.read(&q_last, &ctx).await.unwrap();
    let last_tags: Vec<Option<String>> = res_last
        .records
        .iter()
        .map(|r| r.get_value_str("tag").map(str::to_owned))
        .collect();
    assert_eq!(
        last_tags,
        vec![Some("a".into()), Some("b".into()), Some("c".into()), None],
        "NULLS LAST ASC LIMIT 4"
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// Test 5 — count_total composes with the bounded heap (F-53a)
// ─────────────────────────────────────────────────────────────────────────────

/// Pre-F-53a, `count_total` forced the O(N) full-sort fallback because the
/// heap "could not supply a total". F-53a decouples `count_total` into an
/// independent running counter over WHERE-passing rows, so the bounded heap
/// now serves it. Assert: the page is the true top-K, AND `total_count` is the
/// TRUE full match count (not the page size).
#[tokio::test]
async fn streaming_topk_count_total_is_full_match_count() {
    let (tbl, _mvcc) = make_plain_mvcc_table().await;
    for &s in &[30i64, 10, 50, 20, 40] {
        insert_scored(&tbl, s).await;
    }
    let interner = tbl.interner().get().await.unwrap();
    let refs = new_map();
    let ctx = FilterContext::new(interner, &refs);

    let mut q = ReadQuery::new("t")
        .order_by(OrderBy::desc("score"))
        .limit(2);
    q.count_total = true;
    let res = tbl.read(&q, &ctx).await.unwrap();

    // Page = top-2 by score DESC → [50, 40].
    let scores: Vec<i64> = res
        .records
        .iter()
        .filter_map(|r| r.get_value_i64("score"))
        .collect();
    assert_eq!(scores, vec![50, 40], "page is the true top-2");

    // total_count = 5 (all matched rows), NOT 2 (the page size) — proving the
    // heap's K-row window did not truncate the count.
    let pagination = res
        .pagination
        .as_ref()
        .expect("count_total must populate pagination metadata");
    assert_eq!(
        pagination.total_count,
        Some(5),
        "count_total must be the full match count, not the page size"
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// Test 6 — with_version composes with the bounded heap (F-53a)
// ─────────────────────────────────────────────────────────────────────────────

/// Pre-F-53a, `with_version` forced the O(N) full-sort fallback because the
/// heap "could not thread a RecordId vector through the reorder". F-53a carries
/// each heap item's RecordId, so the surviving page's ids come out aligned and
/// `collect_versions` rebuilds the per-record array unchanged. Assert:
/// `versions[i]` is the canonical version of the record NOW at position i
/// (post-sort), for the surviving top-K page.
#[tokio::test]
async fn streaming_topk_with_version_aligned_to_survivors() {
    let (tbl, mvcc) = make_plain_mvcc_table().await;
    // Insert in NON-sorted order; capture each id + ground-truth version.
    let mut inserted: Vec<(i64, RecordId, u64)> = Vec::new();
    for &s in &[30i64, 10, 50, 20, 40] {
        let id = insert_scored(&tbl, s).await;
        let v = mvcc.version_of(id.as_bytes());
        inserted.push((s, id, v));
    }
    let interner = tbl.interner().get().await.unwrap();
    let refs = new_map();
    let ctx = FilterContext::new(interner, &refs);

    let mut q = ReadQuery::new("t").order_by(OrderBy::asc("score")).limit(3);
    q.with_version = true;
    let res = tbl.read(&q, &ctx).await.unwrap();

    let scores: Vec<i64> = res
        .records
        .iter()
        .filter_map(|r| r.get_value_i64("score"))
        .collect();
    assert_eq!(scores, vec![10, 20, 30], "top-3 ASC");

    let versions = res
        .versions
        .as_ref()
        .expect("with_version + top-K must populate versions");
    assert_eq!(
        versions.len(),
        scores.len(),
        "versions index-aligned with the surviving page"
    );

    // Ground truth: sort the inserted (score, version) pairs by score ASC and
    // take the first 3 — the versions must match that order exactly.
    let mut truth: Vec<(i64, u64)> = inserted.iter().map(|(s, _, v)| (*s, *v)).collect();
    truth.sort_by_key(|(s, _)| *s);
    let expected_versions: Vec<u64> = truth.iter().take(3).map(|(_, v)| *v).collect();
    assert_eq!(
        versions, &expected_versions,
        "versions[i] must be the version of the record now at position i (post-heap-sort)"
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// Test 7 — count_total + with_version together on the bounded heap
// ─────────────────────────────────────────────────────────────────────────────

/// Both F-53a relaxations at once: `count_total` AND `with_version` on the same
/// ORDER BY + LIMIT query, both served by the inline heap. Asserts the two
/// independent concerns do not interfere: the page is correct, total_count is
/// the true full count, and versions are aligned with the page.
#[tokio::test]
async fn streaming_topk_count_total_and_with_version_compose() {
    let (tbl, mvcc) = make_plain_mvcc_table().await;
    let mut inserted: Vec<(i64, u64)> = Vec::new();
    for &s in &[30i64, 10, 50, 20, 40] {
        let id = insert_scored(&tbl, s).await;
        inserted.push((s, mvcc.version_of(id.as_bytes())));
    }
    let interner = tbl.interner().get().await.unwrap();
    let refs = new_map();
    let ctx = FilterContext::new(interner, &refs);

    let mut q = ReadQuery::new("t").order_by(OrderBy::asc("score")).limit(2);
    q.count_total = true;
    q.with_version = true;
    let res = tbl.read(&q, &ctx).await.unwrap();

    let scores: Vec<i64> = res
        .records
        .iter()
        .filter_map(|r| r.get_value_i64("score"))
        .collect();
    assert_eq!(scores, vec![10, 20], "top-2 ASC");

    let pagination = res.pagination.as_ref().expect("pagination present");
    assert_eq!(pagination.total_count, Some(5), "true total, not page size");

    let versions = res.versions.as_ref().expect("versions present");
    let mut truth = inserted.clone();
    truth.sort_by_key(|(s, _)| *s);
    let expected: Vec<u64> = truth.iter().take(2).map(|(_, v)| *v).collect();
    assert_eq!(versions, &expected, "versions aligned with the top-2 page");
}
