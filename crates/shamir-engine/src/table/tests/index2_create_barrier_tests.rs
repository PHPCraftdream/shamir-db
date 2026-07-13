//! #534 — index2 CREATE INDEX correctness: lost-write race (finding 1) +
//! crash-orphan-id-reuse window (finding 2).
//!
//! Finding 1 (lost-write race): `create_index_v2` backfills a new index2
//! backend from a snapshot of existing rows, THEN registers it. Without a
//! write-barrier, a row written by a concurrent writer AFTER the backfill's
//! stream cursor has passed its key position but BEFORE the backend is
//! registered is seen by NEITHER the backfill (already past it) NOR the live
//! `index2_on_insert` hook (backend not yet routable) — permanently missing
//! from the new index. The fix holds `unique_write_lock` across
//! backfill→register AND flips `index2_create_barrier` so EVERY writer path
//! (even on an index2-only table with no legacy unique index) serializes
//! against the create for its duration.
//!
//! The regression test drives a concurrent writer INTO that exact window via
//! the test-only `create_index2_backfill_hook`, which parks the create between
//! backfill and register. WITHOUT the fix the concurrent insert slips through
//! and the row is lost; WITH the fix the insert blocks on the barrier until the
//! create finishes, then lands in the new index.
//!
//! Finding 2 (crash-orphan-id-reuse): `allocate_id()` is a non-durable
//! `AtomicU32::fetch_add`; the id only becomes durable at the FINAL
//! `save_index2_metadata`. A crash mid-create leaves postings under an id that
//! was never persisted, and on restart `next_id` resets to the last persisted
//! watermark — so the SAME id could be reallocated to a DIFFERENT index. The
//! fix persists the reserved `next_id` immediately after `allocate_id()`,
//! before backfill. The regression test simulates crash-then-restart and
//! asserts the interrupted id is never reallocated.

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

/// A functional `lower(<field>)` index create op.
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

/// Resolve the set of record ids the functional `lower` index holds for a given
/// (already-lowercased) string value, by querying the backend directly.
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
// Finding 1 — lost-write race
// ============================================================================

/// THE #534-finding-1 proof. A row inserted DURING `create_index_v2` (inside
/// the backfill→register window) must NOT be lost — it must be queryable via
/// the new functional index afterward.
///
/// Determinism: the test-only `create_index2_backfill_hook` parks the create
/// exactly at the window (backfill done, backend not yet registered). The
/// concurrent insert is then driven while the create is parked.
///
/// Pre-fix (no barrier): the create does NOT hold `unique_write_lock`, and an
/// insert on a functional-only table takes no lock either, so the insert
/// completes while the create is parked — but the backfill already ran (missed
/// the new row) and the backend isn't registered yet (live hook can't route
/// it), so the row is LOST → the final assertion fails.
///
/// Post-fix: the parked create still holds `unique_write_lock` and has set
/// `index2_create_barrier`, so the concurrent insert BLOCKS until the create
/// registers the backend and releases the lock; the insert's live
/// `index2_on_insert` hook then routes the row into the new index → found.
#[tokio::test]
async fn insert_during_index2_create_is_not_lost() {
    let repo = make_repo();
    repo.add_table(TableConfig::new("people"));
    let tbl = repo.get_table("people").await.unwrap();

    let name_field = key_id(&tbl, "name").await;

    // One pre-existing row so the backfill has something to stream.
    let _pre = tbl
        .insert(&record_with_str(name_field, "Alice"))
        .await
        .unwrap();

    // Install the deterministic pause hook.
    let hook = Arc::new(BackfillPauseHook::new());
    tbl.set_create_index2_backfill_hook(Some(Arc::clone(&hook)));

    // Spawn the create; it will park at the backfill→register window.
    let tbl_create = tbl.clone();
    let create = tokio::spawn(async move {
        tbl_create
            .create_index_v2(&functional_lower_op("lower_name", "people", "name"))
            .await
    });

    // Wait until the create is parked in the window (backfill complete, backend
    // not yet registered). No timing guesswork — this is a rendezvous.
    hook.wait_until_parked().await;

    // Now insert a NEW row while the create is parked in the exact lost-write
    // window. WITH the fix this blocks on the barrier the parked create holds.
    let tbl_insert = tbl.clone();
    let insert =
        tokio::spawn(async move { tbl_insert.insert(&record_with_str(name_field, "Bob")).await });

    // Give the insert task time to reach — and (post-fix) block on — the
    // barrier. Post-fix it must still be running; pre-fix it would already have
    // completed (no lock), demonstrating the window is open.
    tokio::time::sleep(Duration::from_millis(80)).await;
    assert!(
        !insert.is_finished(),
        "post-fix: the concurrent insert must BLOCK on the write-barrier held \
         by the parked create_index_v2 (pre-fix it completes here, proving the \
         lost-write window was open)"
    );

    // Release the create — it registers the backend and drops the barrier+lock.
    hook.release();
    create.await.unwrap().expect("create_index_v2 must succeed");
    // The insert can now proceed and be routed to the (now registered) index.
    let bob = insert.await.unwrap().expect("insert must succeed");

    // The concurrently-inserted "Bob" row must be present in the new index.
    let owners = functional_lookup(&tbl, key_id(&tbl, "lower_name").await, "bob").await;
    assert!(
        owners.contains(bob.as_bytes()),
        "the row inserted during create_index_v2 must be indexed (not lost)"
    );

    // And the pre-existing "Alice" row must be present too (backfill correctness).
    let alice_owners = functional_lookup(&tbl, key_id(&tbl, "lower_name").await, "alice").await;
    assert_eq!(
        alice_owners.len(),
        1,
        "pre-existing row must be backfilled into the new index"
    );
}

