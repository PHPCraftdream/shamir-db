//! Cancellation convergence tests (S5c-X PR6b-2 item 6, revised for the
//! repository-owned worker): the pipeline runs on a DETACHED task, so
//! dropping the caller future at ANY phase boundary (ReceiptPrepared,
//! ReceiptMaterialized, ReceiptCheckpointed) does NOT cancel the operation —
//! the worker runs to completion from its durable pause point. The tests
//! assert the deterministic PARKED state (durable receipt outstanding, the
//! guards still held, watermark untouched), then resume the worker and
//! verify convergence: exactly one history version per key, no reopen, the
//! durability watermark at the batch end, and a re-apply of the SAME batch a
//! Skipped no-op (the production path consults the durable bookmark before
//! allocation, so the already-applied leader versions are skipped).
//!
//! Deterministic: parks on the `test_seam::Hook` notify handshake plus
//! bounded condition spins — no sleeps.

use std::path::Path;
use std::sync::Arc;

use futures::StreamExt;
use shamir_storage::error::DbError;
use shamir_storage::types::Store as _;
use shamir_tx::{decode_version_key, ChangeOp, ChangelogEvent, RecordChange};
use shamir_types::access::Actor;
use shamir_types::types::record_id::RecordId;
use shamir_types::types::value::InnerValue;

use crate::meta::repl::bookmark_v3::load_bookmark_v3;
use crate::meta::repl::receipt::receipt_key;
use crate::meta::repl::ReceiptPhase;
use crate::repo::{BoxRepoFactory, RepoInstance};
use crate::table::TableConfig;
use crate::tx::apply_replicated::follower_durability::test_seam::{self, Phase};
use crate::tx::apply_replicated::{apply_replicated_batch, ApplyOutcome, ApplyPolicy};
use crate::tx::ReplicatedEvent;

use super::super::checkpoint;

fn event(leader_version: u64, id: u8) -> ReplicatedEvent {
    ReplicatedEvent {
        event: ChangelogEvent {
            repo: "leader".into(),
            commit_version: leader_version,
            tx_id: leader_version,
            actor: Actor::System,
            timestamp_ns: 0,
            changes: vec![RecordChange {
                table: "items".into(),
                key: RecordId([id; 16]).to_bytes(),
                op: ChangeOp::Put,
                value: Some(InnerValue::Str(format!("row-{id}")).to_bytes().unwrap()),
            }],
        },
        field_bindings: Vec::new(),
    }
}

async fn open(path: &Path, name: &str) -> RepoInstance {
    RepoInstance::from_factory(
        name.into(),
        BoxRepoFactory::fjall_raw(path),
        vec![TableConfig::new("items")],
    )
    .await
    .unwrap()
}

async fn close(repo: RepoInstance) {
    let handle = repo.changefeed().await.unwrap();
    handle.feed.shutdown_and_wait().await.unwrap();
    drop(handle);
    drop(repo);
}

fn repo_name(tag: &str, path: &Path) -> String {
    format!("{tag}-{}", path.display().to_string().replace('\\', "_"))
}

async fn key_bytes(id: u8) -> Vec<u8> {
    RecordId([id; 16]).to_bytes().to_vec()
}

/// Every local version at which `key` has a history row (copied from
/// receipt_apply_tests — not refactored there).
async fn history_versions(repo: &RepoInstance, key: &[u8]) -> Vec<u64> {
    let store = repo.raw_store("__history__items").await.unwrap();
    let mut versions = Vec::new();
    for batch in store.iter_stream(128).collect::<Vec<_>>().await {
        for (record_key, _) in batch.unwrap() {
            if let Some((row_key, version)) = decode_version_key(record_key.as_ref()) {
                if row_key == key {
                    versions.push(version);
                }
            }
        }
    }
    versions.sort_unstable();
    versions
}

/// `(bookmark v3 receipt id, stored receipt phase)` (copied from
/// receipt_apply_tests).
async fn outstanding_phase(repo: &RepoInstance) -> Option<ReceiptPhase> {
    let info = repo.tx_info_store().await.unwrap();
    let v3 = load_bookmark_v3(&info).await.unwrap();
    let Some(id) = v3.as_ref().and_then(|v3| v3.outstanding_receipt) else {
        return None;
    };
    match info.get(receipt_key()).await {
        Ok(bytes) => Some(
            crate::meta::repl::decode_receipt(&id, &bytes)
                .unwrap()
                .phase,
        ),
        Err(DbError::NotFound(_)) => None,
        Err(error) => panic!("receipt read failed: {error}"),
    }
}

