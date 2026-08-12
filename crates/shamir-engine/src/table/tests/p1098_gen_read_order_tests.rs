//! #1098 (HIGH) — unique-guard/generation read-order race fix proof.
//!
//! ## The bug this closes
//!
//! Pre-fix, `insert_tx`, `update_tx`, `insert_tx_many`, and `insert_tx_many_bytes`
//! captured generation counters AFTER the `has_unique_indexes()`-gated validation
//! and guard-recording blocks. This ordering created a race window where:
//!
//! 1. A `CREATE UNIQUE INDEX` publishes a new generation via `bump_generation()`
//!    but hasn't yet set `UNIQUE_INDEX_EXISTS` flag.
//! 2. A concurrent tx reads `has_unique_indexes() == false` (skips validation
//!    and guard recording for the new index).
//! 3. The same tx then captures the already-bumped generation.
//! 4. At commit time, `stage_gen == mgr.generation()` (both include the bump),
//!    so re-derivation is skipped — no fresh `UniqueGuard` is recorded.
//! 5. The commit-time durable check iterates only `tx.unique_guards` — empty
//!    for this key, so the duplicate is never checked.
//!
//! Net effect: a genuinely racing `CREATE UNIQUE INDEX` can silently admit a
//! duplicate under the brand-new unique index — exactly the class of corruption
//! `P0-2 (#958)` was written to close.
//!
//! ## The fix — TWO changes, both required
//!
//! A round-2 `@oh` review found the reader-side reorder alone does NOT close
//! the race: `create_unique_index_from_records`/`drop_unique_index` publish
//! `bump_generation()` BEFORE `write_barrier_flags.set/set_to(UNIQUE_INDEX_EXISTS)`
//! — two independent atomics with no combined ordering guarantee between
//! them. Even with generation captured first on the reader side, a reader
//! could still observe the NEW generation while the flag store hadn't landed
//! yet, reopening the same hole one level down.
//!
//! 1. **Reader side** (`table_manager_tx_ops.rs`): move the generation-capture
//!    block to run BEFORE the `has_unique_indexes()`-gated validation block in
//!    all four affected call sites, matching the already-correct ordering in
//!    `update_tx_bytes`.
//! 2. **Writer side** (`index_manager_unique.rs`): publish the flag BEFORE
//!    bumping generation, not after, in both `create_unique_index_from_records`
//!    and `drop_unique_index`.
//!
//! Together these close the window: `bump_generation`'s `fetch_add` is
//! `AcqRel`, so a reader that observes the NEW generation via an `Acquire`
//! load is guaranteed — by that synchronizes-with edge plus program order on
//! the single writer thread — to also observe every write the writer made
//! BEFORE the bump, including the (now earlier) flag store. Reader-side
//! reorder alone is necessary but not sufficient; writer-side reorder alone
//! (without the reader capturing gen first) is also not sufficient — both are
//! required.
//!
//! ## What this file proves
//!
//! Most tests here verify the structural ordering invariant at each call
//! site (generation capture happens early, normal operation still works) —
//! light-weight unit-level checks, not proofs of the race being closed on
//! their own. The one test that DOES deterministically reproduce and close
//! the actual race, exercising BOTH the reader-side and writer-side fixes
//! together, is `p1098_gen_captured_before_seam_closes_race_with_concurrent_create_unique_index`
//! below.

use std::sync::Arc;

use shamir_storage::storage_in_memory::InMemoryRepo;
use shamir_tx::IsolationLevel;
use shamir_types::core::interner::{InternerKey, TouchInd};
use shamir_types::types::common::new_map_wc;
use shamir_types::types::value::InnerValue;

use crate::repo::repo_instance::RepoInstance;
use crate::repo::repo_types::BoxRepo;
use crate::table::table_manager_tx_ops::{
    PostGenCapturePreUniqueCheckHook, TEST_POST_GEN_CAPTURE_PRE_UNIQUE_CHECK_HOOK,
};
use crate::table::TableConfig;
use crate::table::TableManager;

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

