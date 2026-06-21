//! End-to-end tests for ShamirDb::execute.

use shamir_query_builder::batch::Batch;
use shamir_query_builder::filter::eq;
use shamir_query_builder::write::{delete, doc, insert, update, UpdateReturnMode};
use shamir_query_builder::Query;

use crate::engine::repo::repo_types::BoxRepoFactory;
use crate::engine::repo::RepoConfig;
use crate::engine::table::TableConfig;
use crate::ShamirDb;

async fn setup_shamir() -> ShamirDb {
    let shamir = ShamirDb::init_memory().await.unwrap();
    let db = shamir.create_db("testdb").await;

    let repo_config = RepoConfig::new("main", BoxRepoFactory::in_memory())
        .add_table(TableConfig::new("users"))
        .add_table(TableConfig::new("orders"));

    db.add_repo(repo_config).await.unwrap();
    shamir
}

// ============================================================================
// Basic single operations
// ============================================================================

#[tokio::test]
async fn test_execute_single_insert() {
    let shamir = setup_shamir().await;

    let mut b = Batch::new();
    b.id(1);
    b.insert(
        "ins",
        insert("users")
            .row(doc().set("name", "Alice").set("age", 30))
            .row(doc().set("name", "Bob").set("age", 25)),
    );
    let req = b.to_request_via_msgpack();

    let resp = shamir.execute("testdb", &req).await.unwrap();
    assert_eq!(resp.results["ins"].records.len(), 2);
}

#[tokio::test]
async fn test_execute_single_read() {
    let shamir = setup_shamir().await;

    // Seed
    let mut b = Batch::new();
    b.id(1);
    b.op_silent(
        "s",
        insert("users")
            .row(doc().set("name", "Alice"))
            .row(doc().set("name", "Bob")),
    );
    let seed = b.to_request_via_msgpack();
    shamir.execute("testdb", &seed).await.unwrap();

    // Read
    let mut b = Batch::new();
    b.id(1);
    b.query("users", Query::from("users"));
    let req = b.to_request_via_msgpack();
    let resp = shamir.execute("testdb", &req).await.unwrap();

    assert_eq!(resp.results["users"].records.len(), 2);
}

// ============================================================================
// Full CRUD pipeline in one batch
// ============================================================================

#[tokio::test]
async fn test_execute_crud_pipeline() {
    let shamir = setup_shamir().await;

    // 1. Insert users
    let mut b = Batch::new();
    b.id(1);
    b.op_silent(
        "ins",
        insert("users")
            .row(doc().set("name", "Alice").set("status", "active"))
            .row(doc().set("name", "Bob").set("status", "inactive"))
            .row(doc().set("name", "Carol").set("status", "active")),
    );
    let q1 = b.to_request_via_msgpack();
    shamir.execute("testdb", &q1).await.unwrap();

    // 2. Update: activate Bob
    let mut b = Batch::new();
    b.id(1);
    b.update(
        "upd",
        update("users")
            .where_(eq("name", "Bob"))
            .set(doc().set("status", "active"))
            .returning(UpdateReturnMode::Changed),
    );
    let q2 = b.to_request_via_msgpack();
    let resp = shamir.execute("testdb", &q2).await.unwrap();
    assert_eq!(resp.results["upd"].records.len(), 1);
    assert_eq!(
        resp.results["upd"].records[0].get_value_str("status"),
        Some("active")
    );

    // 3. Delete Carol + read remaining
    let mut b = Batch::new();
    b.id(1);
    b.delete("del", delete("users").where_(eq("name", "Carol")));
    b.query("remaining", Query::from("users"));
    let q3 = b.to_request_via_msgpack();
    let resp = shamir.execute("testdb", &q3).await.unwrap();

    assert_eq!(resp.results["remaining"].records.len(), 2);
}

// ============================================================================
// Multi-table batch with $query dependency
// ============================================================================

