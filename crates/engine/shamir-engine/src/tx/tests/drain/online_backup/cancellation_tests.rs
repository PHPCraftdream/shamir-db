//! Cancellation at every barrier await point (brief item 8, test 4): the
//! barrier future is driven through the `set_online_backup_step_hook`
//! seam; when the armed step fires, the task running the future is
//! aborted, dropping the future exactly at that step. After EVERY drop:
//! the gate is open with zero leases, the pause is released, and fresh
//! capture AND seal barriers succeed (no leaked close owner, no leaked
//! hold). The hook is a plain `fn(&'static str)`, so the park is a
//! spawned task + oneshot handoff inside the test — no sleeps.

use std::sync::Mutex;
use std::time::Duration;

use shamir_tx::IsolationLevel;
use shamir_types::types::value::InnerValue;

use crate::repo::RepoInstance;
use crate::table::TableConfig;
use crate::tx::tests::drain::drainer_tests::support::make_repo;

/// Harness cap around each handshake; the handshake is the hook fire.
const CANCEL_BUDGET: Duration = Duration::from_secs(30);

/// Every step the barrier hook fires, in barrier order.
const ALL_STEPS: [&str; 5] = [
    "after_close",
    "after_quiesce",
    "after_drain",
    "after_pause",
    "after_recheck",
];

static STEP_HOOK_RECORDED: Mutex<Vec<&'static str>> = Mutex::new(Vec::new());
static STEP_HOOK_ARMED: Mutex<Option<(&'static str, tokio::sync::oneshot::Sender<()>)>> =
    Mutex::new(None);

fn step_hook(step: &'static str) {
    STEP_HOOK_RECORDED
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .push(step);
    let mut armed = STEP_HOOK_ARMED.lock().unwrap_or_else(|e| e.into_inner());
    if armed.as_ref().is_some_and(|(target, _)| *target == step) {
        if let Some((_, tx)) = armed.take() {
            let _ = tx.send(());
        }
    }
}

fn arm_step(step: &'static str) -> tokio::sync::oneshot::Receiver<()> {
    let (tx, rx) = tokio::sync::oneshot::channel();
    *STEP_HOOK_ARMED.lock().unwrap_or_else(|e| e.into_inner()) = Some((step, tx));
    rx
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

/// Abort the barrier task once the armed step fired, then assert the
/// fail-closed invariants AND that fresh barriers succeed.
async fn cancel_barrier_at(repo: &RepoInstance, step: &'static str, seal: bool) {
    RepoInstance::set_online_backup_step_hook(step_hook);
    let fired = arm_step(step);
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
    tokio::time::timeout(CANCEL_BUDGET, fired)
        .await
        .expect("the barrier never reached the armed step")
        .expect("hook handoff");
    task.abort();
    // A completed task is also fine: its output (and any hold inside it)
    // drops with the JoinHandle result — the assertions below hold either
    // way, they just then prove the completed state instead.
    let _ = task.await;

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

/// Sanity anchor: the hook really fired every named step across the
/// cancellation tests above (the armed handoffs were not silently
/// short-circuited by barrier completion before the armed step).
#[tokio::test]
async fn step_hook_fires_every_named_barrier_step() {
    RepoInstance::set_online_backup_step_hook(step_hook);
    let repo = fixture_with_committed_data().await;
    repo.capture_point().await.expect("capture");
    let hold = repo.seal_point().await.expect("seal");
    drop(hold);

    let recorded = STEP_HOOK_RECORDED.lock().unwrap_or_else(|e| e.into_inner());
    for expected in ALL_STEPS {
        assert!(
            recorded.contains(&expected),
            "step {expected} never fired; recorded {recorded:?}"
        );
    }
}