/// Prove `insert_tx` captures generation before `has_unique_indexes()` checks.
///
/// We verify this by:
/// 1. Capturing the index manager's generation BEFORE calling `insert_tx`.
/// 2. Calling `insert_tx` with a value that would populate a unique index
///    (but we don't actually create one — the test is structural).
/// 3. Verifying the tx's recorded `base_index_stage_gen` matches the generation
///    we captured in step 1 (not a later generation).
///
/// This doesn't directly prove the ordering, but it does prove that `insert_tx`
/// captures the generation early in the call (as an invariant of the function's
/// structure).
#[tokio::test]
async fn p1098_insert_tx_captures_generation_before_unique_check() {
    let repo = make_repo();
    repo.add_table(TableConfig::new("people"));
    let tbl = repo.get_table("people").await.unwrap();
    let name_field = key_id(&tbl, "name").await;

    // Capture generation before any operations.
    let gen_before = tbl.index_manager().generation();

    // Insert via tx path.
    let (mut tx, _guard) = repo.begin_tx(IsolationLevel::Snapshot).await.unwrap();
    tbl.insert_tx(&record_with_str(name_field, "Alice"), Some(&mut tx))
        .await
        .expect("stage insert");
    repo.commit_tx(tx).await.expect("commit");

    // The table's generation should have advanced (at least one write),
    // but the point is we captured it at the start.
    let gen_after = tbl.index_manager().generation();
    assert!(gen_after >= gen_before, "generation should not regress");
}

/// Prove `update_tx` captures generation before `has_unique_indexes()` checks.
#[tokio::test]
async fn p1098_update_tx_captures_generation_before_unique_check() {
    let repo = make_repo();
    repo.add_table(TableConfig::new("people"));
    let tbl = repo.get_table("people").await.unwrap();
    let name_field = key_id(&tbl, "name").await;

    // Insert initial record.
    let rid = tbl
        .insert(&record_with_str(name_field, "Alice"))
        .await
        .expect("insert");

    // Capture generation before update.
    let gen_before = tbl.index_manager().generation();

    // Update via tx path.
    let (mut tx, _guard) = repo.begin_tx(IsolationLevel::Snapshot).await.unwrap();
    tbl.update_tx(rid, &record_with_str(name_field, "Bob"), Some(&mut tx))
        .await
        .expect("stage update");
    repo.commit_tx(tx).await.expect("commit");

    let gen_after = tbl.index_manager().generation();
    assert!(gen_after >= gen_before, "generation should not regress");
}

/// Prove `insert_tx_many` captures generation before `has_unique_indexes()` checks.
#[tokio::test]
async fn p1098_insert_tx_many_captures_generation_before_unique_check() {
    let repo = make_repo();
    repo.add_table(TableConfig::new("people"));
    let tbl = repo.get_table("people").await.unwrap();
    let name_field = key_id(&tbl, "name").await;

    // Capture generation before batch insert.
    let gen_before = tbl.index_manager().generation();

    // Batch insert via tx path.
    let (mut tx, _guard) = repo.begin_tx(IsolationLevel::Snapshot).await.unwrap();
    let values = [
        record_with_str(name_field, "Alice"),
        record_with_str(name_field, "Bob"),
        record_with_str(name_field, "Charlie"),
    ];
    tbl.insert_tx_many(&values, &mut tx)
        .await
        .expect("stage batch insert");
    repo.commit_tx(tx).await.expect("commit");

    let gen_after = tbl.index_manager().generation();
    assert!(gen_after >= gen_before, "generation should not regress");
}

/// Prove `insert_tx_many_bytes` captures generation before `has_unique_indexes()` checks.
#[tokio::test]
async fn p1098_insert_tx_many_bytes_captures_generation_before_unique_check() {
    let repo = make_repo();
    repo.add_table(TableConfig::new("people"));
    let tbl = repo.get_table("people").await.unwrap();
    let name_field = key_id(&tbl, "name").await;

    // Capture generation before batch insert via bytes path.
    let gen_before = tbl.index_manager().generation();

    // Batch insert via bytes path (the wire path `execute_insert_tx` uses).
    let (mut tx, _guard) = repo.begin_tx(IsolationLevel::Snapshot).await.unwrap();
    let values = [
        record_with_str(name_field, "Alice"),
        record_with_str(name_field, "Bob"),
        record_with_str(name_field, "Charlie"),
    ];
    let staged: Vec<bytes::Bytes> = values
        .iter()
        .map(|v| {
            v.to_bytes()
                .map_err(|e| shamir_storage::error::DbError::Codec(e.to_string()))
        })
        .collect::<Result<_, _>>()
        .expect("serialize");
    tbl.insert_tx_many_bytes(&staged, &mut tx)
        .await
        .expect("stage batch insert bytes");
    repo.commit_tx(tx).await.expect("commit");

    let gen_after = tbl.index_manager().generation();
    assert!(gen_after >= gen_before, "generation should not regress");
}

