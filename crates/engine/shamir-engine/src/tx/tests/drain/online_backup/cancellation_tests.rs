//! Cancellation at every barrier await point (brief item 8, test 4): the
//! barrier future is dropped EXACTLY at a named step via a deterministic
//! unwind — the `set_online_backup_step_hook` seam (a plain
//! `fn(&'static str)`) is armed per-test through THREAD-LOCAL state, and
//! the armed fire panics with a sentinel payload inside the barrier task.
//! The panic payload recovered from the `JoinHandle` proves the future
//! was mid-flight AT that step (no vacuous abort of an already-completed
//! task, and no cross-test arm: each test runs on its own thread under
//! the current-thread test runtime). After EVERY drop: the gate is open
//! with zero leases, the pause is released, and fresh capture AND seal
//! barriers succeed (no leaked close owner, no leaked hold). No sleeps —
//! the unwind IS the handshake; every await is bounded.

use std::cell::RefCell;
use std::time::Duration;

use shamir_tx::IsolationLevel;
use shamir_types::types::value::InnerValue;

use crate::repo::RepoInstance;
use crate::table::TableConfig;
use crate::tx::tests::drain::drainer_tests::support::make_repo;

/// Harness cap around each bounded await; the synchronization itself is
/// the hook fire / the abort join.
const CANCEL_BUDGET: Duration = Duration::from_secs(30);

/// Every step the barrier hook fires, in barrier order.
const ALL_STEPS: [&str; 5] = [
    "after_close",
    "after_quiesce",
    "after_drain",
    "after_pause",
    "after_recheck",
];

/// Marker inside the unwind panic payload.
const SENTINEL: &str = "step-hook unwind sentinel";

/// Per-thread hook state. Tests under `#[tokio::test]` (current-thread
/// runtime) run the whole barrier task on their own thread, so the armed
/// step and the recorded steps are isolated per test even though the
/// production hook slot is a process-global `OnceLock` (first installer
/// wins; every test here installs the same `step_hook` fn).
#[derive(Default)]
struct HookState {
    /// One-shot: the step whose next fire unwinds the barrier task.
    armed: Option<&'static str>,
    /// Every step this thread's barriers fired.
    recorded: Vec<&'static str>,
}

thread_local! {
    static STEP_HOOK_STATE: RefCell<HookState> = RefCell::new(HookState::default());
}

fn step_hook(step: &'static str) {
    STEP_HOOK_STATE.with(|state| {
        let mut state = state.borrow_mut();
        state.recorded.push(step);
        if state.armed == Some(step) {
            state.armed = None;
            panic!("{SENTINEL}: {step}");
        }
    });
}

fn arm_step(step: &'static str) {
    STEP_HOOK_STATE.with(|state| {
        let mut state = state.borrow_mut();
        assert!(
            state.armed.is_none(),
            "a step is already armed on this thread"
        );
        state.armed = Some(step);
    });
}

async fn fixture_with_committed_data() -> RepoInstance {
    let repo = make_repo();
    repo.add_table_for_test(TableConfig::new("t"));
    let table = repo.get_table("t").await.unwrap();
    let (mut tx, _guard) = repo.begin_tx(IsolationLevel::Snapshot).await.unwrap();
    table
        .insert_tx(&InnerValue::Str("cancellation-seed".into()), Some(&mut tx))
        .await
        .unwrap();
    repo.commit_tx(tx).await.unwrap();
    repo
}

