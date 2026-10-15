//! DDL interlock tests (S5c-X PR6b-2 item 5): a follower receipt apply holds
//! `ddl_admission` for its whole run, so a concurrently spawned
//! `RepoInstance::create_index` cannot complete until the apply future is
//! dropped — and the reverse ordering (DDL first, apply second) is
//! deadlock-free.
//!
//! Deterministic: no sleeps and no yield-count loops — the admission guard
//! fires the cfg(test) `entered` notify BEFORE awaiting the mutex (so
//! `hook.entered.notified()` proves the DDL is parked at the admission
//! point), and `done`+`exited` fire from the guard's `Drop`.

use std::sync::atomic::Ordering;
use std::sync::Arc;

use futures::StreamExt;
use shamir_storage::types::Store as _;
use shamir_tx::{ChangeOp, ChangelogEvent, RecordChange};
use shamir_types::access::Actor;
use shamir_types::types::record_id::RecordId;
use shamir_types::types::value::InnerValue;
use std::path::Path;

use crate::repo::changelog_store::ddl_hook;
use crate::repo::{BoxRepoFactory, RepoInstance};
use crate::table::TableConfig;
use crate::tx::apply_replicated::follower_durability::test_seam::{self, Phase};
use crate::tx::apply_replicated::{apply_replicated_batch, ApplyOutcome, ApplyPolicy};
use crate::tx::ReplicatedEvent;

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

async fn open(path: &Path, name: &str, tables: &[&str]) -> RepoInstance {
    RepoInstance::from_factory(
        name.into(),
        BoxRepoFactory::fjall_raw(path),
        tables.iter().map(|t| TableConfig::new(t)).collect(),
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

/// History versions of `key` (copied from receipt_apply_tests — not
/// refactored there).
async fn history_versions(repo: &RepoInstance, key: &[u8]) -> Vec<u64> {
    use shamir_tx::decode_version_key;
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

// While the apply future is parked at ReceiptPrepared it HOLDS
// `ddl_admission`; a concurrent `create_index` must not complete until the
// apply is released. After that the DDL completes and a re-apply of the
// same batch is a deterministic Skipped no-op (see the tail comment).
#[tokio::test]
async fn parked_apply_blocks_ddl_until_dropped() {
    let temp = tempfile::TempDir::new().unwrap();
    let name = repo_name("ddl-interlock", temp.path());
    let repo = open(temp.path(), &name, &["items"]).await;

    let hook = Arc::new(test_seam::Hook::default());
    *hook.park_at.lock().unwrap() = Some(Phase::ReceiptPrepared);
    test_seam::install(&name, Arc::clone(&hook));
    let mut apply = Box::pin(apply_replicated_batch(
        &repo,
        &[event(1, 1)],
        0,
        ApplyPolicy::Trusted,
    ));
    let reached = hook.reached.notified();
    tokio::pin!(reached);
    reached.as_mut().enable();
    tokio::select! {
        _ = &mut reached => {}
        result = &mut apply => {
            panic!("apply completed before ReceiptPrepared: {result:?}")
        }
    }

    // Parked apply holds ddl_admission: the DDL reaches the admission point
    // (the guard fires `entered` BEFORE awaiting the mutex) and then blocks.
    // The DDL hook registry keys by repo name and is get-or-insert, so the
    // spawned create_index and this test observe the SAME hook. Register the
    // `entered` waiter BEFORE spawning (notify_waiters has no permit, so the
    // waiter must exist first).
    let ddl_hook = ddl_hook(&name);
    let entered = ddl_hook.entered.notified();
    tokio::pin!(entered);
    entered.as_mut().enable();
    let ddl_repo = repo.clone();
    let ddl = tokio::spawn(async move {
        ddl_repo
            .create_index("items", "ix", &["value"])
            .await
            .unwrap();
    });
    entered.await;
    // Sound WITHOUT any spin: the parked apply still holds the guard, so
    // the guard's Drop (the ONLY writer of `done`) is impossible.
    assert!(
        !ddl_hook.done.load(Ordering::Acquire),
        "DDL guard dropped while the receipt apply held ddl_admission"
    );

    // Register the `exited` waiter BEFORE the guard can drop (notify_waiters
    // only wakes already-registered waiters), then release the apply (drop
    // at the park point) — the guard drops with it and the blocked DDL
    // proceeds.
    let exited = ddl_hook.exited.notified();
    tokio::pin!(exited);
    exited.as_mut().enable();
    hook.resume.notify_waiters();
    drop(apply);
    test_seam::remove(&name);
    ddl.await.unwrap();
    assert!(ddl_hook.done.load(Ordering::Acquire));
    exited.await;

    // The receipt pipeline runs on a DETACHED repository-owned worker: the
    // `hook.resume.notify_waiters()` above lets it finish the WHOLE batch
    // (materialize → checkpoint → publish) while it still holds
    // `marker_write_mutex` + `ddl_admission`; only then does the parked
    // create_index take admission. The re-apply below spawns a NEW worker
    // that queues behind those same mutexes, so by the time it runs the
    // receipt is retired and the durable v3 bookmark is 1. `apply_fresh`
    // consults that durable bookmark BEFORE any preflight, so
    // `prepare_event` short-circuits on the watermark floor and the batch is
    // a deterministic `ApplyOutcome::Skipped` no-op. The post-DDL indexed
    // rejection (`ApplyRejectReason::IndexedTable`) is never reached here —
    // the interlock claim under test is the admission ORDERING, not
    // post-index application of an already-applied batch.
    assert_eq!(
        apply_replicated_batch(&repo, &[event(1, 1)], 0, ApplyPolicy::Trusted)
            .await
            .unwrap(),
        ApplyOutcome::Skipped
    );
    let key1 = key_bytes(1).await;
    assert_eq!(history_versions(&repo, &key1).await, vec![1]);
    let gate = repo.tx_gate().await.unwrap();
    assert!(!gate.requires_reopen());

    // Exercise the guarded drop path too: dropping the index does not change
    // the outcome — the durable-bookmark floor keeps the re-apply Skipped
    // regardless of the table's index state.
    repo.drop_index("items", "ix").await.unwrap();
    assert_eq!(
        apply_replicated_batch(&repo, &[event(1, 1)], 0, ApplyPolicy::Trusted)
            .await
            .unwrap(),
        ApplyOutcome::Skipped
    );
    assert_eq!(history_versions(&repo, &key1).await, vec![1]);

    close(repo).await;
}

// Reverse ordering: a completed DDL first, then a normal receipt apply —
// both succeed, no deadlock, no reopen.
#[tokio::test]
async fn ddl_first_then_apply_no_deadlock() {
    let temp = tempfile::TempDir::new().unwrap();
    let name = repo_name("ddl-first", temp.path());
    // TWO tables: the index must go on "plain", a table the follower batch
    // never touches — the follower preflight REJECTS indexed tables
    // (tx/apply_replicated.rs, the legacy-index scan), so indexing "items"
    // (the batch's target) would make the apply legitimately Rejected, not
    // deadlocked.
    let repo = open(temp.path(), &name, &["items", "plain"]).await;

    repo.create_index("plain", "ix", &["value"]).await.unwrap();
    assert_eq!(
        apply_replicated_batch(&repo, &[event(1, 1)], 0, ApplyPolicy::Trusted)
            .await
            .unwrap(),
        ApplyOutcome::Applied { local_version: 1 }
    );
    let key1 = key_bytes(1).await;
    assert_eq!(history_versions(&repo, &key1).await, vec![1]);
    let gate = repo.tx_gate().await.unwrap();
    assert!(!gate.requires_reopen());

    close(repo).await;
}