/// Start `events` and drop the caller future the moment `phase` fires (park
/// on the seam, then drop the JoinHandle await at that point). The
/// repository-owned worker stays PARKED at the phase — a durable pause
/// point — so asserts against the parked state are deterministic. The
/// caller resumes the worker later via `hook.resume.notify_waiters()`.
async fn crash_at_phase(
    repo: &RepoInstance,
    name: &str,
    phase: Phase,
    events: &[ReplicatedEvent],
) -> Arc<test_seam::Hook> {
    let hook = Arc::new(test_seam::Hook::default());
    *hook.park_at.lock().unwrap() = Some(phase);
    test_seam::install(name, Arc::clone(&hook));
    let mut apply = Box::pin(apply_replicated_batch(
        repo,
        events,
        0,
        ApplyPolicy::Trusted,
    ));
    let reached = hook.reached.notified();
    tokio::pin!(reached);
    reached.as_mut().enable();
    tokio::select! {
        _ = &mut reached => {}
        result = &mut apply => {
            panic!("apply completed before {phase:?}: {result:?}")
        }
    }
    // Dropping the caller future drops only the JoinHandle await; the
    // detached worker stays parked inside the seam.
    drop(apply);
    test_seam::remove(name);
    hook
}

/// Resume the parked worker and condition-wait (bounded yield spin, no
/// sleeps) until it completes: the detached worker owns the operation, and
/// the re-apply below queues behind the admission mutex, so by the time it
/// runs the worker has retired the receipt and advanced the durable
/// bookmark.
async fn resume_and_settle(repo: &RepoInstance, hook: &test_seam::Hook) {
    hook.resume.notify_waiters();
    spin_until_settled(repo).await;
}

/// Assert the settled post-convergence state: exactly one history version
/// (its own) per key, no reopen, durability watermark at the batch end, and
/// the same batch re-delivered is a Skipped no-op that leaves the watermark
/// untouched.
async fn assert_settled(repo: &RepoInstance) {
    for id in 1u8..=2 {
        let key = key_bytes(id).await;
        assert_eq!(
            history_versions(repo, &key).await,
            vec![u64::from(id)],
            "key {id} must exist at exactly version {id}"
        );
    }
    let gate = repo.tx_gate().await.unwrap();
    assert!(!gate.requires_reopen());
    assert_eq!(gate.last_committed(), 2);
    assert_eq!(gate.durable_watermark(), 2);
    assert_eq!(
        apply_replicated_batch(&repo, &[event(1, 1), event(2, 2)], 0, ApplyPolicy::Trusted)
            .await
            .unwrap(),
        ApplyOutcome::Skipped
    );
    assert_eq!(gate.durable_watermark(), 2);
}

// One test per phase boundary; a fresh repo per phase.
#[tokio::test]
async fn drop_at_receipt_prepared_converges_on_reapply() {
    let temp = tempfile::TempDir::new().unwrap();
    let name = repo_name("cancel-prepared", temp.path());
    let repo = open(temp.path(), &name).await;

    let hook = crash_at_phase(
        &repo,
        &name,
        Phase::ReceiptPrepared,
        &[event(1, 1), event(2, 2)],
    )
    .await;
    // Parked at the durable pause point: the durable Prepared receipt owns
    // the versions, and the worker still holds the guards — so no Aborted
    // mark moved the watermark.
    assert_eq!(
        outstanding_phase(&repo).await,
        Some(ReceiptPhase::Prepared),
        "the durable Prepared receipt must own the versions"
    );
    let gate = repo.tx_gate().await.unwrap();
    assert_eq!(gate.last_committed(), 0);
    assert_eq!(gate.durable_watermark(), 0);
    // Resume the detached worker and let it FINISH — a dropped caller cannot
    // cancel the repository-owned operation. The re-apply below therefore
    // queues behind the admission mutex and runs against the completed
    // state: the durable bookmark (2) classifies every event as applied, so
    // it is a deterministic Skipped no-op.
    resume_and_settle(&repo, &hook).await;
    assert_settled(&repo).await;

    close(repo).await;
}

