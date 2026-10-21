//! End-to-end tests for per-repo durability overrides: CREATE persistence and
//! restart reload, ALTER merge/live swap, validation failures, ACLs, legacy
//! records, backend behavior, and unprotected-path negative controls.

use core::num::NonZeroU64;

use shamir_collections::TMap;
use shamir_db::access::Actor;
use shamir_db::engine::repo::durability_policy::WalCommitTier;
use shamir_db::query::batch::BatchError;
use shamir_db::{ShamirDb, SystemStoreConfig};
use shamir_query_builder::batch::Batch;
use shamir_query_builder::ddl;
use shamir_query_builder::write::{doc, Insert};
use shamir_query_types::durability::{DurabilityOverride, ReplicationServe, RetentionOverflow};
use shamir_types::types::value::QueryValue;

async fn setup_memory() -> ShamirDb {
    let db = ShamirDb::init_memory().await.unwrap();
    db.create_db("testdb").await;
    db
}

async fn describe_repo(db: &ShamirDb, name: &str) -> QueryValue {
    let mut batch = Batch::new();
    batch.id("describe_repo");
    batch.describe_repo("describe", ddl::describe_repo(name).repo("testdb"));
    let response = db
        .execute("testdb", &batch.to_request_via_msgpack())
        .await
        .unwrap();
    response.results["describe"].records[0]
        .as_value()
        .into_owned()
}

/// Re-open a durable ShamirDb on the same system-store path, retrying
/// briefly while the dropped instance's store still holds the fjall file lock
/// (the lock is released lazily after the drop).
async fn reopen_durable(system_path: std::path::PathBuf) -> ShamirDb {
    for _ in 0..100 {
        match ShamirDb::init(SystemStoreConfig::Fjall(system_path.clone())).await {
            Ok(db) => return db,
            Err(e) => {
                let message = e.to_string();
                assert!(
                    message.contains("Locked"),
                    "unexpected reopen error: {message}",
                );
                tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            }
        }
    }
    panic!("fjall lock was not released within the retry window");
}

fn synced_override() -> DurabilityOverride {
    DurabilityOverride {
        wal_commit: Some(WalCommitTier::Synced),
        ..Default::default()
    }
}

fn big_retention_override() -> DurabilityOverride {
    DurabilityOverride {
        wal_retention_max_bytes: Some(NonZeroU64::new(64 * 1024 * 1024).unwrap()),
        ..Default::default()
    }
}

fn open_policy(db: &ShamirDb, database: &str, repo: &str) -> WalCommitTier {
    db.get_db(database)
        .unwrap()
        .get_repo(repo)
        .unwrap()
        .durability_policy()
        .wal_commit()
}

fn err_code(error: &BatchError) -> Option<&str> {
    match error {
        BatchError::QueryError { code, .. } => code.as_deref(),
        _ => None,
    }
}

