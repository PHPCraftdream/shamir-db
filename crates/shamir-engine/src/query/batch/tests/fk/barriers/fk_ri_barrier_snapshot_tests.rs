use shamir_query_builder::batch::Batch;
use shamir_query_builder::filter;
use shamir_query_builder::write;
use shamir_query_builder::write::doc;
use shamir_query_types::admin::FkAction;
use shamir_types::access::Actor;

use super::support::{
    row_count, setup_race_test, setup_race_test_on_update, InjectedWriter, TxTestResolver,
};
use crate::query::batch::{
    commit_interactive_tx, execute_batch_unchecked as execute_batch,
    execute_in_open_tx_unchecked as execute_in_open_tx, open_interactive_tx,
};
use crate::tx::CommitError;

#[tokio::test]
async fn explicit_snapshot_restrict_race_closed_via_ri_barrier() {
    let (resolver, repo) = setup_race_test(FkAction::Restrict).await;

    // Seed the parent row.
    let mut b = Batch::new();
    b.id(1);
    b.insert(
        "ins_parent",
        write::insert("parent").row(doc().set("id", 1).set("name", "Alice")),
    );
    execute_batch(&b.build(), &resolver, None, None, Actor::System, "test")
        .await
        .unwrap();

    // Arm the injected writer: a concurrent autocommit insert of a NEW child
    // row referencing the SAME parent, fired at the exact after-scan /
    // before-commit seam (resolve_repo ordinal 2).
    let mut wb = Batch::new();
    wb.id("writer");
    wb.insert(
        "ins_child_race",
        write::insert("child").row(doc().set("parent_id", 1).set("label", "race")),
    );
    *resolver.writer.lock().await = Some(InjectedWriter {
        req: wb.build(),
        resolver: TxTestResolver { repo: repo.clone() },
    });

    // Open an EXPLICIT Snapshot transaction. This is the gap F-40 describes:
    // the client-chosen isolation is fixed here, BEFORE query_runner sees any
    // op, so the implicit-only Serializable upgrade never fires.
    let (mut tx, guard) = open_interactive_tx(&repo, shamir_tx::IsolationLevel::Snapshot)
        .await
        .unwrap();

    resolver.reset_counter();

    // Execute the DELETE inside the open Snapshot tx. The RESTRICT scan runs
    // (records the RI barrier token regardless of isolation), then the
    // injected writer fires at resolve_repo #2 (after-scan / before-commit).
    let mut del = Batch::new();
    del.id(2);
    del.try_delete(
        "del_parent",
        write::delete("parent").where_(filter::eq("id", 1)),
    )
    .unwrap();
    let exec_resp = execute_in_open_tx(
        &del.build(),
        &resolver,
        None,
        None,
        &Actor::System,
        "test",
        &mut tx,
    )
    .await;
    assert!(
        exec_resp.is_ok(),
        "RESTRICT scan should pass before the race window opens, got: {exec_resp:?}"
    );

    // Verify the injected writer actually ran (the race genuinely fired).
    assert!(
        resolver.writer.lock().await.is_none(),
        "injected writer must have been consumed (race window must have fired)"
    );

    // Commit the explicit tx. Under the RI barrier, Phase 2-bis now fires for
    // the Snapshot tx (widened guard) and detects the writer's footprint on
    // the child table → PhantomConflict.
    let outcome = commit_interactive_tx(&repo, tx).await;
    drop(guard);

    match outcome {
        Err(CommitError::PhantomConflict { .. }) => { /* expected: barrier caught the race */ }
        Ok(_) => panic!(
            "explicit Snapshot parent delete must NOT commit silently past a \
             racing child insert — this is the exact dangling-reference bug \
             the RI barrier closes"
        ),
        Err(e) => panic!("expected PhantomConflict (RI barrier abort), got: {e:?}"),
    }

    // Invariant: parent still exists (delete aborted), child still exists —
    // no orphan, no dangling reference.
    let parent_table = repo.get_table("parent").await.unwrap();
    let child_table = repo.get_table("child").await.unwrap();
    assert_eq!(
        row_count(&parent_table).await,
        1,
        "parent must still exist post-abort"
    );
    assert_eq!(
        row_count(&child_table).await,
        1,
        "the raced-in child row must still exist post-abort"
    );
}