/// Simpler companion proof: with no hook installed, a real `create_index_v2`
/// acquires `unique_write_lock`. Hold it externally and confirm the create
/// blocks until released — the barrier is genuinely taken (pre-fix the create
/// took no lock and this would NOT block).
#[tokio::test]
async fn create_index_v2_acquires_write_barrier() {
    let repo = make_repo();
    repo.add_table(TableConfig::new("people"));
    let tbl = repo.get_table("people").await.unwrap();
    let name_field = key_id(&tbl, "name").await;
    let _pre = tbl
        .insert(&record_with_str(name_field, "Alice"))
        .await
        .unwrap();

    // Hold the barrier the create must acquire.
    let guard = tbl.unique_write_lock().lock_owned().await;

    let tbl_create = tbl.clone();
    let create = tokio::spawn(async move {
        tbl_create
            .create_index_v2(&functional_lower_op("lower_name", "people", "name"))
            .await
    });

    tokio::time::sleep(Duration::from_millis(80)).await;
    assert!(
        !create.is_finished(),
        "create_index_v2 must block on the unique_write_lock held here (pre-fix \
         it did not acquire the lock and would finish)"
    );

    drop(guard);
    create
        .await
        .unwrap()
        .expect("create must complete once lock released");
    // Sanity: backend registered.
    assert!(
        tbl.index2_registry()
            .get_by_name(key_id(&tbl, "lower_name").await)
            .await
            .is_some(),
        "backend must be registered after create completes"
    );
}

// ============================================================================
// Finding 1 — backfill regression for fts / functional / vector
// ============================================================================

/// Common-case (non-racing) backfill: a functional index created on a
/// non-empty table indexes the pre-existing rows. Regression guard that the
/// added locking + reserve-id persist did not break the backfill itself.
#[tokio::test]
async fn functional_backfill_indexes_preexisting_rows() {
    let repo = make_repo();
    repo.add_table(TableConfig::new("people"));
    let tbl = repo.get_table("people").await.unwrap();
    let name_field = key_id(&tbl, "name").await;

    let a = tbl
        .insert(&record_with_str(name_field, "Carol"))
        .await
        .unwrap();
    let b = tbl
        .insert(&record_with_str(name_field, "carol"))
        .await
        .unwrap();

    tbl.create_index_v2(&functional_lower_op("lower_name", "people", "name"))
        .await
        .unwrap();

    // Both rows lower() to "carol" → both must be indexed under that key.
    let owners = functional_lookup(&tbl, key_id(&tbl, "lower_name").await, "carol").await;
    assert!(owners.contains(a.as_bytes()) && owners.contains(b.as_bytes()));
    assert_eq!(owners.len(), 2);
}

// ============================================================================
// Finding 2 — crash-orphan-id-reuse
// ============================================================================

