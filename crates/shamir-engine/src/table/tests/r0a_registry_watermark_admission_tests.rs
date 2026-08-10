//! R0-A (#1006 + #1012) — registry generation watermark + full DDL admission
//! coverage.
//!
//! See `docs/dev-artifacts/prompts/ddl-lifecycle/03-registry-watermark-and-full-admission.md`
//! for the full design this file implements tests for.
//!
//! # What this file proves
//!
//! 1. `create_a_drop_a_stage_create_b_commit_indexes_b` — the end-to-end
//!    (real tx commit path) sibling of `shamir-index`'s registry-level
//!    `create_a_drop_a_stage_create_b_commit_does_not_skip_rederivation`:
//!    CREATE A -> DROP A -> stage an insert -> CREATE B (parked mid-backfill,
//!    proving the commit genuinely lands DURING B's window, not after) ->
//!    commit -> B's posting must be present. Before R0-A this scenario could
//!    lose B's posting because the two-counter `insert_ticket`/`generation`
//!    split let `generation()` plateau across the DROP-then-CREATE sequence.
//!
//! 2. Admission-coverage tests for each newly-covered gap: `drop_sorted_index`,
//!    `drop_index2`, the sorted branch of `rename_index`, and the index2
//!    branch of `rename_index` now all acquire `begin_write_barrier` for
//!    their ENTIRE critical section — proven by holding the SAME
//!    `unique_write_lock` externally and confirming each call blocks until
//!    released (mirrors `create_index_v2_acquires_write_barrier`'s pattern in
//!    `index2_create_barrier_tests.rs`).
//!
//! 3. `two_ddls_on_index2_family_are_never_concurrently_inside_barrier` — the
//!    structural proof that out-of-order publish (Part 1's second scenario)
//!    cannot happen post-fix: a paused `create_index_v2` and a concurrent
//!    `drop_index2` on a DIFFERENT name (same family, same bit) can never
//!    both be inside their critical section at once — the second blocks on
//!    `ddl_admission` until the first's guard drops.

use std::sync::Arc;
use std::time::Duration;

use shamir_query_builder::write;
use shamir_query_types::admin::types::CreateIndexOp;
use shamir_tx::IsolationLevel;
use shamir_types::core::interner::{InternerKey, TouchInd};
use shamir_types::mpack;
use shamir_types::types::common::new_map_wc;
use shamir_types::types::value::InnerValue;

use crate::index2::backend::{IndexQuery, IndexResult};
use crate::index2::functional_backend::FunctionalBackend;
use crate::repo::repo_instance::RepoInstance;
use crate::repo::repo_types::BoxRepo;
use crate::table::index2_backfill_hook::BackfillPauseHook;
use crate::table::TableConfig;
use crate::table::TableManager;
use shamir_storage::storage_in_memory::InMemoryRepo;

fn make_repo() -> RepoInstance {
    let repo = Arc::new(InMemoryRepo::new());
    RepoInstance::new("test".into(), BoxRepo::InMemory(repo), Vec::new())
}

async fn key_id(tbl: &TableManager, name: &str) -> u64 {
    let interner = tbl.interner().get().await.unwrap();
    match interner.touch_ind(name).unwrap() {
        TouchInd::Exists(k) | TouchInd::New(k) => k.id(),
    }
}

fn record_with_str(key: u64, val: &str) -> InnerValue {
    let mut m = new_map_wc(1);
    m.insert(InternerKey::new(key), InnerValue::Str(val.into()));
    InnerValue::Map(m)
}

fn functional_lower_op(name: &str, table: &str, field: &str) -> CreateIndexOp {
    CreateIndexOp {
        create_index: name.into(),
        table: table.into(),
        fields: vec![vec![field.into()]],
        unique: false,
        sorted: false,
        repo: "main".into(),
        index_type: Some("functional".into()),
        fts_tokenizer: None,
        fts_language: None,
        functional_op: Some("lower".into()),
        functional_args: None,
        vector_dim: None,
        vector_metric: None,
        vector_quantization: None,
        include: Vec::new(),
        if_not_exists: false,
    }
}

async fn functional_lookup(tbl: &TableManager, index_name_id: u64, lowered: &str) -> Vec<[u8; 16]> {
    let backend = tbl
        .index2_registry()
        .get_by_name(index_name_id)
        .await
        .expect("functional backend must be registered");
    let key = FunctionalBackend::hash_value(&InnerValue::Str(lowered.into()));
    let mut keys: smallvec::SmallVec<[Vec<u8>; 4]> = smallvec::SmallVec::new();
    keys.push(key.to_vec());
    match backend.lookup(IndexQuery::Point { keys }).await.unwrap() {
        IndexResult::Set(s) => s.iter().map(|rid| *rid.as_bytes()).collect(),
        IndexResult::Ranked(v) => v.iter().map(|(rid, _)| *rid.as_bytes()).collect(),
    }
}