#[tokio::test]
async fn drop_at_receipt_materialized_converges_on_reapply() {
    let temp = tempfile::TempDir::new().unwrap();
    let name = repo_name("cancel-materialized", temp.path());
    let repo = open(temp.path(), &name).await;

    let hook = crash_at_phase(
        &repo,
        &name,
        Phase::ReceiptMaterialized,
        &[event(1, 1), event(2, 2)],
    )
    .await;
    // Parked at the durable pause point: the durable Materialized receipt owns
    // the versions, and the worker still holds the guards — so no Aborted
    // mark moved the watermark.
    assert_eq!(
        outstanding_phase(&repo).await,
        Some(ReceiptPhase::Materialized),
        "the durable Materialized receipt must own the versions"
    );
    let gate = repo.tx_gate().await.unwrap();
    assert_eq!(gate.last_committed(), 0);
    assert_eq!(gate.durable_watermark(), 0);
    // Resume the detached worker and let it FINISH — a dropped caller cannot
    // cancel the repository-owned operation. The re-apply below therefore
    // queues behind the admission mutex and runs against the completed
    // state: the durable bookmark (2) classifies every event as applied, so
    // it is a deterministic Skipped no-op.
    resume_and_settle(&repo, &hook).await;
    assert_settled(&repo).await;

    close(repo).await;
}

// Safe because the Checkpointed phase fires only after the (now synchronous)
// publication completed: the drop cannot split commit from publication.
#[tokio::test]
async fn drop_at_receipt_checkpointed_is_fully_converged() {
    let temp = tempfile::TempDir::new().unwrap();
    let name = repo_name("cancel-checkpointed", temp.path());
    let repo = open(temp.path(), &name).await;

    let hook = crash_at_phase(
        &repo,
        &name,
        Phase::ReceiptCheckpointed,
        &[event(1, 1), event(2, 2)],
    )
    .await;
    // The phase fires only AFTER the (synchronous) publication completed,
    // so while parked the batch is already fully committed and published —
    // deterministically, because the worker is parked. Resume it, let it
    // flush, and confirm the re-apply is a durable-bookmark Skipped no-op.
    assert_eq!(outstanding_phase(&repo).await, None);
    resume_and_settle(&repo, &hook).await;
    assert_settled(&repo).await;

    close(repo).await;
}

// Reviewer-finding-5 regression: dropping the caller future at
// ReceiptMaterialized must NOT advance the watermark — the repository-owned
// worker still holds the chunk guards, so no Aborted mark moves
// `last_committed`/`durable_watermark` past the unpublished versions.
#[tokio::test]
async fn watermark_never_passes_unpublished_receipt_versions() {
    let temp = tempfile::TempDir::new().unwrap();
    let name = repo_name("cancel-watermark", temp.path());
    let repo = open(temp.path(), &name).await;

    let hook = crash_at_phase(
        &repo,
        &name,
        Phase::ReceiptMaterialized,
        &[event(1, 1), event(2, 2)],
    )
    .await;
    // BEFORE any re-apply, while the worker is parked at the durable pause
    // point: the receipt is Materialized-outstanding and the watermark is
    // untouched — the worker holds the chunk guards, so no Aborted mark
    // advanced `last_committed`/`durable_watermark` past the unpublished
    // versions (a fresh repo has no other commits).
    assert_eq!(
        outstanding_phase(&repo).await,
        Some(ReceiptPhase::Materialized),
        "the receipt must still be Materialized-outstanding"
    );
    let gate = repo.tx_gate().await.unwrap();
    assert_eq!(gate.last_committed(), 0);
    assert_eq!(gate.durable_watermark(), 0);

    // Resume the detached worker (it owns the operation) and let it finish;
    // the subsequent re-apply consults the durable bookmark (2) and is a
    // deterministic Skipped no-op at watermark 2.
    resume_and_settle(&repo, &hook).await;
    assert_eq!(gate.last_committed(), 2);
    assert_eq!(gate.durable_watermark(), 2);
    assert_settled(&repo).await;

    close(repo).await;
}