/// CREATE persists the sparse override, DESCRIBE separates effective from
/// overridden fields, and a fresh ShamirDb reloads it ahead of defaults.
#[tokio::test]
async fn create_with_override_describe_and_restart_reload() {
    let dir = tempfile::tempdir().unwrap();
    let system_path = dir.path().join("meta.redb");
    let db = ShamirDb::init(SystemStoreConfig::Fjall(system_path.clone()))
        .await
        .unwrap();
    db.create_db("testdb").await;

    let override_policy = DurabilityOverride {
        wal_commit: Some(WalCommitTier::Synced),
        wal_retention_max_bytes: Some(NonZeroU64::new(64 * 1024 * 1024).unwrap()),
        on_wal_retention_exceeded: Some(RetentionOverflow::RefuseWrites),
        replication_serve: Some(ReplicationServe::Durable),
    };
    let mut batch = Batch::new();
    batch.id(1);
    batch.create_repo(
        "create_override",
        ddl::create_repo("dur").durability(override_policy),
    );
    db.execute("testdb", &batch.to_request_via_msgpack())
        .await
        .unwrap();

    let mut batch = Batch::new();
    batch.id(2);
    batch.create_repo("create_plain", ddl::create_repo("plain"));
    db.execute("testdb", &batch.to_request_via_msgpack())
        .await
        .unwrap();

    let described = describe_repo(&db, "dur").await;
    assert_eq!(
        described["durability"]["wal_commit"].as_str(),
        Some("synced")
    );
    assert_eq!(
        described["durability_override"]["wal_commit"].as_str(),
        Some("synced")
    );
    assert_eq!(
        described["durability_override"]["wal_retention_max_bytes"].as_u64(),
        Some(64 * 1024 * 1024)
    );
    let plain = describe_repo(&db, "plain").await;
    assert_eq!(plain["durability"]["wal_commit"].as_str(), Some("buffered"));
    assert_eq!(plain["durability_override"], QueryValue::Null);

    drop(db);
    let reopened = reopen_durable(system_path).await;
    assert_eq!(
        open_policy(&reopened, "testdb", "dur"),
        WalCommitTier::Synced
    );
    assert_eq!(
        open_policy(&reopened, "testdb", "plain"),
        WalCommitTier::Buffered
    );
}

/// ALTER of an inherited repo swaps the live ArcSwap policy without changing
/// the server default; a later sparse patch preserves earlier fields.
#[tokio::test]
async fn alter_live_swaps_and_merges_open_repo_policy() {
    let db = setup_memory().await;
    let mut batch = Batch::new();
    batch.id(1);
    batch.create_repo("create", ddl::create_repo("r1"));
    db.execute("testdb", &batch.to_request_via_msgpack())
        .await
        .unwrap();

    let mut batch = Batch::new();
    batch.id(2);
    batch.alter_repo(
        "alter",
        ddl::alter_repo("r1").durability(Some(synced_override())),
    );
    db.execute("testdb", &batch.to_request_via_msgpack())
        .await
        .unwrap();
    assert_eq!(open_policy(&db, "testdb", "r1"), WalCommitTier::Synced);
    assert_eq!(db.durability_policy().wal_commit(), WalCommitTier::Buffered);

    let patch = DurabilityOverride {
        replication_serve: Some(ReplicationServe::Durable),
        ..Default::default()
    };
    let mut batch = Batch::new();
    batch.id(3);
    batch.alter_repo("alter_patch", ddl::alter_repo("r1").durability(Some(patch)));
    db.execute("testdb", &batch.to_request_via_msgpack())
        .await
        .unwrap();
    let described = describe_repo(&db, "r1").await;
    assert_eq!(
        described["durability_override"]["wal_commit"].as_str(),
        Some("synced")
    );
}

/// Invalid CREATE overrides (retention under 2× the segment) leave no open
/// repository behind; `completed_reseed` is a VALID serve mode since PR8
/// chunk 1 and must create cleanly instead.
#[tokio::test]
async fn invalid_create_overrides_leave_no_repo() {
    let db = setup_memory().await;
    for (id, name, over) in [(
        1,
        "too_small",
        DurabilityOverride {
            wal_retention_max_bytes: Some(NonZeroU64::new(1024).unwrap()),
            ..Default::default()
        },
    )] {
        let mut batch = Batch::new();
        batch.id(id);
        batch.create_repo("create", ddl::create_repo(name).durability(over));
        let error = db
            .execute("testdb", &batch.to_request_via_msgpack())
            .await
            .unwrap_err();
        assert_eq!(err_code(&error), Some("invalid_request"));
        assert!(!db.get_db("testdb").unwrap().has_repo(name));
    }
    let mut batch = Batch::new();
    batch.id(2);
    batch.create_repo(
        "create",
        ddl::create_repo("reseed").durability(DurabilityOverride {
            replication_serve: Some(ReplicationServe::CompletedReseed),
            ..Default::default()
        }),
    );
    db.execute("testdb", &batch.to_request_via_msgpack())
        .await
        .unwrap();
    assert!(db.get_db("testdb").unwrap().has_repo("reseed"));
}

