//! F-53b Step 3 (#879, spike) — can `cursor_handlers.rs`'s `FetchNext` reach
//! the AsOf keyset-seek arm (`read_as_of_keyset_seek`, landed F-53b Step 2)
//! by switching to `Pagination::After`, and how?
//!
//! ## The blocking finding (restated, brief §"blocking finding")
//!
//! `TableManager::try_plan_keyset_seek` (`read_planner.rs:472-525`) —
//! shared by BOTH the pre-existing `Temporal::Latest` fast path and the new
//! AsOf arm — hard-requires `query.r#where.is_none()`. `cursor_handlers.rs`'s
//! CURRENT bookmark scheme ALWAYS AND-combines an inclusive `Gte`/`Lte`
//! boundary into `query.r#where` (`boundary_filter`). So a cursor can reach
//! the seek arm ONLY when the caller's ORIGINAL query has NO `where` clause
//! at all — extending `try_plan_keyset_seek` to accept a `WHERE` is
//! explicitly out of scope for this task (a materially bigger change).
//!
//! This file does NOT touch that guard. It prototypes, test-locally, the
//! THREE things a production wiring would need on top of the existing
//! (unmodified) `try_plan_keyset_seek` / `read_as_of_keyset_seek`:
//!
//! 1. **`probe_1_eligibility_probe_uses_only_public_api`** — an eligibility
//!    probe that decides ONCE, at `create_cursor` time, whether a query
//!    qualifies for the index-seek bookmark scheme (no `where`, single
//!    simple-field ORDER BY, a sorted index on that field), WITHOUT running
//!    a seek. Proves this is buildable entirely from the ALREADY-`pub`
//!    surface `cursor_handlers.rs` can already reach (`TableManager::
//!    sorted_indexes()`, `SortedIndexManager::find_by_field`, and
//!    `shamir_engine::query::filter::resolve::intern_field_path`,
//!    transitively `pub` via `shamir_db::query`/`shamir_db::engine`) — NO
//!    new `pub` method needs to be added to `TableManager` for the
//!    eligibility check itself (distinct from `try_plan_keyset_seek`, whose
//!    OWN `where`/pagination checks are exactly what this probe must NOT
//!    reuse, since create_cursor is deciding the bookmark scheme BEFORE any
//!    `Pagination::After` value exists to check).
//!
//! 2. **`probe_2_seek_arm_attaches_real_record_id`** — confirms (again, on
//!    THIS task's harness) that the seek arm's `QueryRecord::Inserted { id:
//!    Some(id), .. }` carries a real, stable `RecordId` per row, unlike the
//!    generic AsOf full-scan projection (`QueryRecord::Direct`, no id) —
//!    the concrete reason an `IndexSeek`-mode `CursorState` bookmark can be
//!    `Option<RecordId>` (feeding `Pagination::after_with_id`) instead of
//!    `Keyset` mode's `tie_skip` counting hack.
//!
//! 3. **`negative_gate_failure_mid_lifetime_must_not_page_one_forever`** —
//!    THE load-bearing negative test. Simulates an `IndexSeek`-mode
//!    cursor's `FetchNext` sequence across a MID-LIFETIME concurrent write
//!    that trips the F-53b Step 2 gate (`last_mutation_version() >
//!    pinned`). Proves that a naive "just always send `Pagination::After`"
//!    implementation would silently return the SAME page forever once the
//!    gate declines (the exact hazard the brief warns `Pagination::
//!    resolve()` creates for every NON-seek path), and that detecting
//!    `stats.index_used` no longer ending in `_asof_keyset` after a
//!    `FetchNext` is a reliable, cheap per-call signal to trigger the
//!    CR-D1-precedented one-time `IndexSeek -> Offset` fallback — proving
//!    the fallback (not a naive always-`After` scheme) is what must ship.
//!
//! No production code is touched. `try_plan_keyset_seek`'s `where.is_some()`
//! guard is used, never modified. See the settled design memo:
//! `docs/dev-artifacts/research/f53b-step3-cursor-pagination-after-spike.md`.

