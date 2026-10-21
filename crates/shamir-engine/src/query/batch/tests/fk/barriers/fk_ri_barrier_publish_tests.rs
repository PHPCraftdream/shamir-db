use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use shamir_query_builder::batch::Batch;
use shamir_query_builder::filter;
use shamir_query_builder::write;
use shamir_query_builder::write::doc;
use shamir_query_types::admin::FkAction;
use shamir_query_types::batch::BatchError;
use shamir_types::access::Actor;

use super::support::{row_count, setup_race_test, setup_race_test_on_update, TxTestResolver};
use crate::query::batch::{
    commit_interactive_tx, execute_batch_unchecked as execute_batch,
    execute_in_open_tx_unchecked as execute_in_open_tx, open_interactive_tx,
};
use crate::tx::CommitError;

use crate::tx::commit::{PostValidatePrePublishHook, TEST_POST_VALIDATE_PRE_PUBLISH_HOOK};

/// Bounded yield budget for "prove the writer is genuinely parked on
/// `commit_lock`, not just slow". Generous enough that any writer NOT
/// contending on the lock finishes trivially within a handful of
/// cooperative yields on a single-threaded-per-task async executor: an
/// autocommit single-row insert with no other work does at most a few
/// `.await` points, each a `yield_now`-equivalent resumption. A writer that
/// IS blocked on `tokio::sync::Mutex::lock().await` cannot make ANY progress
/// in this loop no matter how many iterations, because that mutex only
/// wakes a waiter when the guard holder drops — never spuriously — so this
/// budget cannot produce a false "still blocked" reading due to scheduler
/// slowness.
const BLOCKED_PROBE_YIELDS: usize = 500;

/// Busy-yield (cooperative, no sleeps) until `hook.reached` shows the
/// commit-under-test has parked at the seam, or panic past a generous
/// iteration budget (would indicate the seam was never reached — a harness
/// bug, not a timing flake, since reaching the hook requires no external
/// event).
async fn wait_for_hook_reached(hook: &PostValidatePrePublishHook) {
    for _ in 0..100_000usize {
        if hook.reached.load(Ordering::SeqCst) >= 1 {
            return;
        }
        tokio::task::yield_now().await;
    }
    panic!("commit-under-test never reached TEST_POST_VALIDATE_PRE_PUBLISH_HOOK");
}

/// Outcome of one concurrent-publish-during-commit-window trial.
struct ConcurrentPublishTrial {
    parent_outcome: Result<crate::tx::TxOutcome, CommitError>,
    writer_finished_before_release: bool,
    writer_outcome: Result<shamir_query_types::batch::BatchResponse, BatchError>,
    parent_row_count: u64,
    child_row_count: u64,
}

