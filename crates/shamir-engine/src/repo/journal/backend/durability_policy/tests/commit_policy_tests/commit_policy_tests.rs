//! `wal_commit` policy tests (PR1 Chunk B): the repo's Synced tier makes
//! commits ack at the fsynced watermark; a request-level `buffered` batch
//! must NOT weaken a server-level Synced policy.

use shamir_query_builder::batch::{Batch, Durability};
use shamir_query_builder::write::{self, doc};
use shamir_query_types::batch::VisibilityWait;
use shamir_storage::error::DbResult;
use shamir_tx::IsolationLevel;
use shamir_types::access::Actor;
use shamir_types::types::value::InnerValue;

use crate::query::batch::{execute_batch_unchecked, TableResolver};
use crate::query::TableRef;
use crate::repo::durability_policy::{
    DurabilityPolicy, WalCommitTier, DEFAULT_WAL_RETENTION_MAX_BYTES,
};
use crate::repo::RepoInstance;
use crate::table::TableManager;
use shamir_query_types::{ReplicationServe, RetentionOverflow};

use super::super::helpers;

const STATUS: VisibilityWait = VisibilityWait::Bounded { timeout_ms: 0 };

struct Resolver(RepoInstance);

#[async_trait::async_trait]
impl TableResolver for Resolver {
    async fn resolve(&self, table: &TableRef) -> DbResult<TableManager> {
        self.0.get_table(&table.table).await
    }

    async fn resolve_repo(&self, _: &str) -> DbResult<RepoInstance> {
        Ok(self.0.clone())
    }
}

async fn synced_repo(name: &str) -> helpers::DiskFixture {
    let fx = helpers::disk_repo(name).await;
    fx.repo
        .set_durability(DurabilityPolicy::new(
            WalCommitTier::Synced,
            DEFAULT_WAL_RETENTION_MAX_BYTES,
            RetentionOverflow::RefuseWrites,
            ReplicationServe::Durable,
        ))
        .unwrap();
    assert_eq!(
        fx.repo.durability_policy().effective_wal_commit(),
        WalCommitTier::Synced
    );
    assert_eq!(fx.repo.durability_metrics_snapshot().gauges[0], 1);
    fx
}

/// (c) policy `Synced`: a real commit's WAL window fsyncs, so S covers the
/// commit version the moment the commit acks.
#[tokio::test]
async fn synced_policy_commit_leaves_s_at_the_commit_version() {
    let fx = synced_repo("policy_synced_commit").await;
    let repo = &fx.repo;
    let table = repo.get_table("items").await.unwrap();
    let (mut tx, pin) = repo.begin_tx(IsolationLevel::Snapshot).await.unwrap();
    table
        .insert_tx(&InnerValue::Str("durable".into()), Some(&mut tx))
        .await
        .unwrap();
    let info = repo.commit_tx_with_options(tx, STATUS).await.unwrap();
    let version = info.commit_version;
    drop(pin);
    assert_eq!(
        repo.wal_group().await.unwrap().recoverable_through(),
        version,
        "Synced policy must ack at the fsynced watermark"
    );
}

/// (e) a request WITHOUT durability "synced" (i.e. request-level buffered)
/// never weakens the server-level Synced policy: the commit seam still
/// reads `repo.durability()` and appends at the Synced tier.
#[tokio::test]
async fn buffered_request_does_not_weaken_synced_server_policy() {
    let fx = synced_repo("policy_synced_batch").await;
    let repo = &fx.repo;
    let mut batch = Batch::new();
    batch
        .transactional()
        .visibility_wait(STATUS)
        .durability(Durability::Buffered);
    batch.insert(
        "written",
        write::insert("items").row(doc().set("name", "v").unwrap()),
    );
    let resolver = Resolver(repo.clone());
    let task = tokio::spawn(async move {
        execute_batch_unchecked(&batch.build(), &resolver, None, None, Actor::System, "test").await
    });
    let response = task.await.unwrap().unwrap();
    let version = response.transaction.unwrap().commit_version.unwrap();
    assert_eq!(
        repo.wal_group().await.unwrap().recoverable_through(),
        version,
        "request buffered must not downgrade the Synced commit tier"
    );
}

// ---- Amendment B2 (points 1 + 3): shared policy cell + ONE resolve ----

/// Amendment B2 (1): the policy is a SHARED `Arc<ArcSwap<…>>` cell — a
/// store through one clone is visible through another clone immediately.
/// Against the old plain `Copy`-per-handle field this fails by
/// construction (the second clone kept the stale default policy).
#[tokio::test]
async fn policy_store_through_one_clone_is_visible_through_another() {
    let fx = helpers::disk_repo("policy_shared_cell").await;
    let a = &fx.repo;
    let b = a.clone();

    a.set_durability(
        DurabilityPolicy::new(
            WalCommitTier::Synced,
            DEFAULT_WAL_RETENTION_MAX_BYTES,
            RetentionOverflow::DeclareGap,
            ReplicationServe::Durable,
        )
        .for_wal_backend(false),
    )
    .unwrap();

    let seen = b.durability_policy();
    assert_eq!(seen.wal_commit(), WalCommitTier::Synced);
    assert_eq!(seen.effective_wal_commit(), WalCommitTier::Synced);
    assert_eq!(
        seen.on_wal_retention_exceeded(),
        RetentionOverflow::DeclareGap
    );
    assert_eq!(
        seen.wal_retention_max_bytes(),
        DEFAULT_WAL_RETENTION_MAX_BYTES
    );
    drop(seen);
    b.set_durability(DurabilityPolicy::default()).unwrap();
    assert_eq!(a.durability_policy().wal_commit(), WalCommitTier::Buffered);
    assert_eq!(
        a.durability_policy().effective_wal_commit(),
        WalCommitTier::Buffered
    );
}

/// Hang canary (default current-thread runtime, no paused time): ONE plain
/// buffered commit on a FILE-backed repo must complete well inside the nextest
/// kill budget. Every PR1 wake link the commit touches (ticket completion, the
/// completion-watermark advance through the coverage observer, the strict
/// visibility wait) is exercised; if any link loses a wakeup, this future
/// never finishes and nextest fails the test — the post-condition is the wait.
#[tokio::test]
async fn buffered_commit_completes_on_default_runtime_file_wal() {
    let fx = helpers::disk_repo("canary_buffered_commit_file").await;
    let repo = &fx.repo;
    let table = repo.get_table("items").await.unwrap();
    let (mut tx, pin) = repo.begin_tx(IsolationLevel::Snapshot).await.unwrap();
    table
        .insert_tx(&InnerValue::Str("buffered".into()), Some(&mut tx))
        .await
        .unwrap();
    let info = repo
        .commit_tx_with_options(tx, VisibilityWait::Strict)
        .await
        .unwrap();
    drop(pin);
    assert_eq!(
        info.visibility,
        shamir_query_types::batch::VisibilityState::Visible,
        "a settled commit must ack visible on the strict wait"
    );
    assert!(
        repo.wal_group().await.unwrap().max_committed_version() >= info.commit_version,
        "the buffered window must have written the commit version"
    );
}