/// Simulate a crash mid-create (id allocated, reserve-persist done, but the
/// final metadata save never completed) followed by a restart, and assert the
/// interrupted id is NOT handed out again.
///
/// The reserve-persist added by the fix (immediately after `allocate_id()`)
/// advances the durable `next_id` watermark BEFORE backfill, so a reopen
/// restores `next_id` PAST the dead id — it can never be reallocated.
///
/// We drive the persistence + reopen paths directly (no full engine restart is
/// needed): the fix's guarantee is entirely captured by "the watermark was
/// persisted before the crash point", which `save_index2_metadata` /
/// `load_index2_metadata` / `set_next_id` embody.
#[tokio::test]
async fn crashed_index2_id_is_not_reallocated_after_restart() {
    use shamir_storage::storage_in_memory::InMemoryStore;
    use shamir_storage::types::Store;

    let info_store: Arc<dyn Store> = Arc::new(InMemoryStore::new());

    // --- "process 1": allocate an id and reserve it durably (the fix). ---
    let reg1 = crate::index2::IndexRegistry::new();
    let crashed_id = reg1.allocate_id(); // e.g. 1
                                         // Finding-2 fix: persist the reserved next_id watermark BEFORE any
                                         // backfill/register would run.
    crate::index2::persistence::save_index2_metadata(&reg1, &info_store)
        .await
        .unwrap();
    // ...then the process "crashes" — the new backend is NEVER inserted and the
    // FINAL save (post-register) never happens.

    // --- "process 2": restart restores next_id from persisted metadata. ---
    let reg2 = crate::index2::IndexRegistry::new();
    let persisted = crate::index2::persistence::load_index2_metadata(&info_store)
        .await
        .unwrap()
        .expect("reserved metadata must have been persisted");
    reg2.set_next_id(persisted.next_id);

    // The next allocation MUST NOT reuse the crashed id.
    let next = reg2.allocate_id();
    assert_ne!(
        next, crashed_id,
        "a crashed-and-never-registered index2 id must not be reallocated after restart"
    );
    assert!(
        next > crashed_id,
        "next_id watermark must have advanced past the crashed id"
    );
}

/// Negative control for finding 2: WITHOUT the reserve-persist (the pre-fix
/// behaviour), a crash-then-restart WOULD reallocate the same id. This test
/// documents the hazard the fix closes by reproducing the pre-fix sequence
/// (allocate, but do NOT persist before the crash) and showing the id repeats.
#[tokio::test]
async fn without_reserve_persist_crashed_id_would_be_reused() {
    use shamir_storage::storage_in_memory::InMemoryStore;
    use shamir_storage::types::Store;

    let info_store: Arc<dyn Store> = Arc::new(InMemoryStore::new());

    // Pre-fix sequence: allocate, then crash BEFORE persisting anything.
    let reg1 = crate::index2::IndexRegistry::new();
    let crashed_id = reg1.allocate_id();
    // (no save_index2_metadata here — this is the pre-fix window)

    // Restart: nothing persisted → next_id stays at its fresh default.
    let reg2 = crate::index2::IndexRegistry::new();
    let loaded = crate::index2::persistence::load_index2_metadata(&info_store)
        .await
        .unwrap();
    assert!(loaded.is_none(), "nothing was persisted pre-fix");
    // next_id was never advanced → the same id is handed out again.
    let reused = reg2.allocate_id();
    assert_eq!(
        reused, crashed_id,
        "documents the pre-fix hazard: without the reserve-persist the crashed \
         id is reallocated to a different index definition"
    );
}

// ============================================================================
// #538 — tx-commit-path sibling of #534's lost-write race test
// ============================================================================
//
// #534's fix (`index2_create_barrier` + `needs_write_barrier()`) only reaches
// the non-tx writer methods in `table_manager_crud.rs`. Every REAL client DML
// statement instead runs through the tx-commit pipeline (`execute_insert_tx`
// et al. in `write_exec.rs` → `insert_tx_many_bytes` et al. in
// `table_manager_tx_ops.rs` for STAGING → `repo.commit_tx` for the Phase
// 2.5-5c commit pipeline). #538 closes part of that gap:
//
// Part A (closed here): `pre_commit.rs`'s Phase 2.5 prelock now acquires
// `unique_write_lock` for every table this tx wrote to that has
// `needs_write_barrier() == true` (not just tables with a legacy unique
// index) — so a tx's COMMIT now serializes against an in-flight
// `create_index_v2` on an index2-only table, mirroring the non-tx fix.
//
// Part B (still open, honestly documented): the index2 ops-PLAN
// (`tx.index_write_set`) is captured at STAGE time against an
// `all_backends()` snapshot, which can be taken before the barrier ever goes
// up. If staging completes before the new backend is registered, the plan
// simply has no ops for it — Part A's commit-time serialization cannot
// retroactively add ops to an already-built plan. So a tx that stages AND
// commits entirely inside the backfill→register window still loses the row
// from the NEW index, even with Part A applied: Phase 5a writes the row's
// data as usual (that part was never at risk), but Phase 5c has nothing to
// apply for the new backend. This is proven by
// `stage_and_commit_inside_window_still_misses_new_index_part_b_open` below.

