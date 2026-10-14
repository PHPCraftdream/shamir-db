use shamir_query_types::{ReplicationServe, RetentionOverflow, WalCommitTier};
use shamir_storage::error::DbResult;
use shamir_storage::storage_in_memory::InMemoryRepo;
use std::sync::Arc;

use crate::repo::{BoxRepo, BoxRepoFactory, DurabilityPolicy, RepoInstance};
use crate::table::TableConfig;
use crate::tx::clean_horizon::retention::{enforce_retention, RetentionAction};
use crate::tx::drainer::Drainer;
use futures::{pin_mut, StreamExt};

async fn count_rows(table: &crate::table::Table) -> usize {
    let stream = table.list_stream(256);
    pin_mut!(stream);
    let mut count = 0;
    while let Some(batch) = stream.next().await {
        count += batch.expect("list table rows").len();
    }
    count
}

fn policy(cap: u64, action: RetentionOverflow) -> DurabilityPolicy {
    DurabilityPolicy::new(
        WalCommitTier::Buffered,
        cap,
        action,
        ReplicationServe::Durable,
    )
}

async fn open_disk(
    name: &str,
    path: &std::path::Path,
    mode: RetentionOverflow,
) -> DbResult<RepoInstance> {
    RepoInstance::from_factory_with_test_segment_bytes(
        name.into(),
        BoxRepoFactory::fjall_raw(path.to_path_buf()),
        vec![TableConfig::new("t")],
        policy(512, mode),
        256,
    )
    .await
}

async fn seed(repo: &RepoInstance, count: usize) {
    let table = repo.get_table("t").await.unwrap();
    for index in 0..count {
        let (mut tx, _guard) = repo
            .begin_tx(crate::tx::IsolationLevel::Snapshot)
            .await
            .unwrap();
        table
            .insert_tx(
                &shamir_types::types::value::InnerValue::Str(format!(
                    "row-{index}-{}",
                    "x".repeat(512)
                )),
                Some(&mut tx),
            )
            .await
            .unwrap();
        repo.commit_tx(tx).await.unwrap();
    }
}

fn wal_file_count(path: &std::path::Path) -> usize {
    let mut dir = path.file_name().unwrap().to_os_string();
    dir.push(".shamirwal");
    std::fs::read_dir(path.with_file_name(dir))
        .unwrap()
        .filter_map(Result::ok)
        .filter(|entry| entry.path().extension().is_some_and(|ext| ext == "wal"))
        .count()
}

#[tokio::test]
async fn declare_gap_never_unlinks_history_unsafe_segments() {
    let temp = tempfile::TempDir::new().unwrap();
    let path = temp.path().join("declare");
    let repo = open_disk("declare", &path, RetentionOverflow::DeclareGap)
        .await
        .unwrap();
    seed(&repo, 80).await;
    let before = wal_file_count(&path);
    assert!(before > 1);
    let safe = repo.tx_gate().await.unwrap().durable_watermark();
    assert_eq!(safe, 0);
    assert_eq!(
        enforce_retention(&repo, safe).await.unwrap(),
        RetentionAction::Refused
    );
    let feed = repo.changefeed().await.unwrap().feed;
    assert!(feed.certified_gap_interval().is_none());
    assert_eq!(wal_file_count(&path), before);
}

#[tokio::test]
async fn declare_gap_persists_floor_and_unlinks_safe_retained_wal() {
    let temp = tempfile::TempDir::new().unwrap();
    let path = temp.path().join("declare_safe");
    let repo = open_disk("declare_safe", &path, RetentionOverflow::DeclareGap)
        .await
        .unwrap();
    seed(&repo, 80).await;
    let before = wal_file_count(&path);
    assert!(before > 1);
    let safe = repo.tx_gate().await.unwrap().completion().watermark();
    assert!(safe > 0);
    let entries = repo.repo_wal().await.unwrap().recover().await.unwrap();
    assert_eq!(
        entries.len(),
        80,
        "all 80 commits must remain in retained WAL"
    );
    for entry in entries {
        crate::tx::replay_v2_entry(&entry, &repo).await.unwrap();
    }
    repo.repo_interner().await.unwrap().persist().await.unwrap();
    let gate = repo.tx_gate().await.unwrap();
    gate.mark_durable(safe);
    repo.flush_all_history().await.unwrap();

    assert_eq!(
        enforce_retention(&repo, safe).await.unwrap(),
        RetentionAction::NoOverflow
    );
    let feed = repo.changefeed().await.unwrap().feed;
    assert!(feed.certified_gap_interval().is_some());
    let store = repo.repo.store_get("__changelog__").await.unwrap();
    let raw_j = store
        .get(shamir_tx::changefeed::clean_horizon_key().into())
        .await
        .unwrap();
    let j = shamir_tx::changefeed::decode_clean_horizon(&raw_j).unwrap();
    assert_eq!(feed.clean_horizon(), j);
    assert!(j >= safe);
    let info = repo.tx_info_store().await.unwrap();
    let reserved = crate::meta::recovery_marker::load_last_committed(&info)
        .await
        .unwrap()
        .unwrap();
    assert!(reserved >= j);
    assert!(wal_file_count(&path) < before);
}

