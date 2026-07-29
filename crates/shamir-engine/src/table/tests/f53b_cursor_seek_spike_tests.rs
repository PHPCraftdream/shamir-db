//! F-53b Step 1 (#875, spike) — AsOf-aware cursor index-seek: red→green proof.
//!
//! Settles the brief's central question: can a cursor's pinned-snapshot
//! (`Temporal::AsOf { at: Version(pinned) }`) read reach the sorted-index
//! SEEK fast path (`lookup_range_first_k_page`) WITHOUT weakening the
//! snapshot-isolation guarantee (a row committed AFTER `CreateCursor` must
//! never appear in any later `FetchNext` page)?
//!
//! The prototype below is a TEST-LOCAL reimplementation of the seek +
//! per-candidate MVCC-version filter. It is deliberately NOT wired into the
//! production `read()` path — that is Step 2's job, scoped by the decision
//! memo at `docs/dev-artifacts/research/f53b-cursor-seek-spike.md`. It uses
//! only public / `pub(crate)` engine APIs (`sorted_indexes()`,
//! `mvcc_store_ref()`, `MvccStore::version_of` / `get_at`), so it proves the
//! mechanism is reachable from the existing surface with no new index-layer
//! primitive required.
//!
//! ## What each test proves
//!
//! - `red_current_asof_path_scans_full_table_per_page` — the CURRENT
//!   production `read_as_of` path (every cursor `FetchNext`) re-scans the
//!   whole matched set per page: `records_scanned ≈ N` on EVERY page, so
//!   paging K pages costs cumulative `O(K × N)`, not `O(K × page_size)`.
//!   This is the honestly-documented limitation the spike targets.
//!
//! - `green_asof_seek_matches_baseline_with_insert_after_pin` — the
//!   prototype seek yields BYTE-IDENTICAL page contents to the production
//!   full-scan baseline across 3 pages, while a row INSERTed after the
//!   snapshot pin (before page 2, and again before page 3) NEVER appears
//!   (the per-candidate `version_of > pinned` + `get_at == None` check
//!   excludes it). `candidates_examined` per page is `O(page_size)`, not
//!   `O(N)` — the asymptotic win, measured.
//!
//! - `negative_update_indexed_field_after_pin_seek_cannot_place_row` — an
//!   UPDATE to the ORDER BY field committed after pin MOVES the index
//!   posting; the current-state index no longer carries the record's
//!   pinned-version position, so the seek CANNOT place it. The baseline
//!   (full scan + `get_at`) returns the row at its OLD position. This is the
//!   irreducible reason a safe production path MUST gate on "no concurrent
//!   write touched this index since pin" and fall back to the full scan.
//!
//! - `negative_delete_after_pin_seek_misses_row` — a DELETE after pin
//!   removes the posting entirely; the seek never yields the record id, so a
//!   row that existed at pin (and that the baseline returns via
//!   tombstone-inclusive enumeration) is INVISIBLE to the seek. Same
//!   conclusion: the gate + fallback is mandatory.

use std::sync::Arc;

use bytes::Bytes;
use shamir_query_types::filter::{FieldPath, Filter, FilterValue};
use shamir_query_types::read::select::Select;
use shamir_query_types::read::{At, OrderBy, ReadQuery, Temporal};
use shamir_storage::storage_in_memory::InMemoryStore;
use shamir_storage::types::Store;
use shamir_tx::{MvccStore, RepoTxGate, Retention};
use shamir_types::core::sort_codec;
use shamir_types::record_view::RecordView;
use shamir_types::types::common::new_map;
use shamir_types::types::record_id::RecordId;
use shamir_types::types::value::InnerValue;

use crate::query::filter::eval_context::FilterContext;
use crate::query::read::QueryResult;
use crate::table::TableManager;