use std::sync::Arc;

use shamir_query_types::read::select::Select;
use shamir_query_types::read::{At, OrderBy, Pagination, ReadQuery, Temporal};
use shamir_storage::storage_in_memory::InMemoryStore;
use shamir_storage::types::Store;
use shamir_tx::{MvccStore, RepoTxGate, Retention};
use shamir_types::core::interner::Interner;
use shamir_types::types::common::new_map;
use shamir_types::types::record_id::RecordId;
use shamir_types::types::value::{InnerValue, QueryValue};

use crate::query::filter::eval_context::FilterContext;
use crate::query::filter::resolve::intern_field_path;
use crate::query::read::QueryRecord;
use crate::table::TableManager;

// ─────────────────────────────────────────────────────────────────────────────
// Harness — identical shape to f53b_asof_seek_tests.rs / the Step 1 spike.
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
    mvcc.set_retention(Retention::keep_history()).unwrap();

    let tbl = base.with_mvcc_store(Arc::clone(&mvcc));
    tbl.create_sorted_index("score_idx", &["score"])
        .await
        .unwrap();
    (tbl, mvcc, gate)
}

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

fn pinned_at(gate: &RepoTxGate) -> u64 {
    gate.last_committed()
}

fn baseline_scores(result: &crate::query::read::QueryResult) -> Vec<i64> {
    result
        .records
        .iter()
        .filter_map(|r| r.get_value_i64("score"))
        .collect()
}

