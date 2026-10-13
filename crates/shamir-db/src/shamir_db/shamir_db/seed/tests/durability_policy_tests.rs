//! PR1 Chunk C tests: durability-policy plumbing through `ShamirDb`.
//!
//! Covers policy plumbing, backend behavior and a repo-filtered log capture.

use shamir_engine::repo::durability_policy::{DurabilityPolicy, DEFAULT_WAL_RETENTION_MAX_BYTES};
use shamir_engine::repo::durability_policy::{ReplicationServe, RetentionOverflow, WalCommitTier};
use shamir_query_builder::batch::{Batch, Durability};
use shamir_query_builder::doc;
use shamir_query_builder::write;

use crate::engine::repo::{BoxRepoFactory, RepoConfig};
use crate::engine::table::TableConfig;
use crate::shamir_db::SystemStoreConfig;
use crate::ShamirDb;

fn synced_policy() -> DurabilityPolicy {
    DurabilityPolicy::new(
        WalCommitTier::Synced,
        DEFAULT_WAL_RETENTION_MAX_BYTES,
        RetentionOverflow::RefuseWrites,
        ReplicationServe::Durable,
    )
}

/// One transactional insert through the db; returns the commit version.
async fn commit_one(shamir: &ShamirDb, db: &str, repo: &str, table: &str) -> u64 {
    let mut b = Batch::new();
    b.id(1);
    b.transactional().durability(Durability::Buffered);
    b.insert(
        "ins",
        write::Insert::with_repo(repo, table).row(doc! { "name" => "widget" }),
    );
    let req = b.to_request_via_msgpack();
    let resp = shamir.execute(db, &req).await.unwrap();
    resp.transaction
        .and_then(|t| t.commit_version)
        .expect("transactional batch must report a commit version")
}

/// (a) plain `init` (default policy) → a repo opened through the db reads
/// back the Buffered defaults.
#[tokio::test]
async fn default_policy_reads_back_buffered() {
    let shamir = ShamirDb::init(SystemStoreConfig::InMemory).await.unwrap();
    assert_eq!(
        shamir.durability_policy().wal_commit(),
        WalCommitTier::Buffered
    );
    shamir.create_db("appdb").await;
    shamir
        .add_repo(
            "appdb",
            RepoConfig::new("data", BoxRepoFactory::in_memory()),
        )
        .await
        .unwrap();
    let repo = shamir
        .get_db("appdb")
        .unwrap()
        .get_repo("data")
        .expect("repo present");
    let p = repo.durability_policy();
    assert_eq!(p.wal_commit(), WalCommitTier::Buffered);
    assert_eq!(p.wal_retention_max_bytes(), DEFAULT_WAL_RETENTION_MAX_BYTES);
    assert_eq!(
        p.on_wal_retention_exceeded(),
        RetentionOverflow::RefuseWrites
    );
    assert_eq!(p.replication_serve(), ReplicationServe::Durable);
}

/// (b) `init_with_durability(Synced)` → the repo reads back Synced AND a
/// commit through the db leaves the WAL's recoverability prefix `R` at
/// the commit version (Synced acks at the fsynced watermark).
#[tokio::test]
async fn synced_policy_reaches_repo_and_acks_fsynced() {
    let dir = tempfile::tempdir().unwrap();
    let sys_path = dir.path().join("meta.redb");
    let repo_path = dir.path().join("data.redb");
    let shamir =
        ShamirDb::init_with_durability(SystemStoreConfig::Fjall(sys_path), synced_policy())
            .await
            .unwrap();
    shamir.create_db("appdb").await;
    shamir
        .add_repo(
            "appdb",
            RepoConfig::new("data", BoxRepoFactory::fjall(&repo_path))
                .add_table(TableConfig::new("items")),
        )
        .await
        .unwrap();
    let repo = shamir
        .get_db("appdb")
        .unwrap()
        .get_repo("data")
        .expect("repo present");
    assert_eq!(repo.durability_policy().wal_commit(), WalCommitTier::Synced);
    let group = repo.wal_group().await.unwrap();
    let calls = group.instance_sync_calls();
    let version = commit_one(&shamir, "appdb", "data", "items").await;
    if group.is_mem_sink() {
        assert_eq!(group.instance_sync_calls(), calls);
        assert_eq!(group.instance_sync_rounds(), 0);
    }
    assert_eq!(
        repo.wal_group().await.unwrap().recoverable_through(),
        version,
        "Synced policy must ack at the fsynced watermark R"
    );
}

/// (c) `wal_retention_max_bytes` below the `2 × effective segment size`
/// floor → init fails with the resolver error naming the key.
#[tokio::test]
async fn retention_below_floor_fails_init() {
    let tiny = DurabilityPolicy::new(
        WalCommitTier::Buffered,
        1,
        RetentionOverflow::RefuseWrites,
        ReplicationServe::Durable,
    );
    let err = ShamirDb::init_with_durability(SystemStoreConfig::InMemory, tiny)
        .await
        .err()
        .expect("below-floor retention must fail init");
    assert!(
        err.to_string().contains("wal_retention_max_bytes"),
        "error must name the configuration key: {err}"
    );
}