// ─────────────────────────────────────────────────────────────────────────────
// Harness — MVCC table with a single-column sorted index on `score` (Int ASC).
// Mirrors `asof_read_tests` (MVCC + RepoTxGate) and `keyset_seek_tests`
// (sorted index on `score`).
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
    // Single-column ORDER BY index on `score` — the target case. NOTE: this
    // is a NON-covering index (no INCLUDES), so its postings carry NO
    // covering-projection version stamp (the value is `Bytes::new()`). The
    // spike's mechanism therefore relies on `MvccStore::version_of`, not the
    // envelope stamp — see the memo's §1 for why the stamp does not apply.
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

/// Interner id of the `score` field (the ORDER BY / indexed column).
async fn score_interned_id(tbl: &TableManager) -> u64 {
    let interner = tbl.interner().get().await.unwrap();
    interner.touch_ind("score").unwrap().key().id()
}

/// Find the record id whose AS-OF value at `pinned` has `score == target`.
/// (`get_at` is async, so this cannot be a sync `Iterator::find` closure.)
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

/// Extract the `score` (i64) from a materialised AS-OF record byte buffer,
/// via the zero-copy `RecordView` lens (the same path `read_as_of` uses to
/// evaluate WHERE on AS-OF bytes).
fn score_from_bytes(bytes: &Bytes, score_id: u64) -> Option<i64> {
    let view = RecordView::new(bytes).ok()?;
    match view.get_by_id(score_id)? {
        shamir_types::record_view::RecordValue::Int(i) => Some(i),
        _ => None,
    }
}

/// `score` (i64) for each row of a `QueryResult`, in result order. The AsOf
/// path emits `QueryRecord::Direct` (no `_id`), so the score sequence is the
/// page-content signature; with distinct scores it uniquely identifies rows.
fn baseline_scores(result: &QueryResult) -> Vec<i64> {
    result
        .records
        .iter()
        .filter_map(|r| r.get_value_i64("score"))
        .collect()
}

/// Inclusive boundary filter `score >= n` — mirrors the cursor's
/// `boundary_filter()` (CR-A4: inclusive on the seek key).
fn score_gte(n: i64) -> Filter {
    Filter::Gte {
        field: FieldPath::from(vec!["score".to_string()]),
        value: FilterValue::Int(n),
    }
}

/// The cursor's pinned snapshot version = the MVCC high-water mark after the
/// initial inserts (mirrors `gate.open_snapshot().version()`; full history is
/// retained so no live guard is needed for correctness in the test).
fn pinned_at(gate: &RepoTxGate) -> u64 {
    gate.last_committed()
}

// ─────────────────────────────────────────────────────────────────────────────
// SPIKE PROTOTYPE — AsOf-aware sorted-index keyset seek.
// ─────────────────────────────────────────────────────────────────────────────

/// One page produced by the prototype seek.
struct AsofSeekPage {
    /// `(record_id, as-of bytes)` in index (ORDER BY) order.
    rows: Vec<(RecordId, Bytes)>,
    /// Opaque resume bookmark for the next page (`None` ⇒ range exhausted,
    /// this was the last page).
    resume_key: Option<Bytes>,
    /// Number of physical index postings the seek walked to fill this page.
    /// The asymptotic-cost metric: `O(page_size)` here vs `O(N)` for the
    /// full-scan baseline.
    candidates_examined: u64,
    /// Postings whose record did NOT exist at `pinned_version` (an INSERT
    /// committed after the pin) — correctly EXCLUDED from the page. This is
    /// the snapshot-isolation proof.
    excluded_inserts: u64,
    /// Postings whose record EXISTED at `pinned_version` but was MODIFIED
    /// after the pin (an UPDATE/DELETE-after-pin). A current-state index
    /// cannot place such a record at its pinned-version position, so the
    /// prototype flags it and a production path MUST fall back to the full
    /// scan. Zero in the insert-only scenario.
    concurrent_modified: u64,
}