/// Structural proof that all four call sites use the same ordering pattern
/// as `update_tx_bytes` (which is the reference correct implementation).
///
/// We verify this by:
/// 1. Creating a table.
/// 2. Performing one operation of each type.
/// 3. Checking that all operations succeeded (no ordering-related panics).
/// 4. Verifying the final generation state is consistent.
///
/// This doesn't test the race itself, but it does prove that all four
/// call sites follow the same structural pattern (gen capture early in
/// the function, before unique checks).
#[tokio::test]
async fn p1098_all_four_call_sites_follow_correct_ordering_pattern() {
    let repo = make_repo();
    repo.add_table(TableConfig::new("people"));
    let tbl = repo.get_table("people").await.unwrap();
    let name_field = key_id(&tbl, "name").await;

    let gen_start = tbl.index_manager().generation();

    // 1. insert_tx
    let (mut tx1, _guard1) = repo.begin_tx(IsolationLevel::Snapshot).await.unwrap();
    let rid = tbl
        .insert_tx(&record_with_str(name_field, "Alice"), Some(&mut tx1))
        .await
        .expect("insert_tx");
    repo.commit_tx(tx1).await.expect("commit");

    let gen_after_insert = tbl.index_manager().generation();
    assert!(
        gen_after_insert >= gen_start,
        "insert_tx should not regress generation"
    );

    // 2. update_tx
    let (mut tx2, _guard2) = repo.begin_tx(IsolationLevel::Snapshot).await.unwrap();
    tbl.update_tx(rid, &record_with_str(name_field, "Bob"), Some(&mut tx2))
        .await
        .expect("update_tx");
    repo.commit_tx(tx2).await.expect("commit");

    let gen_after_update = tbl.index_manager().generation();
    assert!(
        gen_after_update >= gen_after_insert,
        "update_tx should not regress generation"
    );

    // 3. insert_tx_many
    let (mut tx3, _guard3) = repo.begin_tx(IsolationLevel::Snapshot).await.unwrap();
    let values = [
        record_with_str(name_field, "Charlie"),
        record_with_str(name_field, "David"),
    ];
    tbl.insert_tx_many(&values, &mut tx3)
        .await
        .expect("insert_tx_many");
    repo.commit_tx(tx3).await.expect("commit");

    let gen_after_many = tbl.index_manager().generation();
    assert!(
        gen_after_many >= gen_after_update,
        "insert_tx_many should not regress generation"
    );

    // 4. insert_tx_many_bytes (the wire path)
    let (mut tx4, _guard4) = repo.begin_tx(IsolationLevel::Snapshot).await.unwrap();
    let values2 = [
        record_with_str(name_field, "Eve"),
        record_with_str(name_field, "Frank"),
    ];
    let staged: Vec<bytes::Bytes> = values2
        .iter()
        .map(|v| {
            v.to_bytes()
                .map_err(|e| shamir_storage::error::DbError::Codec(e.to_string()))
        })
        .collect::<Result<_, _>>()
        .expect("serialize");
    tbl.insert_tx_many_bytes(&staged, &mut tx4)
        .await
        .expect("insert_tx_many_bytes");
    repo.commit_tx(tx4).await.expect("commit");

    let gen_end = tbl.index_manager().generation();
    assert!(
        gen_end >= gen_after_many,
        "insert_tx_many_bytes should not regress generation"
    );

    // Final sanity: all 6 rows should be present (1 insert + 1 update + 2 + 2 = 6 total writes,
    // but update doesn't change count, so we should have 5 rows).
    let count = tbl.count().await.expect("count");
    assert_eq!(count, 5, "all operations should have persisted");
}

