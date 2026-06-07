//! Tests for `ddl` constructors — assert wire JSON matches the engine's
//! expected shape and round-trips through `BatchOp` serde.

use serde_json::json;
use shamir_query_types::admin::{BufferConfigDto, BufferConfigPatch};
use shamir_query_types::auth::{Action, Effect, Permission, Resource};
use shamir_query_types::batch::BatchOp;

use crate::ddl;

// ── helpers ────────────────────────────────────────────────────────────

/// Serialize a `BatchOp` to a `serde_json::Value`, then deserialize it
/// back and assert equality.
fn roundtrip(op: &BatchOp) -> serde_json::Value {
    let val = serde_json::to_value(op).expect("serialize");
    let back: BatchOp = serde_json::from_value(val.clone()).expect("deserialize");
    assert_eq!(&back, op, "round-trip mismatch");
    val
}

// ============================================================================
// Database DDL
// ============================================================================

#[test]
fn create_db_wire() {
    let op = ddl::create_db("mydb").build();
    let j = roundtrip(&op);
    assert_eq!(
        j,
        json!({
            "create_db": "mydb"
        })
    );
    assert!(op.is_admin());
}

#[test]
fn drop_db_no_hmac() {
    let op = ddl::drop_db("mydb").build();
    let j = roundtrip(&op);
    assert_eq!(
        j,
        json!({
            "drop_db": "mydb"
        })
    );
}

#[test]
fn drop_db_with_hmac() {
    let op = ddl::drop_db("mydb").hmac("abc123").build();
    let j = roundtrip(&op);
    assert_eq!(
        j,
        json!({
            "drop_db": "mydb",
            "hmac": "abc123"
        })
    );
}

// ============================================================================
// Repository DDL
// ============================================================================

#[test]
fn create_repo_minimal() {
    let op = ddl::create_repo("hot_cache").build();
    let j = roundtrip(&op);
    assert_eq!(
        j,
        json!({
            "create_repo": "hot_cache"
        })
    );
    assert!(op.is_admin());
}

#[test]
fn create_repo_full() {
    let op = ddl::create_repo("hot_cache")
        .engine("in_memory")
        .tables(["sessions", "tokens"])
        .build();
    let j = roundtrip(&op);
    assert_eq!(
        j,
        json!({
            "create_repo": "hot_cache",
            "engine": "in_memory",
            "tables": ["sessions", "tokens"]
        })
    );
}

#[test]
fn drop_repo_wire() {
    let op = ddl::drop_repo("temp").build();
    let j = roundtrip(&op);
    assert_eq!(
        j,
        json!({
            "drop_repo": "temp"
        })
    );
}

// ============================================================================
// Table DDL
// ============================================================================

#[test]
fn create_table_default_repo() {
    let op = ddl::create_table("products").build();
    let j = roundtrip(&op);
    assert_eq!(
        j,
        json!({
            "create_table": "products",
            "repo": "main"
        })
    );
    assert!(op.is_admin());
}

#[test]
fn create_table_custom_repo() {
    let op = ddl::create_table("products").repo("hot").build();
    let j = roundtrip(&op);
    assert_eq!(
        j,
        json!({
            "create_table": "products",
            "repo": "hot"
        })
    );
}

#[test]
fn drop_table_default() {
    let op = ddl::drop_table("users").build();
    let j = roundtrip(&op);
    assert_eq!(
        j,
        json!({
            "drop_table": "users",
            "repo": "main"
        })
    );
}

#[test]
fn drop_table_with_hmac() {
    let op = ddl::drop_table("users").hmac("ff00").repo("cold").build();
    let j = roundtrip(&op);
    assert_eq!(
        j,
        json!({
            "drop_table": "users",
            "repo": "cold",
            "hmac": "ff00"
        })
    );
}

// ============================================================================
// Index DDL
// ============================================================================