/// Invalid ALTER must not update either the live policy or stored override.
#[tokio::test]
async fn invalid_alter_has_no_effect_on_live_policy_or_record() {
    let db = setup_memory().await;
    let mut batch = Batch::new();
    batch.id(1);
    batch.create_repo("create", ddl::create_repo("r1"));
    db.execute("testdb", &batch.to_request_via_msgpack())
        .await
        .unwrap();

    let invalid = DurabilityOverride {
        wal_retention_max_bytes: Some(NonZeroU64::new(1024).unwrap()),
        ..Default::default()
    };
    let mut batch = Batch::new();
    batch.id(2);
    batch.alter_repo("alter", ddl::alter_repo("r1").durability(Some(invalid)));
    let error = db
        .execute("testdb", &batch.to_request_via_msgpack())
        .await
        .unwrap_err();
    assert_eq!(err_code(&error), Some("invalid_request"));
    assert_eq!(open_policy(&db, "testdb", "r1"), WalCommitTier::Buffered);
    assert_eq!(
        describe_repo(&db, "r1").await["durability_override"],
        QueryValue::Null
    );
}

/// An actor without repo Write gets the same access_denied response shape as
/// rename_repo; neither policy nor record changes.
#[tokio::test]
async fn alter_repo_requires_write_permission() {
    let db = setup_memory().await;
    let mut batch = Batch::new();
    batch.id(1);
    batch.create_repo("create", ddl::create_repo("r1"));
    db.execute("testdb", &batch.to_request_via_msgpack())
        .await
        .unwrap();

    let mut batch = Batch::new();
    batch.id(2);
    batch.alter_repo(
        "alter",
        ddl::alter_repo("r1").durability(Some(synced_override())),
    );
    let error = db
        .execute_as(Actor::User(999), "testdb", &batch.to_request_via_msgpack())
        .await
        .unwrap_err();
    assert_eq!(err_code(&error), Some("access_denied"));
    assert_eq!(open_policy(&db, "testdb", "r1"), WalCommitTier::Buffered);
    assert_eq!(
        describe_repo(&db, "r1").await["durability_override"],
        QueryValue::Null
    );
}

/// Unknown override fields are rejected by deny_unknown_fields during
/// MessagePack decoding, before the admin handler runs.
#[tokio::test]
async fn unknown_durability_key_rejected_at_decode() {
    let mut durability: TMap<String, QueryValue> = TMap::default();
    durability.insert("wal_commit".into(), QueryValue::Str("synced".into()));
    durability.insert("not_a_policy".into(), QueryValue::Bool(true));
    let mut entry: TMap<String, QueryValue> = TMap::default();
    entry.insert("create_repo".into(), QueryValue::Str("bad".into()));
    entry.insert("durability".into(), QueryValue::Map(durability));
    entry.insert("return_result".into(), QueryValue::Bool(true));
    let mut queries: TMap<String, QueryValue> = TMap::default();
    queries.insert("create".into(), QueryValue::Map(entry));
    let mut request: TMap<String, QueryValue> = TMap::default();
    request.insert("id".into(), QueryValue::Str("unknown-field".into()));
    request.insert("queries".into(), QueryValue::Map(queries));
    let bytes = rmp_serde::to_vec(&QueryValue::Map(request)).unwrap();
    let decoded: Result<shamir_db::query::batch::BatchRequest, _> = rmp_serde::from_slice(&bytes);
    assert!(decoded.is_err());
}