#[tokio::test]
async fn refuse_writes_gate_clears_after_safe_drain() {
    let temp = tempfile::TempDir::new().unwrap();
    let path = temp.path().join("refuse");
    let repo = open_disk("refuse", &path, RetentionOverflow::RefuseWrites)
        .await
        .unwrap();
    seed(&repo, 80).await;
    assert!(
        repo.wal_group()
            .await
            .unwrap()
            .sealed_retained_bytes()
            .await
            .unwrap()
            > 1
    );
    assert_eq!(
        enforce_retention(&repo, 0).await.unwrap(),
        RetentionAction::Refused
    );
    assert!(repo.wal_retention_refuse_writes());
    Drainer::new().drain_all(&repo).await.unwrap();
    assert!(!repo.wal_retention_refuse_writes());
}

#[tokio::test]
async fn dead_writer_declare_gap_refuses_without_unlinking() {
    let temp = tempfile::TempDir::new().unwrap();
    let path = temp.path().join("dead");
    let repo = open_disk("dead", &path, RetentionOverflow::DeclareGap)
        .await
        .unwrap();
    seed(&repo, 80).await;
    let before = wal_file_count(&path);
    repo.changefeed()
        .await
        .unwrap()
        .feed
        .shutdown_and_wait()
        .await
        .unwrap();
    assert_eq!(
        enforce_retention(&repo, u64::MAX).await.unwrap(),
        RetentionAction::Refused
    );
    assert!(repo.wal_retention_refuse_writes());
    assert_eq!(wal_file_count(&path), before);
}

#[tokio::test]
async fn refused_admission_does_not_publish_and_drain_allows_retry() {
    let temp = tempfile::TempDir::new().unwrap();
    let path = temp.path().join("admission");
    let repo = open_disk("admission", &path, RetentionOverflow::RefuseWrites)
        .await
        .unwrap();
    seed(&repo, 80).await;
    assert_eq!(
        enforce_retention(&repo, 0).await.unwrap(),
        RetentionAction::Refused
    );

    let table = repo.get_table("t").await.unwrap();
    let current = repo.current_commit_version().await.unwrap();
    let wal = repo.repo_wal().await.unwrap();
    let before_entries = wal.recover().await.unwrap().len();
    let (mut rejected, rejected_guard) = repo
        .begin_tx(crate::tx::IsolationLevel::Snapshot)
        .await
        .unwrap();
    table
        .insert_tx(
            &shamir_types::types::value::InnerValue::Str("admission-retry".into()),
            Some(&mut rejected),
        )
        .await
        .unwrap();
    assert!(matches!(
        repo.commit_tx(rejected).await,
        Err(crate::tx::CommitError::WalRetentionExceeded)
    ));
    drop(rejected_guard);
    assert_eq!(repo.current_commit_version().await.unwrap(), current);
    assert_eq!(wal.recover().await.unwrap().len(), before_entries);
    assert_eq!(count_rows(&table).await, 80);

    Drainer::new().drain_all(&repo).await.unwrap();
    assert!(!repo.wal_retention_refuse_writes());
    let (mut accepted, _guard) = repo
        .begin_tx(crate::tx::IsolationLevel::Snapshot)
        .await
        .unwrap();
    table
        .insert_tx(
            &shamir_types::types::value::InnerValue::Str("admission-retry".into()),
            Some(&mut accepted),
        )
        .await
        .unwrap();
    let outcome = repo.commit_tx(accepted).await.unwrap();
    assert!(outcome.commit_version > current);
    assert_eq!(count_rows(&table).await, 81);
}

#[tokio::test]
async fn memory_wal_bypasses_retention_gate() {
    let repo = RepoInstance::new(
        "memory".into(),
        BoxRepo::InMemory(Arc::new(InMemoryRepo::new())),
        vec![TableConfig::new("t")],
    );
    assert_eq!(
        enforce_retention(&repo, 0).await.unwrap(),
        RetentionAction::NoOverflow
    );
    assert!(!repo.wal_retention_refuse_writes());
}

#[tokio::test]
async fn hybrid_wal_bypasses_retention_gate_and_checkpoint() {
    let temp = tempfile::TempDir::new().unwrap();
    let repo = RepoInstance::from_factory_with_test_segment_bytes(
        "hybrid_clean_horizon".into(),
        BoxRepoFactory::hybrid(temp.path()),
        vec![TableConfig::new("t")],
        policy(512, RetentionOverflow::RefuseWrites),
        256,
    )
    .await
    .unwrap();
    let feed = repo.changefeed().await.unwrap().feed;
    let initial_j = feed.clean_horizon();
    assert_eq!(
        enforce_retention(&repo, u64::MAX).await.unwrap(),
        RetentionAction::NoOverflow,
    );
    assert!(!repo.wal_retention_refuse_writes());
    let _ = test_seam::take_order(&repo);
    assert!(crate::tx::clean_horizon::checkpoint::checkpoint_before_truncate(&repo).await);
    assert_eq!(feed.clean_horizon(), initial_j);
    assert!(crate::tx::clean_horizon::checkpoint::test_seam::take_order(&repo).is_empty());
}

#[tokio::test]
async fn successful_truncate_remeasures_retention_and_clears_gate() {
    let temp = tempfile::TempDir::new().unwrap();
    let path = temp.path().join("remeasure");
    let repo = open_disk("remeasure", &path, RetentionOverflow::RefuseWrites)
        .await
        .unwrap();
    seed(&repo, 80).await;
    assert_eq!(
        enforce_retention(&repo, 0).await.unwrap(),
        RetentionAction::Refused
    );
    assert!(repo.wal_retention_refuse_writes());
    Drainer::new().drain_all(&repo).await.unwrap();
    let bytes = repo
        .wal_group()
        .await
        .unwrap()
        .sealed_retained_bytes()
        .await
        .unwrap();
    assert_eq!(bytes, 0);
    assert!(!repo.wal_retention_refuse_writes());
}