/// Runs the concurrent-publish-during-parent-commit-lock-window race for one
/// FK on_delete `action` (RESTRICT / CASCADE / SET NULL), against whatever
/// engine code is live when this test runs.
async fn run_concurrent_publish_during_commit_window_delete(
    action: FkAction,
) -> ConcurrentPublishTrial {
    let (resolver, repo) = setup_race_test(action).await;

    // Seed the parent row. No concurrent writer is armed on `resolver` here —
    // this harness drives the concurrent writer itself, via the new seam.
    let mut b = Batch::new();
    b.id(1);
    b.insert(
        "ins_parent",
        write::insert("parent").row(doc().set("id", 1).set("name", "Alice")),
    );
    execute_batch(&b.build(), &resolver, None, None, Actor::System, "test")
        .await
        .unwrap();

    // Install the post-validate/pre-publish pause hook.
    let hook = Arc::new(PostValidatePrePublishHook {
        reached: AtomicUsize::new(0),
        resume: tokio::sync::Notify::new(),
        armed: std::sync::atomic::AtomicBool::new(true),
    });
    TEST_POST_VALIDATE_PRE_PUBLISH_HOOK
        .set(Arc::clone(&hook))
        .expect("hook installed exactly once per test process (nextest: one process per test)");

    // Open the explicit Snapshot tx and run the RESTRICT/CASCADE/SET NULL
    // scan. No concurrent writer has touched the child table yet, so the
    // scan legitimately finds no reference (or plans against zero fan-out)
    // and records the RI barrier token.
    let (mut tx, guard) = open_interactive_tx(&repo, shamir_tx::IsolationLevel::Snapshot)
        .await
        .unwrap();
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
        "{action:?} scan/plan should pass (no concurrent writer yet), got: {exec_resp:?}"
    );

    // Sanity: the repo's gate is a real handle we can probe non-blockingly.
    let gate = repo.tx_gate().await.unwrap();

    // Drive the parent's commit on a spawned task: it will run Phase 2-bis
    // (pass — nothing conflicts yet) under `commit_lock`, then park at the
    // hook, STILL HOLDING `commit_lock`.
    let repo_for_parent = repo.clone();
    let parent_task =
        tokio::spawn(async move { commit_interactive_tx(&repo_for_parent, tx).await });

    wait_for_hook_reached(&hook).await;

    // While the parent is parked mid-commit-lock-window, the gate must be
    // genuinely held (non-blocking probe fails) — confirms this harness is
    // actually testing the claimed interleaving, not a no-op.
    assert!(
        gate.try_commit_lock().is_none(),
        "parent must still hold commit_lock while parked at the post-validate/pre-publish hook"
    );

    // Fire the concurrent writer: a plain autocommit INSERT of a NEW child
    // row referencing the same parent, via the SAME live repo. On the
    // UNFIXED code this writer has only `footprint_tokens` set (never
    // `ri_barrier_tokens`), so it does NOT await `commit_lock` and races
    // straight through to a full commit while the parent is still parked. On
    // the FIXED code (footprint_tokens widened into the lock condition) it
    // blocks on `commit_lock` until the parent releases it.
    let mut wb = Batch::new();
    wb.id("writer");
    wb.insert(
        "ins_child_race",
        write::insert("child").row(doc().set("parent_id", 1).set("label", "race")),
    );
    let writer_resolver = TxTestResolver { repo: repo.clone() };
    let writer_task = tokio::spawn(async move {
        execute_batch(
            &wb.build(),
            &writer_resolver,
            None,
            None,
            Actor::System,
            "test",
        )
        .await
    });

    // Busy-yield a bounded, generous budget. If the writer is NOT contending
    // on commit_lock (the bug) it completes trivially inside this budget; if
    // it IS blocked on commit_lock (the fix) it can NEVER complete during
    // this loop, by the non-spurious-wake property of `tokio::sync::Mutex`.
    for _ in 0..BLOCKED_PROBE_YIELDS {
        tokio::task::yield_now().await;
    }
    let writer_finished_before_release = writer_task.is_finished();

    // Release the parent from the hook. `armed` is one-shot (see
    // `PostValidatePrePublishHook`'s doc): only the parent's arrival at the
    // seam consumed it, so when the writer reaches the SAME program point
    // after acquiring `commit_lock` in turn, it passes straight through —
    // a single `notify_one()` here is sufficient for the whole trial.
    hook.resume.notify_one();

    let parent_outcome = parent_task.await.unwrap();
    drop(guard);
    let writer_outcome = writer_task.await.unwrap();

    let parent_table = repo.get_table("parent").await.unwrap();
    let child_table = repo.get_table("child").await.unwrap();
    ConcurrentPublishTrial {
        parent_outcome,
        writer_finished_before_release,
        writer_outcome,
        parent_row_count: row_count(&parent_table).await,
        child_row_count: row_count(&child_table).await,
    }
}

