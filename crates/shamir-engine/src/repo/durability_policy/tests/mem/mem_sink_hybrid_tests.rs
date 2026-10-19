//! Hybrid-backend (Mem WAL sink over a persisted identity) policy tests.
//! Requires the `fjall` feature: `BoxRepoFactory::hybrid` wraps the
//! fjall-backed half of the hybrid factory.

use shamir_query_types::batch::VisibilityWait;
use shamir_tx::IsolationLevel;
use shamir_types::types::value::InnerValue;

const STATUS: VisibilityWait = VisibilityWait::Bounded { timeout_ms: 0 };

#[tokio::test]
async fn hybrid_reopen_starts_a_new_lineage_and_keeps_no_table_data() {
    use crate::repo::{BoxRepoFactory, RepoInstance};
    use crate::table::TableConfig;
    let dir = tempfile::TempDir::new().unwrap();
    let path = dir.path().join("hybrid-info");
    let policy = super::super::super::DurabilityPolicy::new(
        super::super::super::WalCommitTier::Synced,
        super::super::super::DEFAULT_WAL_RETENTION_MAX_BYTES,
        super::super::super::RetentionOverflow::RefuseWrites,
        super::super::super::ReplicationServe::Durable,
    )
    // Hybrid backend: Mem WAL sink, no persistent WAL; the Synced tier is
    // silently downgraded by `for_wal_backend`.
    ;
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
