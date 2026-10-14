//! Repository override commit seam tests (S5c-X PR1b): exercise the
//! existing commit pipeline with the policy produced by resolve, plus a
//! negative control with the server-default Buffered policy left installed.

use shamir_query_types::batch::VisibilityWait;
use shamir_tx::IsolationLevel;
use shamir_types::types::value::InnerValue;

use crate::repo::durability_policy::{
    effective_wal_segment_bytes, resolve, DurabilityPolicy, WalCommitTier,
};
use crate::repo::RepoInstance;

use super::super::helpers;

const STATUS: VisibilityWait = VisibilityWait::Bounded { timeout_ms: 0 };

async fn commit_one(repo: &RepoInstance, value: &str) -> (u64, u64) {
    let table = repo.get_table("items").await.unwrap();
    let (mut tx, pin) = repo.begin_tx(IsolationLevel::Snapshot).await.unwrap();
    table
        .insert_tx(&InnerValue::Str(value.into()), Some(&mut tx))
        .await
        .unwrap();
    let info = repo.commit_tx_with_options(tx, STATUS).await.unwrap();
    let committed = info.commit_version;
    drop(pin);
    let recoverable = repo.wal_group().await.unwrap().recoverable_through();
    (committed, recoverable)
}

/// CREATE/ALTER's resolved Synced policy drives the existing commit seam:
/// commit ACK does not return until its version is recoverable through fsync.
#[tokio::test]
async fn resolved_synced_override_commit_is_fsynced_before_ack() {
    let fx = helpers::disk_repo("repo_override_synced_commit").await;
    let policy = resolve(
        &DurabilityPolicy::default(),
        Some(&shamir_query_types::DurabilityOverride {
            wal_commit: Some(WalCommitTier::Synced),
            ..Default::default()
        }),
        effective_wal_segment_bytes(),
    )
    .unwrap();
    fx.repo.set_durability(policy).unwrap();

    let (commit_version, recoverable_through) = commit_one(&fx.repo, "override-synced").await;
    assert_eq!(
        recoverable_through, commit_version,
        "resolved per-repo Synced policy must ack only after fsync covers its commit"
    );
}

/// Negative control for the unprotected path: a repo left at the server
/// default (Buffered) commits through the same existing seam but must NOT
/// publish an immediate recoverable-through watermark for that version.
/// This goes red if the default policy is accidentally treated as Synced.
#[tokio::test]
async fn server_default_buffered_negative_control_does_not_sync_on_commit() {
    let fx = helpers::disk_repo("repo_override_default_buffered").await;
    assert_eq!(
        fx.repo.durability_policy().effective_wal_commit(),
        WalCommitTier::Buffered
    );
    let group = fx.repo.wal_group().await.unwrap();
    shamir_wal::WalSink::arm_pause_next_file_sync();
    let sync = {
        let group = group.clone();
        tokio::spawn(async move { group.sync_now().await })
    };
    shamir_wal::WalSink::wait_file_sync_paused().await;
    let (commit_version, recoverable_through) = commit_one(&fx.repo, "default-buffered").await;
    assert!(
        recoverable_through < commit_version,
        "the unaltered server-default Buffered commit must ack before fsync makes it recoverable"
    );
    shamir_wal::WalSink::resume_file_sync();
    sync.await.unwrap().unwrap();
}
