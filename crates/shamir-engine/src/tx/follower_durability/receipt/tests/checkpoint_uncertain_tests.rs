//! Checkpoint-Uncertain reconcile tests (S5c-X PR6b-2 item 7): with the
//! test-only seam rewriting a `Committed` checkpoint outcome to `Uncertain`,
//! the read-back reconcile runs and the repo settles identically — verified
//! on the live handle AND across two reopen cycles.
//!
//! Deterministic: no sleeps and no yield budgets (review finding 5). The
//! old branch's bounded 20 ms fjall-open retry loop is gone — every reopen
//! here follows a GRACEFUL close, so the store lock is always free and the
//! plain open cannot fail spuriously.

use std::path::Path;

use bytes::Bytes;
use futures::StreamExt;
use shamir_storage::error::DbError;
use shamir_storage::types::RecordKey;
use shamir_tx::{decode_version_key, ChangeOp, ChangelogEvent, RecordChange};
use shamir_types::access::Actor;
use shamir_types::types::record_id::RecordId;
use shamir_types::types::value::InnerValue;

use crate::meta::repl::bookmark_v3::load_bookmark_v3;
use crate::meta::repl::receipt::receipt_key;
use crate::repo::{BoxRepoFactory, RepoInstance};
use crate::table::TableConfig;
use crate::tx::apply_replicated::{apply_replicated_batch, ApplyOutcome, ApplyPolicy};
use crate::tx::ReplicatedEvent;

use super::super::checkpoint::{
    checkpoint_reconciles, checkpoint_test_lock, disable_uncertain_reconcile,
    set_simulate_checkpoint_uncertain, set_uncertain_without_commit,
};

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

fn repo_name(path: &Path) -> String {
    format!(
        "checkpoint-uncertain-{}",
        path.display().to_string().replace('\\', "_")
    )
}

async fn journal(repo: &RepoInstance, version: u64) -> Option<Bytes> {
    repo.follower_journal_store()
        .await
        .unwrap()
        .get(RecordKey::from_slice(&version.to_be_bytes()))
        .await
        .ok()
}

/// Every local version at which `key` has a history row.
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

async fn key_bytes(id: u8) -> Vec<u8> {
    RecordId([id; 16]).to_bytes().to_vec()
}

/// The settled post-checkpoint state, asserted identically on the live
/// handle and after each reopen: bookmark v3 advanced with no outstanding
/// receipt, the receipt key retired, one history version per key, both
/// journal rows present, and the same batch re-delivered is a Skipped no-op.
async fn assert_settled(repo: &RepoInstance) {
    let info = repo.tx_info_store().await.unwrap();
    let v3 = load_bookmark_v3(&info).await.unwrap().unwrap();
    assert_eq!(v3.bookmark, 2);
    assert_eq!(v3.clean_through, Some(2));
    assert_eq!(v3.outstanding_receipt, None);
    assert!(matches!(
        info.get(receipt_key()).await,
        Err(DbError::NotFound(_))
    ));
    drop(info);

    let key1 = key_bytes(1).await;
    let key2 = key_bytes(2).await;
    assert_eq!(history_versions(repo, &key1).await, vec![1]);
    assert_eq!(history_versions(repo, &key2).await, vec![2]);
    assert!(journal(repo, 1).await.is_some());
    assert!(journal(repo, 2).await.is_some());

    assert_eq!(
        apply_replicated_batch(repo, &[event(1, 1), event(2, 2)], 0, ApplyPolicy::Trusted)
            .await
            .unwrap(),
        ApplyOutcome::Skipped
    );
}

// With the seam on, the checkpoint's Committed outcome is rewritten to
// Uncertain so the read-back reconcile runs; the repo settles cleanly, and
// the settled state survives two reopen cycles. The reconcile branch is
// observably entered: `checkpoint_reconciles()` increases by exactly 1
// (baseline sampled before the apply).
#[tokio::test]
async fn uncertain_checkpoint_reconciles_and_survives_reopens() {
    // The checkpoint seams and the reconcile counter are process-global:
    // serialize against the other tests that touch them.
    let _lock = checkpoint_test_lock();
    set_simulate_checkpoint_uncertain(false);
    let temp = tempfile::TempDir::new().unwrap();
    let name = repo_name(temp.path());
    let repo = open(temp.path(), &name).await;

    let baseline = checkpoint_reconciles();
    set_simulate_checkpoint_uncertain(true);
    let outcome =
        apply_replicated_batch(&repo, &[event(1, 1), event(2, 2)], 0, ApplyPolicy::Trusted).await;
    set_simulate_checkpoint_uncertain(false);
    // The pipeline returns the allocation high-water mark: 2 for the
    // two-event batch.
    assert_eq!(outcome.unwrap(), ApplyOutcome::Applied { local_version: 2 });
    assert_eq!(checkpoint_reconciles(), baseline + 1);
    assert_settled(&repo).await;

    // Reopen twice; the settled state must be identical every time. The
    // final re-apply inside `assert_settled` is a Skipped no-op because the
    // production path consults the DURABLE bookmark before allocation: the
    // replayed batch's leader versions are already ≤ the durable bookmark.
    close(repo).await;
    let repo = open(temp.path(), &name).await;
    assert_settled(&repo).await;
    close(repo).await;
    let repo = open(temp.path(), &name).await;
    assert_settled(&repo).await;

    close(repo).await;
}

