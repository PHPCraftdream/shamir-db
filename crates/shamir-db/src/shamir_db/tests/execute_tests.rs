//! End-to-end tests for ShamirDb::execute.

use serde_json::json;

use crate::engine::repo::repo_types::BoxRepoFactory;
use crate::engine::repo::RepoConfig;
use crate::engine::table::TableConfig;
use crate::query::batch::BatchRequest;
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

    let req: BatchRequest = serde_json::from_value(json!({
        "id": 1,
        "queries": {
            "ins": {
                "insert_into": "users",
                "values": [
                    {"name": "Alice", "age": 30},
                    {"name": "Bob", "age": 25}
                ]
            }
        }
    }))
    .unwrap();

    let resp = shamir.execute("testdb", &req).await.unwrap();
    assert_eq!(resp.results["ins"].records.len(), 2);
}

#[tokio::test]
async fn test_execute_single_read() {
    let shamir = setup_shamir().await;

    // Seed
    let seed: BatchRequest = serde_json::from_value(json!({
        "id": 1,
        "queries": {
            "s": {
                "insert_into": "users",
                "values": [{"name": "Alice"}, {"name": "Bob"}],
                "return_result": false
            }
        }
    }))
    .unwrap();
    shamir.execute("testdb", &seed).await.unwrap();

    // Read
    let req: BatchRequest = serde_json::from_value(json!({
        "id": 1,
        "queries": {
            "users": {"from": "users"}
        }
    }))
    .unwrap();
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
    let q1: BatchRequest = serde_json::from_value(json!({
        "id": 1,
        "queries": {
            "ins": {
                "insert_into": "users",
                "values": [
                    {"name": "Alice", "status": "active"},
                    {"name": "Bob", "status": "inactive"},
                    {"name": "Carol", "status": "active"}
                ],
                "return_result": false
            }
        }
    }))
    .unwrap();
    shamir.execute("testdb", &q1).await.unwrap();

    // 2. Update: activate Bob
    let q2: BatchRequest = serde_json::from_value(json!({
        "id": 1,
        "queries": {
            "upd": {
                "update": "users",
                "where": {"op": "eq", "field": ["name"], "value": "Bob"},
                "set": {"status": "active"},
                "select": {
                    "return_mode": "changed"
                }
            }
        }
    }))
    .unwrap();
    let resp = shamir.execute("testdb", &q2).await.unwrap();
    assert_eq!(resp.results["upd"].records.len(), 1);
    assert_eq!(resp.results["upd"].records[0]["status"], "active");

    // 3. Delete Carol + read remaining
    let q3: BatchRequest = serde_json::from_value(json!({
        "id": 1,
        "queries": {
            "del": {
                "delete_from": "users",
                "where": {"op": "eq", "field": ["name"], "value": "Carol"}
            },
            "remaining": {"from": "users"}
        }
    }))
    .unwrap();
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
    let seed: BatchRequest = serde_json::from_value(json!({
        "id": 1,
        "queries": {
            "s1": {
                "insert_into": "users",
                "values": [
                    {"name": "Alice", "tier": "vip"},
                    {"name": "Bob", "tier": "basic"}
                ],
                "return_result": false
            },
            "s2": {
                "insert_into": "orders",
                "values": [
                    {"user": "Alice", "amount": 100},
                    {"user": "Bob", "amount": 50},
                    {"user": "Alice", "amount": 200}
                ],
                "return_result": false
            }
        }
    }))
    .unwrap();
    shamir.execute("testdb", &seed).await.unwrap();

    // Query: find VIP users, then find their orders
    let req: BatchRequest = serde_json::from_value(json!({
        "id": 1,
        "queries": {
            "vips": {
                "from": "users",
                "where": {"op": "eq", "field": ["tier"], "value": "vip"}
            },
            "vip_orders": {
                "from": "orders",
                "where": {
                    "op": "eq",
                    "field": ["user"],
                    "value": {"$query": "vips", "path": "[0].name"}
                }
            }
        }
    }))
    .unwrap();

    let resp = shamir.execute("testdb", &req).await.unwrap();

    // Stage 1: vips → Alice
    assert_eq!(resp.results["vips"].records.len(), 1);
    // Stage 2: vip_orders → Alice's orders (2)
    assert_eq!(resp.results["vip_orders"].records.len(), 2);
    assert_eq!(resp.execution_plan.len(), 2);
}