/// Drive the barrier to the armed step (a deterministic unwind there),
/// then assert the fail-closed invariants AND that fresh barriers
/// succeed. The panic payload proves the future was dropped mid-flight at
/// exactly the armed step.
async fn cancel_barrier_at(repo: &RepoInstance, step: &'static str, seal: bool) {
    RepoInstance::set_online_backup_step_hook(step_hook);
    arm_step(step);
    let owned = repo.clone();
    let task = if seal {
        tokio::spawn(Box::pin(
            async move { owned.seal_point().await.map(|_| ()) },
        ))
    } else {
        tokio::spawn(Box::pin(
            async move { owned.capture_point().await.map(|_| ()) },
        ))
    };
    let outcome = tokio::time::timeout(CANCEL_BUDGET, task)
        .await
        .expect("the barrier never reached the armed step")
        .expect_err("the barrier must unwind at the armed step, not complete");
    assert!(
        outcome.is_panic(),
        "the barrier completed before the armed step fired"
    );
    let payload = outcome.into_panic();
    let expected = format!("{SENTINEL}: {step}");
    let message = payload
        .downcast_ref::<String>()
        .map(String::as_str)
        .or_else(|| payload.downcast_ref::<&'static str>().copied());
    assert_eq!(
        message,
        Some(expected.as_str()),
        "the unwind must have fired exactly the armed step"
    );

    let admission = repo.core().commit_admission();
    assert!(!admission.is_closed(), "gate must reopen after the drop");
    assert_eq!(admission.active_count(), 0, "no leaked admission lease");
    assert!(
        !repo.drainer().pause_held(),
        "pause released after the drop"
    );

    // No leaked close owner / hold: both barriers complete cleanly.
    let position = tokio::time::timeout(CANCEL_BUDGET, repo.capture_point())
        .await
        .expect("fresh capture completes after cancellation")
        .expect("fresh capture succeeds after cancellation");
    assert_eq!(position, repo.tx_gate().await.unwrap().last_committed());
    let hold = tokio::time::timeout(CANCEL_BUDGET, repo.seal_point())
        .await
        .expect("fresh seal completes after cancellation")
        .expect("fresh seal succeeds after cancellation");
    drop(hold);
    assert!(!repo.drainer().pause_held());
}

#[tokio::test]
async fn capture_cancelled_after_close_leaves_no_owner() {
    let repo = fixture_with_committed_data().await;
    cancel_barrier_at(&repo, "after_close", false).await;
}

#[tokio::test]
async fn capture_cancelled_after_quiesce_leaves_no_owner() {
    let repo = fixture_with_committed_data().await;
    cancel_barrier_at(&repo, "after_quiesce", false).await;
}

#[tokio::test]
async fn seal_cancelled_after_drain_leaves_no_owner_or_pause() {
    let repo = fixture_with_committed_data().await;
    cancel_barrier_at(&repo, "after_drain", true).await;
}

#[tokio::test]
async fn seal_cancelled_after_pause_releases_the_pause() {
    let repo = fixture_with_committed_data().await;
    cancel_barrier_at(&repo, "after_pause", true).await;
}

#[tokio::test]
async fn seal_cancelled_after_recheck_releases_the_pause() {
    let repo = fixture_with_committed_data().await;
    cancel_barrier_at(&repo, "after_recheck", true).await;
}

/// The hold itself lives in a task: while the hold is alive BOTH gates
/// are held (admission closed, drain paused); aborting the owner task
/// unwinds the hold — pause released FIRST, gate reopened LAST — and a
/// fresh writer commits and both fresh barriers succeed.
#[tokio::test]
async fn hold_owning_task_abort_releases_both_gates() {
    let repo = fixture_with_committed_data().await;
    let (got_hold_tx, got_hold_rx) = tokio::sync::oneshot::channel();
    let owned = repo.clone();
    let task = tokio::spawn(Box::pin(async move {
        let hold = owned.seal_point().await.expect("seal in the owner task");
        got_hold_tx.send(()).expect("hold handoff");
        std::future::pending::<()>().await;
        drop(hold);
    }));
    tokio::time::timeout(CANCEL_BUDGET, got_hold_rx)
        .await
        .expect("the owner task never obtained the hold")
        .expect("hold handoff");

    // While the hold lives inside the task, the hold keeps BOTH gates.
    let admission = repo.core().commit_admission();
    assert!(admission.is_closed(), "the hold keeps the gate closed");
    assert!(
        repo.drainer().pause_held(),
        "the hold keeps the drain paused"
    );

    // Bounded abort: the unwind drops the hold (pause FIRST, gate LAST).
    task.abort();
    let outcome = tokio::time::timeout(CANCEL_BUDGET, task)
        .await
        .expect("the abort join hung")
        .expect_err("abort join");
    assert!(outcome.is_cancelled(), "the abort must cancel the task");
    assert!(!repo.drainer().pause_held(), "unwind released the pause");
    assert!(!admission.is_closed(), "unwind reopened the gate");

    // Fresh writer succeeds once both gates are open.
    let table = repo.get_table("t").await.unwrap();
    let (mut tx, _guard) = repo.begin_tx(IsolationLevel::Snapshot).await.unwrap();
    let id = table
        .insert_tx(&InnerValue::Str("post-abort".into()), Some(&mut tx))
        .await
        .unwrap();
    repo.commit_tx(tx).await.unwrap();
    let got = table.get(id).await.unwrap();
    assert!(
        matches!(got, InnerValue::Str(ref s) if s == "post-abort"),
        "fresh writer data readable after the abort, got {got:?}"
    );

    // Fresh barriers succeed: no leaked close owner, no leaked hold.
    let position = tokio::time::timeout(CANCEL_BUDGET, repo.capture_point())
        .await
        .expect("fresh capture completes after the abort")
        .expect("fresh capture succeeds after the abort");
    assert_eq!(position, repo.tx_gate().await.unwrap().last_committed());
    let hold = tokio::time::timeout(CANCEL_BUDGET, repo.seal_point())
        .await
        .expect("fresh seal completes after the abort")
        .expect("fresh seal succeeds after the abort");
    drop(hold);
    assert!(!repo.drainer().pause_held());
}

/// Sanity anchor: the hook really fired every named step across the
/// barriers run on this thread (the armed handoffs were not silently
/// short-circuited by barrier completion before the armed step).
#[tokio::test]
async fn step_hook_fires_every_named_barrier_step() {
    RepoInstance::set_online_backup_step_hook(step_hook);
    let repo = fixture_with_committed_data().await;
    repo.capture_point().await.expect("capture");
    let hold = repo.seal_point().await.expect("seal");
    drop(hold);

    let recorded = STEP_HOOK_STATE.with(|state| state.borrow().recorded.clone());
    for expected in ALL_STEPS {
        assert!(
            recorded.contains(&expected),
            "step {expected} never fired; recorded {recorded:?}"
        );
    }
}