#[test]
fn create_index_regular() {
    let op = ddl::create_index("name_idx", "users")
        .fields(vec![vec!["name".into()]])
        .build();
    let j = roundtrip(&op);
    assert_eq!(
        j,
        json!({
            "create_index": "name_idx",
            "table": "users",
            "fields": [["name"]],
            "unique": false,
            "sorted": false,
            "repo": "main"
        })
    );
    assert!(op.is_admin());
}

#[test]
fn create_index_unique() {
    let op = ddl::create_index("email_idx", "users")
        .field("email")
        .unique()
        .build();
    let j = roundtrip(&op);
    assert_eq!(j["create_index"], "email_idx");
    assert_eq!(j["unique"], true);
    assert_eq!(j["fields"], json!([["email"]]));
}

#[test]
fn create_index_fts() {
    let op = ddl::create_index("body_fts", "posts")
        .field("body")
        .index_type("fts")
        .fts_tokenizer("unicode")
        .fts_language("en")
        .build();
    let j = roundtrip(&op);
    assert_eq!(j["index_type"], "fts");
    assert_eq!(j["fts_tokenizer"], "unicode");
    assert_eq!(j["fts_language"], "en");
}

#[test]
fn create_index_vector() {
    let op = ddl::create_index("embed_idx", "docs")
        .field("embedding")
        .index_type("vector")
        .vector_dim(128)
        .vector_metric("cosine")
        .build();
    let j = roundtrip(&op);
    assert_eq!(j["index_type"], "vector");
    assert_eq!(j["vector_dim"], 128);
    assert_eq!(j["vector_metric"], "cosine");
}

#[test]
fn create_index_functional() {
    let op = ddl::create_index("lower_name", "users")
        .field("name")
        .index_type("functional")
        .functional_op("lower")
        .build();
    let j = roundtrip(&op);
    assert_eq!(j["index_type"], "functional");
    assert_eq!(j["functional_op"], "lower");
}

#[test]
fn create_index_sorted() {
    let op = ddl::create_index("price_sorted", "products")
        .field("price")
        .sorted()
        .build();
    let j = roundtrip(&op);
    assert_eq!(j["sorted"], true);
    assert_eq!(j["unique"], false);
}

#[test]
fn create_index_sorted_with_include() {
    let op = ddl::create_index("score_sorted", "users")
        .field("score")
        .sorted()
        .include(vec![vec!["email".to_string()], vec!["name".to_string()]])
        .build();
    let j = roundtrip(&op);
    assert_eq!(j["sorted"], true);
    assert_eq!(j["include"], json!([["email"], ["name"]]));
    // `include` must be absent when empty (skip_serializing_if).
    let op_no_include = ddl::create_index("score_sorted2", "users")
        .field("score")
        .sorted()
        .build();
    let j2 = serde_json::to_value(&op_no_include).unwrap();
    assert!(
        j2.get("include").is_none(),
        "empty include should be omitted from JSON"
    );
}

#[test]
fn drop_index_wire() {
    let op = ddl::drop_index("name_idx", "users").build();
    let j = roundtrip(&op);
    assert_eq!(
        j,
        json!({
            "drop_index": "name_idx",
            "table": "users",
            "unique": false,
            "repo": "main"
        })
    );
}

#[test]
fn drop_index_unique_with_hmac() {
    let op = ddl::drop_index("email_idx", "users")
        .unique()
        .hmac("dead")
        .build();
    let j = roundtrip(&op);
    assert_eq!(j["unique"], true);
    assert_eq!(j["hmac"], "dead");
}

// ============================================================================
// Buffer config DDL
// ============================================================================

#[test]
fn set_buffer_config_wire() {
    let cfg = BufferConfigDto {
        max_bytes: 1024,
        max_entries: 100,
        ttl_ms: Some(5000),
        flush_interval_ms: 500,
        flush_batch_size: 50,
    };
    let op = ddl::set_buffer_config("users", cfg).build();
    let j = roundtrip(&op);
    assert_eq!(j["set_buffer_config"], "users");
    assert_eq!(j["repo"], "main");
    assert_eq!(j["config"]["max_bytes"], 1024);
    assert_eq!(j["config"]["ttl_ms"], 5000);
}