fn row_id(record: &QueryRecord) -> Option<RecordId> {
    match record {
        QueryRecord::Inserted(rec) => rec.id,
        _ => None,
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// 1. ELIGIBILITY PROBE — mirrors try_plan_keyset_seek's shape guards MINUS
//    the where/pagination checks, using only the ALREADY-pub surface.
// ─────────────────────────────────────────────────────────────────────────────

/// PROTOTYPE `create_cursor`-time eligibility probe for the settled
/// `PaginationMode::IndexSeek` (see the design memo). Returns the interned
/// sorted-index name when:
///
/// - `query.r#where.is_none()` (mirrors `try_plan_keyset_seek`'s hard
///   requirement — checked here too, since a cursor whose ORIGINAL query
///   already carries a caller `WHERE` can NEVER reach the seek arm, no
///   matter what pagination shape `cursor_handlers.rs` sends; there's no
///   point pinning `IndexSeek` mode for it),
/// - `query.order_by` is `Some` with exactly one single-segment field,
/// - a sorted index covers that field.
///
/// Deliberately does NOT check `query.pagination` — at `create_cursor` time
/// there is no `Pagination::After` yet to inspect (the cursor doesn't even
/// have a bookmark yet); that's exactly why this can't just delegate to
/// `try_plan_keyset_seek` itself even setting `where` aside.
///
/// Built ENTIRELY from methods already `pub` today:
/// `TableManager::sorted_indexes()` (`table_manager.rs:715`),
/// `SortedIndexManager::find_by_field` (`sorted_index_manager.rs:240`), and
/// `crate::query::filter::resolve::intern_field_path` (`resolve.rs:50`,
/// transitively reachable from `shamir-server` via `shamir_db::engine`/
/// `shamir_db::query` re-exports — see the design memo §1 for the grep
/// proof). No new `pub` method on `TableManager` is required for this
/// specific check.
fn probe_index_seek_eligible(
    table: &TableManager,
    query: &ReadQuery,
    interner: &Interner,
) -> Option<u64> {
    if query.r#where.is_some() {
        return None;
    }
    let order_by = query.order_by.as_ref()?;
    if order_by.items.len() != 1 {
        return None;
    }
    let item = &order_by.items[0];
    if item.field.len() != 1 {
        return None;
    }
    let field_path = intern_field_path(&item.field, interner)?;
    let def = table.sorted_indexes().find_by_field(&field_path)?;
    Some(def.name_interned)
}

/// Proves the probe is buildable from public API alone and correctly
/// distinguishes the four eligibility axes: a plain indexed ORDER BY passes;
/// ANY caller `WHERE` disqualifies (even one that would itself be
/// satisfiable); an unindexed ORDER BY field disqualifies; a multi-column
/// ORDER BY disqualifies.
#[tokio::test]
async fn probe_1_eligibility_probe_uses_only_public_api() {
    let (tbl, mvcc, _gate) = make_mvcc_score_table().await;
    for s in [10, 20, 30] {
        let _ = insert_score(&tbl, &mvcc, s, &format!("r{s}")).await;
    }
    let interner = tbl.interner().get().await.unwrap();

    // Eligible: no WHERE, single indexed ORDER BY column.
    let q_eligible = ReadQuery::new("t")
        .select(Select::fields(["score"]))
        .order_by(OrderBy::asc("score"));
    assert!(
        probe_index_seek_eligible(&tbl, &q_eligible, interner).is_some(),
        "a plain ORDER BY over an indexed column must be probe-eligible"
    );

    // Ineligible: ANY WHERE clause, even on an unrelated field.
    let mut q_where = q_eligible.clone();
    q_where.r#where = Some(shamir_query_types::filter::Filter::Gte {
        field: shamir_query_types::filter::FieldPath::from(vec!["score".to_string()]),
        value: shamir_query_types::filter::FilterValue::Int(0),
    });
    assert!(
        probe_index_seek_eligible(&tbl, &q_where, interner).is_none(),
        "a caller WHERE clause must disqualify the IndexSeek bookmark scheme, \
         per try_plan_keyset_seek's own (unmodified) where.is_some() guard"
    );

    // Ineligible: ORDER BY on a column with no sorted index.
    let q_unindexed = ReadQuery::new("t")
        .select(Select::fields(["label"]))
        .order_by(OrderBy::asc("label"));
    assert!(
        probe_index_seek_eligible(&tbl, &q_unindexed, interner).is_none(),
        "ORDER BY on an unindexed column must not be probe-eligible"
    );

    // Ineligible: multi-column ORDER BY (no composite seek primitive).
    let mut q_multi = q_eligible.clone();
    q_multi.order_by = Some(shamir_query_types::read::OrderBy {
        items: vec![
            shamir_query_types::read::OrderByItem {
                field: vec!["score".to_string()],
                direction: shamir_query_types::read::OrderDirection::Asc,
                nulls: None,
            },
            shamir_query_types::read::OrderByItem {
                field: vec!["label".to_string()],
                direction: shamir_query_types::read::OrderDirection::Asc,
                nulls: None,
            },
        ],
    });
    assert!(
        probe_index_seek_eligible(&tbl, &q_multi, interner).is_none(),
        "multi-column ORDER BY must not be probe-eligible"
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// 2. The seek arm's real RecordId — the CursorState bookmark advantage.
// ─────────────────────────────────────────────────────────────────────────────

/// Confirms `read_as_of_keyset_seek`'s output rows carry a real, stable
/// `RecordId` (`QueryRecord::Inserted { id: Some(id), .. }`) — the concrete
/// fact that makes an `IndexSeek`-mode `CursorState` bookmark a plain
/// `Option<RecordId>` (feeding `Pagination::after_with_id`) rather than
/// `Keyset` mode's `tie_skip` row-counting substitute (which exists ONLY
/// because the generic AsOf full-scan projection discards `_id` — see
/// `cursor_handlers.rs`'s CR-A4 module doc).
#[tokio::test]
async fn probe_2_seek_arm_attaches_real_record_id() {
    let (tbl, mvcc, gate) = make_mvcc_score_table().await;
    let mut ids = Vec::new();
    for s in [10, 20, 30] {
        ids.push(insert_score(&tbl, &mvcc, s, &format!("r{s}")).await.0);
    }
    let pinned = pinned_at(&gate);

    let interner = tbl.interner().get().await.unwrap();
    let refs = new_map();
    let ctx = FilterContext::new(interner, &refs);

    let mut q = ReadQuery::new("t")
        .select(Select::fields(["score"]))
        .order_by(OrderBy::asc("score"))
        .pagination(Pagination::after_with_id(
            vec![QueryValue::Int(i64::MIN)],
            Some(100),
            None,
        ));
    q.temporal = Temporal::AsOf {
        at: At::Version(pinned),
    };

    let result = tbl.read(&q, &ctx).await.unwrap();
    assert_eq!(
        result
            .stats
            .as_ref()
            .and_then(|s| s.index_used.as_deref())
            .unwrap_or(""),
        format!(
            "sorted_idx_{}_asof_keyset",
            tbl.sorted_indexes().iter_indexes()[0].name_interned
        ),
        "must have taken the AsOf keyset-seek arm for this to be a meaningful proof"
    );
    assert_eq!(result.records.len(), 3);
    for record in &result.records {
        let id = row_id(record);
        assert!(
            id.is_some(),
            "every seek-arm row must carry a real RecordId; got {record:?}"
        );
        assert!(
            ids.contains(&id.unwrap()),
            "the attached RecordId must be one of the rows actually inserted"
        );
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// 3. THE NEGATIVE TEST — gate fails mid-lifetime; must NOT page-one-forever.
// ─────────────────────────────────────────────────────────────────────────────

/// PROTOTYPE of what a naive `IndexSeek`-mode `FetchNext` would do if it
/// ALWAYS sent `Pagination::After` without checking whether the fast arm
/// actually fired. Mirrors the shape `cursor_handlers.rs`'s `fetch_next`
/// would build: re-sends `Pagination::after_with_id` with the LAST bookmark
/// unconditionally.
async fn naive_after_fetch(
    tbl: &TableManager,
    ctx: &FilterContext<'_>,
    pinned: u64,
    seek_val: i64,
    after_id: Option<RecordId>,
    limit: u64,
) -> crate::query::read::QueryResult {
    let mut q = ReadQuery::new("t")
        .select(Select::fields(["score"]))
        .order_by(OrderBy::asc("score"))
        .pagination(Pagination::after_with_id(
            vec![QueryValue::Int(seek_val)],
            Some(limit),
            after_id,
        ));
    q.temporal = Temporal::AsOf {
        at: At::Version(pinned),
    };
    tbl.read(q_ref(&mut q), ctx).await.unwrap()
}

/// Trivial identity helper so `naive_after_fetch` can take `&mut` locally
/// without fighting borrow-checker noise in the call site above.
fn q_ref(q: &mut ReadQuery) -> &ReadQuery {
    q
}

/// THE central proof. Sequence:
///
/// 1. Insert 30 rows (scores 0..290), pin the snapshot, fetch page 1 via
///    `Pagination::After` — the seek arm fires (`index_used` ends in
///    `_asof_keyset`), returning `[0..90]` and a real `after_id`.
/// 2. A CONCURRENT write (an UPDATE to the indexed field, the sharpest of
///    the three gate-tripping mutations per the Step 1 spike's negative
///    tests) lands AFTER page 1 but BEFORE page 2's `FetchNext` — the exact
///    "intervening write mid-cursor-lifetime" scenario the brief names.
/// 3. Page 2's `Pagination::After` fetch is issued with the SAME bookmark
///    shape a naive always-`After` `fetch_next` would send. PROVE:
///    a. `index_used` no longer ends in `_asof_keyset` (the gate declined —
///       `last_mutation_version() > pinned`), so `read_as_of` fell through
///       to its full-scan tail.
///    b. Critically: this does NOT silently repeat page 1 forever. Unlike
///       the non-seek paths the brief warns about (where `Pagination::
///       resolve()` maps `After` to `(skip=0, take=limit)` UNCONDITIONALLY,
///       so a fallback path with NO seek-key filter in `where` would
///       re-return the FIRST `limit` rows of the WHOLE table every time),
///       `read_as_of`'s full-scan tail still consults `query.order_by` +
///       `Pagination::resolve()`'s `(skip=0, take=limit)` over the res of
///       ALL matched rows with NO boundary filter — i.e. the fallback also
///       has no seek-key filter, so it ALSO reproduces page 1
///       (`[0..90]`) rather than page 2. This is the load-bearing finding:
///       a bare mode-agnostic "just resend After" scheme is NOT safe by
///       itself even for `IndexSeek` — it silently duplicates page 1
///       instead of erroring or advancing, precisely because `query.r#where`
///       carries no boundary once the seek arm declines.
/// 4. The FIX (mirroring CR-D1's `StuckAtCeiling -> Offset` precedent):
///    `cursor_handlers.rs` must inspect `stats.index_used` after EVERY
///    `IndexSeek`-mode `FetchNext` and, the FIRST time it doesn't end in
///    `_asof_keyset`, permanently flip `CursorState.mode` to
///    `PaginationMode::Offset` for the rest of the cursor's lifetime AND
///    retry THIS call via the offset bookmark (`state.offset` — already
///    known to be `30` after page 1, from `page.records.len()` summed) so
///    the CALLER gets a correct page 2, not a duplicate page 1. This test
///    proves that recovery: the SAME offset-bookmark fetch (`LimitOffset {
///    offset: 10, limit: 10 }` at the SAME pinned version) returns exactly
///    `[100..190]` (score 100..190 — RECALL the update moved score 30->35,
///    which is invisible to a `score` ORDER BY read using ORIGINAL row
///    values elsewhere in the table; this test's data has 30 real distinct
///    scores, so the offset fallback still returns the correct NEXT 10 rows
///    by the pinned-snapshot's stable enumeration order) with NO
///    duplication of page 1's rows and NO silent repeat.
#[tokio::test]
async fn negative_gate_failure_mid_lifetime_must_not_page_one_forever() {
    let (tbl, mvcc, gate) = make_mvcc_score_table().await;
    // 30 rows: scores 0,10,...,290 -> 3 pages of 10 (page1=[0..90]).
    let mut ids = Vec::new();
    for s in (0..30).map(|i| i * 10) {
        ids.push(insert_score(&tbl, &mvcc, s, &format!("r{s}")).await.0);
    }
    let pinned = pinned_at(&gate);
    let idx_name = tbl.sorted_indexes().iter_indexes()[0].name_interned;
    assert!(
        tbl.sorted_indexes().last_mutation_version(idx_name) <= pinned,
        "pre-condition: gate must pass before the concurrent write"
    );

    let interner = tbl.interner().get().await.unwrap();
    let refs = new_map();
    let ctx = FilterContext::new(interner, &refs);

    let page_size = 10u64;

    // ── Page 1: the fast arm fires. ──────────────────────────────────────
    let p1 = naive_after_fetch(&tbl, &ctx, pinned, i64::MIN, None, page_size).await;
    let label1 = p1
        .stats
        .as_ref()
        .and_then(|s| s.index_used.as_deref())
        .unwrap_or("<none>")
        .to_string();
    assert!(
        label1.ends_with("_asof_keyset"),
        "page 1 must take the seek arm; got index_used = {label1:?}"
    );
    assert_eq!(
        baseline_scores(&p1),
        (0..10).map(|i| i * 10).collect::<Vec<_>>()
    );
    let last_score_1 = p1.records.last().unwrap().get_value_i64("score").unwrap();
    let last_id_1 = row_id(p1.records.last().unwrap());

    // ── Intervening write: UPDATE the indexed field on an UNRELATED row
    //    (score 200 -> 205), landing strictly between page 1 and page 2's
    //    FetchNext — "the gate fails due to an intervening write" from the
    //    brief. This trips the F-53b Step 2 high-water gate for the WHOLE
    //    table's sorted index (a one-way ratchet, per that gate's own doc
    //    comment), regardless of which row moved.
    let row200 = ids[20];
    let score_id = interner_score_id(&tbl).await;
    let label_id = interner_label_id(&tbl).await;
    let mut m = new_map();
    m.insert(
        shamir_types::core::interner::InternerKey::new(score_id),
        InnerValue::Int(205),
    );
    m.insert(
        shamir_types::core::interner::InternerKey::new(label_id),
        InnerValue::Str("moved".to_owned()),
    );
    tbl.set(row200, &InnerValue::Map(m)).await.unwrap();
    assert!(
        tbl.sorted_indexes().last_mutation_version(idx_name) > pinned,
        "post-update: the gate must have advanced past the pin"
    );

    // ── Page 2, naive always-After fetch: the SAME kind of call a
    //    fetch_next that unconditionally sends Pagination::After would make.
    let p2_naive = naive_after_fetch(&tbl, &ctx, pinned, last_score_1, last_id_1, page_size).await;
    let label2 = p2_naive
        .stats
        .as_ref()
        .and_then(|s| s.index_used.as_deref())
        .unwrap_or("<none>")
        .to_string();
    // (a) The gate decline is OBSERVABLE via index_used.
    assert!(
        !label2.ends_with("_asof_keyset"),
        "page 2 must have declined the seek arm after the concurrent write; \
         got index_used = {label2:?}"
    );
    // (b) THE HAZARD: a bare always-After scheme silently reproduces page 1,
    //     not page 2 — no error, no advance. This is exactly the
    //     "returns PAGE ONE FOREVER" shape the brief's Pagination::resolve()
    //     analysis warns about, now demonstrated concretely for the
    //     IndexSeek-mode gate-failure case specifically (not merely the
    //     no-order-by/no-index case the brief already knew about).
    assert_eq!(
        baseline_scores(&p2_naive),
        (0..10).map(|i| i * 10).collect::<Vec<_>>(),
        "HAZARD CONFIRMED: without a mode fallback, a gate-declined \
         Pagination::After re-fetch silently duplicates page 1 — no error, \
         no progress, no data loss signal. This is why cursor_handlers.rs \
         MUST detect this per-call and flip to the CR-D1-precedented \
         permanent Offset fallback rather than ever relying on `After` alone."
    );

    // ── THE FIX, prototyped: detect (b) via index_used, then recover via
    //    the SAME offset bookmark CR-D1 already uses for StuckAtCeiling.
    //    `state.offset` after page 1 is exactly `page.records.len()` (10) —
    //    already tracked in parallel the same way CR-D1 requires.
    let offset_after_page1 = p1.records.len() as u64;
    let mut fallback_query = ReadQuery::new("t")
        .select(Select::fields(["score"]))
        .order_by(OrderBy::asc("score"));
    fallback_query.pagination = Pagination::LimitOffset {
        limit: Some(page_size + 1), // +1 peek, mirrors fetch_offset_page.
        offset: offset_after_page1,
    };
    fallback_query.temporal = Temporal::AsOf {
        at: At::Version(pinned),
    };
    let mut p2_fixed = tbl.read(&fallback_query, &ctx).await.unwrap();
    let has_more = p2_fixed.records.len() as u64 > page_size;
    if has_more {
        p2_fixed.records.truncate(page_size as usize);
    }

    // Correct recovery: page 2 is [100..190], NOT a repeat of [0..90], and
    // NOT missing/duplicating any row relative to the pinned snapshot.
    assert_eq!(
        baseline_scores(&p2_fixed),
        (10..20).map(|i| i * 10).collect::<Vec<_>>(),
        "the CR-D1-style offset fallback must recover the TRUE next page, \
         not repeat page 1 and not skip/duplicate rows"
    );
    // The moved row's NEW value (205) must not leak into a page it doesn't
    // belong to under the PINNED snapshot's ordering (it's still logically
    // "200" as of `pinned`, appearing in the fallback's [200..290] page --
    // proven not to appear early/duplicated here).
    assert!(
        !baseline_scores(&p2_fixed).contains(&205),
        "the pinned-snapshot fallback must not see the post-pin update's new value"
    );
}

async fn interner_score_id(tbl: &TableManager) -> u64 {
    tbl.interner()
        .get()
        .await
        .unwrap()
        .touch_ind("score")
        .unwrap()
        .key()
        .id()
}

async fn interner_label_id(tbl: &TableManager) -> u64 {
    tbl.interner()
        .get()
        .await
        .unwrap()
        .touch_ind("label")
        .unwrap()
        .key()
        .id()
}
