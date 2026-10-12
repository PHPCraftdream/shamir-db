use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use bytes::Bytes;
use shamir_tx::{IndexWriteOp, IsolationLevel, TxContext};
use shamir_types::types::value::InnerValue;
use tokio::sync::Notify;

use crate::repo::{BoxRepoFactory, RepoInstance};
use crate::table::table_manager::table_token_for;
use crate::table::TableConfig;
use crate::tx::commit::{PostWalPrePublishHook, TEST_POST_WAL_PRE_PUBLISH_HOOK};

pub(super) async fn fixture(name: &str) -> (RepoInstance, Arc<PostWalPrePublishHook>) {
    let hook = Arc::new(PostWalPrePublishHook {
        repo_name: name.into(),
        pre_wal: AtomicBool::new(false),
        after_counter: AtomicBool::new(false),
        fail_orphan_markers: AtomicBool::new(false),
        panic_after_resume: AtomicBool::new(false),
        panic_resume: Notify::new(),
        reached: Notify::new(),
        resume: Notify::new(),
        armed: AtomicBool::new(false),
    });
    // The supported runner isolates each test in its own process. Install
    // before any drainer access: Drainer::spawn returns for this repo name.
    TEST_POST_WAL_PRE_PUBLISH_HOOK
        .set(Arc::clone(&hook))
        .ok()
        .unwrap();
    // InMemory has no MemBuffer or disk fsync worker. repo_wal still uses
    // begin_grouped with a Mem sink, so this exercises the real WAL path.
    let repo = RepoInstance::from_factory(
        name.into(),
        BoxRepoFactory::in_memory(),
        vec![TableConfig::new("a"), TableConfig::new("b")],
    )
    .await
    .unwrap();
    (repo, hook)
}

pub(super) async fn panic_post_wal(
    repo: &RepoInstance,
    hook: &PostWalPrePublishHook,
    tx: TxContext,
) -> u64 {
    // The hook captures panic mode before signalling reached; arm it before
    // spawning, not after the committing task has parked.
    hook.panic_after_resume.store(true, Ordering::Release);
    hook.armed.store(true, Ordering::Release);
    let owned = repo.clone();
    let task = tokio::spawn(async move { owned.commit_tx(tx).await });
    // Notify retains the permit even if the spawned task arrives first.
    // The default seam follows WAL acceptance, window offer and Phase 5a,
    // but precedes Phase 5b counters, Phase 5c postings and Phase 6 publish.
    // after_counter instead parks after tracked 5b, still before 5c/publish.
    hook.reached.notified().await;
    let gate = repo.tx_gate().await.unwrap();
    let version = *repo.drainer().window_keys().last().unwrap();
    assert!(gate.is_active_post_wal(version));
    // Unwind the inline driver: cancellation would detach this same tail
    // and finish it, rather than transfer replay ownership to the drainer.
    hook.panic_resume.notify_one();
    assert!(task.await.unwrap_err().is_panic());
    assert!(!gate.is_active_post_wal(version));
    assert!(gate.is_orphaned_post_wal(version));
    assert_eq!(gate.orphan_max(), version);
    assert!(gate.durable_watermark() < version);
    version
}

// Coverage, not a claimed red test: current orphan completion is expected
// to publish both tables and apply each counter exactly once.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn post_wal_panic_multi_table_values_counters_and_durability() {
    let (repo, hook) = fixture("post_wal_multi_table").await;
    let a = repo.get_table("a").await.unwrap();
    let b = repo.get_table("b").await.unwrap();
    let gate = repo.tx_gate().await.unwrap();
    let (mut tx, snapshot) = repo.begin_tx(IsolationLevel::Snapshot).await.unwrap();
    let a_rid = a
        .insert_tx(&InnerValue::Str("orphan-a".into()), Some(&mut tx))
        .await
        .unwrap();
    let b_rid = b
        .insert_tx(&InnerValue::Str("orphan-b".into()), Some(&mut tx))
        .await
        .unwrap();
    let version = panic_post_wal(&repo, &hook, tx).await;
    drop(snapshot);
    assert_eq!(a.counter().get().await.unwrap(), 0);
    assert_eq!(b.counter().get().await.unwrap(), 0);

    assert_eq!(repo.drainer().drain_all(&repo).await.unwrap(), 1);
    assert_eq!(
        a.get(a_rid).await.unwrap(),
        InnerValue::Str("orphan-a".into())
    );
    assert_eq!(
        b.get(b_rid).await.unwrap(),
        InnerValue::Str("orphan-b".into())
    );
    assert_eq!(a.counter().get().await.unwrap(), 1);
    assert_eq!(b.counter().get().await.unwrap(), 1);
    assert_eq!(gate.last_committed(), version);
    assert_eq!(gate.durable_watermark(), version);
    assert!(!gate.is_orphaned_post_wal(version));
    assert_eq!(repo.drainer().drain_all(&repo).await.unwrap(), 0);
    assert_eq!(a.counter().get().await.unwrap(), 1);
    assert_eq!(b.counter().get().await.unwrap(), 1);
}