#[test]
fn get_buffer_config_wire() {
    let op = ddl::get_buffer_config("users").build();
    let j = roundtrip(&op);
    assert_eq!(
        j,
        json!({
            "get_buffer_config": "users",
            "repo": "main"
        })
    );
}

#[test]
fn alter_buffer_config_wire() {
    let patch = BufferConfigPatch {
        max_bytes: Some(2048),
        ..Default::default()
    };
    let op = ddl::alter_buffer_config("users", patch)
        .repo("cold")
        .build();
    let j = roundtrip(&op);
    assert_eq!(j["alter_buffer_config"], "users");
    assert_eq!(j["repo"], "cold");
    assert_eq!(j["patch"]["max_bytes"], 2048);
}

// ============================================================================
// List operations
// ============================================================================

#[test]
fn list_databases_wire() {
    let op = ddl::list_databases();
    let j = roundtrip(&op);
    assert_eq!(
        j,
        json!({
            "list": "databases"
        })
    );
    assert!(op.is_admin());
}

#[test]
fn list_repos_wire() {
    let op = ddl::list_repos();
    let j = roundtrip(&op);
    assert_eq!(
        j,
        json!({
            "list": "repos"
        })
    );
}

#[test]
fn list_tables_wire() {
    let op = ddl::list_tables().build();
    let j = roundtrip(&op);
    assert_eq!(
        j,
        json!({
            "list": "tables",
            "repo": "main"
        })
    );
}

#[test]
fn list_tables_custom_repo() {
    let op = ddl::list_tables().repo("hot").build();
    let j = roundtrip(&op);
    assert_eq!(
        j,
        json!({
            "list": "tables",
            "repo": "hot"
        })
    );
}

#[test]
fn list_indexes_wire() {
    let op = ddl::list_indexes("users").build();
    let j = roundtrip(&op);
    assert_eq!(
        j,
        json!({
            "list": "indexes",
            "table": "users",
            "repo": "main"
        })
    );
}

#[test]
fn list_users_wire() {
    let op = ddl::list_users();
    let j = roundtrip(&op);
    assert_eq!(
        j,
        json!({
            "list": "users"
        })
    );
}

#[test]
fn list_roles_wire() {
    let op = ddl::list_roles();
    let j = roundtrip(&op);
    assert_eq!(
        j,
        json!({
            "list": "roles"
        })
    );
}

// ============================================================================
// Migration DDL
// ============================================================================

#[test]
fn start_migration_wire() {
    let op = ddl::start_migration("users", "cold", "redb")
        .dst_path("/data/cold")
        .hmac("deadbeef")
        .build();
    let j = roundtrip(&op);
    assert_eq!(
        j,
        json!({
            "start_migration": "users",
            "repo": "main",
            "dst_repo": "cold",
            "dst_engine": "redb",
            "dst_path": "/data/cold",
            "hmac": "deadbeef"
        })
    );
    assert!(op.is_admin());
}

#[test]
fn start_migration_minimal() {
    let op = ddl::start_migration("logs", "archive", "fjall").build();
    let j = roundtrip(&op);
    assert_eq!(j["start_migration"], "logs");
    assert_eq!(j["repo"], "main");
    assert_eq!(j["dst_repo"], "archive");
    assert_eq!(j["dst_engine"], "fjall");
    assert!(j.get("dst_path").is_none());
    assert!(j.get("hmac").is_none());
}

#[test]
fn commit_migration_wire() {
    let op = ddl::commit_migration("mig-001").hmac("abcd1234").build();
    let j = roundtrip(&op);
    assert_eq!(
        j,
        json!({
            "commit_migration": "mig-001",
            "hmac": "abcd1234"
        })
    );
}

#[test]
fn rollback_migration_wire() {
    let op = ddl::rollback_migration("mig-001").hmac("ff00").build();
    let j = roundtrip(&op);
    assert_eq!(
        j,
        json!({
            "rollback_migration": "mig-001",
            "hmac": "ff00"
        })
    );
}