/// RED proof (on the code as it stood before this task's fix): the
/// concurrent child writer completes its FULL commit (WAL + publish) WHILE
/// the parent is still parked mid-commit-lock-window — i.e.
/// `writer_finished_before_release` is `true`. This is the exact defeat of
/// the RI barrier's mutual-exclusion intent the 2026-07-28 review's P0-1
/// describes: the parent's `predicate_conflicts_batch` check already passed
/// against a child-table state that changed before the parent published, so
/// RESTRICT can allow a delete despite a concurrently-inserted child, or
/// CASCADE/SET NULL can miss the concurrently-inserted row entirely.
///
/// GREEN proof (after the fix — widening `commit_tx_lockfree`'s lock
/// condition to also check `!tx.footprint_tokens.is_empty()`): the writer's
/// Snapshot insert into the FK-child table also takes `commit_lock`, so it
/// CANNOT complete while the parent holds it — `writer_finished_before_release`
/// must be `false`, and the two commits are strictly ordered (parent first,
/// since it took the lock first): both succeed, and the raced-in child row
/// genuinely did not exist at the time the parent's RESTRICT/CASCADE/SET NULL
/// decision was made.
///
/// One `#[tokio::test]` PER action, not a `for action in [...]` loop inside a
/// single test — `TEST_POST_VALIDATE_PRE_PUBLISH_HOOK` is a process-wide
/// `OnceLock` that can be `.set()` exactly once per process. nextest gives
/// each TEST FUNCTION its own process, so looping 3 actions through
/// `run_concurrent_publish_during_commit_window_delete` (which calls
/// `.set()` every call) inside one `#[tokio::test]` would panic on the
/// second iteration's `.set()` — matches the established per-action-function
/// pattern this file already uses for the other multi-action race tests
/// (`explicit_snapshot_cascade_race_closed_via_ri_barrier` /
/// `..._set_null_...` above, each its own `#[tokio::test]`).
fn assert_concurrent_publish_trial(action: FkAction, trial: &ConcurrentPublishTrial) {
    assert!(
        !trial.writer_finished_before_release,
        "{action:?}: the concurrent FK-child writer must NOT be able to complete its \
         commit while the parent still holds commit_lock mid-validate/pre-publish window — \
         a writer finishing here means it never took commit_lock at all, defeating the RI \
         barrier's mutual-exclusion intent (2026-07-28 review P0-1)"
    );

    assert!(
        trial.parent_outcome.is_ok(),
        "{action:?}: with the writer genuinely serialized behind commit_lock, the parent's \
         own RI barrier check (which already passed before the writer could touch the child \
         table) must be allowed to commit cleanly, got: {:?}",
        trial.parent_outcome
    );
    assert!(
        trial.writer_outcome.is_ok(),
        "{action:?}: the writer must commit successfully once the parent releases \
         commit_lock, got: {:?}",
        trial.writer_outcome
    );

    // CASCADE deletes any pre-existing referencing child rows (there are
    // none here — the raced-in row lands AFTER the scan), so the parent
    // table always ends up empty for CASCADE regardless of ordering.
    // RESTRICT/SET NULL never remove the parent's own row via this path
    // in a way that depends on the race, so this assertion is about the
    // COMMIT outcome, not row survival — the row-count invariants below
    // are the correctness statement that matters.
    assert_eq!(
        trial.parent_row_count, 0,
        "{action:?}: parent delete committed"
    );
    assert_eq!(
        trial.child_row_count, 1,
        "{action:?}: the raced-in child row must exist post-commit — it was inserted \
         strictly AFTER the parent's RESTRICT/CASCADE/SET NULL decision was made, so it \
         correctly survives untouched by that decision"
    );
}

#[tokio::test]
async fn concurrent_child_publish_during_parent_commit_window_is_serialized_on_delete_restrict() {
    let trial = run_concurrent_publish_during_commit_window_delete(FkAction::Restrict).await;
    assert_concurrent_publish_trial(FkAction::Restrict, &trial);
}

#[tokio::test]
async fn concurrent_child_publish_during_parent_commit_window_is_serialized_on_delete_cascade() {
    let trial = run_concurrent_publish_during_commit_window_delete(FkAction::Cascade).await;
    assert_concurrent_publish_trial(FkAction::Cascade, &trial);
}

#[tokio::test]
async fn concurrent_child_publish_during_parent_commit_window_is_serialized_on_delete_set_null() {
    let trial = run_concurrent_publish_during_commit_window_delete(FkAction::SetNull).await;
    assert_concurrent_publish_trial(FkAction::SetNull, &trial);
}

// ============================================================================
// 7. Same concurrent-publish-during-commit-window race, for the ON UPDATE
//    CASCADE path (`fk_on_update.rs::plan_fk_on_update`'s recording site).
// ============================================================================