// Coverage, not a claimed red test: metric failure is advisory today.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn post_wal_panic_multi_table_counter_underflow_does_not_block_durability() {
    let (repo, hook) = fixture("post_wal_multi_table_underflow").await;
    let a = repo.get_table("a").await.unwrap();
    let b = repo.get_table("b").await.unwrap();
    let gate = repo.tx_gate().await.unwrap();
    let (mut tx, snapshot) = repo.begin_tx(IsolationLevel::Snapshot).await.unwrap();
    let a_rid = a
        .insert_tx(&InnerValue::Str("counter-drift".into()), Some(&mut tx))
        .await
        .unwrap();
    let b_rid = b
        .insert_tx(&InnerValue::Str("healthy-counter".into()), Some(&mut tx))
        .await
        .unwrap();
    // Preserve the real data Put while injecting the same metric drift as
    // the existing single-table test: -1 cannot be applied to a zero count.
    tx.counter_deltas.insert(table_token_for("a"), -1);
    let version = panic_post_wal(&repo, &hook, tx).await;
    drop(snapshot);
    assert_eq!(a.counter().get().await.unwrap(), 0);
    let error = a.counter().increment(-1).await.unwrap_err();
    assert!(error.to_string().contains("cannot go below zero"));

    assert_eq!(repo.drainer().drain_all(&repo).await.unwrap(), 1);
    assert_eq!(
        a.get(a_rid).await.unwrap(),
        InnerValue::Str("counter-drift".into()),
    );
    assert_eq!(
        b.get(b_rid).await.unwrap(),
        InnerValue::Str("healthy-counter".into()),
    );
    assert_eq!(a.counter().get().await.unwrap(), 0);
    assert_eq!(b.counter().get().await.unwrap(), 1);
    assert_eq!(gate.last_committed(), version);
    assert_eq!(gate.durable_watermark(), version);
    assert!(!gate.is_orphaned_post_wal(version));
    assert_eq!(repo.drainer().drain_all(&repo).await.unwrap(), 0);
    assert_eq!(a.counter().get().await.unwrap(), 0);
    assert_eq!(b.counter().get().await.unwrap(), 1);
}

// Expected red by static inspection, not execution: the orphan's IndexPut
// is replayed after the later inline replacement, but the later IndexPut
// is skipped by index_applied. Versioned record history must also keep the
// later value; the unversioned posting must not regress to the orphan value.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn post_wal_panic_same_record_later_writer_posting_survives_drain() {
    let (repo, hook) = fixture("post_wal_same_record_posting").await;
    let table = repo.get_table("a").await.unwrap();
    let gate = repo.tx_gate().await.unwrap();
    let token = table_token_for("a");
    let key = Bytes::from_static(b"post_wal_same_record_posting");
    let orphan_posting = Bytes::from_static(b"orphan-posting");
    let later_posting = Bytes::from_static(b"later-posting");

    let (mut tx, snapshot) = repo.begin_tx(IsolationLevel::Snapshot).await.unwrap();
    let rid = table
        .insert_tx(&InnerValue::Str("seed".into()), Some(&mut tx))
        .await
        .unwrap();
    repo.commit_tx(tx).await.unwrap();
    drop(snapshot);
    assert_eq!(repo.drainer().drain_all(&repo).await.unwrap(), 1);
    let durable_before = gate.durable_watermark();

    let (mut tx, snapshot) = repo.begin_tx(IsolationLevel::Snapshot).await.unwrap();
    assert!(table
        .set_tx(rid, &InnerValue::Str("orphan".into()), Some(&mut tx))
        .await
        .unwrap());
    // Like post_wal_cancel_tests, stage a posting directly to isolate the
    // commit/replay contract from index planners and concurrent DDL.
    tx.index_write_set.push((
        token,
        IndexWriteOp::SetPosting {
            key: key.clone(),
            value: orphan_posting,
            provenance: Default::default(),
        },
    ));
    let orphan_version = panic_post_wal(&repo, &hook, tx).await;
    drop(snapshot);

    // Start only AFTER panic unwind has completed, and do not drain the orphan.
    let (mut tx, snapshot) = repo.begin_tx(IsolationLevel::Snapshot).await.unwrap();
    assert!(table
        .set_tx(rid, &InnerValue::Str("later".into()), Some(&mut tx))
        .await
        .unwrap());
    tx.index_write_set.push((
        token,
        IndexWriteOp::SetPosting {
            key: key.clone(),
            value: later_posting.clone(),
            provenance: Default::default(),
        },
    ));
    let (later_version, task) =
        super::snapshot_visibility_park_tests::spawn_settled_commit(&repo, tx).await;
    drop(snapshot);
    assert_eq!(later_version, orphan_version + 1);
    assert!(!task.is_finished());
    assert_eq!(gate.durable_watermark(), durable_before);
    assert!(gate.is_orphaned_post_wal(orphan_version));
    assert!(repo.drainer().window_entry(orphan_version).is_some());
    assert_eq!(
        table.get(rid).await.unwrap(),
        InnerValue::Str("later".into()),
    );
    assert_eq!(
        table.info_store().get(key.clone().into()).await.unwrap(),
        later_posting,
    );

    assert_eq!(repo.drainer().drain_all(&repo).await.unwrap(), 2);
    let outcome = task.await.unwrap().unwrap();
    assert_eq!(outcome.commit_version, later_version);
    assert_eq!(gate.durable_watermark(), outcome.commit_version);
    assert_eq!(gate.last_committed(), outcome.commit_version);
    assert!(!gate.is_orphaned_post_wal(orphan_version));
    assert_eq!(
        table.get(rid).await.unwrap(),
        InnerValue::Str("later".into()),
    );
    assert_eq!(
        table.info_store().get(key.clone().into()).await.unwrap(),
        later_posting,
        "draining an older orphan must not overwrite the later inline posting",
    );
    assert_eq!(repo.drainer().drain_all(&repo).await.unwrap(), 0);
    assert_eq!(
        table.get(rid).await.unwrap(),
        InnerValue::Str("later".into()),
    );
    assert_eq!(
        table.info_store().get(key.into()).await.unwrap(),
        later_posting,
    );
}