#[test]
fn migration_status_wire() {
    let op = ddl::migration_status("mig-001");
    let j = roundtrip(&op);
    assert_eq!(
        j,
        json!({
            "migration_status": "mig-001"
        })
    );
    assert!(op.is_admin());
}

// ============================================================================
// Access tree
// ============================================================================

#[test]
fn access_tree_defaults() {
    let op = ddl::access_tree().build();
    let j = roundtrip(&op);
    assert_eq!(
        j,
        json!({
            "access_tree": true
        })
    );
    assert!(op.is_admin());
}

#[test]
fn access_tree_with_depth() {
    let op = ddl::access_tree().depth(2).build();
    let j = roundtrip(&op);
    assert_eq!(
        j,
        json!({
            "access_tree": true,
            "depth": 2
        })
    );
}

#[test]
fn access_tree_with_db() {
    let op = ddl::access_tree().db("mydb").depth(1).build();
    let j = roundtrip(&op);
    assert_eq!(
        j,
        json!({
            "access_tree": true,
            "depth": 1,
            "db": "mydb"
        })
    );
}

// ============================================================================
// chmod / chown / chgrp
// ============================================================================

#[test]
fn chmod_table_wire() {
    let op = ddl::chmod(ddl::res::table("mydb", "main", "users"), 0o700);
    let j = roundtrip(&op);
    assert_eq!(
        j,
        json!({
            "chmod": {
                "table": ["mydb", "main", "users"]
            },
            "mode": 448
        })
    );
    assert!(op.is_admin());
}

#[test]
fn chmod_function_namespace_wire() {
    let op = ddl::chmod(ddl::res::function_namespace(), 0o755);
    let j = roundtrip(&op);
    assert_eq!(
        j,
        json!({
            "chmod": {
                "function_namespace": true
            },
            "mode": 493
        })
    );
}

#[test]
fn chown_database_wire() {
    let op = ddl::chown(ddl::res::database("testdb"), 7);
    let j = roundtrip(&op);
    assert_eq!(
        j,
        json!({
            "chown": {
                "database": "testdb"
            },
            "owner": 7
        })
    );
    assert!(op.is_admin());
}

#[test]
fn chown_function_wire() {
    let op = ddl::chown(ddl::res::function("my_fn"), 10);
    let j = roundtrip(&op);
    assert_eq!(
        j,
        json!({
            "chown": {
                "function": "my_fn"
            },
            "owner": 10
        })
    );
}

#[test]
fn chgrp_store_wire() {
    let op = ddl::chgrp(ddl::res::store("testdb", "main"), Some(3));
    let j = roundtrip(&op);
    assert_eq!(
        j,
        json!({
            "chgrp": {
                "store": ["testdb", "main"]
            },
            "group": 3
        })
    );
}

#[test]
fn chgrp_null_group_wire() {
    let op = ddl::chgrp(ddl::res::database("testdb"), None);
    let j = roundtrip(&op);
    assert_eq!(
        j,
        json!({
            "chgrp": {
                "database": "testdb"
            },
            "group": null
        })
    );
}

// ============================================================================
// Group DDL
// ============================================================================

#[test]
fn create_group_wire() {
    let op = ddl::create_group("devs");
    let j = roundtrip(&op);
    assert_eq!(
        j,
        json!({
            "create_group": "devs"
        })
    );
    assert!(op.is_admin());
}

#[test]
fn drop_group_by_name_wire() {
    let op = ddl::drop_group(ddl::GroupRef::Name {
        name: "devs".into(),
    });
    let j = roundtrip(&op);
    assert_eq!(
        j,
        json!({
            "drop_group": {
                "name": "devs"
            }
        })
    );
}

#[test]
fn drop_group_by_id_wire() {
    let op = ddl::drop_group(ddl::GroupRef::Id { id: 3 });
    let j = roundtrip(&op);
    assert_eq!(
        j,
        json!({
            "drop_group": {
                "id": 3
            }
        })
    );
}