/// Bounded deterministic condition wait (yield spin, no sleeps): poll until
/// `condition` holds, giving the detached worker task scheduling room.
async fn spin_until_settled(repo: &RepoInstance) {
    for _ in 0..500u32 {
        let info = repo.tx_info_store().await.unwrap();
        let settled = load_bookmark_v3(&info)
            .await
            .unwrap()
            .is_some_and(|v3| v3.bookmark >= 2 && v3.outstanding_receipt.is_none())
            && outstanding_phase(repo).await.is_none();
        drop(info);
        if settled {
            return;
        }
        tokio::task::yield_now().await;
    }
    panic!("detached worker did not complete");
}

// Item 2a: aborting the CALLER task (whose future awaits only the worker's
// JoinHandle) while the repository-owned worker is parked before the
// checkpoint commit must leave the DETACHED worker in control — it completes
// the commit AND the RAM publication, and the repo settles identically to an
// uninterrupted run.
#[tokio::test]
async fn drop_during_checkpoint_commit_still_completes_and_publishes() {
    let temp = tempfile::TempDir::new().unwrap();
    let name = repo_name("cancel-commit-park", temp.path());
    let repo = open(temp.path(), &name).await;

    checkpoint::arm_commit_park(true);
    let worker_repo = repo.clone();
    let events = vec![event(1, 1), event(2, 2)];
    let caller = tokio::spawn(async move {
        apply_replicated_batch(&worker_repo, &events, 0, ApplyPolicy::Trusted).await
    });
    checkpoint::wait_commit_park_reached().await;
    caller.abort();
    checkpoint::release_commit_park();

    spin_until_settled(&repo).await;
    assert_eq!(history_versions(&repo, &key_bytes(1).await).await, vec![1]);
    assert_eq!(history_versions(&repo, &key_bytes(2).await).await, vec![2]);
    let gate = repo.tx_gate().await.unwrap();
    assert!(!gate.requires_reopen());
    assert_eq!(gate.durable_watermark(), 2);
    assert_eq!(outstanding_phase(&repo).await, None);
    let info = repo.tx_info_store().await.unwrap();
    assert!(matches!(
        info.get(receipt_key()).await,
        Err(DbError::NotFound(_))
    ));

    checkpoint::arm_commit_park(false);
    close(repo).await;
}

// Item 2b: aborting the caller while the Uncertain read-back reconcile is
// parked must still settle — the detached worker finishes the reconcile, and
// the reconcile branch is observably entered (`checkpoint_reconciles()`
// increased by >= 1 versus the pre-apply baseline).
#[tokio::test]
async fn drop_during_uncertain_readback_still_completes_and_publishes() {
    let temp = tempfile::TempDir::new().unwrap();
    let name = repo_name("cancel-reconcile-park", temp.path());
    let repo = open(temp.path(), &name).await;

    // The checkpoint seams are process-global: serialize against the other
    // tests that read the reconcile counter.
    let _lock = checkpoint::checkpoint_test_lock();
    let baseline = checkpoint::checkpoint_reconciles();
    checkpoint::set_simulate_checkpoint_uncertain(true);
    checkpoint::arm_reconcile_park(true);
    let worker_repo = repo.clone();
    let events = vec![event(1, 1), event(2, 2)];
    let caller = tokio::spawn(async move {
        apply_replicated_batch(&worker_repo, &events, 0, ApplyPolicy::Trusted).await
    });
    checkpoint::wait_reconcile_park_reached().await;
    caller.abort();
    checkpoint::release_reconcile_park();

    spin_until_settled(&repo).await;
    checkpoint::set_simulate_checkpoint_uncertain(false);
    checkpoint::arm_reconcile_park(false);
    assert_eq!(history_versions(&repo, &key_bytes(1).await).await, vec![1]);
    assert_eq!(history_versions(&repo, &key_bytes(2).await).await, vec![2]);
    let gate = repo.tx_gate().await.unwrap();
    assert!(!gate.requires_reopen());
    assert_eq!(gate.durable_watermark(), 2);
    assert_eq!(outstanding_phase(&repo).await, None);
    assert!(
        checkpoint::checkpoint_reconciles() >= baseline + 1,
        "the reconcile branch must have run"
    );

    close(repo).await;
}