// ============================================================================
// 2. Quiescent-DB non-regression — no concurrent writer at all.
//
// An explicit-Snapshot FK-parent delete with NO concurrent writer must NOT
// spuriously abort. Mirrors the F-28 Step 3 / 5 spike's 50-trial quiescent
// assertion.
// ============================================================================

#[tokio::test]
async fn quiescent_explicit_snapshot_restrict_delete_does_not_spuriously_abort() {
    let mut spurious_aborts = 0u32;
    for trial in 0..50u32 {
        let (resolver, repo) = setup_race_test(FkAction::Restrict).await;

        let mut b = Batch::new();
        b.id(1);
        b.insert(
            "ins_parent",
            write::insert("parent").row(doc().set("id", 1).set("name", "Alice")),
        );
        execute_batch(&b.build(), &resolver, None, None, Actor::System, "test")
            .await
            .unwrap();

        // No writer armed — the resolver's resolve_repo hook is a no-op.
        let (mut tx, guard) = open_interactive_tx(&repo, shamir_tx::IsolationLevel::Snapshot)
            .await
            .unwrap();
        resolver.reset_counter();

        let mut del = Batch::new();
        del.id(2);
        del.try_delete(
            "del_parent",
            write::delete("parent").where_(filter::eq("id", 1)),
        )
        .unwrap();
        let exec_resp = execute_in_open_tx(
            &del.build(),
            &resolver,
            None,
            None,
            &Actor::System,
            "test",
            &mut tx,
        )
        .await;
        assert!(
            exec_resp.is_ok(),
            "trial {trial}: RESTRICT scan should pass (no child), got: {exec_resp:?}"
        );

        let outcome = commit_interactive_tx(&repo, tx).await;
        drop(guard);
        if outcome.is_err() {
            spurious_aborts += 1;
        }
    }
    assert_eq!(
        spurious_aborts, 0,
        "quiescent explicit-Snapshot FK-parent delete must NOT spuriously abort \
         via the RI barrier: {spurious_aborts}/50 spurious aborts"
    );
}

// ============================================================================
// 3. End-to-end race closure — EXPLICIT Snapshot parent DELETE (CASCADE /
//    SET NULL).
//
// Same harness as the RESTRICT race above; only the FK `on_delete` action
// differs. The cascade / setnull child scan
// (`fk_actions.rs::plan_cascade_recursive`) records the RI barrier token at
// scan entry (Step 2's new recording sites), regardless of isolation. A
// concurrent writer's child insert lands between this tx's snapshot and its
// commit; the barrier's Phase 2-bis re-check catches it → PhantomConflict.
//
// CASCADE uses a referencing writer child (proves the FK-specific orphan
// race); SET NULL uses a non-referencing writer child (see the helper's doc
// for the apply-path visibility mismatch that a referencing child would
// trigger). Both prove the cascade scan's recording site is wired into the
// barrier.
// ============================================================================

