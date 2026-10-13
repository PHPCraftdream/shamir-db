//! RAM recoverability follows completed W, never raw appended max.

use shamir_query_types::batch::VisibilityWait;
use shamir_tx::IsolationLevel;
use shamir_types::types::value::InnerValue;

use super::helpers;

const STATUS: VisibilityWait = VisibilityWait::Bounded { timeout_ms: 0 };

#[tokio::test]
async fn mem_sink_keeps_r_equal_w_regardless_of_policy() {
    let repo = helpers::mem_repo("mem_r_equals_w");
    let clone = repo.clone();
    clone
        .set_durability(super::super::DurabilityPolicy::new(
            super::super::WalCommitTier::Synced,
            super::super::DEFAULT_WAL_RETENTION_MAX_BYTES,
            super::super::RetentionOverflow::RefuseWrites,
            super::super::ReplicationServe::Durable,
        ))
        .unwrap();
    let policy = repo.durability_policy();
    assert_eq!(policy.wal_commit(), super::super::WalCommitTier::Synced);
    assert_eq!(
        policy.effective_wal_commit(),
        super::super::WalCommitTier::Buffered
    );
    assert_eq!(
        clone.durability_policy().effective_wal_commit(),
        super::super::WalCommitTier::Buffered
    );
    drop(policy);
    assert_eq!(repo.durability_metrics_snapshot().gauges[0], 1);
    let group = repo.wal_group().await.unwrap();
    assert_eq!(group.recoverable_through(), 0);
    let table = repo.get_table("items").await.unwrap();
    let (mut tx, pin) = repo.begin_tx(IsolationLevel::Snapshot).await.unwrap();
    table
        .insert_tx(&InnerValue::Str("mem".into()), Some(&mut tx))
        .await
        .unwrap();
    let info = repo.commit_tx_with_options(tx, STATUS).await.unwrap();
    let version = info.commit_version;
    drop(pin);
    assert_eq!(group.instance_sync_calls(), 0);
    assert_eq!(group.instance_sync_rounds(), 0);
    assert_eq!(repo.durability_metrics_snapshot().gauges[0], 1);
    // Completion W, not append alone, publishes the RAM prefix.
    assert_eq!(group.recoverable_through(), version);
}

/// Sources must be isolated per repo while clones share one metrics state.
#[tokio::test]
async fn durability_metrics_are_shared_by_clones_and_isolated_between_repos() {
    let a = helpers::mem_repo("metrics_a");
    let b = helpers::mem_repo("metrics_b");
    let clone = a.clone();
    let ga = a.wal_group().await.unwrap();
    let gb = b.wal_group().await.unwrap();
    ga.sync_now().await.unwrap();
    let sa = clone.durability_metrics_snapshot();
    let sb = b.durability_metrics_snapshot();
    assert_eq!(
        sa.counters[0], 1,
        "clone must see a's completed sync demand"
    );
    assert_eq!(sa.counters[1], 1);
    assert_eq!(sb.counters[0], 0, "b must not inherit a's sync demand");
    assert_eq!(gb.instance_sync_calls(), 0);
}

#[tokio::test]
async fn hybrid_reopen_starts_a_new_lineage_and_keeps_no_table_data() {
    use crate::repo::{BoxRepoFactory, RepoInstance};
    use crate::table::TableConfig;
    let dir = tempfile::TempDir::new().unwrap();
    let path = dir.path().join("hybrid-info");
    let policy = super::super::DurabilityPolicy::new(
        super::super::WalCommitTier::Synced,
        super::super::DEFAULT_WAL_RETENTION_MAX_BYTES,
        super::super::RetentionOverflow::RefuseWrites,
        super::super::ReplicationServe::Durable,
    );
    let repo = RepoInstance::from_factory_with_durability(
        "hybrid-policy".into(),
        BoxRepoFactory::hybrid(path.clone()),
        vec![TableConfig::new("items")],
        policy,
    )
    .await
    .unwrap();
    let identity = repo.repo_identity().await.unwrap();
    let table = repo.get_table("items").await.unwrap();
    let (mut tx, pin) = repo.begin_tx(IsolationLevel::Snapshot).await.unwrap();
    table
        .insert_tx(&InnerValue::Str("ephemeral".into()), Some(&mut tx))
        .await
        .unwrap();
    let info = repo.commit_tx_with_options(tx, STATUS).await.unwrap();
    drop(pin);
    let group = repo.wal_group().await.unwrap();
    assert!(group.is_mem_sink());
    assert_eq!(group.instance_sync_calls(), 0);
    assert_eq!(group.instance_sync_rounds(), 0);
    assert_eq!(group.recoverable_through(), info.commit_version);
    repo.shutdown_clean(std::time::Duration::from_secs(10))
        .await
        .unwrap();
    repo.stop_journal_writer_for_test().await.unwrap();
    drop(table);
    drop(group);
    drop(repo);
    let reopened = RepoInstance::from_factory_with_durability(
        "hybrid-policy".into(),
        BoxRepoFactory::hybrid(path),
        vec![TableConfig::new("items")],
        policy,
    )
    .await
    .unwrap();
    // PR #160 lineage fence: hybrid repositories mint a NEW lineage at every
    // process start (nothing survives a restart), so the reopened identity
    // must differ from the first incarnation's.
    assert_ne!(reopened.repo_identity().await.unwrap(), identity);
    assert!(reopened.wal_group().await.unwrap().is_mem_sink());
    assert_eq!(
        reopened
            .get_table("items")
            .await
            .unwrap()
            .counter()
            .get()
            .await
            .unwrap(),
        0
    );
    reopened.stop_journal_writer_for_test().await.unwrap();
}
