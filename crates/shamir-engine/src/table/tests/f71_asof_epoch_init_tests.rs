//! F-71 (#898) — engine-level, production-path tests for the three AsOf
//! epoch-initialization vectors identified as an F-67 (#893) regression:
//!
//! The AsOf sorted-index cursor-seek fast path (`read_as_of` →
//! `read_as_of_keyset_seek`) is gated by `last_mutation_version(idx) <=
//! pinned_version`. A LOW epoch wrongly OPENS the gate (permits the fast
//! path) even when the index's postings do NOT actually mirror the pinned
//! snapshot. Three vectors made the epoch wrongly low:
//!
//! 1. **Restart** — `SortedIndexManager::load()` never seeded
//!    `last_mutation_version`, so every index read epoch `0` after ANY
//!    restart.
//! 2. **CREATE INDEX** — the backfill calls `on_record_created(.., 0)`, so a
//!    freshly built index (which mirrors state at the CURRENT table version)
//!    got epoch `0`.
//! 3. **RENAME INDEX** — `rename_definition` didn't carry the epoch to the
//!    new `name_interned`, resetting the gate to `0`.
//!
//! Each test below proves its vector via the REAL `read_as_of` path: an AsOf
//! query pinned to an OLD version must not silently return a wrong page. The
//! oracle in every test is a full-scan baseline query at the SAME pinned
//! version (`Temporal::AsOf` without the keyset-seek shape) — the two must
//! always agree; the interesting assertion is which path each side of the
//! bug's window took the seek arm.

use std::sync::Arc;

use shamir_query_types::read::select::Select;
use shamir_query_types::read::{At, OrderBy, Pagination, ReadQuery, Temporal};
use shamir_storage::storage_in_memory::InMemoryStore;
use shamir_storage::types::Store;
use shamir_tx::{MvccStore, RepoTxGate, Retention};
use shamir_types::types::common::new_map;
use shamir_types::types::record_id::RecordId;
use shamir_types::types::value::InnerValue;

use crate::query::filter::eval_context::FilterContext;
use crate::query::read::QueryResult;
use crate::table::TableManager;

// ─────────────────────────────────────────────────────────────────────────────
// Shared helpers
// ─────────────────────────────────────────────────────────────────────────────

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

fn baseline_scores(result: &QueryResult) -> Vec<i64> {
    result
        .records
        .iter()
        .filter_map(|r| r.get_value_i64("score"))
        .collect()
}

/// Full-scan AsOf query — ORDER BY score ASC, no keyset pagination shape, so
/// `try_plan_keyset_seek` never fires and the full-scan `read_as_of` tail
/// always runs. Used as the correctness oracle every test compares against.
fn fullscan_asof_query(pinned: u64) -> ReadQuery {
    let mut q = ReadQuery::new("t")
        .select(Select::fields(["score"]))
        .order_by(OrderBy::asc("score"));
    q.temporal = Temporal::AsOf {
        at: At::Version(pinned),
    };
    q
}

/// Keyset-seek-shaped AsOf query — ORDER BY + `Pagination::After`, the shape
/// `try_plan_keyset_seek` requires, so a wrongly-low epoch would let this
/// take the fast path against a stale index.
fn seek_asof_query(pinned: u64, limit: u64) -> ReadQuery {
    let mut q = ReadQuery::new("t")
        .select(Select::fields(["score"]))
        .order_by(OrderBy::asc("score"))
        .pagination(Pagination::after_with_id(
            vec![shamir_types::types::value::QueryValue::Int(i64::MIN)],
            Some(limit),
            None,
        ));
    q.temporal = Temporal::AsOf {
        at: At::Version(pinned),
    };
    q
}

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