/// Shared DELETE-path race harness for CASCADE and SET NULL.
///
/// `writer_parent_id` controls whether the raced-in child references the
/// parent being deleted:
/// - **CASCADE** uses a referencing child (`parent_id = 1`). Its `delete_tx`
///   apply step is id-only (no point-read of the child row), so the plan /
///   apply succeeds and the barrier catches the writer's footprint at commit
///   → `PhantomConflict`. This proves the FK-specific orphan race is closed.
/// - **SET NULL** uses a NON-referencing child (`parent_id = 999`). A
///   referencing child would make the cascade scan (which reads
///   committed-at-now via `list_stream_tx`) see it and plan a SET NULL
///   mutation whose `read_one_tx_bytes` apply step reads at the tx snapshot
///   and can't resolve the raced-in row — a pre-existing apply-path
///   visibility mismatch documented in `fk_race_closure_tests.rs`'s on_update
///   CASCADE test (lines ~982-990), which fails closed *before* the barrier
///   can fire. The non-referencing child still touches the child table, so
///   the barrier's `TableScan { table_token }` dep catches it at Phase 2-bis
///   → `PhantomConflict`, proving the SET NULL scan's recording site
///   (`fk_actions.rs::plan_cascade_recursive`) is wired into the barrier.
///
/// Either way the scan records the RI barrier token and the barrier fires at
/// commit — the two actions exercise the same recording site and guard.
async fn run_explicit_snapshot_delete_race(action: FkAction, writer_parent_id: i64) {
    let (resolver, repo) = setup_race_test(action).await;

    let mut b = Batch::new();
    b.id(1);
    b.insert(
        "ins_parent",
        write::insert("parent").row(doc().set("id", 1).set("name", "Alice")),
    );
    execute_batch(&b.build(), &resolver, None, None, Actor::System, "test")
        .await
        .unwrap();

    // Arm the injected writer: a concurrent autocommit insert of a NEW child
    // row, fired at resolve_repo ordinal 2.
    let mut wb = Batch::new();
    wb.id("writer");
    wb.insert(
        "ins_child_race",
        write::insert("child").row(
            doc()
                .set("parent_id", writer_parent_id)
                .set("label", "race"),
        ),
    );
    *resolver.writer.lock().await = Some(InjectedWriter {
        req: wb.build(),
        resolver: TxTestResolver { repo: repo.clone() },
    });

    let (mut tx, guard) = open_interactive_tx(&repo, shamir_tx::IsolationLevel::Snapshot)
        .await
        .unwrap();
    resolver.reset_counter();

    let mut del = Batch::new();
    del.id(2);
    del.try_delete(
        "del_parent",
        write::delete("parent").where_(filter::eq("id", 1)),
    )
    .unwrap();
    let exec_resp = execute_in_open_tx(
        &del.build(),
        &resolver,
        None,
        None,
        &Actor::System,
        "test",
        &mut tx,
    )
    .await;
    assert!(
        exec_resp.is_ok(),
        "{action:?} scan/plan should pass before the race window opens, got: {exec_resp:?}"
    );
    assert!(
        resolver.writer.lock().await.is_none(),
        "injected writer must have been consumed (race window must have fired)"
    );

    let outcome = commit_interactive_tx(&repo, tx).await;
    drop(guard);

    match outcome {
        Err(CommitError::PhantomConflict { .. }) => { /* expected: barrier caught the race */ }
        Ok(_) => panic!(
            "explicit Snapshot parent delete (on_delete={action:?}) must NOT commit silently \
             past a racing child write — the RI barrier must catch it"
        ),
        Err(e) => panic!("expected PhantomConflict (RI barrier abort), got: {e:?}"),
    }

    // Invariant: parent + raced-in child both still exist post-abort.
    let parent_table = repo.get_table("parent").await.unwrap();
    let child_table = repo.get_table("child").await.unwrap();
    assert_eq!(
        row_count(&parent_table).await,
        1,
        "parent must still exist post-abort"
    );
    assert_eq!(
        row_count(&child_table).await,
        1,
        "raced-in child must still exist post-abort"
    );
}

#[tokio::test]
async fn explicit_snapshot_cascade_race_closed_via_ri_barrier() {
    // Referencing child: CASCADE's id-only delete_tx apply succeeds, so the
    // barrier catches the writer's footprint at commit → PhantomConflict.
    run_explicit_snapshot_delete_race(FkAction::Cascade, 1).await;
}

#[tokio::test]
async fn explicit_snapshot_set_null_race_closed_via_ri_barrier() {
    // Non-referencing child: avoids the apply-path visibility mismatch (see
    // the helper's doc) while still touching the child table so the barrier's
    // TableScan dep fires → PhantomConflict.
    run_explicit_snapshot_delete_race(FkAction::SetNull, 999).await;
}