async fn run_concurrent_publish_during_commit_window_on_update() -> ConcurrentPublishTrial {
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

    let hook = Arc::new(PostValidatePrePublishHook {
        reached: AtomicUsize::new(0),
        resume: tokio::sync::Notify::new(),
        armed: std::sync::atomic::AtomicBool::new(true),
    });
    TEST_POST_VALIDATE_PRE_PUBLISH_HOOK
        .set(Arc::clone(&hook))
        .expect("hook installed exactly once per test process (nextest: one process per test)");

    let (mut tx, guard) = open_interactive_tx(&repo, shamir_tx::IsolationLevel::Snapshot)
        .await
        .unwrap();

    // Re-key the parent's referenced field 1 → 2. No child rows exist yet,
    // so the on-update scan records the RI barrier token against an empty
    // fan-out and passes.
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
        "on-update scan/plan should pass (no concurrent writer yet), got: {exec_resp:?}"
    );

    let gate = repo.tx_gate().await.unwrap();

    let repo_for_parent = repo.clone();
    let parent_task =
        tokio::spawn(async move { commit_interactive_tx(&repo_for_parent, tx).await });

    wait_for_hook_reached(&hook).await;

    assert!(
        gate.try_commit_lock().is_none(),
        "parent must still hold commit_lock while parked at the post-validate/pre-publish hook"
    );

    // Concurrent writer: a NON-referencing child row (parent_id=999) — a
    // referencing child (parent_id=1, the OLD value) would make the
    // on-update scan's apply step hit the pre-existing
    // read_one_tx_bytes-at-snapshot visibility mismatch documented earlier in
    // this file, which fails closed *before* the barrier can fire; the
    // non-referencing child still exercises the SAME recording site + guard
    // (it touches the child table, so the `TableScan { table_token }` dep is
    // what's under test here, not row-level FK matching).
    let mut wb = Batch::new();
    wb.id("writer");
    wb.insert(
        "ins_child_race",
        write::insert("child").row(doc().set("parent_id", 999).set("label", "race")),
    );
    let writer_resolver = TxTestResolver { repo: repo.clone() };
    let writer_task = tokio::spawn(async move {
        execute_batch(
            &wb.build(),
            &writer_resolver,
            None,
            None,
            Actor::System,
            "test",
        )
        .await
    });

    for _ in 0..BLOCKED_PROBE_YIELDS {
        tokio::task::yield_now().await;
    }
    let writer_finished_before_release = writer_task.is_finished();

    hook.resume.notify_one();

    let parent_outcome = parent_task.await.unwrap();
    drop(guard);
    let writer_outcome = writer_task.await.unwrap();

    let parent_table = repo.get_table("parent").await.unwrap();
    let child_table = repo.get_table("child").await.unwrap();
    ConcurrentPublishTrial {
        parent_outcome,
        writer_finished_before_release,
        writer_outcome,
        parent_row_count: row_count(&parent_table).await,
        child_row_count: row_count(&child_table).await,
    }
}

#[tokio::test]
async fn concurrent_child_publish_during_parent_commit_window_is_serialized_on_update() {
    let trial = run_concurrent_publish_during_commit_window_on_update().await;

    assert!(
        !trial.writer_finished_before_release,
        "ON UPDATE CASCADE: the concurrent FK-child writer must NOT be able to complete its \
         commit while the parent still holds commit_lock mid-validate/pre-publish window"
    );
    assert!(
        trial.parent_outcome.is_ok(),
        "ON UPDATE CASCADE: parent must commit cleanly once genuinely serialized, got: {:?}",
        trial.parent_outcome
    );
    assert!(
        trial.writer_outcome.is_ok(),
        "ON UPDATE CASCADE: writer must commit successfully once released, got: {:?}",
        trial.writer_outcome
    );
    assert_eq!(
        trial.parent_row_count, 1,
        "parent row (re-keyed to id=2) must still exist post-commit"
    );
    assert_eq!(
        trial.child_row_count, 1,
        "the raced-in non-referencing child row must exist post-commit"
    );
}