/// F-53b Step 1 SPIKE PROTOTYPE: AsOf-aware sorted-index keyset seek.
///
/// Walks the sorted index in ORDER BY direction via
/// `lookup_range_first_k_page` (the SAME primitive the `Temporal::Latest`
/// keyset-seek fast path uses), and for EACH candidate record id applies a
/// pinned-version visibility check against the MVCC store:
///
/// - `version_of(id) != 0 && <= pinned` — the record's CURRENT committed
///   value was written at/before the pin, so (MVCC immutability) its AS-OF
///   value EQUALS its current value and the index posting's position is
///   correct for the pinned snapshot. Read it via `get_at(id, pinned)` and
///   include it.
/// - otherwise (a write landed on this key after the pin, or the cell is
///   cold) — fetch `get_at(id, pinned)`:
///   - `None` ⇒ the record did not exist at pin (INSERT-after-pin) ⇒ EXCLUDE.
///   - `Some` ⇒ the record existed at pin but was modified after
///     (UPDATE/DELETE-after-pin) ⇒ flag `concurrent_modified`; the posting's
///     current position is NOT trustworthy for the pinned snapshot, so the
///     prototype drops it and a production path must defer to the full scan.
///
/// The internal loop resumes the seek (chaining the index cursor) until
/// `limit` visible rows accumulate or the range is exhausted — exactly
/// `read_keyset_seek`'s stale-posting loop, generalised to also resume past
/// version-filtered candidates.
#[allow(clippy::too_many_arguments)]
async fn asof_index_seek_spike(
    tbl: &TableManager,
    pinned_version: u64,
    index_name: u64,
    seek_encoded: &[u8],
    after_key: Option<&[u8]>,
    after_id: Option<&RecordId>,
    limit: usize,
    forward: bool,
) -> AsofSeekPage {
    let mvcc = tbl
        .mvcc_store_ref()
        .expect("AsOf seek requires an MVCC-backed table");

    let mut rows: Vec<(RecordId, Bytes)> = Vec::with_capacity(limit);
    let mut next_after_key: Option<Bytes> = after_key.map(Bytes::copy_from_slice);
    let mut resume_key: Option<Bytes> = None;
    let mut candidates_examined: u64 = 0;
    let mut excluded_inserts: u64 = 0;
    let mut concurrent_modified: u64 = 0;

    loop {
        let need = limit - rows.len();
        if need == 0 {
            break;
        }
        let (id_vec, cursor) = tbl
            .sorted_indexes()
            .lookup_range_first_k_page(
                index_name,
                seek_encoded,
                next_after_key.as_deref(),
                after_id,
                need,
                forward,
            )
            .await
            .expect("index seek");
        candidates_examined += id_vec.len() as u64;

        for id in &id_vec {
            let key = id.to_bytes();
            let v = mvcc.version_of(&key);
            if v != 0 && v <= pinned_version {
                // Common path: no concurrent write on this key since the pin.
                // The current value IS the pinned value (MVCC immutability),
                // so the posting's position is correct for the snapshot.
                if let Some(bytes) = mvcc.get_at(&key, pinned_version).await.unwrap() {
                    rows.push((*id, bytes));
                }
                // A `None` here is a tombstone/no-value edge — skip.
            } else {
                // A write landed on this key after the pin (or cold cell).
                match mvcc.get_at(&key, pinned_version).await.unwrap() {
                    None => excluded_inserts += 1,       // INSERT-after-pin: drop.
                    Some(_) => concurrent_modified += 1, // MODIFY-after-pin: flag.
                }
            }
            if rows.len() == limit {
                break;
            }
        }

        next_after_key = cursor.clone();
        match cursor {
            Some(c) => {
                resume_key = Some(c);
                if rows.len() < limit {
                    continue; // need more visible rows; resume the seek.
                }
                break; // page full.
            }
            None => {
                resume_key = None; // range exhausted — genuine last page.
                break;
            }
        }
    }

    AsofSeekPage {
        rows,
        resume_key,
        candidates_examined,
        excluded_inserts,
        concurrent_modified,
    }
}