// ============================================================================
// 4. End-to-end race closure — EXPLICIT Snapshot parent UPDATE (ON UPDATE
//    CASCADE).
//
// The parent UPDATE re-keys the referenced `id` field; a concurrent writer
// inserts a NEW child row. The on-update child scan
// (`fk_on_update.rs::plan_fk_on_update`) records the RI barrier token at
// scan entry (Step 2's new recording site), and Phase 2-bis catches the
// writer's footprint on the child table → PhantomConflict.
//
// The writer inserts a NON-referencing child (parent_id=999): a referencing
// child would make the on-update scan (which reads committed-at-now) see it
// and plan a re-key whose `read_one_tx_bytes` apply step reads at the tx
// snapshot and can't resolve the raced-in row — a pre-existing apply-path
// visibility mismatch documented in `fk_race_closure_tests.rs`'s on_update
// CASCADE test (~lines 982-990), which fails closed *before* the barrier can
// fire. The non-referencing child still touches the child table, so the
// barrier's `TableScan { table_token }` dep catches it at Phase 2-bis,
// proving the on-update scan's recording site is wired into the barrier.
//
// The explicit-tx UPDATE arm has 2 `resolve_repo` calls:
// 1. `require_footprint_if_fk_child` (child-side hook; the parent is not an FK
//    child here, so it is a no-op but still consumes the ordinal).
// 2. `plan_fk_on_update` → `discover_on_update_refs` (after this returns, the
//    child scan runs and records the barrier).
// ============================================================================

#[tokio::test]
async fn explicit_snapshot_on_update_race_closed_via_ri_barrier() {
    let (resolver, repo) = setup_race_test_on_update(FkAction::Cascade).await;

    // Seed the parent row with id=1.
    let mut b = Batch::new();
    b.id(1);
    b.insert(
        "ins_parent",
        write::insert("parent").row(doc().set("id", 1).set("name", "Alice")),
    );
    execute_batch(&b.build(), &resolver, None, None, Actor::System, "test")
        .await
        .unwrap();

    // Arm the injected writer: a concurrent autocommit insert of a NEW child
    // row with a NON-referencing parent_id (999), fired at resolve_repo
    // ordinal 2 (discover_on_update_refs). A referencing child (parent_id=1,
    // the old value) would make the on-update scan (which reads
    // committed-at-now) see it and plan a re-key whose `read_one_tx_bytes`
    // apply step reads at the tx snapshot and can't resolve the raced-in row
    // — a pre-existing apply-path visibility mismatch documented in
    // `fk_race_closure_tests.rs`'s on_update CASCADE test (~lines 982-990).
    // The non-referencing child still touches the child table, so the
    // barrier's `TableScan { table_token }` dep catches it at Phase 2-bis.
    let mut wb = Batch::new();
    wb.id("writer");
    wb.insert(
        "ins_child_race",
        write::insert("child").row(doc().set("parent_id", 999).set("label", "race")),
    );
    *resolver.writer.lock().await = Some(InjectedWriter {
        req: wb.build(),
        resolver: TxTestResolver { repo: repo.clone() },
    });

    let (mut tx, guard) = open_interactive_tx(&repo, shamir_tx::IsolationLevel::Snapshot)
        .await
        .unwrap();
    resolver.reset_counter();

    // Re-key the parent's referenced field from 1 → 2 inside the open tx.
    // The on-update child scan runs, records the RI barrier token (Step 2's
    // new recording site in `plan_fk_on_update`); the injected writer fires
    // at ordinal 2.
    let mut upd = Batch::new();
    upd.id(2);
    upd.try_update(
        "upd_parent",
        write::update("parent")
            .where_(filter::eq("id", 1))
            .set(doc().set("id", 2)),
    )
    .unwrap();
    let exec_resp = execute_in_open_tx(
        &upd.build(),
        &resolver,
        None,
        None,
        &Actor::System,
        "test",
        &mut tx,
    )
    .await;
    assert!(
        exec_resp.is_ok(),
        "on-update scan/plan should pass before the race window opens, got: {exec_resp:?}"
    );
    assert!(
        resolver.writer.lock().await.is_none(),
        "injected writer must have been consumed (race window must have fired)"
    );

    let outcome = commit_interactive_tx(&repo, tx).await;
    drop(guard);

    match outcome {
        Err(CommitError::PhantomConflict { .. }) => { /* expected: barrier caught the race */ }
        Ok(_) => panic!(
            "explicit Snapshot parent update (on_update=Cascade) must NOT commit silently \
             past a racing child write — the RI barrier must catch it"
        ),
        Err(e) => panic!("expected PhantomConflict (RI barrier abort), got: {e:?}"),
    }

    // Invariant: parent still exists with id=1 (update aborted), raced-in
    // child still exists — no silent commit.
    let parent_table = repo.get_table("parent").await.unwrap();
    let child_table = repo.get_table("child").await.unwrap();
    assert_eq!(
        row_count(&parent_table).await,
        1,
        "parent must still exist post-abort"
    );
    assert_eq!(
        row_count(&child_table).await,
        1,
        "raced-in child must still exist post-abort"
    );
}