/// Missing ALTER/DESCRIBE targets are rejected with QueryError.
#[tokio::test]
async fn alter_and_describe_missing_repo_refused() {
    let db = setup_memory().await;
    let mut batch = Batch::new();
    batch.id(1);
    batch.alter_repo(
        "alter",
        ddl::alter_repo("ghost").durability(Some(synced_override())),
    );
    assert!(db
        .execute("testdb", &batch.to_request_via_msgpack())
        .await
        .is_err());

    let mut batch = Batch::new();
    batch.id(2);
    batch.describe_repo("describe", ddl::describe_repo("ghost").repo("testdb"));
    assert!(db
        .execute("testdb", &batch.to_request_via_msgpack())
        .await
        .is_err());
}

/// In-memory WAL ignores the synced commit tier effectively while the
/// stored override remains visible as Synced.
#[tokio::test]
async fn in_memory_override_is_stored_but_not_effective_for_wal() {
    let db = setup_memory().await;
    let mut batch = Batch::new();
    batch.id(1);
    batch.create_repo(
        "create",
        ddl::create_repo("mem")
            .engine("in_memory")
            .durability(synced_override()),
    );
    db.execute("testdb", &batch.to_request_via_msgpack())
        .await
        .unwrap();
    let repo = db.get_db("testdb").unwrap().get_repo("mem").unwrap();
    assert_eq!(repo.durability_policy().wal_commit(), WalCommitTier::Synced);
    assert_eq!(
        repo.durability_policy().effective_wal_commit(),
        WalCommitTier::Buffered
    );
    let described = describe_repo(&db, "mem").await;
    assert_eq!(
        described["durability"]["wal_commit"].as_str(),
        Some("buffered")
    );
    assert_eq!(
        described["durability_override"]["wal_commit"].as_str(),
        Some("synced")
    );
}

/// A disk-backed hybrid repository accepts and applies Synced override policy.
#[tokio::test]
async fn hybrid_repo_accepts_durability_override() {
    let dir = tempfile::tempdir().unwrap();
    let system_path = dir.path().join("meta.redb");
    let db = ShamirDb::init(SystemStoreConfig::Fjall(system_path))
        .await
        .unwrap();
    db.create_db("testdb").await;
    let mut batch = Batch::new();
    batch.id(1);
    batch.create_repo(
        "create",
        ddl::create_repo("hybrid_repo")
            .engine("hybrid")
            .durability(synced_override()),
    );
    db.execute("testdb", &batch.to_request_via_msgpack())
        .await
        .unwrap();
    let repo = db
        .get_db("testdb")
        .unwrap()
        .get_repo("hybrid_repo")
        .unwrap();
    assert_eq!(
        repo.durability_policy().effective_wal_commit(),
        WalCommitTier::Buffered
    );
    let described = describe_repo(&db, "hybrid_repo").await;
    assert_eq!(
        described["durability"]["wal_commit"].as_str(),
        Some("buffered")
    );
    assert_eq!(
        described["durability_override"]["wal_commit"].as_str(),
        Some("synced")
    );
}