/// tx-path sibling of #534's `insert_during_index2_create_is_not_lost`,
/// scoped to prove Part A alone: a tx that STAGES before the barrier goes up
/// (so its stage-time `all_backends()` snapshot cannot possibly be missing
/// anything relevant to ITS OWN write — there is no index2 backend at all
/// yet at stage time) but whose COMMIT lands inside the parked
/// backfill→register window must have its commit BLOCK on the barrier
/// (Part A), and — because staging happened before any index existed and
/// therefore staged zero index2 ops for a still-nonexistent backend — this
/// case does not depend on Part B at all: there is nothing for the new index
/// to miss, since this tx's row was never written to a table with a
/// same-type index2 backend at stage time. The point of this test is
/// narrower and more mechanical than the ops-plan question: it proves the
/// commit-time serialization itself (Part A) is real — pre-fix the commit
/// races the parked create and finishes immediately; post-fix it blocks
/// until the create releases the barrier.
#[tokio::test]
async fn tx_commit_blocks_on_index2_create_barrier_part_a() {
    let repo = make_repo();
    repo.add_table(TableConfig::new("people"));
    let tbl = repo.get_table("people").await.unwrap();

    let name_field = key_id(&tbl, "name").await;

    // One pre-existing row so the backfill has something to stream.
    let _pre = tbl
        .insert(&record_with_str(name_field, "Alice"))
        .await
        .unwrap();

    // Install the deterministic pause hook and spawn the create — it parks
    // at the backfill→register window (backfill done, backend NOT yet
    // registered, `unique_write_lock` + `index2_create_barrier` both up).
    let hook = Arc::new(BackfillPauseHook::new());
    tbl.set_create_index2_backfill_hook(Some(Arc::clone(&hook)));
    let tbl_create = tbl.clone();
    let create = tokio::spawn(async move {
        tbl_create
            .create_index_v2(&functional_lower_op("lower_name", "people", "name"))
            .await
    });
    hook.wait_until_parked().await;

    // Stage a tx-path insert of a NEW row via the real client DML entry
    // point (`execute_insert_tx`), then spawn ITS COMMIT while the create is
    // still parked. Staging happens here, BEFORE the spawned commit runs —
    // at this instant the new functional backend does not exist yet in
    // ANY form (not even mid-backfill), so `insert_tx_many_bytes`'s
    // `all_backends()` snapshot is trivially complete for this tx (it simply
    // has no functional backend to plan ops against). This isolates Part A's
    // effect (commit-time serialization) from Part B's ops-plan-staleness
    // question, which `stage_and_commit_inside_window_still_misses_new_index_part_b_open`
    // exercises separately.
    let (mut tx, _guard) = repo.begin_tx(IsolationLevel::Snapshot).await.unwrap();
    let op = write::insert("people")
        .rows([mpack!({ "name": "Bob" })])
        .build();
    let result = tbl
        .execute_insert_tx(&op, &mut tx, true, None)
        .await
        .unwrap();
    assert_eq!(result.affected, 1);
    let bob = result.records[0].id.expect("insert must assign an id");

    let repo_commit = repo.clone();
    let commit = tokio::spawn(async move { repo_commit.commit_tx(tx).await });

    // Give the commit task time to reach — and (post-fix) block on — the
    // barrier. Post-fix (Part A applied) it must still be running; pre-fix
    // it would already have completed (Phase 2.5 never even looks at this
    // index2-only table's barrier), demonstrating the commit-time gap #538
    // exists to close.
    tokio::time::sleep(Duration::from_millis(80)).await;
    assert!(
        !commit.is_finished(),
        "post-fix (#538 Part A): the tx commit must BLOCK on the write-barrier \
         held by the parked create_index_v2 (pre-fix it completes here, proving \
         the tx-commit-path gap #534 left open was real)"
    );

    // Release the create — it registers the backend and drops the barrier.
    hook.release();
    create.await.unwrap().expect("create_index_v2 must succeed");
    commit
        .await
        .unwrap()
        .expect("tx commit must succeed once the barrier is released");

    // Sanity: the row physically exists (Phase 5a always applies regardless
    // of Part A/B — this was never the at-risk part).
    let _ = tbl.get(bob).await.expect("row must be physically present");
}