// ============================================================================
// 5. Quiescent-DB non-regression for the 3 new actions (CASCADE / SET NULL /
//    ON UPDATE).
//
// No concurrent writer at all. An explicit-Snapshot FK-parent delete (CASCADE
// / SET NULL) or update (ON UPDATE CASCADE) with an empty child table must
// NOT spuriously abort via the RI barrier. Mirrors the RESTRICT quiescent
// trial above (test 2).
// ============================================================================

#[tokio::test]
async fn quiescent_explicit_snapshot_cascade_delete_does_not_spuriously_abort() {
    let mut spurious_aborts = 0u32;
    for trial in 0..30u32 {
        let (resolver, repo) = setup_race_test(FkAction::Cascade).await;

        let mut b = Batch::new();
        b.id(1);
        b.insert(
            "ins_parent",
            write::insert("parent").row(doc().set("id", 1).set("name", "Alice")),
        );
        execute_batch(&b.build(), &resolver, None, None, Actor::System, "test")
            .await
            .unwrap();

        let (mut tx, guard) = open_interactive_tx(&repo, shamir_tx::IsolationLevel::Snapshot)
            .await
            .unwrap();
        resolver.reset_counter();

        let mut del = Batch::new();
        del.id(2);
        del.try_delete(
            "del_parent",
            write::delete("parent").where_(filter::eq("id", 1)),
        )
        .unwrap();
        let exec_resp = execute_in_open_tx(
            &del.build(),
            &resolver,
            None,
            None,
            &Actor::System,
            "test",
            &mut tx,
        )
        .await;
        assert!(
            exec_resp.is_ok(),
            "trial {trial}: CASCADE scan should pass (no child), got: {exec_resp:?}"
        );

        let outcome = commit_interactive_tx(&repo, tx).await;
        drop(guard);
        if outcome.is_err() {
            spurious_aborts += 1;
        }
    }
    assert_eq!(
        spurious_aborts, 0,
        "quiescent explicit-Snapshot CASCADE delete must NOT spuriously abort: \
         {spurious_aborts}/30 spurious aborts"
    );
}

#[tokio::test]
async fn quiescent_explicit_snapshot_set_null_delete_does_not_spuriously_abort() {
    let mut spurious_aborts = 0u32;
    for trial in 0..30u32 {
        let (resolver, repo) = setup_race_test(FkAction::SetNull).await;

        let mut b = Batch::new();
        b.id(1);
        b.insert(
            "ins_parent",
            write::insert("parent").row(doc().set("id", 1).set("name", "Alice")),
        );
        execute_batch(&b.build(), &resolver, None, None, Actor::System, "test")
            .await
            .unwrap();

        let (mut tx, guard) = open_interactive_tx(&repo, shamir_tx::IsolationLevel::Snapshot)
            .await
            .unwrap();
        resolver.reset_counter();

        let mut del = Batch::new();
        del.id(2);
        del.try_delete(
            "del_parent",
            write::delete("parent").where_(filter::eq("id", 1)),
        )
        .unwrap();
        let exec_resp = execute_in_open_tx(
            &del.build(),
            &resolver,
            None,
            None,
            &Actor::System,
            "test",
            &mut tx,
        )
        .await;
        assert!(
            exec_resp.is_ok(),
            "trial {trial}: SET NULL scan should pass (no child), got: {exec_resp:?}"
        );

        let outcome = commit_interactive_tx(&repo, tx).await;
        drop(guard);
        if outcome.is_err() {
            spurious_aborts += 1;
        }
    }
    assert_eq!(
        spurious_aborts, 0,
        "quiescent explicit-Snapshot SET NULL delete must NOT spuriously abort: \
         {spurious_aborts}/30 spurious aborts"
    );
}