// ============================================================================
// Error: unknown database
// ============================================================================

#[tokio::test]
async fn test_execute_unknown_db() {
    let shamir = setup_shamir().await;

    let req: BatchRequest = serde_json::from_value(json!({
        "id": 1,
        "queries": {
            "r": {"from": "users"}
        }
    }))
    .unwrap();

    let err = shamir.execute("nonexistent", &req).await.unwrap_err();
    assert!(matches!(
        err,
        crate::query::batch::BatchError::QueryError { .. }
    ));
}

// ============================================================================
// Migration ops — Phase A stubs return "not yet implemented"
// ============================================================================

#[tokio::test]
async fn test_migration_lifecycle_in_memory() {
    let shamir = setup_shamir().await;

    // Seed some data
    let seed: BatchRequest = serde_json::from_value(json!({
        "id": 0,
        "queries": {
            "s": {
                "insert_into": "users",
                "values": [{"name": "Alice"}, {"name": "Bob"}, {"name": "Carol"}],
                "return_result": false
            }
        }
    }))
    .unwrap();
    shamir.execute("testdb", &seed).await.unwrap();

    // Start migration: users from main → cold (in_memory)
    let req: BatchRequest = serde_json::from_value(json!({
        "id": 1,
        "queries": {
            "mig": {
                "start_migration": "users",
                "repo": "main",
                "dst_repo": "cold",
                "dst_engine": "in_memory"
            }
        }
    }))
    .unwrap();
    let resp = shamir.execute("testdb", &req).await.unwrap();
    let mig_result = &resp.results["mig"].records[0];
    assert_eq!(mig_result["phase"], "cutover_ready");
    let migration_id = mig_result["migration_id"].as_str().unwrap().to_string();

    // Query status
    let status_req: BatchRequest = serde_json::from_value(json!({
        "id": 2,
        "queries": {
            "s": {"migration_status": migration_id}
        }
    }))
    .unwrap();
    let status_resp = shamir.execute("testdb", &status_req).await.unwrap();
    let status = &status_resp.results["s"].records[0];
    assert_eq!(status["phase"], "cutover_ready");
    assert_eq!(status["records_copied"], 3);

    // Commit
    let commit_req: BatchRequest = serde_json::from_value(json!({
        "id": 3,
        "queries": {
            "c": {"commit_migration": migration_id}
        }
    }))
    .unwrap();
    let commit_resp = shamir.execute("testdb", &commit_req).await.unwrap();
    let commit = &commit_resp.results["c"].records[0];
    assert_eq!(commit["phase"], "committed");
    assert_eq!(commit["src_records"], 3);
    assert_eq!(commit["dst_records"], 3);

    // Read from the destination table
    let read_req: BatchRequest = serde_json::from_value(json!({
        "id": 4,
        "queries": {
            "r": {"from": ["cold", "users"]}
        }
    }))
    .unwrap();
    let read_resp = shamir.execute("testdb", &read_req).await.unwrap();
    assert_eq!(read_resp.results["r"].records.len(), 3);
}

#[tokio::test]
async fn test_migration_rollback() {
    let shamir = setup_shamir().await;

    // Seed
    let seed: BatchRequest = serde_json::from_value(json!({
        "id": 0,
        "queries": {
            "s": {
                "insert_into": "users",
                "values": [{"name": "Alice"}],
                "return_result": false
            }
        }
    }))
    .unwrap();
    shamir.execute("testdb", &seed).await.unwrap();

    // Start migration
    let req: BatchRequest = serde_json::from_value(json!({
        "id": 1,
        "queries": {
            "mig": {
                "start_migration": "users",
                "repo": "main",
                "dst_repo": "rollback_dst",
                "dst_engine": "in_memory"
            }
        }
    }))
    .unwrap();
    let resp = shamir.execute("testdb", &req).await.unwrap();
    let migration_id = resp.results["mig"].records[0]["migration_id"]
        .as_str()
        .unwrap()
        .to_string();

    // Rollback
    let rb_req: BatchRequest = serde_json::from_value(json!({
        "id": 2,
        "queries": {
            "r": {"rollback_migration": migration_id}
        }
    }))
    .unwrap();
    let rb_resp = shamir.execute("testdb", &rb_req).await.unwrap();
    assert_eq!(rb_resp.results["r"].records[0]["phase"], "rolled_back");

    // Status should fail (migration removed)
    let status_req: BatchRequest = serde_json::from_value(json!({
        "id": 3,
        "queries": {
            "s": {"migration_status": migration_id}
        }
    }))
    .unwrap();
    let status_err = shamir.execute("testdb", &status_req).await.unwrap_err();
    assert!(matches!(
        status_err,
        crate::query::batch::BatchError::QueryError { .. }
    ));
}