// ============================================================================
// 1. End-to-end CREATE A -> DROP A -> stage -> CREATE B -> commit
// ============================================================================

/// THE R0-A end-to-end proof (real tx commit path sibling of
/// `shamir-index`'s registry-level test). CREATE index A, DROP it, then
/// stage+commit an insert while CREATE index B (a DIFFERENT functional
/// index) is parked mid-backfill — the commit must land DURING B's
/// backfill→register window (proven via the pause hook rendezvous), and
/// once B registers, the tx's commit-time re-derivation (`pre_commit.rs`)
/// must append B's posting so the row is indexed.
///
/// Pre-R0-A, this could fail via the Part-1 sequential scenario: A's
/// CREATE published generation 1, DROP A bumped it to 2 (ticket untouched),
/// the tx's stage-time snapshot captured `stage_gen = 2`, and B's CREATE
/// (ticket 2, `fetch_max(2)` a no-op since generation was already 2) left
/// `generation()` at 2 == `stage_gen` — the commit-time gate wrongly
/// skipped re-derivation and the row was never indexed under B.
#[tokio::test]
async fn create_a_drop_a_stage_create_b_commit_indexes_b() {
    let repo = make_repo();
    repo.add_table(TableConfig::new("people"));
    let tbl = repo.get_table("people").await.unwrap();

    // CREATE A, then DROP A — establishes the pre-existing generation churn
    // (Part 1's "CREATE A -> DROP A" prefix) before B ever exists.
    tbl.create_index_v2(&functional_lower_op("lower_a", "people", "name"))
        .await
        .unwrap();
    assert!(
        tbl.drop_index2("lower_a", None).await.unwrap(),
        "DROP A must succeed"
    );

    // Install the pause hook and spawn CREATE B — it parks at the
    // backfill->register window (backfill done, backend NOT yet
    // registered).
    let hook = Arc::new(BackfillPauseHook::new());
    tbl.set_create_index2_backfill_hook(Some(Arc::clone(&hook)));
    let tbl_create = tbl.clone();
    let create_b = tokio::spawn(async move {
        tbl_create
            .create_index_v2(&functional_lower_op("lower_b", "people", "name"))
            .await
    });
    hook.wait_until_parked().await;

    // Stage (via the real tx-path DML entry point) a new row BEFORE B is
    // registered — its stage-time `all_backends()`/generation snapshot
    // cannot possibly include B (B does not exist in ANY form yet, not even
    // mid-backfill, from the registry's point of view: it publishes only at
    // the END of create_index_v2, which is still parked).
    let (mut tx, _guard) = repo.begin_tx(IsolationLevel::Snapshot).await.unwrap();
    let op = write::insert("people")
        .rows([mpack!({ "name": "Carol" })])
        .build();
    let result = tbl
        .execute_insert_tx(
            &op,
            &mut tx,
            true,
            None,
            &shamir_types::access::Actor::System,
        )
        .await
        .unwrap();
    let carol = result.records[0].id.expect("insert must assign an id");

    // Spawn the commit while CREATE B is STILL parked — the commit's Phase
    // 2.7 re-derivation must block on B's admission-held barrier, then
    // (once B publishes) re-derive against the live registry.
    let repo_commit = repo.clone();
    let commit = tokio::spawn(async move { repo_commit.commit_tx(tx).await });

    // Give the commit time to reach (and block on) the barrier CREATE B
    // holds, then release B.
    tokio::time::sleep(Duration::from_millis(80)).await;
    hook.release();
    create_b
        .await
        .unwrap()
        .expect("create_index_v2(B) must succeed");
    commit
        .await
        .unwrap()
        .expect("tx commit must succeed once B's barrier releases");

    // The row must be physically present (never at risk) AND indexed under
    // B — the R0-A fix's whole point.
    let _ = tbl
        .get(carol)
        .await
        .expect("row must be physically present");
    let lower_b_id = key_id(&tbl, "lower_b").await;
    let owners = functional_lookup(&tbl, lower_b_id, "carol").await;
    assert!(
        owners.contains(carol.as_bytes()),
        "R0-A (#1006): the row staged+committed after CREATE A -> DROP A, \
         during CREATE B's backfill->register window, MUST be indexed under \
         B — if this fails, the merged generation counter regressed (or the \
         commit-time re-derivation gate's `generation() == stage_gen` \
         shortcut wrongly skipped B)"
    );

    // A must genuinely be gone (sanity: DROP A actually took effect).
    assert!(
        tbl.index2_registry()
            .get_by_name(key_id(&tbl, "lower_a").await)
            .await
            .is_none(),
        "A must remain dropped"
    );
}