#[tokio::test]
async fn quiescent_explicit_snapshot_on_update_does_not_spuriously_abort() {
    let mut spurious_aborts = 0u32;
    for trial in 0..30u32 {
        let (resolver, repo) = setup_race_test_on_update(FkAction::Cascade).await;

        let mut b = Batch::new();
        b.id(1);
        b.insert(
            "ins_parent",
            write::insert("parent").row(doc().set("id", 1).set("name", "Alice")),
        );
        execute_batch(&b.build(), &resolver, None, None, Actor::System, "test")
            .await
            .unwrap();

        let (mut tx, guard) = open_interactive_tx(&repo, shamir_tx::IsolationLevel::Snapshot)
            .await
            .unwrap();
        resolver.reset_counter();

        // Re-key the parent id 1 → 2. No child rows at snapshot → empty
        // fan-out plan → the update should commit cleanly.
        let mut upd = Batch::new();
        upd.id(2);
        upd.try_update(
            "upd_parent",
            write::update("parent")
                .where_(filter::eq("id", 1))
                .set(doc().set("id", 2)),
        )
        .unwrap();
        let exec_resp = execute_in_open_tx(
            &upd.build(),
            &resolver,
            None,
            None,
            &Actor::System,
            "test",
            &mut tx,
        )
        .await;
        assert!(
            exec_resp.is_ok(),
            "trial {trial}: on-update scan should pass (no child), got: {exec_resp:?}"
        );

        let outcome = commit_interactive_tx(&repo, tx).await;
        drop(guard);
        if outcome.is_err() {
            spurious_aborts += 1;
        }
    }
    assert_eq!(
        spurious_aborts, 0,
        "quiescent explicit-Snapshot ON UPDATE must NOT spuriously abort: \
         {spurious_aborts}/30 spurious aborts"
    );
}

// ============================================================================
// 6. F-46 (#857, P0) — mutual commit-lock serialization for a CONCURRENT
//    PUBLISH inside the parent's own commit-lock window (not
//    before-parent-commit like the tests above).
//
// The 2026-07-28 readonly review's P0-1 finding: `commit_tx_lockfree`'s
// commit-lock condition checks `ri_barrier_tokens` but not
// `footprint_tokens`. A concurrent Snapshot writer into an FK-child table
// only ever gets `footprint_tokens` populated (never `ri_barrier_tokens`),
// so on the UNFIXED code it never takes `gate.commit_lock()` — it can
// publish (WAL write + `record_commit_writes` + materialize) WHILE the
// parent is holding `commit_lock` and has already passed its own Phase 2-bis
// `predicate_conflicts_batch` re-check. Neither side then detects the other:
// the parent's RESTRICT/CASCADE/SET NULL/ON UPDATE decision was made against
// a child-table state a concurrent writer subsequently changed.
//
// This is a DIFFERENT interleaving than tests 1-5 above: those inject the
// concurrent writer at a `resolve_repo()` call ordinal, which fires and
// fully resolves BEFORE the parent even calls `commit_interactive_tx` — a
// backward-looking recheck only. This harness instead uses the new
// `crate::tx::commit::TEST_POST_VALIDATE_PRE_PUBLISH_HOOK` seam
// (`commit.rs`), which parks the parent's OWN commit task strictly AFTER its
// Phase 2-bis check has passed and strictly BEFORE its Phase 4 WAL/publish —
// i.e. genuinely DURING the parent's commit-lock window, while the parent
// still holds `commit_lock`.
//
// Determinism (no sleeps, no wall-clock races): the parent's commit runs on
// a spawned task and parks at the hook; the harness busy-yields (via
// `tokio::task::yield_now`, not `sleep`) until `hook.reached` confirms the
// park. It then spawns the concurrent child-table writer and busy-yields a
// bounded number of times. `tokio::sync::Mutex` (what `commit_lock` is) can
// only complete a `.lock().await` waiter when the current holder's guard
// drops — it never spuriously wakes — so after enough yields with no
// progress, "the writer has not finished" is a PROOF that it is genuinely
// parked on `commit_lock`, not merely "still running": on the UNFIXED code
// the writer never takes that lock at all, so it always finishes within a
// handful of yields (no contention to wait out). This is the same
// no-sleeps-just-yield idiom `fk_reverse_cache_race_tests.rs` uses for its
// own build-pause handshake.
//
// nextest runs each test in its own process, so the global
// `TEST_POST_VALIDATE_PRE_PUBLISH_HOOK` can be `.set()` at most once per test
// function without leaking across tests in this file.
// ============================================================================