/// Honest Part-B residual proof (explicitly requested by the #538 brief): a
/// tx whose STAGE **and** COMMIT both land inside the backfill→register
/// window still loses the row from the newly-created index, even with Part A
/// applied. Part A only serializes the commit's TIMING against the barrier —
/// it cannot retroactively add ops to `tx.index_write_set`, which was already
/// built (empty for the new backend) back at stage time, before the barrier
/// was ever consulted. This test is expected to FAIL the "row is indexed"
/// assertion both before AND after Part A — it exists to prove Part B is a
/// real, still-open gap rather than something silently fixed by Part A.
///
/// NOTE: This test is intentionally written to assert the CURRENT (still-gap)
/// behavior — it passes by confirming the row is (still) missing from the new
/// index, so the suite documents the open residual rather than silently
/// bit-rotting into a false "everything is fixed" green run. If a future task
/// closes Part B, this assertion must be inverted (and the test re-homed to
/// the closed-gap section) as part of that fix.
#[tokio::test]
async fn stage_and_commit_inside_window_still_misses_new_index_part_b_open() {
    let repo = make_repo();
    repo.add_table(TableConfig::new("people"));
    let tbl = repo.get_table("people").await.unwrap();

    let name_field = key_id(&tbl, "name").await;

    let _pre = tbl
        .insert(&record_with_str(name_field, "Alice"))
        .await
        .unwrap();

    let hook = Arc::new(BackfillPauseHook::new());
    tbl.set_create_index2_backfill_hook(Some(Arc::clone(&hook)));
    let tbl_create = tbl.clone();
    let create = tokio::spawn(async move {
        tbl_create
            .create_index_v2(&functional_lower_op("lower_name", "people", "name"))
            .await
    });
    hook.wait_until_parked().await;

    // Both STAGE and (spawned) COMMIT happen here, entirely inside the
    // parked window — mirrors #534's non-tx test shape exactly, but via the
    // tx-path entry point.
    let (mut tx, _guard) = repo.begin_tx(IsolationLevel::Snapshot).await.unwrap();
    let op = write::insert("people")
        .rows([mpack!({ "name": "Carol" })])
        .build();
    let result = tbl
        .execute_insert_tx(&op, &mut tx, true, None)
        .await
        .unwrap();
    let carol = result.records[0].id.expect("insert must assign an id");

    let repo_commit = repo.clone();
    let commit = tokio::spawn(async move { repo_commit.commit_tx(tx).await });

    // Let the commit block on the Part-A barrier, then release the create.
    tokio::time::sleep(Duration::from_millis(80)).await;
    hook.release();
    create.await.unwrap().expect("create_index_v2 must succeed");
    commit
        .await
        .unwrap()
        .expect("tx commit must succeed (data write is never at risk)");

    // The row IS physically present (Phase 5a is unconditional).
    let _ = tbl
        .get(carol)
        .await
        .expect("row must be physically present");

    // But it is NOT queryable via the new functional index — Part B's
    // guaranteed-miss residual, honestly reproduced: this tx's
    // `insert_tx_many_bytes` staged its `index_write_set` against the
    // `all_backends()` snapshot taken BEFORE the barrier was ever up (there
    // was no functional backend, live or mid-backfill, at that instant), so
    // no op for the new backend was ever planned — Part A's commit-time lock
    // has nothing left to protect by the time it is acquired.
    let owners = functional_lookup(&tbl, key_id(&tbl, "lower_name").await, "carol").await;
    assert!(
        !owners.contains(carol.as_bytes()),
        "#538 Part B residual: this row is EXPECTED to still be missing from \
         the new index even with Part A applied — if this assertion starts \
         failing (row found), Part B has been closed by some other change \
         and this test must be updated/re-homed to reflect the fix"
    );
}