#[tokio::test]
async fn test_execute_multi_table_with_dependency() {
    let shamir = setup_shamir().await;

    // Seed users and orders
    let mut b = Batch::new();
    b.id(1);
    b.op_silent(
        "s1",
        insert("users")
            .row(doc().set("name", "Alice").set("tier", "vip"))
            .row(doc().set("name", "Bob").set("tier", "basic")),
    );
    b.op_silent(
        "s2",
        insert("orders")
            .row(doc().set("user", "Alice").set("amount", 100))
            .row(doc().set("user", "Bob").set("amount", 50))
            .row(doc().set("user", "Alice").set("amount", 200)),
    );
    let seed = b.to_request_via_msgpack();
    shamir.execute("testdb", &seed).await.unwrap();

    // Query: find VIP users, then find their orders
    let mut b = Batch::new();
    b.id(1);
    let vips = b.query("vips", Query::from("users").where_eq("tier", "vip"));
    b.query(
        "vip_orders",
        Query::from("orders").where_eq("user", vips.first().field("name")),
    );
    let req = b.to_request_via_msgpack();

    let resp = shamir.execute("testdb", &req).await.unwrap();

    // Stage 1: vips -> Alice
    assert_eq!(resp.results["vips"].records.len(), 1);
    // Stage 2: vip_orders -> Alice's orders (2)
    assert_eq!(resp.results["vip_orders"].records.len(), 2);
    assert_eq!(resp.execution_plan.len(), 2);
}

// ============================================================================
// Error: unknown database
// ============================================================================

#[tokio::test]
async fn test_execute_unknown_db() {
    let shamir = setup_shamir().await;

    let mut b = Batch::new();
    b.id(1);
    b.query("r", Query::from("users"));
    let req = b.to_request_via_msgpack();

    let err = shamir.execute("nonexistent", &req).await.unwrap_err();
    assert!(matches!(
        err,
        crate::query::batch::BatchError::QueryError { .. }
    ));
}

// ============================================================================
// Migration ops -- Phase A stubs return "not yet implemented"
// ============================================================================

#[tokio::test]
async fn test_migration_lifecycle_in_memory() {
    use shamir_query_builder::ddl;

    let shamir = setup_shamir().await;

    // Seed some data
    let mut b = Batch::new();
    b.id(0);
    b.op_silent(
        "s",
        insert("users")
            .row(doc().set("name", "Alice"))
            .row(doc().set("name", "Bob"))
            .row(doc().set("name", "Carol")),
    );
    let seed = b.to_request_via_msgpack();
    shamir.execute("testdb", &seed).await.unwrap();

    // Start migration: users from main -> cold (in_memory)
    let mut b = Batch::new();
    b.id(1);
    b.start_migration("mig", ddl::start_migration("users", "cold", "in_memory"));
    let req = b.to_request_via_msgpack();
    let resp = shamir.execute("testdb", &req).await.unwrap();
    let mig_result = &resp.results["mig"].records[0];
    assert_eq!(mig_result.get_value_str("phase"), Some("cutover_ready"));
    let migration_id = mig_result
        .get_value_str("migration_id")
        .unwrap()
        .to_string();

    // Query status
    let mut b = Batch::new();
    b.id(2);
    b.migration_status("s", ddl::migration_status(&migration_id));
    let status_req = b.to_request_via_msgpack();
    let status_resp = shamir.execute("testdb", &status_req).await.unwrap();
    let status = &status_resp.results["s"].records[0];
    assert_eq!(status.get_value_str("phase"), Some("cutover_ready"));
    assert_eq!(status.get_value_i64("records_copied"), Some(3));

    // Commit
    let mut b = Batch::new();
    b.id(3);
    b.commit_migration("c", ddl::commit_migration(&migration_id));
    let commit_req = b.to_request_via_msgpack();
    let commit_resp = shamir.execute("testdb", &commit_req).await.unwrap();
    let commit = &commit_resp.results["c"].records[0];
    assert_eq!(commit.get_value_str("phase"), Some("committed"));
    assert_eq!(commit.get_value_i64("src_records"), Some(3));
    assert_eq!(commit.get_value_i64("dst_records"), Some(3));

    // Read from the destination table
    let mut b = Batch::new();
    b.id(4);
    b.query("r", Query::with_repo("cold", "users"));
    let read_req = b.to_request_via_msgpack();
    let read_resp = shamir.execute("testdb", &read_req).await.unwrap();
    assert_eq!(read_resp.results["r"].records.len(), 3);
}