#[test]
fn add_group_member_wire() {
    let op = ddl::add_group_member(
        ddl::GroupRef::Name {
            name: "devs".into(),
        },
        42,
    );
    let j = roundtrip(&op);
    assert_eq!(
        j,
        json!({
            "add_group_member": {
                "name": "devs"
            },
            "user": 42
        })
    );
    assert!(op.is_admin());
}

#[test]
fn remove_group_member_wire() {
    let op = ddl::remove_group_member(ddl::GroupRef::Id { id: 1 }, 42);
    let j = roundtrip(&op);
    assert_eq!(
        j,
        json!({
            "remove_group_member": {
                "id": 1
            },
            "user": 42
        })
    );
}

// ============================================================================
// Auth DDL (users + roles)
// ============================================================================

#[test]
fn create_user_minimal() {
    let op = ddl::create_user("alice", "s3cret").build();
    let j = roundtrip(&op);
    assert_eq!(j["create_user"], "alice");
    assert_eq!(j["password"], "s3cret");
    assert_eq!(j["roles"], json!([]));
    assert!(op.is_admin());
}

#[test]
fn create_user_full() {
    let op = ddl::create_user("bob", "hunter2")
        .roles(["admin", "viewer"])
        .profile(json!({"department": "eng"}))
        .build();
    let j = roundtrip(&op);
    assert_eq!(j["create_user"], "bob");
    assert_eq!(j["roles"], json!(["admin", "viewer"]));
    assert_eq!(j["profile"], json!({"department": "eng"}));
}

#[test]
fn drop_user_wire() {
    let op = ddl::drop_user("alice").build();
    let j = roundtrip(&op);
    assert_eq!(
        j,
        json!({
            "drop_user": "alice"
        })
    );
}

#[test]
fn drop_user_with_hmac() {
    let op = ddl::drop_user("alice").hmac("abc").build();
    let j = roundtrip(&op);
    assert_eq!(
        j,
        json!({
            "drop_user": "alice",
            "hmac": "abc"
        })
    );
}

#[test]
fn create_role_wire() {
    let perms = vec![Permission {
        effect: Effect::Allow,
        actions: vec![Action::Read],
        resource: Resource::Global,
        row_filter: None,
    }];
    let op = ddl::create_role("viewer", perms);
    let j = roundtrip(&op);
    assert_eq!(j["create_role"], "viewer");
    assert_eq!(j["permissions"][0]["effect"], "allow");
    assert_eq!(j["permissions"][0]["actions"], json!(["read"]));
    assert!(op.is_admin());
}

#[test]
fn drop_role_wire() {
    let op = ddl::drop_role("viewer").build();
    let j = roundtrip(&op);
    assert_eq!(
        j,
        json!({
            "drop_role": "viewer"
        })
    );
}

#[test]
fn drop_role_with_hmac() {
    let op = ddl::drop_role("viewer").hmac("ff").build();
    let j = roundtrip(&op);
    assert_eq!(
        j,
        json!({
            "drop_role": "viewer",
            "hmac": "ff"
        })
    );
}

#[test]
fn grant_role_wire() {
    let op = ddl::grant_role("admin", "alice");
    let j = roundtrip(&op);
    assert_eq!(
        j,
        json!({
            "grant_role": "admin",
            "user": "alice"
        })
    );
    assert!(op.is_admin());
}

#[test]
fn revoke_role_wire() {
    let op = ddl::revoke_role("admin", "alice");
    let j = roundtrip(&op);
    assert_eq!(
        j,
        json!({
            "revoke_role": "admin",
            "user": "alice"
        })
    );
}

// ============================================================================
// res helpers
// ============================================================================

#[test]
fn res_database() {
    let r = ddl::res::database("mydb");
    assert_eq!(
        serde_json::to_value(&r).expect("ser"),
        json!({"database": "mydb"})
    );
}

#[test]
fn res_store() {
    let r = ddl::res::store("mydb", "main");
    assert_eq!(
        serde_json::to_value(&r).expect("ser"),
        json!({"store": ["mydb", "main"]})
    );
}