// ============================================================================
// 2. Admission-coverage proofs for the newly-covered gaps (#1012)
// ============================================================================

/// `drop_sorted_index` must acquire the write barrier for its full critical
/// section. Hold `unique_write_lock` externally (the same lock
/// `begin_write_barrier` takes) and confirm the drop blocks until released —
/// pre-fix, `drop_sorted_index` took no lock at all and this would NOT
/// block.
#[tokio::test]
async fn drop_sorted_index_acquires_write_barrier() {
    let repo = make_repo();
    repo.add_table(TableConfig::new("users"));
    let tbl = repo.get_table("users").await.unwrap();
    tbl.create_sorted_index("by_age", &["age"]).await.unwrap();

    let guard = tbl.unique_write_lock().lock_owned().await;

    let tbl_drop = tbl.clone();
    let drop_task = tokio::spawn(async move { tbl_drop.drop_sorted_index("by_age", None).await });

    tokio::time::sleep(Duration::from_millis(80)).await;
    assert!(
        !drop_task.is_finished(),
        "R0-A (#1012): drop_sorted_index must block on the unique_write_lock \
         held here (pre-fix it took no lock and this would finish immediately)"
    );

    drop(guard);
    let dropped = drop_task
        .await
        .unwrap()
        .expect("drop must complete once lock released");
    assert!(
        dropped,
        "the sorted index must have existed and been dropped"
    );
}

/// `drop_index2` must acquire the write barrier for its full critical
/// section (tombstone -> remove_by_id -> sweep -> persist -> clear
/// tombstone).
#[tokio::test]
async fn drop_index2_acquires_write_barrier() {
    let repo = make_repo();
    repo.add_table(TableConfig::new("people"));
    let tbl = repo.get_table("people").await.unwrap();
    let name_field = key_id(&tbl, "name").await;
    let _pre = tbl
        .insert(&record_with_str(name_field, "Alice"))
        .await
        .unwrap();
    tbl.create_index_v2(&functional_lower_op("lower_name", "people", "name"))
        .await
        .unwrap();

    let guard = tbl.unique_write_lock().lock_owned().await;

    let tbl_drop = tbl.clone();
    let drop_task = tokio::spawn(async move { tbl_drop.drop_index2("lower_name", None).await });

    tokio::time::sleep(Duration::from_millis(80)).await;
    assert!(
        !drop_task.is_finished(),
        "R0-A (#1012): drop_index2 must block on the unique_write_lock held \
         here (pre-fix it took no lock and this would finish immediately)"
    );

    drop(guard);
    let dropped = drop_task
        .await
        .unwrap()
        .expect("drop must complete once lock released");
    assert!(
        dropped,
        "the index2 backend must have existed and been dropped"
    );
}

/// The sorted branch of `rename_index` must acquire the write barrier for
/// its full critical section (tombstone -> definition swap -> rekey ->
/// tombstone clear, inside `rename_index_sorted`).
#[tokio::test]
async fn rename_index_sorted_branch_acquires_write_barrier() {
    let repo = make_repo();
    repo.add_table(TableConfig::new("users"));
    let tbl = repo.get_table("users").await.unwrap();
    tbl.create_sorted_index("by_age", &["age"]).await.unwrap();

    let guard = tbl.unique_write_lock().lock_owned().await;

    let tbl_rename = tbl.clone();
    let rename_task =
        tokio::spawn(async move { tbl_rename.rename_index("by_age", "by_age_new", None).await });

    tokio::time::sleep(Duration::from_millis(80)).await;
    assert!(
        !rename_task.is_finished(),
        "R0-A (#1012): rename_index's sorted branch must block on the \
         unique_write_lock held here (pre-fix it took no lock and this \
         would finish immediately)"
    );

    drop(guard);
    rename_task
        .await
        .unwrap()
        .expect("rename must complete once lock released");
    assert!(tbl
        .sorted_indexes()
        .find_by_name_interned(key_id(&tbl, "by_age_new").await)
        .is_some());
}

/// The index2 branch of `rename_index` must acquire the write barrier for
/// its full critical section (`rename_entry` + `save_index2_metadata`).
#[tokio::test]
async fn rename_index_index2_branch_acquires_write_barrier() {
    let repo = make_repo();
    repo.add_table(TableConfig::new("people"));
    let tbl = repo.get_table("people").await.unwrap();
    let name_field = key_id(&tbl, "name").await;
    let _pre = tbl
        .insert(&record_with_str(name_field, "Alice"))
        .await
        .unwrap();
    tbl.create_index_v2(&functional_lower_op("lower_name", "people", "name"))
        .await
        .unwrap();

    let guard = tbl.unique_write_lock().lock_owned().await;

    let tbl_rename = tbl.clone();
    let rename_task = tokio::spawn(async move {
        tbl_rename
            .rename_index("lower_name", "lower_name_new", None)
            .await
    });

    tokio::time::sleep(Duration::from_millis(80)).await;
    assert!(
        !rename_task.is_finished(),
        "R0-A (#1012): rename_index's index2 branch must block on the \
         unique_write_lock held here (pre-fix it took no lock and this \
         would finish immediately)"
    );

    drop(guard);
    rename_task
        .await
        .unwrap()
        .expect("rename must complete once lock released");
    assert!(tbl
        .index2_registry()
        .get_by_name(key_id(&tbl, "lower_name_new").await)
        .await
        .is_some());
}