/// (d) `replication_serve: completed_reseed` → init errors with the exact
/// rejection message.
#[tokio::test]
async fn completed_reseed_fails_init() {
    let policy = DurabilityPolicy::new(
        WalCommitTier::Buffered,
        DEFAULT_WAL_RETENTION_MAX_BYTES,
        RetentionOverflow::RefuseWrites,
        ReplicationServe::CompletedReseed,
    );
    let err = ShamirDb::init_with_durability(SystemStoreConfig::InMemory, policy)
        .await
        .err()
        .expect("completed_reseed must be rejected");
    assert!(
        err.to_string().contains(
            "completed_reseed is not available yet (planned: epoch bump + universal fencing)"
        ),
        "exact rejection message expected, got: {err}"
    );
}

/// Mem-backed Synced policy must issue no coordinator demand or round.
#[tokio::test]
async fn mem_repo_synced_policy_commits_recoverably() {
    let shamir = ShamirDb::init_with_durability(SystemStoreConfig::InMemory, synced_policy())
        .await
        .unwrap();
    shamir.create_db("appdb").await;
    shamir
        .add_repo(
            "appdb",
            RepoConfig::new("data", BoxRepoFactory::in_memory())
                .add_table(TableConfig::new("items")),
        )
        .await
        .unwrap();
    let repo = shamir
        .get_db("appdb")
        .unwrap()
        .get_repo("data")
        .expect("repo present");
    // The policy IS installed (the ignore is a backend capability, not a
    // plumbing gap).
    assert_eq!(repo.durability_policy().wal_commit(), WalCommitTier::Synced);
    let group = repo.wal_group().await.unwrap();
    let calls = group.instance_sync_calls();
    let version = commit_one(&shamir, "appdb", "data", "items").await;
    if group.is_mem_sink() {
        assert_eq!(group.instance_sync_calls(), calls);
        assert_eq!(group.instance_sync_rounds(), 0);
    }
    assert_eq!(
        repo.wal_group().await.unwrap().recoverable_through(),
        version,
        "Mem sink: R == W, the commit is recoverable immediately"
    );
}

/// Boot catalogue reopening must install the supplied policy before recovery.
#[tokio::test]
async fn boot_reopen_inherits_policy_and_recovered_prefix() {
    let dir = tempfile::TempDir::new().unwrap();
    let meta = dir.path().join("metadata");
    let data = dir.path().join("data");
    let shamir =
        ShamirDb::init_with_durability(SystemStoreConfig::Fjall(meta.clone()), synced_policy())
            .await
            .unwrap();
    shamir.create_db("appdb").await;
    shamir
        .add_repo(
            "appdb",
            RepoConfig::new("data", BoxRepoFactory::fjall(&data))
                .add_table(TableConfig::new("items")),
        )
        .await
        .unwrap();
    let version = commit_one(&shamir, "appdb", "data", "items").await;
    shamir.flush_all().await.unwrap();
    drop(shamir);
    let reopened = ShamirDb::init_with_durability(SystemStoreConfig::Fjall(meta), synced_policy())
        .await
        .unwrap();
    let repo = reopened.get_db("appdb").unwrap().get_repo("data").unwrap();
    assert_eq!(repo.durability_policy().wal_commit(), WalCommitTier::Synced);
    assert_eq!(
        repo.wal_group().await.unwrap().recoverable_through(),
        version
    );
    assert_eq!(
        reopened
            .system_store()
            .system_repo()
            .unwrap()
            .durability_policy()
            .wal_commit(),
        WalCommitTier::Synced
    );
    let next = commit_one(&reopened, "appdb", "data", "items").await;
    assert_eq!(next, version + 1);
    assert_eq!(repo.wal_group().await.unwrap().recoverable_through(), next);
    reopened.flush_all().await.unwrap();
}

#[tokio::test]
async fn mem_policy_warning_is_once_per_open_not_per_commit() {
    struct Warnings(std::sync::atomic::AtomicUsize);
    impl log::Log for Warnings {
        fn enabled(&self, metadata: &log::Metadata<'_>) -> bool {
            metadata.level() == log::Level::Warn
        }
        fn log(&self, record: &log::Record<'_>) {
            if self.enabled(record.metadata()) {
                let message = record.args().to_string();
                if message.contains("repo 'warn-once': Mem WAL sink") {
                    self.0.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                }
            }
        }
        fn flush(&self) {}
    }
    static WARNINGS: Warnings = Warnings(std::sync::atomic::AtomicUsize::new(0));
    // Another in-process test may already own the logger. Preserve behavior
    // coverage then, but only assert capture when this logger was installed.
    let capturing = log::set_logger(&WARNINGS).is_ok();
    if capturing {
        log::set_max_level(log::LevelFilter::Warn);
    }
    let shamir = ShamirDb::init_with_durability(SystemStoreConfig::InMemory, synced_policy())
        .await
        .unwrap();
    shamir.create_db("appdb").await;
    shamir
        .add_repo(
            "appdb",
            RepoConfig::new("warn-once", BoxRepoFactory::in_memory())
                .add_table(TableConfig::new("items")),
        )
        .await
        .unwrap();
    let repo = shamir
        .get_db("appdb")
        .unwrap()
        .get_repo("warn-once")
        .unwrap();
    let clone = repo.clone();
    repo.repo_wal().await.unwrap();
    clone.repo_wal().await.unwrap();
    commit_one(&shamir, "appdb", "warn-once", "items").await;
    commit_one(&shamir, "appdb", "warn-once", "items").await;
    if capturing {
        assert_eq!(WARNINGS.0.load(std::sync::atomic::Ordering::Relaxed), 1);
    }
    shamir.flush_all().await.unwrap();
}