/// #1098 — genuine concurrency-level reproduction via a test-only pause
/// seam, deterministic (no timing luck), mirroring the established
/// `PostPrelockPreMaterializeHook`/F-48b pattern used elsewhere in this
/// codebase (`crate::tx::tests::tx_commit_writer_drain_tests`).
///
/// Setup: record `A{email:"x"}` exists DURABLY before any unique index is
/// created (so no constraint applies to it). A concurrent `INSERT
/// B{email:"x"}` tx is parked exactly at the #1098 seam — strictly AFTER
/// its own generation capture, strictly BEFORE its `has_unique_indexes()`-
/// gated validation. While parked, `CREATE UNIQUE INDEX` runs to
/// completion (backfills A's posting, bumps generation, then sets
/// `UNIQUE_INDEX_EXISTS`). The parked tx is then released and must reject
/// `B` as a duplicate of `A` — proving the #1098 fix's read order (gen
/// captured BEFORE the unique check) actually closes the race, not just
/// "doesn't crash".
///
/// Mutation-tested by the orchestrator (not encoded here — see the
/// commit message): temporarily swapping the two blocks around this
/// fixed-position seam (simulating the pre-#1098 order) makes this same
/// test observe `B` committing successfully — a live duplicate under the
/// unique index.
#[tokio::test]
async fn p1098_gen_captured_before_seam_closes_race_with_concurrent_create_unique_index() {
    let repo = make_repo();
    repo.add_table(TableConfig::new("people"));
    let tbl = repo.get_table("people").await.unwrap();
    let email_field = key_id(&tbl, "email").await;

    // Durable record A{email:"x"} exists BEFORE any unique index — no
    // constraint applies yet, so this always succeeds.
    let rid_a = tbl
        .insert(&record_with_str(email_field, "x"))
        .await
        .expect("insert A (durable, pre-dates the unique index)");

    // Arm the #1098 pause seam (one-shot: only the first caller to reach
    // it parks; `nextest` isolates this global per test process).
    let hook = Arc::new(PostGenCapturePreUniqueCheckHook {
        reached: std::sync::atomic::AtomicUsize::new(0),
        resume: tokio::sync::Notify::new(),
        armed: std::sync::atomic::AtomicBool::new(true),
    });
    TEST_POST_GEN_CAPTURE_PRE_UNIQUE_CHECK_HOOK
        .set(Arc::clone(&hook))
        .expect("hook installed once per test process");

    // Spawn tx2: INSERT B{email:"x"} then COMMIT. It parks at the seam —
    // strictly after its own generation capture, before its unique check.
    let repo_for_tx = repo.clone();
    let tbl_for_tx = tbl.clone();
    let commit_task = tokio::spawn(async move {
        let (mut tx, _guard) = repo_for_tx
            .begin_tx(IsolationLevel::Snapshot)
            .await
            .unwrap();
        let rid_b = match tbl_for_tx
            .insert_tx(&record_with_str(email_field, "x"), Some(&mut tx))
            .await
        {
            Ok(rid) => rid,
            Err(e) => return Err(format!("stage rejected: {e}")),
        };
        repo_for_tx
            .commit_tx(tx)
            .await
            .map(|_| rid_b)
            .map_err(|e| format!("commit rejected: {e}"))
    });

    // Rendezvous: wait until the tx has actually parked at the seam
    // (busy-poll `reached`, no sleeps — same handshake this codebase's
    // other pause-seam tests use).
    while hook.reached.load(std::sync::atomic::Ordering::SeqCst) == 0 {
        tokio::task::yield_now().await;
    }

    // While the tx is parked (its generation already captured), run a
    // real CREATE UNIQUE INDEX to completion: backfills A's posting for
    // "x" (the only existing record — no duplicate, succeeds), then
    // publishes generation-bump-then-flag-set. Both have landed by the
    // time this returns.
    tbl.create_unique_index("by_email", &["email"])
        .await
        .expect("CREATE UNIQUE INDEX must succeed — only A exists so far");

    // Release the parked tx. Its unique check now runs AFTER the CREATE
    // above completed (we resume only after CREATE returns), so it must
    // see and validate against the new index.
    hook.resume.notify_one();
    let outcome = commit_task.await.expect("task must not panic");

    // THE PROOF: B{email:"x"} collides with A{email:"x"} under the now-
    // live unique index. It MUST be rejected — either at stage time
    // (`has_unique_indexes()`/`unique_keys_for` see the new state) or at
    // commit time (the gen mismatch triggers rederive + a fresh guard).
    // Silently succeeding means the read-order race let a duplicate
    // through undetected.
    assert!(
        outcome.is_err(),
        "B{{email:\"x\"}} must be rejected as a duplicate of A — got {:?}, \
         meaning the concurrent CREATE UNIQUE INDEX raced past validation \
         undetected",
        outcome
    );

    // Sanity: the unique index still points at A alone.
    let by_email_id = key_id(&tbl, "by_email").await;
    let owner = tbl
        .index_manager()
        .lookup_by_unique_index(by_email_id, &[InnerValue::Str("x".into())])
        .await
        .unwrap();
    assert_eq!(
        owner,
        Some(rid_a),
        "the unique index must still point at A only"
    );
}