#[tokio::test]
async fn test_migration_rollback() {
    use shamir_query_builder::ddl;

    let shamir = setup_shamir().await;

    // Seed
    let mut b = Batch::new();
    b.id(0);
    b.op_silent("s", insert("users").row(doc().set("name", "Alice")));
    let seed = b.to_request_via_msgpack();
    shamir.execute("testdb", &seed).await.unwrap();

    // Start migration
    let mut b = Batch::new();
    b.id(1);
    b.start_migration(
        "mig",
        ddl::start_migration("users", "rollback_dst", "in_memory"),
    );
    let req = b.to_request_via_msgpack();
    let resp = shamir.execute("testdb", &req).await.unwrap();
    let migration_id = resp.results["mig"].records[0]
        .get_value_str("migration_id")
        .unwrap()
        .to_string();

    // Rollback
    let mut b = Batch::new();
    b.id(2);
    b.rollback_migration("r", ddl::rollback_migration(&migration_id));
    let rb_req = b.to_request_via_msgpack();
    let rb_resp = shamir.execute("testdb", &rb_req).await.unwrap();
    assert_eq!(
        rb_resp.results["r"].records[0].get_value_str("phase"),
        Some("rolled_back")
    );

    // Status should fail (migration removed)
    let mut b = Batch::new();
    b.id(3);
    b.migration_status("s", ddl::migration_status(&migration_id));
    let status_req = b.to_request_via_msgpack();
    let status_err = shamir.execute("testdb", &status_req).await.unwrap_err();
    assert!(matches!(
        status_err,
        crate::query::batch::BatchError::QueryError { .. }
    ));
}

#[tokio::test]
async fn test_migration_unknown_id() {
    use shamir_query_builder::ddl;

    let shamir = setup_shamir().await;

    let mut b = Batch::new();
    b.id(1);
    b.commit_migration("c", ddl::commit_migration("nonexistent"));
    let req = b.to_request_via_msgpack();
    let err = shamir.execute("testdb", &req).await.unwrap_err();
    assert!(matches!(
        err,
        crate::query::batch::BatchError::QueryError { .. }
    ));
}

// ============================================================================
// CreateUser stores an Argon2id hash, never the plaintext password
// ============================================================================

#[tokio::test]
async fn test_create_user_hashes_password_at_rest() {
    use argon2::password_hash::{PasswordHash, PasswordVerifier};
    use argon2::Argon2;
    use shamir_query_builder::ddl;

    let shamir = setup_shamir().await;
    let plaintext = "correct horse battery staple";

    // Create a user through the wire-facing batch path.
    let mut b = Batch::new();
    b.id(1);
    b.create_user(
        "cu",
        ddl::create_user("alice", plaintext).roles(["readonly"]),
    );
    let req = b.to_request_via_msgpack();
    shamir.execute("testdb", &req).await.unwrap();

    // Read the raw record straight from the system-store `users` table
    // (the `list users` path strips `password_hash`, so we go direct).
    let table = shamir.system_store().users_table().await.unwrap();
    let interner = table.interner().get().await.unwrap();
    let refs = crate::types::common::new_map();
    let ctx = crate::query::filter::FilterContext::new(interner, &refs);
    let query =
        crate::query::read::ReadQuery::new("users").filter(crate::query::filter::Filter::Eq {
            field: vec!["name".to_string()],
            value: crate::query::filter::FilterValue::String("alice".to_string()),
        });
    let result = table.read(&query, &ctx).await.unwrap();
    assert_eq!(result.records.len(), 1, "alice must be stored");

    let stored_qv = result.records[0].as_value();
    let stored = stored_qv
        .get("password_hash")
        .and_then(|v| v.as_str())
        .unwrap();

    // The stored value must NOT be the plaintext.
    assert_ne!(
        stored, plaintext,
        "password must not be stored in plaintext"
    );

    // It must be a self-describing Argon2id PHC string.
    assert!(
        stored.starts_with("$argon2id$"),
        "expected an Argon2id PHC string, got {stored}"
    );

    // It must verify against the original password and reject a wrong one.
    let parsed = PasswordHash::new(stored).expect("stored hash must parse as PHC");
    assert!(
        Argon2::default()
            .verify_password(plaintext.as_bytes(), &parsed)
            .is_ok(),
        "stored hash must verify against the original password"
    );
    assert!(
        Argon2::default()
            .verify_password(b"wrong password", &parsed)
            .is_err(),
        "stored hash must reject a wrong password"
    );
}

// ============================================================================
// CreateRepo via the wire/execute path persists the repo to the catalogue
// ============================================================================