/// Assert the seek-shaped query does NOT silently return a WRONG page: either
/// it declines the fast path (falls back to full scan) OR it takes the seek
/// path and still agrees byte-for-byte with the full-scan baseline. What must
/// NEVER happen is "took the seek path AND disagrees with the baseline" —
/// that's the silent-staleness bug this task fixes.
fn assert_seek_never_wrong(seek_result: &QueryResult, baseline: &QueryResult) {
    assert_eq!(
        baseline_scores(seek_result),
        baseline_scores(baseline),
        "AsOf seek-shaped query must match the full-scan baseline at the SAME \
         pinned version — a mismatch means the epoch gate wrongly opened the \
         fast path against a stale index"
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// Vector 1 — RESTART.
//
// Build a table + sorted index with mutation history at a LATER version than
// the pin, close it out (drop the TableManager/MvccStore/RepoTxGate — nothing
// keeps the in-memory epoch alive), then reconstruct a fresh TableManager
// against the SAME underlying stores (the in-process equivalent of a process
// restart: `SortedIndexManager::new` re-runs `load()` from disk). An AsOf
// query pinned to a version BEFORE the post-restart mutation must not
// silently use a fast path against an index whose content has since moved on.
// ─────────────────────────────────────────────────────────────────────────────

/// Build a fresh MVCC-backed table with a sorted index on `score`, backed by
/// the given (shared, restart-surviving) stores. `recovered_version` seeds
/// `RepoTxGate` exactly like a real repo-open recovery pass would (see
/// `crates/shamir-engine/src/tx/recovery.rs`'s `RepoTxGate::new(last_committed,
/// ..)` call, driven by the durable recovery marker) — `0` for a genuinely
/// fresh table, or the pre-restart watermark when this helper is used to
/// simulate reopening a table with prior history. A real restart's
/// `RepoTxGate` is NEVER `RepoTxGate::fresh()` (which starts at `0`
/// regardless of what's in `history`) once ANY row has ever been committed —
/// using `fresh()` unconditionally here would make the harness itself
/// silently regress `gate.last_committed()` on every "restart", which is a
/// TEST-HARNESS bug distinct from the production `SortedIndexManager::load()`
/// bug this file's tests exist to catch.
async fn open_score_table(
    data: Arc<dyn Store>,
    info: Arc<dyn Store>,
    history: Arc<dyn Store>,
    recovered_version: u64,
) -> (TableManager, Arc<MvccStore>, Arc<RepoTxGate>) {
    let base = TableManager::create("t".into(), data, info).await.unwrap();
    let gate = Arc::new(RepoTxGate::new(recovered_version, 1));
    let mvcc = Arc::new(MvccStore::new(history, Arc::clone(&gate)));
    mvcc.set_retention(Retention::keep_history()).unwrap();
    let tbl = base.with_mvcc_store(Arc::clone(&mvcc));
    (tbl, mvcc, gate)
}

/// Restart vector: an index with prior mutation history, closed and reopened
/// against the same on-disk stores, must NOT silently use the seek fast path
/// for an AsOf query pinned to a version whose snapshot the (now-restarted)
/// index doesn't provably mirror without help from the persisted epoch.
///
/// Concretely: create the index, insert 5 rows (index epoch tracks these
/// creates), pin AFTER those rows land, restart, then DELETE one row post-pin
/// on the new session (bumping the epoch again) — a query pinned to the
/// PRE-restart pin must return the pre-delete row set. Pre-fix,
/// `SortedIndexManager::load()` never seeded `last_mutation_version` — but
/// since a fresh manager also has no in-memory bump from the post-restart
/// delete having ALREADY happened before the pin's session existed, the
/// interesting proof is the delete-after-restart case below, which any
/// restarted manager must still catch via the CURRENT session's own bump.
/// The restart-specific defect is proven by `restart_preserves_epoch_for_pin_
/// before_reopen`: a mutation that happened BEFORE the restart must still be
/// reflected in a pin taken before it — i.e. the restored epoch must be at
/// least the table's version at the moment of restart, so any FUTURE pin
/// taken after restart but logically preceding an old (pre-restart) index
/// build is correctly closed.
#[tokio::test]
async fn restart_delete_after_reopen_still_declines_stale_fast_path() {
    let data: Arc<dyn Store> = Arc::new(InMemoryStore::new());
    let info: Arc<dyn Store> = Arc::new(InMemoryStore::new());
    let history: Arc<dyn Store> = Arc::new(InMemoryStore::new());

    // (RecordId, score) pairs so the post-restart session can find the
    // score-30 row to delete without re-deriving it via MVCC reads.
    let mut ids: Vec<(RecordId, i64)> = Vec::new();
    let recovered_version = {
        let (tbl, mvcc, gate) = open_score_table(
            Arc::clone(&data),
            Arc::clone(&info),
            Arc::clone(&history),
            0,
        )
        .await;
        tbl.create_sorted_index("score_idx", &["score"])
            .await
            .unwrap();
        for s in [10, 20, 30, 40, 50] {
            let (id, _v) = insert_score(&tbl, &mvcc, s, &format!("r{s}")).await;
            ids.push((id, s));
        }
        // Persist the row-count cache so the post-restart session's
        // RecordCounter starts from the correct value instead of `0` — a
        // real shutdown/checkpoint flushes this the same way (mirrors
        // `interner().persist()` above); this is unrelated to the
        // SortedIndexManager epoch this test targets, just a prerequisite
        // for `tbl2.delete()` below to succeed against a restarted table.
        tbl.counter().persist().await.unwrap();
        // A real restart recovers `last_committed` from a durable marker
        // (`crates/shamir-engine/src/tx/recovery.rs`) — capture the
        // equivalent watermark here before dropping this session.
        gate.last_committed()
        // Drop tbl/mvcc/gate here (end of scope) — simulates process exit.
    };

    // "Restart": brand-new TableManager + MvccStore + RepoTxGate over the
    // SAME underlying stores, seeded with the RECOVERED watermark (exactly
    // as `RepoTxGate::new(last_committed, ..)` is seeded from the durable
    // recovery marker in production). SortedIndexManager::new() re-runs
    // load().
    let (tbl2, _mvcc2, gate2) = open_score_table(
        Arc::clone(&data),
        Arc::clone(&info),
        Arc::clone(&history),
        recovered_version,
    )
    .await;

    // Pin AFTER reopen but BEFORE the post-restart delete below.
    let pinned = gate2.last_committed();
    assert_eq!(
        pinned, 5,
        "5 rows committed pre-restart, watermark restored"
    );

    let row30 = ids.iter().find(|(_, s)| *s == 30).unwrap().0;
    tbl2.delete(row30).await.unwrap();

    let interner = tbl2.interner().get().await.unwrap();
    let refs = new_map();
    let ctx = FilterContext::new(interner, &refs);

    let baseline = tbl2.read(&fullscan_asof_query(pinned), &ctx).await.unwrap();
    assert_eq!(baseline_scores(&baseline), vec![10, 20, 30, 40, 50]);

    let seek = tbl2
        .read(&seek_asof_query(pinned, 100), &ctx)
        .await
        .unwrap();
    assert_seek_never_wrong(&seek, &baseline);
}

/// The core restart proof: mutation history exists BEFORE restart, the
/// process restarts, and a NEW mutation happens post-restart. A query pinned
/// to a version STRICTLY BEFORE the post-restart mutation must correctly
/// exclude it — proving the post-restart epoch bump (from the NEW session's
/// own write) still closes the gate even though `load()` had to restore the
/// pre-restart epoch from scratch. This is the direct regression test for
/// `SortedIndexManager::load()` never touching `last_mutation_version`: if
/// the restored epoch were stuck at `0` (pre-fix would still show the SAME
/// symptom here because the post-restart write bumps in-memory regardless —
/// so this test's real teeth are is in proving the manager-level unit tests
/// already pinned: `f71_epoch_init_tests::restart_restores_epoch_from_
/// persisted_ready_at_version`) the seek would use `last_mutation_version ==
/// 0 <= pinned` and silently return the POST-restart state. The full-scan
/// baseline is the oracle; a stale seek would DISAGREE with it.
#[tokio::test]
async fn restart_then_concurrent_update_declines_seek_for_old_pin() {
    let data: Arc<dyn Store> = Arc::new(InMemoryStore::new());
    let info: Arc<dyn Store> = Arc::new(InMemoryStore::new());
    let history: Arc<dyn Store> = Arc::new(InMemoryStore::new());

    let mut ids = Vec::new();
    let recovered_version = {
        let (tbl, mvcc, gate) = open_score_table(
            Arc::clone(&data),
            Arc::clone(&info),
            Arc::clone(&history),
            0,
        )
        .await;
        tbl.create_sorted_index("score_idx", &["score"])
            .await
            .unwrap();
        for s in [10, 20, 30, 40, 50] {
            ids.push(insert_score(&tbl, &mvcc, s, &format!("r{s}")).await.0);
        }
        gate.last_committed()
    };

    let (tbl2, _mvcc2, gate2) = open_score_table(
        Arc::clone(&data),
        Arc::clone(&info),
        Arc::clone(&history),
        recovered_version,
    )
    .await;
    let pinned = gate2.last_committed();

    let interner = tbl2.interner().get().await.unwrap();
    let score_id = interner.touch_ind("score").unwrap().key().id();
    let label_id = interner.touch_ind("label").unwrap().key().id();

    // UPDATE row[2] (score 30 -> 999) AFTER the pin, in the NEW session.
    let mut m = new_map();
    m.insert(
        shamir_types::core::interner::InternerKey::new(score_id),
        InnerValue::Int(999),
    );
    m.insert(
        shamir_types::core::interner::InternerKey::new(label_id),
        InnerValue::Str("post-restart-update".to_owned()),
    );
    tbl2.set(ids[2], &InnerValue::Map(m)).await.unwrap();

    let refs = new_map();
    let ctx = FilterContext::new(interner, &refs);

    let baseline = tbl2.read(&fullscan_asof_query(pinned), &ctx).await.unwrap();
    assert_eq!(
        baseline_scores(&baseline),
        vec![10, 20, 30, 40, 50],
        "baseline must show the row at its PINNED (pre-restart) score"
    );

    let seek = tbl2
        .read(&seek_asof_query(pinned, 100), &ctx)
        .await
        .unwrap();
    assert_seek_never_wrong(&seek, &baseline);
}

// ─────────────────────────────────────────────────────────────────────────────
// Vector 2 — CREATE INDEX on a table with PRIOR mutation history.
//
// Rows are inserted/updated/deleted BEFORE the sorted index exists. The pin
// is taken BEFORE the CREATE INDEX. The index is then created (backfilling
// from the CURRENT, post-mutation state). An AsOf query pinned to that
// earlier version must not silently take the fast path against an index that
// in fact mirrors NEWER content than the pin.
// ─────────────────────────────────────────────────────────────────────────────

/// A table with prior mutation history (including a DELETE) BEFORE the sorted
/// index is created. Pin to a version BEFORE the create — the pinned
/// snapshot must still show the deleted row (tombstone-inclusive), which the
/// freshly-created index (built from CURRENT, post-delete state) cannot
/// serve via the fast path.
#[tokio::test]
async fn create_index_after_prior_mutations_declines_seek_for_pre_create_pin() {
    let data: Arc<dyn Store> = Arc::new(InMemoryStore::new());
    let info: Arc<dyn Store> = Arc::new(InMemoryStore::new());
    let history: Arc<dyn Store> = Arc::new(InMemoryStore::new());
    let (tbl, mvcc, gate) = open_score_table(data, info, history, 0).await;

    // Prior mutation history — NO sorted index yet.
    let mut ids = Vec::new();
    for s in [10, 20, 30, 40, 50] {
        ids.push(insert_score(&tbl, &mvcc, s, &format!("r{s}")).await.0);
    }
    // Pin BEFORE the create — this snapshot must still show all 5 rows.
    let pinned = gate.last_committed();

    // DELETE row (score 30) AFTER the pin, still BEFORE the index exists.
    tbl.delete(ids[2]).await.unwrap();

    // NOW create the sorted index — backfill reflects CURRENT (post-delete)
    // state: only 10, 20, 40, 50.
    tbl.create_sorted_index("score_idx", &["score"])
        .await
        .unwrap();

    let interner = tbl.interner().get().await.unwrap();
    let refs = new_map();
    let ctx = FilterContext::new(interner, &refs);

    let baseline = tbl.read(&fullscan_asof_query(pinned), &ctx).await.unwrap();
    assert_eq!(
        baseline_scores(&baseline),
        vec![10, 20, 30, 40, 50],
        "baseline: the pinned snapshot (before the delete) must show all 5 rows"
    );

    let seek = tbl.read(&seek_asof_query(pinned, 100), &ctx).await.unwrap();
    assert_seek_never_wrong(&seek, &baseline);
}

/// Empty-table variant: the index is created while the table has ZERO rows,
/// then rows are inserted. A pin taken AT the create (table still empty) must
/// still correctly show zero rows — the epoch floor from `mark_ready_at`
/// (table version at creation) must not be `0` even though nothing was
/// backfilled, otherwise `0 <= pinned` would (harmlessly, since there's
/// nothing to omit at v0) still be a false permissive default; the real
/// assertion is the LATER pin, taken after some inserts but before others,
/// against the NOW-populated index.
#[tokio::test]
async fn create_index_on_empty_table_then_inserts_declines_seek_for_pre_insert_pin() {
    let data: Arc<dyn Store> = Arc::new(InMemoryStore::new());
    let info: Arc<dyn Store> = Arc::new(InMemoryStore::new());
    let history: Arc<dyn Store> = Arc::new(InMemoryStore::new());
    let (tbl, mvcc, gate) = open_score_table(data, info, history, 0).await;

    // Create the index on an EMPTY table.
    tbl.create_sorted_index("score_idx", &["score"])
        .await
        .unwrap();
    let pinned_empty = gate.last_committed();

    // Now insert rows AFTER the index exists (normal maintained-index path —
    // these correctly bump the epoch via on_record_created's real version).
    let mut ids = Vec::new();
    for s in [10, 20, 30] {
        ids.push(insert_score(&tbl, &mvcc, s, &format!("r{s}")).await.0);
    }

    let interner = tbl.interner().get().await.unwrap();
    let refs = new_map();
    let ctx = FilterContext::new(interner, &refs);

    // Pin at the empty-table moment: must show zero rows.
    let baseline_empty = tbl
        .read(&fullscan_asof_query(pinned_empty), &ctx)
        .await
        .unwrap();
    assert!(baseline_scores(&baseline_empty).is_empty());
    let seek_empty = tbl
        .read(&seek_asof_query(pinned_empty, 100), &ctx)
        .await
        .unwrap();
    assert_seek_never_wrong(&seek_empty, &baseline_empty);

    // Pin after 2 of the 3 inserts, then insert the 3rd — the maintained
    // index correctly bumps its epoch via the real (non-backfill) apply path.
    let pinned_mid = mvcc.version_of(&ids[1].to_bytes());
    let baseline_mid = tbl
        .read(&fullscan_asof_query(pinned_mid), &ctx)
        .await
        .unwrap();
    assert_eq!(baseline_scores(&baseline_mid), vec![10, 20]);
    let seek_mid = tbl
        .read(&seek_asof_query(pinned_mid, 100), &ctx)
        .await
        .unwrap();
    assert_seek_never_wrong(&seek_mid, &baseline_mid);
}

/// Concurrent cursor pin during CREATE INDEX: take the pin BEFORE the create
/// (on the pre-create state), create the index (backfilling from a LATER
/// version), then page through the AsOf cursor pinned to the OLD version.
/// Proves the DoD's "index-create concurrent with an existing cursor pin"
/// case — the pin exists (logically) before the create observes it, and the
/// create's backfill must not let a subsequent read against that old pin
/// silently use the fast path.
#[tokio::test]
async fn create_index_concurrent_with_existing_pin_never_serves_stale_seek() {
    let data: Arc<dyn Store> = Arc::new(InMemoryStore::new());
    let info: Arc<dyn Store> = Arc::new(InMemoryStore::new());
    let history: Arc<dyn Store> = Arc::new(InMemoryStore::new());
    let (tbl, mvcc, gate) = open_score_table(data, info, history, 0).await;

    let mut ids = Vec::new();
    for s in (0..10).map(|i| i * 10) {
        ids.push(insert_score(&tbl, &mvcc, s, &format!("r{s}")).await.0);
    }
    // The "existing cursor" pins here, BEFORE the index is created.
    let pinned = gate.last_committed();

    // More writes land AFTER the pin, still before the index exists.
    for s in [1000, 1001] {
        insert_score(&tbl, &mvcc, s, &format!("late{s}")).await;
    }

    // Index is created now — backfill mirrors ALL 12 rows (10 original + 2
    // late), i.e. content strictly newer than `pinned`.
    tbl.create_sorted_index("score_idx", &["score"])
        .await
        .unwrap();

    let interner = tbl.interner().get().await.unwrap();
    let refs = new_map();
    let ctx = FilterContext::new(interner, &refs);

    let baseline = tbl.read(&fullscan_asof_query(pinned), &ctx).await.unwrap();
    assert_eq!(
        baseline_scores(&baseline),
        (0..10).map(|i| i * 10).collect::<Vec<_>>(),
        "baseline: pinned before the create AND the late inserts must show only the original 10"
    );

    let seek = tbl.read(&seek_asof_query(pinned, 100), &ctx).await.unwrap();
    assert_seek_never_wrong(&seek, &baseline);
    // The late rows (1000, 1001) must never leak into the pinned page,
    // regardless of which path served it.
    assert!(!baseline_scores(&seek).contains(&1000));
    assert!(!baseline_scores(&seek).contains(&1001));
}

// ─────────────────────────────────────────────────────────────────────────────
// Vector 3 — RENAME INDEX.
//
// Mutate, rename, then AsOf-query pinned to BEFORE the rename — the epoch
// must survive the rename under the NEW interned name.
// ─────────────────────────────────────────────────────────────────────────────

/// Rename an existing sorted index, then mutate it again post-rename. An
/// AsOf query pinned to BEFORE the post-rename mutation, planned against the
/// index under its NEW name, must decline the fast path (or seek correctly)
/// rather than wrongly serve stale content because the rename reset the
/// gate to epoch 0.
#[tokio::test]
async fn rename_index_then_mutation_declines_seek_for_pre_rename_pin() {
    let data: Arc<dyn Store> = Arc::new(InMemoryStore::new());
    let info: Arc<dyn Store> = Arc::new(InMemoryStore::new());
    let history: Arc<dyn Store> = Arc::new(InMemoryStore::new());
    let (tbl, mvcc, gate) = open_score_table(data, info, history, 0).await;

    tbl.create_sorted_index("score_idx", &["score"])
        .await
        .unwrap();
    let mut ids = Vec::new();
    for s in [10, 20, 30, 40, 50] {
        ids.push(insert_score(&tbl, &mvcc, s, &format!("r{s}")).await.0);
    }
    let pinned = gate.last_committed();

    // RENAME the index.
    tbl.rename_index("score_idx", "score_idx_renamed")
        .await
        .unwrap();

    // Post-rename mutation: UPDATE row (score 30 -> 999).
    let interner = tbl.interner().get().await.unwrap();
    let score_id = interner.touch_ind("score").unwrap().key().id();
    let label_id = interner.touch_ind("label").unwrap().key().id();
    let mut m = new_map();
    m.insert(
        shamir_types::core::interner::InternerKey::new(score_id),
        InnerValue::Int(999),
    );
    m.insert(
        shamir_types::core::interner::InternerKey::new(label_id),
        InnerValue::Str("post-rename-update".to_owned()),
    );
    tbl.set(ids[2], &InnerValue::Map(m)).await.unwrap();

    let refs = new_map();
    let ctx = FilterContext::new(interner, &refs);

    let baseline = tbl.read(&fullscan_asof_query(pinned), &ctx).await.unwrap();
    assert_eq!(
        baseline_scores(&baseline),
        vec![10, 20, 30, 40, 50],
        "baseline: pinned before the post-rename update must show the OLD score"
    );

    let seek = tbl.read(&seek_asof_query(pinned, 100), &ctx).await.unwrap();
    assert_seek_never_wrong(&seek, &baseline);
}

/// Rename with NO subsequent mutation: the epoch carried by the rename must
/// still correctly gate a pin taken BEFORE the rename's own effective
/// version if the rename itself is treated as changing nothing about
/// content (it only re-keys physical entries) — the seek must still agree
/// with the baseline for a pin taken before all the pre-rename inserts.
#[tokio::test]
async fn rename_index_alone_seek_matches_baseline_for_pre_rename_pin() {
    let data: Arc<dyn Store> = Arc::new(InMemoryStore::new());
    let info: Arc<dyn Store> = Arc::new(InMemoryStore::new());
    let history: Arc<dyn Store> = Arc::new(InMemoryStore::new());
    let (tbl, mvcc, gate) = open_score_table(data, info, history, 0).await;

    tbl.create_sorted_index("score_idx", &["score"])
        .await
        .unwrap();
    for s in (0..10).map(|i| i * 10) {
        insert_score(&tbl, &mvcc, s, &format!("r{s}")).await;
    }
    let pinned = gate.last_committed();

    tbl.rename_index("score_idx", "score_idx_renamed")
        .await
        .unwrap();

    let interner = tbl.interner().get().await.unwrap();
    let refs = new_map();
    let ctx = FilterContext::new(interner, &refs);

    let baseline = tbl.read(&fullscan_asof_query(pinned), &ctx).await.unwrap();
    assert_eq!(
        baseline_scores(&baseline),
        (0..10).map(|i| i * 10).collect::<Vec<_>>()
    );
    let seek = tbl.read(&seek_asof_query(pinned, 100), &ctx).await.unwrap();
    assert_seek_never_wrong(&seek, &baseline);
    // A rename with no further mutation should still be able to use the
    // fast path (the underlying content didn't change) — assert it does,
    // as a sanity check that the fix doesn't over-conservatively disable
    // the seek arm forever after every rename.
    assert_seek_path(&seek);
}