/// `score` (i64) for a prototype page, in index order.
fn spike_scores(page: &AsofSeekPage, score_id: u64) -> Vec<i64> {
    page.rows
        .iter()
        .filter_map(|(id, bytes)| Some((score_from_bytes(bytes, score_id)?, *id)))
        .map(|(s, _)| s)
        .collect()
}

// ─────────────────────────────────────────────────────────────────────────────
// RED — current production `read_as_of` re-scans the whole table per page.
// ─────────────────────────────────────────────────────────────────────────────

/// The cursor's per-`FetchNext` read is `Temporal::AsOf { pinned }` routed to
/// `read_as_of`, which enumerates EVERY record (tombstone-inclusive) and
/// reads each at the pinned version — `records_scanned ≈ N` on EVERY page,
/// independent of how far the cursor has progressed. So fetching K pages
/// costs cumulative `O(K × N)`, not `O(K × page_size)`. This test MEASURES
/// that cost on the real production path (no prototype involved) and pins the
/// numbers the green test then beats.
#[tokio::test]
async fn red_current_asof_path_scans_full_table_per_page() {
    let (tbl, mvcc, gate) = make_mvcc_score_table().await;
    // 30 rows: scores 0,10,...,290. page_size = 10 ⇒ 3 full pages.
    for s in (0..30).map(|i| i * 10) {
        let _ = insert_score(&tbl, &mvcc, s, &format!("r{s}")).await;
    }
    let pinned = pinned_at(&gate);

    let interner = tbl.interner().get().await.unwrap();
    let refs = new_map();
    let ctx = FilterContext::new(interner, &refs);

    let page_size = 10u64;
    // Page 1: no boundary, OFFSET 0.
    let q1 = asof_page_query(None, 0, page_size, pinned);
    let r1 = tbl.read(&q1, &ctx).await.unwrap();
    // Pages 2..3: inclusive boundary on the prior page's last score + OFFSET 1
    // (skip the re-included boundary row — the cursor's `tie_skip`, CR-A4).
    let last1 = r1
        .records
        .last()
        .and_then(|r| r.get_value_i64("score"))
        .unwrap();
    let q2 = asof_page_query(Some(last1), 1, page_size, pinned);
    let r2 = tbl.read(&q2, &ctx).await.unwrap();
    let last2 = r2
        .records
        .last()
        .and_then(|r| r.get_value_i64("score"))
        .unwrap();
    let q3 = asof_page_query(Some(last2), 1, page_size, pinned);
    let r3 = tbl.read(&q3, &ctx).await.unwrap();

    let n = 30u64;
    let scanned1 = r1.stats.as_ref().unwrap().records_scanned;
    let scanned2 = r2.stats.as_ref().unwrap().records_scanned;
    let scanned3 = r3.stats.as_ref().unwrap().records_scanned;

    // Each page enumerates the WHOLE table — the limitation.
    assert!(
        scanned1 >= n && scanned2 >= n && scanned3 >= n,
        "each AsOf page must scan the full table; got {scanned1},{scanned2},{scanned3} (N={n})"
    );
    // Cumulative cost grows linearly with page count: 3 pages ≈ 3N.
    let cumulative = scanned1 + scanned2 + scanned3;
    assert!(
        cumulative >= 3 * n,
        "cumulative scan over 3 pages must be ~3N; got {cumulative} (3N={})",
        3 * n
    );
    // Correctness sanity: 3 distinct, full, in-order pages.
    assert_eq!(
        baseline_scores(&r1),
        (0..10).map(|i| i * 10).collect::<Vec<_>>()
    );
    assert_eq!(
        baseline_scores(&r2),
        (10..20).map(|i| i * 10).collect::<Vec<_>>()
    );
    assert_eq!(
        baseline_scores(&r3),
        (20..30).map(|i| i * 10).collect::<Vec<_>>()
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// GREEN — prototype seek: identical pages, inserts excluded, O(page_size) cost.
// ─────────────────────────────────────────────────────────────────────────────

/// The settled mechanism (per-candidate `version_of` + `get_at` against the
/// pinned snapshot) produces BYTE-IDENTICAL page contents to the production
/// full-scan baseline across 3 pages, while a row INSERTed after the snapshot
/// pin (before page 2, and again before page 3) NEVER appears in any page —
/// proving snapshot isolation holds on the seek path. `candidates_examined`
/// per page is `O(page_size)`, dramatically below the baseline's `O(N)`.
#[tokio::test]
async fn green_asof_seek_matches_baseline_with_insert_after_pin() {
    let (tbl, mvcc, gate) = make_mvcc_score_table().await;
    // 30 rows: scores 0,10,...,290 ⇒ pages [0-90],[100-190],[200-290].
    for s in (0..30).map(|i| i * 10) {
        let _ = insert_score(&tbl, &mvcc, s, &format!("r{s}")).await;
    }
    let pinned = pinned_at(&gate);
    let score_id = score_interned_id(&tbl).await;
    let index_name = tbl
        .sorted_indexes()
        .iter_indexes()
        .into_iter()
        .next()
        .unwrap()
        .name_interned;

    let interner = tbl.interner().get().await.unwrap();
    let refs = new_map();
    let ctx = FilterContext::new(interner, &refs);

    // Lower-bound encoding for "from the beginning" (ASC over all scores).
    let mut lo = Vec::new();
    sort_codec::encode_i64(&mut lo, i64::MIN);

    let page_size = 10usize;

    // ── Page 1 (no concurrent write yet) ─────────────────────────────────
    let p1 =
        asof_index_seek_spike(&tbl, pinned, index_name, &lo, None, None, page_size, true).await;
    let q1 = asof_page_query(None, 0, page_size as u64, pinned);
    let b1 = tbl.read(&q1, &ctx).await.unwrap();
    assert_eq!(
        spike_scores(&p1, score_id),
        baseline_scores(&b1),
        "page 1: prototype must match baseline exactly"
    );
    assert_eq!(p1.excluded_inserts, 0, "no post-pin inserts before page 1");
    assert_eq!(p1.concurrent_modified, 0);
    assert!(
        p1.candidates_examined <= page_size as u64,
        "page 1 candidates_examined must be ~page_size; got {}",
        p1.candidates_examined
    );
    assert!(
        p1.candidates_examined < b1.stats.as_ref().unwrap().records_scanned,
        "page 1: seek must examine fewer candidates than the full scan"
    );
    let mut after_key = p1.resume_key.clone();
    let mut after_id = p1.rows.last().map(|(id, _)| *id);

    // ── INSERT 125 after the pin, before page 2 (sorts into page 2's range) ──
    let (ins_id, ins_v) = insert_score(&tbl, &mvcc, 125, "late125").await;
    assert!(ins_v > pinned, "the late insert must commit after the pin");
    let p1_last_score = score_from_bytes(&p1.rows.last().unwrap().1, score_id).unwrap();

    let p2 = asof_index_seek_spike(
        &tbl,
        pinned,
        index_name,
        &lo,
        after_key.as_deref(),
        after_id.as_ref(),
        page_size,
        true,
    )
    .await;
    let q2 = asof_page_query(Some(p1_last_score), 1, page_size as u64, pinned);
    let b2 = tbl.read(&q2, &ctx).await.unwrap();
    assert_eq!(
        spike_scores(&p2, score_id),
        baseline_scores(&b2),
        "page 2: prototype must match baseline exactly (late insert excluded)"
    );
    // The snapshot-isolation proof: the late insert NEVER appears.
    assert!(
        !p2.rows.iter().any(|(id, _)| *id == ins_id),
        "the post-pin insert must NOT appear in page 2"
    );
    assert!(
        !spike_scores(&p2, score_id).contains(&125),
        "score 125 (late insert) must NOT appear in page 2"
    );
    assert!(
        p2.excluded_inserts >= 1,
        "the seek must have classified the late insert as excluded (got {})",
        p2.excluded_inserts
    );
    assert_eq!(p2.concurrent_modified, 0);
    assert!(
        p2.candidates_examined <= (page_size as u64) + p2.excluded_inserts,
        "page 2 candidates_examined must be ~page_size + excluded; got {}",
        p2.candidates_examined
    );
    assert!(
        p2.candidates_examined < b2.stats.as_ref().unwrap().records_scanned,
        "page 2: seek must examine fewer candidates than the full scan"
    );
    after_key = p2.resume_key.clone();
    after_id = p2.rows.last().map(|(id, _)| *id);

    // ── INSERT 225 after the pin, before page 3 (sorts into page 3's range) ──
    let (ins_id2, ins_v2) = insert_score(&tbl, &mvcc, 225, "late225").await;
    assert!(ins_v2 > pinned);
    let p2_last_score = score_from_bytes(&p2.rows.last().unwrap().1, score_id).unwrap();

    let p3 = asof_index_seek_spike(
        &tbl,
        pinned,
        index_name,
        &lo,
        after_key.as_deref(),
        after_id.as_ref(),
        page_size,
        true,
    )
    .await;
    let q3 = asof_page_query(Some(p2_last_score), 1, page_size as u64, pinned);
    let b3 = tbl.read(&q3, &ctx).await.unwrap();
    assert_eq!(
        spike_scores(&p3, score_id),
        baseline_scores(&b3),
        "page 3: prototype must match baseline exactly (late insert excluded)"
    );
    assert!(
        !p3.rows.iter().any(|(id, _)| *id == ins_id2),
        "the post-pin insert must NOT appear in page 3"
    );
    assert!(
        !spike_scores(&p3, score_id).contains(&225),
        "score 225 (late insert) must NOT appear in page 3"
    );
    assert!(
        p3.excluded_inserts >= 1,
        "page 3 seek must have excluded the late insert (got {})",
        p3.excluded_inserts
    );
    assert_eq!(p3.concurrent_modified, 0);

    // Page 4 must be EMPTY: the 3 pages covered all 30 pinned-version rows.
    // (`lookup_range_first_k_page` returns a `Some` resume cursor even when it
    // yields exactly `k` entries at the tail of the range — it cannot read
    // one entry ahead — so a genuine final page is proven by the NEXT seek
    // returning zero visible rows, not by `resume_key` being `None`.)
    let p3_after_id = p3.rows.last().map(|(id, _)| *id);
    let p4 = asof_index_seek_spike(
        &tbl,
        pinned,
        index_name,
        &lo,
        p3.resume_key.as_deref(),
        p3_after_id.as_ref(),
        page_size,
        true,
    )
    .await;
    assert!(
        p4.rows.is_empty(),
        "page 4 must be empty (all 30 pinned rows consumed in pages 1-3); got {} rows",
        p4.rows.len()
    );
    assert!(p4.resume_key.is_none(), "page 4 exhausts the index range");

    // Cumulative-cost comparison: the seek's total candidates_examined vs the
    // baseline's total records_scanned. Seek is O(pages × page_size); the
    // baseline is O(pages × N).
    let seek_total = p1.candidates_examined + p2.candidates_examined + p3.candidates_examined;
    let baseline_total = b1.stats.as_ref().unwrap().records_scanned
        + b2.stats.as_ref().unwrap().records_scanned
        + b3.stats.as_ref().unwrap().records_scanned;
    assert!(
        seek_total < baseline_total,
        "cumulative seek cost ({seek_total}) must beat the baseline full scan ({baseline_total})"
    );
    // Every original row appears exactly once across the 3 pages, in order.
    let all_spike: Vec<i64> = [&p1, &p2, &p3]
        .iter()
        .flat_map(|p| spike_scores(p, score_id))
        .collect();
    assert_eq!(all_spike, (0..30).map(|i| i * 10).collect::<Vec<_>>());
}

// ─────────────────────────────────────────────────────────────────────────────
// NEGATIVE — UPDATE to the indexed field after pin: the seek cannot place it.
// ─────────────────────────────────────────────────────────────────────────────

/// A record whose ORDER BY field is UPDATED after the pin has its index
/// posting MOVED to the new value's position (the old posting is removed by
/// `plan_record_updated`). The current-state index therefore no longer
/// carries the record's PINNED-version position. The full-scan baseline
/// returns the row at its OLD (pinned) position; the seek cannot — this is
/// the irreducible correctness gap that forces a production path to gate on
/// "no concurrent write touched this index since pin" and fall back to the
/// full scan. The test records the divergence (`concurrent_modified >= 1`).
#[tokio::test]
async fn negative_update_indexed_field_after_pin_seek_cannot_place_row() {
    let (tbl, mvcc, gate) = make_mvcc_score_table().await;
    // 5 rows: scores 10,20,30,40,50.
    let mut ids = Vec::new();
    for s in [10, 20, 30, 40, 50] {
        ids.push(insert_score(&tbl, &mvcc, s, &format!("r{s}")).await.0);
    }
    let pinned = pinned_at(&gate);
    let score_id = score_interned_id(&tbl).await;
    let index_name = tbl
        .sorted_indexes()
        .iter_indexes()
        .into_iter()
        .next()
        .unwrap()
        .name_interned;

    // UPDATE the score-30 row to score-35 AFTER the pin (moves its posting
    // from the 30 position to the 35 position).
    let row30 = find_row_with_score(&mvcc, &ids, 30, pinned, score_id).await;
    let mut m = new_map();
    m.insert(
        shamir_types::core::interner::InternerKey::new(score_id),
        InnerValue::Int(35),
    );
    let label_id = tbl
        .interner()
        .get()
        .await
        .unwrap()
        .touch_ind("label")
        .unwrap()
        .key()
        .id();
    m.insert(
        shamir_types::core::interner::InternerKey::new(label_id),
        InnerValue::Str("r30→35".to_owned()),
    );
    tbl.set(row30, &InnerValue::Map(m)).await.unwrap();

    let interner = tbl.interner().get().await.unwrap();
    let refs = new_map();
    let ctx = FilterContext::new(interner, &refs);

    // Baseline (full scan at pinned): the row is STILL at score 30 (its
    // pinned value), so ASC order is 10,20,30,40,50.
    let q = ReadQuery::new("t")
        .select(Select::fields(["score"]))
        .order_by(OrderBy::asc("score"))
        .into_temporal(Temporal::AsOf {
            at: At::Version(pinned),
        });
    let baseline = tbl.read(&q, &ctx).await.unwrap();
    assert_eq!(
        baseline_scores(&baseline),
        vec![10, 20, 30, 40, 50],
        "baseline must see the pinned (pre-update) value at its old position"
    );

    // Prototype seek over the WHOLE range: the score-30 row's posting has
    // MOVED to 35 (version > pinned), so the seek classifies it as
    // `concurrent_modified` and cannot place it at 30. The visible rows are
    // only those with version_of <= pinned (10,20,40,50) — the moved row is
    // absent from its pinned position. This MISSES a row the baseline
    // correctly returns.
    let mut lo = Vec::new();
    sort_codec::encode_i64(&mut lo, i64::MIN);
    let page = asof_index_seek_spike(&tbl, pinned, index_name, &lo, None, None, 100, true).await;
    let seek_scores = spike_scores(&page, score_id);
    assert!(
        page.concurrent_modified >= 1,
        "the seek must flag the moved row as concurrent_modified (got {})",
        page.concurrent_modified
    );
    // The moved row's pinned value (30) is NOT at its correct position in the
    // seek output — the seek cannot place it. (The prototype conservatively
    // drops it, so 30 is absent from the seek result.)
    assert!(
        !seek_scores.contains(&30),
        "the seek cannot reproduce the baseline under an indexed-field update: \
         seek={seek_scores:?} baseline=[10,20,30,40,50]"
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// NEGATIVE — DELETE after pin: the posting is gone, the seek misses the row.
// ─────────────────────────────────────────────────────────────────────────────

/// A record DELETED after the pin has its index posting REMOVED
/// (`plan_record_deleted` → `RemovePosting`). The seek therefore never yields
/// the record id at all, so a row that existed at pin — and that the baseline
/// returns via `list_stream_with_tombstones` + `get_at` — is INVISIBLE to the
/// seek. Same conclusion as the update case: the gate + full-scan fallback is
/// mandatory.
#[tokio::test]
async fn negative_delete_after_pin_seek_misses_row() {
    let (tbl, mvcc, gate) = make_mvcc_score_table().await;
    // 5 rows: scores 10,20,30,40,50.
    let mut ids = Vec::new();
    for s in [10, 20, 30, 40, 50] {
        ids.push(insert_score(&tbl, &mvcc, s, &format!("r{s}")).await.0);
    }
    let pinned = pinned_at(&gate);
    let score_id = score_interned_id(&tbl).await;
    let index_name = tbl
        .sorted_indexes()
        .iter_indexes()
        .into_iter()
        .next()
        .unwrap()
        .name_interned;

    // DELETE the score-30 row AFTER the pin.
    let row30 = find_row_with_score(&mvcc, &ids, 30, pinned, score_id).await;
    tbl.delete(row30).await.unwrap();

    let interner = tbl.interner().get().await.unwrap();
    let refs = new_map();
    let ctx = FilterContext::new(interner, &refs);

    // Baseline (full scan at pinned): the row STILL existed at pin (its
    // tombstone is post-pin), so the baseline returns it at score 30.
    let q = ReadQuery::new("t")
        .select(Select::fields(["score"]))
        .order_by(OrderBy::asc("score"))
        .into_temporal(Temporal::AsOf {
            at: At::Version(pinned),
        });
    let baseline = tbl.read(&q, &ctx).await.unwrap();
    assert_eq!(
        baseline_scores(&baseline),
        vec![10, 20, 30, 40, 50],
        "baseline must see the deleted-after-pin row at its pinned value"
    );

    // Prototype seek: the score-30 posting was removed, so the seek never
    // yields row30 — it is MISSING from the output, with no
    // `concurrent_modified` signal (the index simply does not contain it).
    let mut lo = Vec::new();
    sort_codec::encode_i64(&mut lo, i64::MIN);
    let page = asof_index_seek_spike(&tbl, pinned, index_name, &lo, None, None, 100, true).await;
    let seek_scores = spike_scores(&page, score_id);
    assert!(
        !seek_scores.contains(&30),
        "the seek must MISS the deleted-after-pin row (posting gone): seek={seek_scores:?}"
    );
    assert_eq!(
        page.concurrent_modified, 0,
        "a delete removes the posting silently — no concurrent_modified signal is possible"
    );
    assert_eq!(seek_scores, vec![10, 20, 40, 50]);
}

// ─────────────────────────────────────────────────────────────────────────────
// Helpers
// ─────────────────────────────────────────────────────────────────────────────

/// Build a cursor-style AsOf page query: ORDER BY score ASC + inclusive
/// boundary + OFFSET tie-skip + LIMIT, at the pinned version.
fn asof_page_query(
    boundary_score: Option<i64>,
    tie_skip: u64,
    limit: u64,
    pinned: u64,
) -> ReadQuery {
    let mut q = ReadQuery::new("t")
        .select(Select::fields(["score"]))
        .order_by(OrderBy::asc("score"))
        .limit(limit)
        .offset(tie_skip)
        .into_temporal(Temporal::AsOf {
            at: At::Version(pinned),
        });
    if let Some(n) = boundary_score {
        q.r#where = Some(score_gte(n));
    }
    q
}

/// `ReadQuery::into_temporal` builder helper (mirrors `asof_read_tests`).
trait IntoTemporal {
    fn into_temporal(self, temporal: Temporal) -> Self;
}

impl IntoTemporal for ReadQuery {
    fn into_temporal(mut self, temporal: Temporal) -> Self {
        self.temporal = temporal;
        self
    }
}