/// Re-open the system store, retrying briefly while the previous session's
/// store still holds the redb file lock (the MemBuffer-wrapped store releases
/// the lock a few ms after the owning `ShamirDb` is dropped). Mirrors the
/// helper in `system_metadata_tests`.
async fn reinit_with_retry(sys_path: std::path::PathBuf) -> ShamirDb {
    use crate::shamir_db::SystemStoreConfig;
    for _ in 0..100 {
        match ShamirDb::init(SystemStoreConfig::Fjall(sys_path.clone())).await {
            Ok(shamir) => return shamir,
            Err(_) => tokio::time::sleep(std::time::Duration::from_millis(20)).await,
        }
    }
    ShamirDb::init(SystemStoreConfig::Fjall(sys_path))
        .await
        .expect("system store still locked after retries")
}

/// A repo created through the wire/execute `CreateRepo` op must be persisted
/// to the system-store catalogue and re-attached after a restart -- symmetry
/// with `CreateTable` (which already routed through the persisting
/// `ShamirDb::add_table`).
///
/// The executor only accepts the `in_memory` engine for wire DDL, so the
/// repo's *data* legitimately does not survive a process restart; what must
/// survive is the catalogue *record* (re-attached as a fresh empty repo). The
/// system store itself is disk-backed (redb) so the record is durable across
/// the drop/reinit. Before this fix `CreateRepo` called the in-memory-only
/// `DbInstance::add_repo`, so the record was absent on restart.
#[tokio::test]
async fn create_repo_via_execute_persists_to_catalogue() {
    use crate::shamir_db::SystemStoreConfig;
    use shamir_query_builder::ddl;

    let sys_dir = tempfile::tempdir().unwrap();
    let sys_path = sys_dir.path().join("system.redb");

    // === Session 1: create a repo via the wire/execute CreateRepo op ===
    {
        let shamir = ShamirDb::init(SystemStoreConfig::Fjall(sys_path.clone()))
            .await
            .unwrap();
        shamir.create_db("production").await;

        let mut b = Batch::new();
        b.id(1);
        b.create_repo(
            "cr",
            ddl::create_repo("events_repo")
                .engine("in_memory")
                .tables(["events"]),
        );
        let req = b.to_request_via_msgpack();
        let resp = shamir.execute("production", &req).await.unwrap();
        assert_eq!(
            resp.results["cr"].records[0].get_value_str("created_repo"),
            Some("events_repo")
        );

        // Persisted to the catalogue immediately (not just in-memory).
        let repos = shamir.system_store().load_repositories().await.unwrap();
        assert!(
            repos
                .iter()
                .any(|r| r["repo_name"] == "events_repo" && r["db_name"] == "production"),
            "CreateRepo must persist the repo record to the system store"
        );
        // Its inline table is persisted too.
        let tables = shamir.system_store().load_tables().await.unwrap();
        assert!(
            tables.iter().any(|t| t["db_name"] == "production"
                && t["repo_name"] == "events_repo"
                && t["table_name"] == "events"),
            "CreateRepo must persist the inline table catalogue"
        );
    }

    // === Session 2: re-init over the SAME system store ===
    let shamir = reinit_with_retry(sys_path).await;
    let db = shamir.get_db("production").expect("db restored");
    assert!(
        db.has_repo("events_repo"),
        "repo created via execute must be re-attached from the catalogue after restart"
    );
    assert!(
        db.has_table("events_repo", "events"),
        "the repo's inline table must be restored after restart"
    );
}

