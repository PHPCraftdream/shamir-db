//! RAM recoverability follows completed W, never raw appended max.

use shamir_query_types::batch::VisibilityWait;
use shamir_tx::IsolationLevel;
use shamir_types::types::value::InnerValue;

use super::super::helpers;

const STATUS: VisibilityWait = VisibilityWait::Bounded { timeout_ms: 0 };

#[tokio::test]
async fn mem_sink_keeps_r_equal_w_regardless_of_policy() {
    let repo = helpers::mem_repo("mem_r_equals_w");
    let clone = repo.clone();
    clone
        .set_durability(super::super::super::DurabilityPolicy::new(
            super::super::super::WalCommitTier::Synced,
            super::super::super::DEFAULT_WAL_RETENTION_MAX_BYTES,
            super::super::super::RetentionOverflow::RefuseWrites,
            super::super::super::ReplicationServe::Durable,
        ))
        .unwrap();
    let policy = repo.durability_policy();
    assert_eq!(
        policy.wal_commit(),
        super::super::super::WalCommitTier::Synced
    );
    assert_eq!(
        policy.effective_wal_commit(),
        super::super::super::WalCommitTier::Buffered
    );
    assert_eq!(
        clone.durability_policy().effective_wal_commit(),
        super::super::super::WalCommitTier::Buffered
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

/// PR8 chunk 1: on an ephemeral backend `completed_reseed` set_durability
/// SUCCEEDS (the only honest serve mode there), and an explicitly
/// requested `synced` commit tier is silently downgraded to `buffered`
/// via `for_wal_backend` — the requested tier stays visible in
/// `wal_commit()` while `effective_wal_commit()` reports the downgrade.
#[tokio::test]
async fn set_durability_downgrades_synced_to_buffered_on_mem_backend() {
    let repo = helpers::mem_repo("mem_completed_reseed_set");
    repo.set_durability(super::super::super::DurabilityPolicy::new(
        super::super::super::WalCommitTier::Synced,
        super::super::super::DEFAULT_WAL_RETENTION_MAX_BYTES,
        super::super::super::RetentionOverflow::RefuseWrites,
        super::super::super::ReplicationServe::CompletedReseed,
    ))
    .unwrap();
    let policy = repo.durability_policy();
    assert_eq!(
        policy.wal_commit(),
        super::super::super::WalCommitTier::Synced,
        "the requested tier is stored unchanged"
    );
    assert_eq!(
        policy.effective_wal_commit(),
        super::super::super::WalCommitTier::Buffered,
        "silent downgrade via for_wal_backend, exactly like default construction"
    );
    assert_eq!(
        policy.replication_serve(),
        super::super::super::ReplicationServe::CompletedReseed
    );
}