#[tokio::test]
async fn test_migration_unknown_id() {
    let shamir = setup_shamir().await;

    let req: BatchRequest = serde_json::from_value(json!({
        "id": 1,
        "queries": {
            "c": {"commit_migration": "nonexistent"}
        }
    }))
    .unwrap();
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

    let shamir = setup_shamir().await;
    let plaintext = "correct horse battery staple";

    // Create a user through the wire-facing batch path.
    let req: BatchRequest = serde_json::from_value(json!({
        "id": 1,
        "queries": {
            "cu": {
                "create_user": "alice",
                "password": plaintext,
                "roles": ["readonly"]
            }
        }
    }))
    .unwrap();
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

    let stored = result.records[0]["password_hash"].as_str().unwrap();

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
        match ShamirDb::init(SystemStoreConfig::Redb(sys_path.clone())).await {
            Ok(shamir) => return shamir,
            Err(_) => tokio::time::sleep(std::time::Duration::from_millis(20)).await,
        }
    }
    ShamirDb::init(SystemStoreConfig::Redb(sys_path))
        .await
        .expect("system store still locked after retries")
}

/// A repo created through the wire/execute `CreateRepo` op must be persisted
/// to the system-store catalogue and re-attached after a restart — symmetry
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

    let sys_dir = tempfile::tempdir().unwrap();
    let sys_path = sys_dir.path().join("system.redb");

    // === Session 1: create a repo via the wire/execute CreateRepo op ===
    {
        let shamir = ShamirDb::init(SystemStoreConfig::Redb(sys_path.clone()))
            .await
            .unwrap();
        shamir.create_db("production").await;

        let req: BatchRequest = serde_json::from_value(json!({
            "id": 1,
            "queries": {
                "cr": {
                    "create_repo": "events_repo",
                    "engine": "in_memory",
                    "tables": [
                        "events"
                    ]
                }
            }
        }))
        .unwrap();
        let resp = shamir.execute("production", &req).await.unwrap();
        assert_eq!(resp.results["cr"].records[0]["created_repo"], "events_repo");

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
/// the catalogue and must NOT resurrect after a restart — symmetry with
/// `CreateRepo`.
#[tokio::test]
async fn drop_repo_via_execute_clears_catalogue() {
    use crate::shamir_db::SystemStoreConfig;

    let sys_dir = tempfile::tempdir().unwrap();
    let sys_path = sys_dir.path().join("system.redb");

    {
        let shamir = ShamirDb::init(SystemStoreConfig::Redb(sys_path.clone()))
            .await
            .unwrap();
        shamir.create_db("production").await;

        let create: BatchRequest = serde_json::from_value(json!({
            "id": 1,
            "queries": {
                "cr": {
                    "create_repo": "scratch_repo",
                    "engine": "in_memory",
                    "tables": []
                }
            }
        }))
        .unwrap();
        shamir.execute("production", &create).await.unwrap();

        let drop: BatchRequest = serde_json::from_value(json!({
            "id": 2,
            "queries": {
                "dr": {
                    "drop_repo": "scratch_repo"
                }
            }
        }))
        .unwrap();
        let resp = shamir.execute("production", &drop).await.unwrap();
        assert_eq!(resp.results["dr"].records[0]["existed"], true);

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
    let req: BatchRequest = serde_json::from_value(json!({
        "id": 1,
        "queries": {
            "r": {
                "from": ["nonexistent", "users"]
            }
        }
    }))
    .unwrap();

    let err = shamir.execute("testdb", &req).await.unwrap_err();
    assert!(matches!(
        err,
        crate::query::batch::BatchError::QueryError { .. }
    ));
}