/// A repo dropped through the wire/execute `DropRepo` op must be removed from
/// the catalogue and must NOT resurrect after a restart -- symmetry with
/// `CreateRepo`.
#[tokio::test]
async fn drop_repo_via_execute_clears_catalogue() {
    use crate::shamir_db::SystemStoreConfig;
    use shamir_query_builder::ddl;

    let sys_dir = tempfile::tempdir().unwrap();
    let sys_path = sys_dir.path().join("system.redb");

    {
        let shamir = ShamirDb::init(SystemStoreConfig::Fjall(sys_path.clone()))
            .await
            .unwrap();
        shamir.create_db("production").await;

        let mut b = Batch::new();
        b.id(1);
        b.create_repo("cr", ddl::create_repo("scratch_repo").engine("in_memory"));
        let create = b.to_request_via_msgpack();
        shamir.execute("production", &create).await.unwrap();

        let mut b = Batch::new();
        b.id(2);
        b.drop_repo("dr", ddl::drop_repo("scratch_repo"));
        let drop_req = b.to_request_via_msgpack();
        let resp = shamir.execute("production", &drop_req).await.unwrap();
        assert_eq!(
            resp.results["dr"].records[0].get_value_bool("existed"),
            Some(true)
        );

        let repos = shamir.system_store().load_repositories().await.unwrap();
        assert!(
            !repos.iter().any(|r| r["repo_name"] == "scratch_repo"),
            "DropRepo must remove the repo record from the system store"
        );
    }

    let shamir = reinit_with_retry(sys_path).await;
    let db = shamir.get_db("production").expect("db restored");
    assert!(
        !db.has_repo("scratch_repo"),
        "dropped repo must not resurrect after restart"
    );
}

// ============================================================================
// Error: unknown repo
// ============================================================================

#[tokio::test]
async fn test_execute_unknown_repo() {
    let shamir = setup_shamir().await;

    // Use a TableRef with a nonexistent repo (array format: ["repo", "table"])
    let mut b = Batch::new();
    b.id(1);
    b.query("r", Query::with_repo("nonexistent", "users"));
    let req = b.to_request_via_msgpack();

    let err = shamir.execute("testdb", &req).await.unwrap_err();
    assert!(matches!(
        err,
        crate::query::batch::BatchError::QueryError { .. }
    ));
}

// ============================================================================
// Path-traversal rejection in CreateRepo / CreateDb (audit fix 1)
// ============================================================================

#[tokio::test]
async fn create_repo_rejects_dotdot_name() {
    use shamir_query_builder::ddl;

    let shamir = setup_shamir().await;
    let mut b = Batch::new();
    b.id(1);
    b.create_repo("cr", ddl::create_repo("..").engine("in_memory"));
    let req = b.to_request_via_msgpack();
    let err = shamir.execute("testdb", &req).await.unwrap_err();
    let msg = format!("{:?}", err);
    assert!(
        msg.contains("disallowed") || msg.contains("'.'") || msg.contains("repo_name"),
        "expected path-traversal rejection, got: {msg}"
    );
}

#[tokio::test]
async fn create_repo_rejects_slash_in_name() {
    use shamir_query_builder::ddl;

    let shamir = setup_shamir().await;
    let mut b = Batch::new();
    b.id(1);
    b.create_repo("cr", ddl::create_repo("a/b").engine("in_memory"));
    let req = b.to_request_via_msgpack();
    let err = shamir.execute("testdb", &req).await.unwrap_err();
    let msg = format!("{:?}", err);
    assert!(
        msg.contains("disallowed") || msg.contains("repo_name"),
        "expected path-traversal rejection, got: {msg}"
    );
}

#[tokio::test]
async fn create_repo_accepts_valid_name() {
    use shamir_query_builder::ddl;

    let shamir = setup_shamir().await;
    let mut b = Batch::new();
    b.id(1);
    b.create_repo("cr", ddl::create_repo("my-repo_01").engine("in_memory"));
    let req = b.to_request_via_msgpack();
    let resp = shamir.execute("testdb", &req).await.unwrap();
    assert_eq!(
        resp.results["cr"].records[0].get_value_str("created_repo"),
        Some("my-repo_01")
    );
}

#[tokio::test]
async fn create_db_rejects_dotdot_name() {
    use shamir_query_builder::ddl;

    let shamir = ShamirDb::init_memory().await.unwrap();
    let mut b = Batch::new();
    b.id(1);
    b.create_db("cd", ddl::create_db(".."));
    let req = b.to_request_via_msgpack();
    // Use execute_as with System actor so the unknown-db auth check
    // passes; we need to reach CreateDb validation.
    // Actually, CreateDb is an admin op so we just use execute.
    // The db_name param here is just routing -- the create_db value is
    // the payload. We need a db that exists.
    shamir.create_db("testdb").await;
    let err = shamir.execute("testdb", &req).await.unwrap_err();
    let msg = format!("{:?}", err);
    assert!(
        msg.contains("disallowed") || msg.contains("'.'") || msg.contains("db_name"),
        "expected path-traversal rejection, got: {msg}"
    );
}