#[test]
fn res_table() {
    let r = ddl::res::table("mydb", "main", "users");
    assert_eq!(
        serde_json::to_value(&r).expect("ser"),
        json!({"table": ["mydb", "main", "users"]})
    );
}

#[test]
fn res_function() {
    let r = ddl::res::function("my_fn");
    assert_eq!(
        serde_json::to_value(&r).expect("ser"),
        json!({"function": "my_fn"})
    );
}

#[test]
fn res_function_namespace() {
    let r = ddl::res::function_namespace();
    assert_eq!(
        serde_json::to_value(&r).expect("ser"),
        json!({"function_namespace": true})
    );
}

// ============================================================================
// Function DDL
// ============================================================================

#[test]
fn create_function_from_source_wire() {
    let op = ddl::create_function("my_fn")
        .source("pub fn main() {}")
        .build();
    let j = roundtrip(&op);
    assert_eq!(
        j,
        json!({
            "create_function": "my_fn",
            "source": "pub fn main() {}",
            "replace": false
        })
    );
    assert!(op.is_admin());
}

#[test]
fn create_function_from_wasm_with_replace() {
    let op = ddl::create_function("my_fn")
        .wasm("AQIDBA==")
        .replace()
        .build();
    let j = roundtrip(&op);
    assert_eq!(j["create_function"], "my_fn");
    assert_eq!(j["wasm"], "AQIDBA==");
    assert_eq!(j["replace"], true);
    assert!(j.get("source").is_none());
}

#[test]
fn drop_function_wire() {
    let op = ddl::drop_function("my_fn");
    let j = roundtrip(&op);
    assert_eq!(
        j,
        json!({
            "drop_function": "my_fn"
        })
    );
    assert!(op.is_admin());
}

#[test]
fn rename_function_wire() {
    let op = ddl::rename_function("old_fn", "new_fn");
    let j = roundtrip(&op);
    assert_eq!(
        j,
        json!({
            "rename_function": "old_fn",
            "to": "new_fn"
        })
    );
    assert!(op.is_admin());
}

// ============================================================================
// Validator DDL
// ============================================================================

#[test]
fn create_validator_from_source_wire() {
    let op = ddl::create_validator("v_age")
        .source("pub fn validate() {}")
        .build();
    let j = roundtrip(&op);
    assert_eq!(
        j,
        json!({
            "create_validator": "v_age",
            "source": "pub fn validate() {}",
            "replace": false
        })
    );
    assert!(op.is_admin());
}

#[test]
fn create_validator_from_wasm_replace() {
    let op = ddl::create_validator("v_age")
        .wasm("AQIDBA==")
        .replace()
        .build();
    let j = roundtrip(&op);
    assert_eq!(j["create_validator"], "v_age");
    assert_eq!(j["wasm"], "AQIDBA==");
    assert_eq!(j["replace"], true);
}

#[test]
fn drop_validator_wire() {
    let op = ddl::drop_validator("v_age");
    let j = roundtrip(&op);
    assert_eq!(
        j,
        json!({
            "drop_validator": "v_age"
        })
    );
    assert!(op.is_admin());
}

#[test]
fn rename_validator_wire() {
    let op = ddl::rename_validator("v_old", "v_new");
    let j = roundtrip(&op);
    assert_eq!(
        j,
        json!({
            "rename_validator": "v_old",
            "to": "v_new"
        })
    );
    assert!(op.is_admin());
}

#[test]
fn bind_validator_wire() {
    use shamir_query_types::WriteOp;
    let op = ddl::bind_validator("v_age", "users")
        .db("testdb")
        .repo("main")
        .ops([WriteOp::Insert, WriteOp::Update])
        .priority(1500)
        .build();
    let j = roundtrip(&op);
    assert_eq!(j["bind_validator"], "v_age");
    assert_eq!(j["db"], "testdb");
    assert_eq!(j["repo"], "main");
    assert_eq!(j["table"], "users");
    assert_eq!(j["ops"], json!(["insert", "update"]));
    assert_eq!(j["priority"], 1500);
    assert!(op.is_admin());
}