/// The public DB path persists and live-swaps ALTER before returning; the
/// subsequent committed prefix is recoverable when sampled after completion.
#[tokio::test]
async fn alter_repo_synced_commit_has_recoverable_prefix_after_return() {
    let dir = tempfile::tempdir().unwrap();
    let data_root = dir.path().join("metadata");
    let db = ShamirDb::init(SystemStoreConfig::Fjall(data_root.join("system.redb")))
        .await
        .unwrap();
    db.create_db("testdb").await;
    let mut batch = Batch::new();
    batch.id(1);
    batch.create_repo("create", ddl::create_repo("r1").engine("fjall"));
    db.execute("testdb", &batch.to_request_via_msgpack())
        .await
        .unwrap();

    let mut batch = Batch::new();
    batch.id(2);
    batch.alter_repo(
        "alter",
        ddl::alter_repo("r1").durability(Some(synced_override())),
    );
    db.execute("testdb", &batch.to_request_via_msgpack())
        .await
        .unwrap();

    let mut create_items = Batch::new();
    create_items.id(3);
    create_items.create_table("create_items", ddl::create_table("items").repo("r1"));
    db.execute("testdb", &create_items.to_request_via_msgpack())
        .await
        .unwrap();

    let mut batch = Batch::new();
    batch.id(4);
    batch.insert(
        "insert_item",
        Insert::with_repo("r1", "items").row(doc().set("value", 1)),
    );
    db.execute("testdb", &batch.to_request_via_msgpack())
        .await
        .unwrap();
    let committed = db
        .get_db("testdb")
        .unwrap()
        .get_repo("r1")
        .unwrap()
        .wal_group()
        .await
        .unwrap()
        .max_committed_version();
    assert!(committed > 0, "the DB client must commit through r1");
    assert_eq!(
        db.get_db("testdb")
            .unwrap()
            .get_repo("r1")
            .unwrap()
            .wal_group()
            .await
            .unwrap()
            .recoverable_through(),
        committed,
        "Synced policy should cover the committed prefix sampled after the request returns"
    );
}

/// Direct record-only persistence is the negative control: record changes,
/// but an already-open repo's live policy does not. The ALTER handler's live
/// swap test above would fail if only this record-writing behavior occurred.
#[tokio::test]
async fn direct_record_only_update_does_not_live_swap_policy() {
    let db = setup_memory().await;
    let mut batch = Batch::new();
    batch.id(1);
    batch.create_repo("create", ddl::create_repo("r1"));
    db.execute("testdb", &batch.to_request_via_msgpack())
        .await
        .unwrap();

    let record = db
        .system_store()
        .load_repository_record("testdb", "r1")
        .await
        .unwrap()
        .unwrap();
    let meta = shamir_db::access::ResourceMeta::from_record(&record);
    db.system_store()
        .save_repository(
            "testdb",
            "r1",
            record["engine"].as_str().unwrap(),
            record["path"].as_str(),
            &meta,
            Some(&synced_override()),
        )
        .await
        .unwrap();
    assert_eq!(open_policy(&db, "testdb", "r1"), WalCommitTier::Buffered);
    let described = describe_repo(&db, "r1").await;
    assert_eq!(
        described["durability_override"]["wal_commit"].as_str(),
        Some("synced")
    );
    assert_eq!(
        described["durability"]["wal_commit"].as_str(),
        Some("buffered")
    );
}

/// Concurrent partial patches preserve both fields: the per-db admin lock
/// serializes load/merge/persist/live-swap sequences.
#[tokio::test]
async fn concurrent_alters_preserve_both_patches() {
    let db = setup_memory().await;
    let mut batch = Batch::new();
    batch.id(1);
    batch.create_repo("create", ddl::create_repo("r1"));
    db.execute("testdb", &batch.to_request_via_msgpack())
        .await
        .unwrap();

    let mut tier_batch = Batch::new();
    tier_batch.id(2);
    tier_batch.alter_repo(
        "alter_tier",
        ddl::alter_repo("r1").durability(Some(synced_override())),
    );
    let mut retention_batch = Batch::new();
    retention_batch.id(3);
    retention_batch.alter_repo(
        "alter_retention",
        ddl::alter_repo("r1").durability(Some(big_retention_override())),
    );
    let tier_request = tier_batch.to_request_via_msgpack();
    let retention_request = retention_batch.to_request_via_msgpack();
    let (tier_result, retention_result) = tokio::join!(
        db.execute("testdb", &tier_request),
        db.execute("testdb", &retention_request),
    );
    tier_result.unwrap();
    retention_result.unwrap();

    let described = describe_repo(&db, "r1").await;
    assert_eq!(
        described["durability_override"]["wal_commit"].as_str(),
        Some("synced")
    );
    assert_eq!(
        described["durability_override"]["wal_retention_max_bytes"].as_u64(),
        Some(64 * 1024 * 1024)
    );
}