// ============================================================================
// 3. Structural proof: out-of-order publish cannot happen post-fix
// ============================================================================

/// The out-of-order-publish scenario from Part 1 of the R0-A brief (insert A
/// parked BEFORE `insert_async` completes while insert B publishes fully,
/// so a reader observes `generation() == B's tag` while A is still not
/// visible in `by_id`) is now STRUCTURALLY IMPOSSIBLE to construct as a race
/// on the SAME table: every registry-mutating DDL op holds
/// `TableManager::ddl_admission` for its entire critical section (including
/// the `IndexRegistry::insert`/`remove_by_id` call), so two such ops on the
/// same table can never both be inside `begin_write_barrier` at once — one
/// MUST fully complete (guard dropped, registry mutation done and
/// published) before the other is even admitted.
///
/// This test proves that invariant directly: a paused `create_index_v2`
/// (name "lower_a") holds admission; a concurrently spawned `drop_index2`
/// (name "lower_b", same family/bit, same table) must NOT enter its own
/// critical section until the first fully releases — proven via a shared
/// event log, mirroring `f95_ddl_admission_tests.rs`'s serialization proof
/// but against the REAL `create_index_v2`/`drop_index2` call sites (not the
/// synthetic `begin_write_barrier`-only harness).
#[tokio::test]
async fn two_ddls_on_index2_family_are_never_concurrently_inside_barrier() {
    let repo = make_repo();
    repo.add_table(TableConfig::new("people"));
    let tbl = repo.get_table("people").await.unwrap();
    let name_field = key_id(&tbl, "name").await;
    let _pre = tbl
        .insert(&record_with_str(name_field, "Alice"))
        .await
        .unwrap();

    // Pre-create "lower_b" (uncontended) so the DROP below has something
    // real to drop.
    tbl.create_index_v2(&functional_lower_op("lower_b", "people", "name"))
        .await
        .unwrap();

    // Now install the pause hook and spawn CREATE ("lower_a") — it parks
    // mid-backfill, holding `ddl_admission` + `INDEX2_CREATE` +
    // `unique_write_lock`.
    let hook = Arc::new(BackfillPauseHook::new());
    tbl.set_create_index2_backfill_hook(Some(Arc::clone(&hook)));
    let tbl_create = tbl.clone();
    let create_a = tokio::spawn(async move {
        tbl_create
            .create_index_v2(&functional_lower_op("lower_a", "people", "name"))
            .await
    });
    hook.wait_until_parked().await;

    // Spawn a concurrent DROP of "lower_b" (a DIFFERENT index, but the SAME
    // family/bit and the SAME table's `ddl_admission`). It must NOT be able
    // to complete while CREATE ("lower_a") is still parked — proving the
    // two registry-mutating ops are mutually exclusive on this table.
    let tbl_drop = tbl.clone();
    let drop_b = tokio::spawn(async move { tbl_drop.drop_index2("lower_b", None).await });

    tokio::time::sleep(Duration::from_millis(80)).await;
    assert!(
        !drop_b.is_finished(),
        "R0-A: a concurrent DROP on the same table's index2 family must be \
         blocked on ddl_admission while CREATE is parked mid-critical- \
         section — if this fails, two registry mutations could be in \
         flight at once, reopening the out-of-order-publish race"
    );

    // Release CREATE — it completes (registers "lower_a", drops its guard),
    // which admits DROP.
    hook.release();
    create_a
        .await
        .unwrap()
        .expect("create_index_v2(lower_a) must succeed");
    let dropped = drop_b
        .await
        .unwrap()
        .expect("drop_index2(lower_b) must succeed once admitted");
    assert!(dropped, "lower_b must have existed and been dropped");

    // Final state: lower_a exists (created), lower_b does not (dropped) —
    // both operations completed, strictly serialized, never overlapping.
    assert!(tbl
        .index2_registry()
        .get_by_name(key_id(&tbl, "lower_a").await)
        .await
        .is_some());
    assert!(tbl
        .index2_registry()
        .get_by_name(key_id(&tbl, "lower_b").await)
        .await
        .is_none());
}