#[test]
fn unbind_validator_wire() {
    let op = ddl::unbind_validator("v_age", "users").db("testdb").build();
    let j = roundtrip(&op);
    assert_eq!(j["unbind_validator"], "v_age");
    assert_eq!(j["db"], "testdb");
    assert_eq!(j["repo"], "main");
    assert_eq!(j["table"], "users");
    assert!(op.is_admin());
}

#[test]
fn list_validators_wire() {
    let op = ddl::list_validators("users")
        .db("testdb")
        .repo("hot")
        .build();
    let j = roundtrip(&op);
    assert_eq!(
        j,
        json!({
            "list_validators": "users",
            "db": "testdb",
            "repo": "hot"
        })
    );
    assert!(op.is_admin());
}

// ============================================================================
// Function folder DDL
// ============================================================================

#[test]
fn create_function_folder_wire() {
    let op = ddl::create_function_folder(["reports", "daily"]);
    let j = roundtrip(&op);
    assert_eq!(
        j,
        json!({
            "create_function_folder": ["reports", "daily"]
        })
    );
    assert!(op.is_admin());
}

#[test]
fn create_function_folder_single_segment() {
    let op = ddl::create_function_folder(["utils"]);
    let j = roundtrip(&op);
    assert_eq!(
        j,
        json!({
            "create_function_folder": ["utils"]
        })
    );
}

// ============================================================================
// create_db with if_not_exists
// ============================================================================

#[test]
fn create_db_if_not_exists_wire() {
    let op = ddl::create_db("newdb").if_not_exists().build();
    let j = roundtrip(&op);
    assert_eq!(
        j,
        json!({
            "create_db": "newdb",
            "if_not_exists": true
        })
    );
    assert!(op.is_admin());
}

// ============================================================================
// list functions / validators / function_folders
// ============================================================================

#[test]
fn list_functions_wire() {
    let op = ddl::list_functions().build();
    let j = roundtrip(&op);
    assert_eq!(
        j,
        json!({
            "list": "functions"
        })
    );
    assert!(op.is_admin());
}

#[test]
fn list_functions_with_folder_wire() {
    let op = ddl::list_functions().folder("math").build();
    let j = roundtrip(&op);
    assert_eq!(
        j,
        json!({
            "list": "functions",
            "folder": "math"
        })
    );
}

#[test]
fn list_all_validators_wire() {
    let op = ddl::list_all_validators();
    let j = roundtrip(&op);
    assert_eq!(
        j,
        json!({
            "list": "validators"
        })
    );
    assert!(op.is_admin());
}

#[test]
fn list_function_folders_wire() {
    let op = ddl::list_function_folders().build();
    let j = roundtrip(&op);
    assert_eq!(
        j,
        json!({
            "list": "function_folders"
        })
    );
    assert!(op.is_admin());
}

#[test]
fn list_function_folders_with_parent_wire() {
    let op = ddl::list_function_folders().parent("alpha").build();
    let j = roundtrip(&op);
    assert_eq!(
        j,
        json!({
            "list": "function_folders",
            "parent": "alpha"
        })
    );
}

// ============================================================================
// Batch composition — builders pass through Batch::op()
// ============================================================================

#[test]
fn builder_composes_with_batch() {
    let mut batch = crate::batch::Batch::new();
    batch.id(1);
    batch.op("db", ddl::create_db("mydb"));
    batch.op("repo", ddl::create_repo("main").engine("in_memory"));
    batch.op("tbl", ddl::create_table("users"));
    batch.op(
        "idx",
        ddl::create_index("email_idx", "users")
            .field("email")
            .unique(),
    );
    let req = batch.build();
    assert_eq!(req.queries.len(), 4);
    assert!(req.queries["db"].op.is_admin());
    assert!(req.queries["repo"].op.is_admin());
    assert!(req.queries["tbl"].op.is_admin());
    assert!(req.queries["idx"].op.is_admin());
}