// Negative control: with the seam OFF the reconcile branch never runs, so
// the counter must not move — `checkpoint_reconciles()` is a real
// discriminator. (If the reconcile branch were removed from the code, the
// seamed test above would keep its Uncertain outcome and fail at its
// `unwrap`; this test would then be the only signal — a flat counter —
// so both halves are needed.)
#[tokio::test]
async fn no_reconcile_counter_without_uncertain() {
    let _lock = checkpoint_test_lock();
    set_simulate_checkpoint_uncertain(false);
    let temp = tempfile::TempDir::new().unwrap();
    let name = repo_name(temp.path());
    let repo = open(temp.path(), &name).await;

    let before = checkpoint_reconciles();
    assert_eq!(
        apply_replicated_batch(&repo, &[event(1, 1), event(2, 2)], 0, ApplyPolicy::Trusted)
            .await
            .unwrap(),
        ApplyOutcome::Applied { local_version: 2 }
    );
    assert_eq!(checkpoint_reconciles(), before);

    close(repo).await;
}

// TRUE reconciliation negative control (review finding 9d): the checkpoint
// commit never lands (`set_uncertain_without_commit`) so the durable state is
// genuinely unsettled, and the read-back/retry protection is removed
// (`disable_uncertain_reconcile`) while the branch is STILL entered — the
// reconcile counter increments, but the settled-state invariants that
// `assert_settled` checks (bookmark advanced, clean horizon, receipt retired)
// provably FAIL. This proves the healthy-path test above discriminates real
// read-back protection, not merely branch entry.
#[tokio::test]
async fn negative_control_without_reconcile_state_is_not_reconciled() {
    let _lock = checkpoint_test_lock();
    set_simulate_checkpoint_uncertain(false);
    let temp = tempfile::TempDir::new().unwrap();
    let name = repo_name(temp.path());
    let repo = open(temp.path(), &name).await;

    let baseline = checkpoint_reconciles();
    set_uncertain_without_commit(true);
    disable_uncertain_reconcile(true);
    let outcome =
        apply_replicated_batch(&repo, &[event(1, 1), event(2, 2)], 0, ApplyPolicy::Trusted).await;
    disable_uncertain_reconcile(false);
    set_uncertain_without_commit(false);

    // Branch entered (counter moved), protection removed: the pipeline still
    // reports success and publishes to RAM, but NOTHING was reconciled
    // durably.
    assert_eq!(outcome.unwrap(), ApplyOutcome::Applied { local_version: 2 });
    assert_eq!(checkpoint_reconciles(), baseline + 1);

    // Provably NOT reconciled — the exact inverse of `assert_settled`'s
    // durable invariants: the bookmark v3 record was never written, the
    // receipt is still outstanding (Prepared), and the re-delivery is NOT a
    // durable Skipped no-op (no durable bookmark classifies it as applied).
    let info = repo.tx_info_store().await.unwrap();
    let v3 = load_bookmark_v3(&info).await.unwrap();
    assert!(
        v3.as_ref().is_none_or(|v3| v3.bookmark < 2),
        "bookmark must NOT have advanced without the reconcile: {v3:?}"
    );
    assert!(
        v3.map_or(true, |v3| v3.outstanding_receipt.is_some()),
        "receipt must still be outstanding without the reconcile"
    );
    assert!(
        !matches!(info.get(receipt_key()).await, Err(DbError::NotFound(_))),
        "receipt key must still be present without the reconcile"
    );
    drop(info);

    close(repo).await;
}

// POSITIVE control for the unsettled-state injection (companion to the
// negative control above): the checkpoint commit never lands
// (`set_uncertain_without_commit`) but the read-back/retry reconcile is LEFT
// ENABLED — so the retry actually repairs the unsettled durable state. The
// pipeline still reports Applied AND `assert_settled` PASSES (bookmark
// advanced, receipt retired, one history version per key, re-delivery
// Skipped). Together with the disabled negative control this proves the
// oracle discriminates real reconciliation: ENABLED + unsettled -> settled,
// DISABLED + unsettled -> provably not settled.
#[tokio::test]
async fn uncertain_without_commit_is_reconciled_when_enabled() {
    let _lock = checkpoint_test_lock();
    set_simulate_checkpoint_uncertain(false);
    let temp = tempfile::TempDir::new().unwrap();
    let name = repo_name(temp.path());
    let repo = open(temp.path(), &name).await;

    let baseline = checkpoint_reconciles();
    set_uncertain_without_commit(true);
    // disable_uncertain_reconcile stays OFF: the reconcile is armed.
    let outcome =
        apply_replicated_batch(&repo, &[event(1, 1), event(2, 2)], 0, ApplyPolicy::Trusted).await;
    set_uncertain_without_commit(false);

    assert_eq!(outcome.unwrap(), ApplyOutcome::Applied { local_version: 2 });
    assert_eq!(checkpoint_reconciles(), baseline + 1);
    // The retry repaired the unsettled state — the FULL settled oracle
    // passes, unlike the disabled negative control where it provably fails.
    assert_settled(&repo).await;

    close(repo).await;
}
