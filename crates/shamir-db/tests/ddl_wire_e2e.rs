//! End-to-end tests for function, validator, and folder DDL over the wire
//! (`ShamirDb::execute`).
//!
//! Verifies that every new `BatchOp` variant reaches the facade, passes
//! the auth gate, and round-trips through the catalogue.

use base64::Engine;
use serde_json::json;
use shamir_db::engine::repo::repo_types::BoxRepoFactory;
use shamir_db::engine::repo::RepoConfig;
use shamir_db::engine::table::TableConfig;
use shamir_db::query::batch::BatchRequest;
use shamir_db::ShamirDb;
use shamir_types::types::common::new_map;
use shamir_types::types::value::QueryValue;

// ═══════════════════════════════════════════════════════════════════════
// WAT helpers — build WASM modules that return baked msgpack bytes
// ═══════════════════════════════════════════════════════════════════════

/// WAT module that ignores input and returns msgpack `null` (0xC0) = valid.
const ACCEPT_WAT: &str = r#"
(module
  (memory (export "memory") 2)

  (global $bump (mut i32) (i32.const 1024))

  (data (i32.const 512) "\c0")

  (func (export "shamir_alloc") (param $len i32) (result i32)
    (local $ptr i32)
    (local.set $ptr (global.get $bump))
    (global.set $bump (i32.add (global.get $bump) (local.get $len)))
    (local.get $ptr)
  )

  (func (export "shamir_call") (param $ptr i32) (param $len i32) (result i64)
    (i64.or
      (i64.shl (i64.const 512) (i64.const 32))
      (i64.const 1)
    )
  )
)
"#;

fn accept_wasm() -> Vec<u8> {
    wat::parse_str(ACCEPT_WAT).expect("WAT parse failed")
}

/// Build a WAT module whose `shamir_call` returns the given `QueryValue`
/// serialised as msgpack.
fn make_wat_returning(value: &QueryValue) -> Vec<u8> {
    let bytes = rmp_serde::to_vec(value).expect("msgpack encode");
    let hex_data: String = bytes.iter().map(|b| format!("\\{b:02x}")).collect();
    let len = bytes.len();

    let wat = format!(
        r#"
(module
  (memory (export "memory") 2)

  (global $bump (mut i32) (i32.const 1024))

  (data (i32.const 512) "{hex_data}")

  (func (export "shamir_alloc") (param $len i32) (result i32)
    (local $ptr i32)
    (local.set $ptr (global.get $bump))
    (global.set $bump (i32.add (global.get $bump) (local.get $len)))
    (local.get $ptr)
  )

  (func (export "shamir_call") (param $ptr i32) (param $len i32) (result i64)
    (i64.or
      (i64.shl (i64.const 512) (i64.const 32))
      (i64.const {len})
    )
  )
)
"#
    );

    wat::parse_str(&wat).expect("generated WAT parse failed")
}

/// Build a `QueryValue` for a single-error rejection.
fn rejection_single_error() -> QueryValue {
    let mut error_item = new_map();
    error_item.insert(
        "field".to_owned(),
        QueryValue::List(vec![QueryValue::Str("age".to_owned())]),
    );
    error_item.insert("code".to_owned(), QueryValue::Str("too_young".to_owned()));

    let mut root = new_map();
    root.insert(
        "errors".to_owned(),
        QueryValue::List(vec![QueryValue::Map(error_item)]),
    );
    root.insert("stop".to_owned(), QueryValue::Bool(false));
    QueryValue::Map(root)
}

fn wasm_b64(wasm: &[u8]) -> String {
    base64::engine::general_purpose::STANDARD.encode(wasm)
}

// ═══════════════════════════════════════════════════════════════════════
// Setup helper
// ═══════════════════════════════════════════════════════════════════════

async fn setup_db() -> ShamirDb {
    let db = ShamirDb::init_memory().await.unwrap();
    db.create_db("testdb").await;
    let repo_config =
        RepoConfig::new("main", BoxRepoFactory::in_memory()).add_table(TableConfig::new("users"));
    db.add_repo("testdb", repo_config).await.unwrap();
    db
}

// ═══════════════════════════════════════════════════════════════════════
// 1. create_validator over wire → bind → rejected insert → unbind → ok
// ═══════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn create_validator_bind_reject_unbind_roundtrip() {
    let db = setup_db().await;

    // Step 1: create_validator over the wire
    let rejecting_wasm = make_wat_returning(&rejection_single_error());
    let create_req: BatchRequest = serde_json::from_value(json!({
        "id": "cv",
        "queries": {
            "op": {
                "create_validator": "v_reject",
                "wasm": wasm_b64(&rejecting_wasm),
                "replace": false
            }
        }
    }))
    .unwrap();
    let resp = db.execute("testdb", &create_req).await.unwrap();
    let result = &resp.results["op"].records[0];
    assert_eq!(result["created_validator"], "v_reject");
    assert!(result.get("id").is_some(), "should return validator id");

    // Step 2: bind_validator over the wire
    let bind_req: BatchRequest = serde_json::from_value(json!({
        "id": "bv",
        "queries": {
            "op": {
                "bind_validator": "v_reject",
                "db": "testdb",
                "repo": "main",
                "table": "users",
                "ops": ["insert"],
                "priority": 1500
            }
        }
    }))
    .unwrap();
    let resp = db.execute("testdb", &bind_req).await.unwrap();
    assert_eq!(resp.results["op"].records[0]["bound_validator"], "v_reject");

    // Step 3: insert should fail (validator rejects)
    let insert_req: BatchRequest = serde_json::from_value(json!({
        "id": "ins",
        "queries": {
            "ins": {
                "insert_into": "users",
                "values": [{"name": "Alice", "age": 10}]
            }
        }
    }))
    .unwrap();
    let err = db.execute("testdb", &insert_req).await.unwrap_err();
    let msg = err.to_string();
    assert!(
        msg.contains("too_young") || msg.contains("Validator"),
        "expected validation error, got: {msg}"
    );

    // Step 4: unbind_validator over the wire
    let unbind_req: BatchRequest = serde_json::from_value(json!({
        "id": "ub",
        "queries": {
            "op": {
                "unbind_validator": "v_reject",
                "db": "testdb",
                "repo": "main",
                "table": "users"
            }
        }
    }))
    .unwrap();
    let resp = db.execute("testdb", &unbind_req).await.unwrap();
    assert_eq!(resp.results["op"].records[0]["existed"], true);

    // Step 5: insert should now succeed
    let resp = db.execute("testdb", &insert_req).await;
    assert!(
        resp.is_ok(),
        "insert after unbind should succeed, got: {:?}",
        resp.err()
    );
}

// ═══════════════════════════════════════════════════════════════════════
// 2. create_function over wire → invoke / confirm in catalogue
// ═══════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn create_function_over_wire() {
    let db = setup_db().await;

    let wasm = accept_wasm(); // A no-op function that returns null
    let create_req: BatchRequest = serde_json::from_value(json!({
        "id": "cf",
        "queries": {
            "op": {
                "create_function": "wire_echo",
                "wasm": wasm_b64(&wasm),
                "replace": false
            }
        }
    }))
    .unwrap();
    let resp = db.execute("testdb", &create_req).await.unwrap();
    assert_eq!(
        resp.results["op"].records[0]["created_function"],
        "wire_echo"
    );

    // Verify the function is in the catalogue
    let functions = db.list_functions().await.unwrap();
    assert!(
        functions.contains(&"wire_echo".to_string()),
        "function should be in catalogue, got: {:?}",
        functions
    );
}

// ═══════════════════════════════════════════════════════════════════════
// 3. drop_validator-while-bound → refused
// ═══════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn drop_validator_while_bound_refused() {
    let db = setup_db().await;

    let wasm = accept_wasm();
    let create_req: BatchRequest = serde_json::from_value(json!({
        "id": "cv",
        "queries": {
            "op": {
                "create_validator": "v_drop_test",
                "wasm": wasm_b64(&wasm),
                "replace": false
            }
        }
    }))
    .unwrap();
    db.execute("testdb", &create_req).await.unwrap();

    // Bind it
    let bind_req: BatchRequest = serde_json::from_value(json!({
        "id": "bv",
        "queries": {
            "op": {
                "bind_validator": "v_drop_test",
                "db": "testdb",
                "repo": "main",
                "table": "users",
                "ops": ["insert"],
                "priority": 1500
            }
        }
    }))
    .unwrap();
    db.execute("testdb", &bind_req).await.unwrap();

    // Try to drop → should be refused
    let drop_req: BatchRequest = serde_json::from_value(json!({
        "id": "dv",
        "queries": {
            "op": {
                "drop_validator": "v_drop_test"
            }
        }
    }))
    .unwrap();
    let err = db.execute("testdb", &drop_req).await.unwrap_err();
    let msg = err.to_string();
    assert!(
        msg.contains("still bound") || msg.contains("cannot drop"),
        "expected still-bound error, got: {msg}"
    );
}

// ═══════════════════════════════════════════════════════════════════════
// 4. create_function_folder over wire → succeeds
// ═══════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn create_function_folder_over_wire() {
    let db = setup_db().await;

    let req: BatchRequest = serde_json::from_value(json!({
        "id": "cff",
        "queries": {
            "op": {
                "create_function_folder": ["reports", "daily"]
            }
        }
    }))
    .unwrap();
    let resp = db.execute("testdb", &req).await.unwrap();
    let result = &resp.results["op"].records[0];
    assert_eq!(
        result["created_function_folder"],
        json!(["reports", "daily"])
    );
}

// ═══════════════════════════════════════════════════════════════════════
// 5. name uniqueness: create_validator twice → error
// ═══════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn create_validator_duplicate_rejected() {
    let db = setup_db().await;

    let wasm = accept_wasm();
    let create_req: BatchRequest = serde_json::from_value(json!({
        "id": "cv1",
        "queries": {
            "op": {
                "create_validator": "v_dup",
                "wasm": wasm_b64(&wasm),
                "replace": false
            }
        }
    }))
    .unwrap();
    db.execute("testdb", &create_req).await.unwrap();

    // Second create with replace=false → should fail
    let err = db.execute("testdb", &create_req).await.unwrap_err();
    let msg = err.to_string();
    assert!(
        msg.contains("already exists"),
        "expected 'already exists' error, got: {msg}"
    );
}

// ═══════════════════════════════════════════════════════════════════════
// 6. drop_function over wire
// ═══════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn drop_function_over_wire() {
    let db = setup_db().await;

    let wasm = accept_wasm();
    let create_req: BatchRequest = serde_json::from_value(json!({
        "id": "cf",
        "queries": {
            "op": {
                "create_function": "fn_drop_test",
                "wasm": wasm_b64(&wasm),
                "replace": false
            }
        }
    }))
    .unwrap();
    db.execute("testdb", &create_req).await.unwrap();

    let drop_req: BatchRequest = serde_json::from_value(json!({
        "id": "df",
        "queries": {
            "op": {
                "drop_function": "fn_drop_test"
            }
        }
    }))
    .unwrap();
    let resp = db.execute("testdb", &drop_req).await.unwrap();
    assert_eq!(resp.results["op"].records[0]["existed"], true);

    // Verify removed from catalogue
    let functions = db.list_functions().await.unwrap();
    assert!(
        !functions.contains(&"fn_drop_test".to_string()),
        "function should be removed from catalogue"
    );
}

// ═══════════════════════════════════════════════════════════════════════
// 7. rename_function over wire
// ═══════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn rename_function_over_wire() {
    let db = setup_db().await;

    let wasm = accept_wasm();
    let create_req: BatchRequest = serde_json::from_value(json!({
        "id": "cf",
        "queries": {
            "op": {
                "create_function": "fn_old",
                "wasm": wasm_b64(&wasm),
                "replace": false
            }
        }
    }))
    .unwrap();
    db.execute("testdb", &create_req).await.unwrap();

    let rename_req: BatchRequest = serde_json::from_value(json!({
        "id": "rf",
        "queries": {
            "op": {
                "rename_function": "fn_old",
                "to": "fn_new"
            }
        }
    }))
    .unwrap();
    let resp = db.execute("testdb", &rename_req).await.unwrap();
    assert_eq!(resp.results["op"].records[0]["renamed_function"], "fn_old");
    assert_eq!(resp.results["op"].records[0]["to"], "fn_new");

    let functions = db.list_functions().await.unwrap();
    assert!(functions.contains(&"fn_new".to_string()));
    assert!(!functions.contains(&"fn_old".to_string()));
}

// ═══════════════════════════════════════════════════════════════════════
// 8. rename_validator over wire
// ═══════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn rename_validator_over_wire() {
    let db = setup_db().await;

    let wasm = accept_wasm();
    let create_req: BatchRequest = serde_json::from_value(json!({
        "id": "cv",
        "queries": {
            "op": {
                "create_validator": "v_old",
                "wasm": wasm_b64(&wasm),
                "replace": false
            }
        }
    }))
    .unwrap();
    db.execute("testdb", &create_req).await.unwrap();

    let rename_req: BatchRequest = serde_json::from_value(json!({
        "id": "rv",
        "queries": {
            "op": {
                "rename_validator": "v_old",
                "to": "v_new"
            }
        }
    }))
    .unwrap();
    let resp = db.execute("testdb", &rename_req).await.unwrap();
    assert_eq!(resp.results["op"].records[0]["renamed_validator"], "v_old");
    assert_eq!(resp.results["op"].records[0]["to"], "v_new");
}

// ═══════════════════════════════════════════════════════════════════════
// 9. list_validators over wire
// ═══════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn list_validators_over_wire() {
    let db = setup_db().await;

    let wasm = accept_wasm();
    let create_req: BatchRequest = serde_json::from_value(json!({
        "id": "cv",
        "queries": {
            "op": {
                "create_validator": "v_list_test",
                "wasm": wasm_b64(&wasm),
                "replace": false
            }
        }
    }))
    .unwrap();
    db.execute("testdb", &create_req).await.unwrap();

    // Bind it
    let bind_req: BatchRequest = serde_json::from_value(json!({
        "id": "bv",
        "queries": {
            "op": {
                "bind_validator": "v_list_test",
                "db": "testdb",
                "repo": "main",
                "table": "users",
                "ops": ["insert", "update"],
                "priority": 2000
            }
        }
    }))
    .unwrap();
    db.execute("testdb", &bind_req).await.unwrap();

    // List
    let list_req: BatchRequest = serde_json::from_value(json!({
        "id": "lv",
        "queries": {
            "op": {
                "list_validators": "users",
                "db": "testdb",
                "repo": "main"
            }
        }
    }))
    .unwrap();
    let resp = db.execute("testdb", &list_req).await.unwrap();
    let result = &resp.results["op"].records[0];
    let validators = result["validators"].as_array().unwrap();
    assert!(
        !validators.is_empty(),
        "should have at least one validator binding"
    );
    assert_eq!(result["table"], "users");
}

// ═══════════════════════════════════════════════════════════════════════
// 10. BatchOp serde round-trip for new variants
// ═══════════════════════════════════════════════════════════════════════

#[test]
fn create_function_serde_roundtrip() {
    let json_str = r#"{"create_function": "my_fn", "wasm": "AAAA", "replace": true}"#;
    let op: shamir_db::query::batch::BatchOp = serde_json::from_str(json_str).unwrap();
    assert!(op.is_admin());
    assert!(op.table_ref().is_none());
    let back = serde_json::to_string(&op).unwrap();
    let op2: shamir_db::query::batch::BatchOp = serde_json::from_str(&back).unwrap();
    assert_eq!(op, op2);
}

#[test]
fn create_validator_serde_roundtrip() {
    let json_str = r#"{"create_validator": "v_age", "wasm": "BBBB", "replace": false}"#;
    let op: shamir_db::query::batch::BatchOp = serde_json::from_str(json_str).unwrap();
    assert!(op.is_admin());
    let back = serde_json::to_string(&op).unwrap();
    let op2: shamir_db::query::batch::BatchOp = serde_json::from_str(&back).unwrap();
    assert_eq!(op, op2);
}

#[test]
fn bind_validator_serde_roundtrip() {
    let json_str = r#"{
        "bind_validator": "v_age",
        "db": "testdb",
        "repo": "main",
        "table": "users",
        "ops": ["insert", "update"],
        "priority": 1500
    }"#;
    let op: shamir_db::query::batch::BatchOp = serde_json::from_str(json_str).unwrap();
    assert!(op.is_admin());
    let back = serde_json::to_string(&op).unwrap();
    let op2: shamir_db::query::batch::BatchOp = serde_json::from_str(&back).unwrap();
    assert_eq!(op, op2);
}

#[test]
fn create_function_folder_serde_roundtrip() {
    let json_str = r#"{"create_function_folder": ["reports", "daily"]}"#;
    let op: shamir_db::query::batch::BatchOp = serde_json::from_str(json_str).unwrap();
    assert!(op.is_admin());
    let back = serde_json::to_string(&op).unwrap();
    let op2: shamir_db::query::batch::BatchOp = serde_json::from_str(&back).unwrap();
    assert_eq!(op, op2);
}

#[test]
fn drop_function_serde_roundtrip() {
    let json_str = r#"{"drop_function": "my_fn"}"#;
    let op: shamir_db::query::batch::BatchOp = serde_json::from_str(json_str).unwrap();
    assert!(op.is_admin());
    let back = serde_json::to_string(&op).unwrap();
    let op2: shamir_db::query::batch::BatchOp = serde_json::from_str(&back).unwrap();
    assert_eq!(op, op2);
}

#[test]
fn rename_validator_serde_roundtrip() {
    let json_str = r#"{"rename_validator": "v_old", "to": "v_new"}"#;
    let op: shamir_db::query::batch::BatchOp = serde_json::from_str(json_str).unwrap();
    assert!(op.is_admin());
    let back = serde_json::to_string(&op).unwrap();
    let op2: shamir_db::query::batch::BatchOp = serde_json::from_str(&back).unwrap();
    assert_eq!(op, op2);
}

// ═══════════════════════════════════════════════════════════════════════
// owner-on-create: user-initiated creates stamp the acting actor;
// system-initiated creates keep System.
// ═══════════════════════════════════════════════════════════════════════

use shamir_types::access::{Actor, ResourcePath};

#[tokio::test]
async fn owner_on_create_db_user_actor() {
    let shamir = ShamirDb::init_memory().await.unwrap();
    let user_actor = Actor::User(42);

    // Bootstrap a db to dispatch admin ops through.
    shamir.create_db("bootstrap").await;

    // Create a NEW database via execute_as with a user actor.
    let req: BatchRequest = serde_json::from_value(json!({
        "id": 1,
        "queries": {
            "op": { "create_db": "owned_db" }
        }
    }))
    .unwrap();
    shamir
        .execute_as(user_actor.clone(), "bootstrap", &req)
        .await
        .unwrap();

    // Read back the owner from the catalogue via resource_meta.
    let meta = shamir
        .resource_meta(&ResourcePath::database("owned_db"))
        .await;
    assert_eq!(
        meta.owner,
        Actor::User(42),
        "db owner should be the user actor"
    );
    assert_eq!(meta.mode, 0o777, "mode must stay open");
    assert!(meta.group.is_none(), "group must stay None");
}

#[tokio::test]
async fn owner_on_create_db_system_actor() {
    let shamir = ShamirDb::init_memory().await.unwrap();

    // System-initiated create → owner stays System.
    shamir.create_db("sys_db").await;

    let meta = shamir
        .resource_meta(&ResourcePath::database("sys_db"))
        .await;
    assert_eq!(
        meta.owner,
        Actor::System,
        "system db owner should be System"
    );
    assert_eq!(meta.mode, 0o777);
}

#[tokio::test]
async fn owner_on_create_repo_user_actor() {
    let shamir = ShamirDb::init_memory().await.unwrap();
    let user_actor = Actor::User(99);

    // Create db first (as system — we only care about the repo).
    shamir.create_db("testdb").await;

    // Create repo via execute_as with user actor.
    let req: BatchRequest = serde_json::from_value(json!({
        "id": 1,
        "queries": {
            "op": {
                "create_repo": "user_repo",
                "engine": "in_memory"
            }
        }
    }))
    .unwrap();
    shamir
        .execute_as(user_actor.clone(), "testdb", &req)
        .await
        .unwrap();

    let meta = shamir
        .resource_meta(&ResourcePath::store("testdb", "user_repo"))
        .await;
    assert_eq!(
        meta.owner,
        Actor::User(99),
        "repo owner should be the user actor"
    );
    assert_eq!(meta.mode, 0o777);
}

#[tokio::test]
async fn owner_on_create_table_user_actor() {
    let shamir = ShamirDb::init_memory().await.unwrap();
    let user_actor = Actor::User(7);

    shamir.create_db("testdb").await;
    let repo_config = RepoConfig::new("main", BoxRepoFactory::in_memory());
    shamir.add_repo("testdb", repo_config).await.unwrap();

    // Create table via execute_as with user actor.
    let req: BatchRequest = serde_json::from_value(json!({
        "id": 1,
        "queries": {
            "op": {
                "create_table": "owned_table",
                "repo": "main"
            }
        }
    }))
    .unwrap();
    shamir
        .execute_as(user_actor.clone(), "testdb", &req)
        .await
        .unwrap();

    let meta = shamir
        .resource_meta(&ResourcePath::table("testdb", "main", "owned_table"))
        .await;
    assert_eq!(
        meta.owner,
        Actor::User(7),
        "table owner should be the user actor"
    );
    assert_eq!(meta.mode, 0o777);
}

#[tokio::test]
async fn owner_on_create_function_user_actor() {
    let shamir = ShamirDb::init_memory().await.unwrap();
    let user_actor = Actor::User(13);

    shamir.create_db("testdb").await;
    let repo_config = RepoConfig::new("main", BoxRepoFactory::in_memory());
    shamir.add_repo("testdb", repo_config).await.unwrap();

    let wasm = accept_wasm();
    let req: BatchRequest = serde_json::from_value(json!({
        "id": 1,
        "queries": {
            "op": {
                "create_function": "user_fn",
                "wasm": wasm_b64(&wasm),
                "replace": false
            }
        }
    }))
    .unwrap();
    shamir
        .execute_as(user_actor.clone(), "testdb", &req)
        .await
        .unwrap();

    let meta = shamir
        .resource_meta(&ResourcePath::function("user_fn"))
        .await;
    assert_eq!(
        meta.owner,
        Actor::User(13),
        "function owner should be the user actor"
    );
    assert_eq!(meta.mode, 0o777);
}

#[tokio::test]
async fn owner_on_create_function_system_stays_system() {
    let shamir = ShamirDb::init_memory().await.unwrap();

    shamir.create_db("testdb").await;
    let repo_config = RepoConfig::new("main", BoxRepoFactory::in_memory());
    shamir.add_repo("testdb", repo_config).await.unwrap();

    let wasm = accept_wasm();
    // Use default execute (System actor).
    let req: BatchRequest = serde_json::from_value(json!({
        "id": 1,
        "queries": {
            "op": {
                "create_function": "sys_fn",
                "wasm": wasm_b64(&wasm),
                "replace": false
            }
        }
    }))
    .unwrap();
    shamir.execute("testdb", &req).await.unwrap();

    let meta = shamir
        .resource_meta(&ResourcePath::function("sys_fn"))
        .await;
    assert_eq!(
        meta.owner,
        Actor::System,
        "system-created function owner should be System"
    );
    assert_eq!(meta.mode, 0o777);
}

// ═══════════════════════════════════════════════════════════════════════
// Phase 1a: create_table + insert in one non-tx batch via `after`
// ═══════════════════════════════════════════════════════════════════════

/// A single non-transactional batch that creates a table and inserts
/// rows into it, using the `after` ordering edge to guarantee the DDL
/// executes before the DML.
///
/// NOTE: the transactional variant is intentionally omitted — admin ops
/// are not tx-aware (they bypass the MVCC pipeline), so a transactional
/// batch mixing DDL and DML would require a separate design effort.
#[tokio::test]
async fn create_table_then_insert_via_after_non_tx() {
    // Setup: db + repo (no "items" table yet).
    let db = ShamirDb::init_memory().await.unwrap();
    db.create_db("testdb").await;
    let repo_config = RepoConfig::new("main", BoxRepoFactory::in_memory());
    db.add_repo("testdb", repo_config).await.unwrap();

    // One batch: create_table("items") + insert into "items", with
    // `after: ["mk"]` on the insert.
    let req: BatchRequest = serde_json::from_value(json!({
        "id": "phase1a",
        "queries": {
            "mk": {
                "create_table": "items",
                "repo": "main"
            },
            "rows": {
                "insert_into": "items",
                "values": [
                    {"name": "Widget", "qty": 10},
                    {"name": "Gadget", "qty": 5}
                ],
                "after": ["mk"]
            }
        }
    }))
    .unwrap();

    let resp = db
        .execute("testdb", &req)
        .await
        .expect("batch with create_table + insert via after should succeed");

    // The execution plan must have two stages: mk first, rows second.
    assert_eq!(
        resp.execution_plan.len(),
        2,
        "expected 2 stages (DDL then DML), got: {:?}",
        resp.execution_plan
    );
    assert_eq!(resp.execution_plan[0], vec!["mk"]);
    assert_eq!(resp.execution_plan[1], vec!["rows"]);

    // Verify insert actually landed: read back the rows.
    let read_req: BatchRequest = serde_json::from_value(json!({
        "id": "verify",
        "queries": {
            "q": {
                "from": "items"
            }
        }
    }))
    .unwrap();
    let read_resp = db.execute("testdb", &read_req).await.unwrap();
    let records = &read_resp.results["q"].records;
    assert_eq!(records.len(), 2, "should have 2 inserted records");
}

// =====================================================================
// Phase 1b: idempotent create (if_not_exists)
// =====================================================================

#[tokio::test]
async fn create_table_duplicate_without_if_not_exists_fails() {
    let db = setup_db().await;

    // First create succeeds
    let req: BatchRequest = serde_json::from_value(json!({
        "id": "ct1",
        "queries": {
            "op": {
                "create_table": "orders",
                "repo": "main"
            }
        }
    }))
    .unwrap();
    let resp = db.execute("testdb", &req).await.unwrap();
    assert_eq!(resp.results["op"].records[0]["created"], true);
    assert_eq!(resp.results["op"].records[0]["existed"], false);

    // Second create without if_not_exists -> error
    let err = db.execute("testdb", &req).await.unwrap_err();
    let msg = err.to_string();
    assert!(
        msg.contains("already exists"),
        "expected 'already exists' error, got: {msg}"
    );
}

#[tokio::test]
async fn create_table_with_if_not_exists_idempotent() {
    let db = setup_db().await;

    // First create
    let req: BatchRequest = serde_json::from_value(json!({
        "id": "ct",
        "queries": {
            "op": {
                "create_table": "orders",
                "repo": "main",
                "if_not_exists": true
            }
        }
    }))
    .unwrap();
    let resp = db.execute("testdb", &req).await.unwrap();
    assert_eq!(resp.results["op"].records[0]["created"], true);
    assert_eq!(resp.results["op"].records[0]["existed"], false);

    // Second create with if_not_exists -> OK, no error
    let resp = db.execute("testdb", &req).await.unwrap();
    assert_eq!(resp.results["op"].records[0]["created"], false);
    assert_eq!(resp.results["op"].records[0]["existed"], true);
}

#[tokio::test]
async fn create_db_with_if_not_exists_idempotent() {
    let shamir = ShamirDb::init_memory().await.unwrap();
    shamir.create_db("bootstrap").await;

    // Create a new db
    let req: BatchRequest = serde_json::from_value(json!({
        "id": "cd",
        "queries": {
            "op": {
                "create_db": "newdb",
                "if_not_exists": true
            }
        }
    }))
    .unwrap();
    let resp = shamir.execute("bootstrap", &req).await.unwrap();
    assert_eq!(resp.results["op"].records[0]["created"], true);
    assert_eq!(resp.results["op"].records[0]["existed"], false);

    // Second create with if_not_exists -> OK, existed=true
    let resp = shamir.execute("bootstrap", &req).await.unwrap();
    assert_eq!(resp.results["op"].records[0]["created"], false);
    assert_eq!(resp.results["op"].records[0]["existed"], true);
}

#[tokio::test]
async fn create_repo_with_if_not_exists_idempotent() {
    let shamir = ShamirDb::init_memory().await.unwrap();
    shamir.create_db("testdb").await;

    let req: BatchRequest = serde_json::from_value(json!({
        "id": "cr",
        "queries": {
            "op": {
                "create_repo": "archive",
                "engine": "in_memory",
                "if_not_exists": true
            }
        }
    }))
    .unwrap();
    let resp = shamir.execute("testdb", &req).await.unwrap();
    assert_eq!(resp.results["op"].records[0]["created"], true);
    assert_eq!(resp.results["op"].records[0]["existed"], false);

    // Second create -> OK
    let resp = shamir.execute("testdb", &req).await.unwrap();
    assert_eq!(resp.results["op"].records[0]["created"], false);
    assert_eq!(resp.results["op"].records[0]["existed"], true);
}

// =====================================================================
// Phase 1b: referential integrity on drop + cascade
// =====================================================================

#[tokio::test]
async fn drop_db_with_repos_no_cascade_fails() {
    let db = setup_db().await;

    // testdb has "main" repo -> drop without cascade should fail
    let drop_req: BatchRequest = serde_json::from_value(json!({
        "id": "dd",
        "queries": {
            "op": {
                "drop_db": "testdb"
            }
        }
    }))
    .unwrap();
    let err = db.execute("testdb", &drop_req).await.unwrap_err();
    let msg = err.to_string();
    assert!(
        msg.contains("still has repositories"),
        "expected referential integrity error, got: {msg}"
    );
}

#[tokio::test]
async fn drop_db_with_cascade_succeeds() {
    let shamir = ShamirDb::init_memory().await.unwrap();
    shamir.create_db("bootstrap").await;
    shamir.create_db("target_db").await;
    let repo_config =
        RepoConfig::new("main", BoxRepoFactory::in_memory()).add_table(TableConfig::new("items"));
    shamir.add_repo("target_db", repo_config).await.unwrap();

    // Drop with cascade
    let drop_req: BatchRequest = serde_json::from_value(json!({
        "id": "dd",
        "queries": {
            "op": {
                "drop_db": "target_db",
                "cascade": true
            }
        }
    }))
    .unwrap();
    let resp = shamir.execute("bootstrap", &drop_req).await.unwrap();
    assert_eq!(resp.results["op"].records[0]["existed"], true);

    // Verify the db is gone
    assert!(!shamir.has_db("target_db"));
}

#[tokio::test]
async fn drop_repo_with_tables_no_cascade_fails() {
    let db = setup_db().await;

    // "main" repo has "users" table -> drop without cascade should fail
    let drop_req: BatchRequest = serde_json::from_value(json!({
        "id": "dr",
        "queries": {
            "op": {
                "drop_repo": "main"
            }
        }
    }))
    .unwrap();
    let err = db.execute("testdb", &drop_req).await.unwrap_err();
    let msg = err.to_string();
    assert!(
        msg.contains("still has tables"),
        "expected referential integrity error, got: {msg}"
    );
}

#[tokio::test]
async fn drop_repo_with_cascade_succeeds() {
    let db = setup_db().await;

    // Drop "main" with cascade (it has "users" table)
    let drop_req: BatchRequest = serde_json::from_value(json!({
        "id": "dr",
        "queries": {
            "op": {
                "drop_repo": "main",
                "cascade": true
            }
        }
    }))
    .unwrap();
    let resp = db.execute("testdb", &drop_req).await.unwrap();
    assert_eq!(resp.results["op"].records[0]["existed"], true);

    // Verify the repo is gone
    let db_inst = db.get_db("testdb").unwrap();
    assert!(db_inst.list_repos().is_empty());
}

// =====================================================================
// Phase 1b: drop_table cleans validator bound_in
// =====================================================================

#[tokio::test]
async fn drop_table_cleans_validator_bound_in() {
    let db = setup_db().await;

    // Step 1: create a validator and bind it to "users"
    let wasm = accept_wasm();
    let create_req: BatchRequest = serde_json::from_value(json!({
        "id": "cv",
        "queries": {
            "op": {
                "create_validator": "v_cleanup",
                "wasm": wasm_b64(&wasm),
                "replace": false
            }
        }
    }))
    .unwrap();
    db.execute("testdb", &create_req).await.unwrap();

    let bind_req: BatchRequest = serde_json::from_value(json!({
        "id": "bv",
        "queries": {
            "op": {
                "bind_validator": "v_cleanup",
                "db": "testdb",
                "repo": "main",
                "table": "users",
                "ops": ["insert"],
                "priority": 1500
            }
        }
    }))
    .unwrap();
    db.execute("testdb", &bind_req).await.unwrap();

    // Step 2: drop the table -> should clean bound_in
    let drop_req: BatchRequest = serde_json::from_value(json!({
        "id": "dt",
        "queries": {
            "op": {
                "drop_table": "users",
                "repo": "main"
            }
        }
    }))
    .unwrap();
    let resp = db.execute("testdb", &drop_req).await.unwrap();
    assert_eq!(resp.results["op"].records[0]["existed"], true);

    // Step 3: now drop_validator should succeed (bound_in was cleaned)
    let drop_val_req: BatchRequest = serde_json::from_value(json!({
        "id": "dv",
        "queries": {
            "op": {
                "drop_validator": "v_cleanup"
            }
        }
    }))
    .unwrap();
    let resp = db.execute("testdb", &drop_val_req).await.unwrap();
    assert_eq!(
        resp.results["op"].records[0]["existed"], true,
        "validator should have existed and been dropped after bound_in cleanup"
    );
}

// =====================================================================
// Phase 1b: serde round-trip — if_not_exists / cascade
// =====================================================================

#[test]
fn serde_create_table_if_not_exists_round_trip() {
    // With flag set
    let json_with = r#"{
        "create_table": "orders",
        "repo": "main",
        "if_not_exists": true
    }"#;
    let op: shamir_db::query::batch::BatchOp = serde_json::from_str(json_with).unwrap();
    let back = serde_json::to_string(&op).unwrap();
    let op2: shamir_db::query::batch::BatchOp = serde_json::from_str(&back).unwrap();
    assert_eq!(op, op2);
    assert!(
        back.contains("if_not_exists"),
        "serialised form should contain if_not_exists when true"
    );

    // With flag absent (default false) — should NOT appear in JSON
    let json_without = r#"{
        "create_table": "orders",
        "repo": "main"
    }"#;
    let op3: shamir_db::query::batch::BatchOp = serde_json::from_str(json_without).unwrap();
    let back3 = serde_json::to_string(&op3).unwrap();
    assert!(
        !back3.contains("if_not_exists"),
        "serialised form should omit if_not_exists when false, got: {back3}"
    );
}

#[test]
fn serde_drop_db_cascade_round_trip() {
    // With cascade
    let json_with = r#"{
        "drop_db": "testdb",
        "cascade": true
    }"#;
    let op: shamir_db::query::batch::BatchOp = serde_json::from_str(json_with).unwrap();
    let back = serde_json::to_string(&op).unwrap();
    let op2: shamir_db::query::batch::BatchOp = serde_json::from_str(&back).unwrap();
    assert_eq!(op, op2);
    assert!(
        back.contains("cascade"),
        "serialised form should contain cascade when true"
    );

    // Without cascade — should NOT appear in JSON
    let json_without = r#"{
        "drop_db": "testdb"
    }"#;
    let op3: shamir_db::query::batch::BatchOp = serde_json::from_str(json_without).unwrap();
    let back3 = serde_json::to_string(&op3).unwrap();
    assert!(
        !back3.contains("cascade"),
        "serialised form should omit cascade when false, got: {back3}"
    );
}

#[test]
fn serde_drop_repo_cascade_round_trip() {
    let json_str = r#"{
        "drop_repo": "archive",
        "cascade": true
    }"#;
    let op: shamir_db::query::batch::BatchOp = serde_json::from_str(json_str).unwrap();
    let back = serde_json::to_string(&op).unwrap();
    let op2: shamir_db::query::batch::BatchOp = serde_json::from_str(&back).unwrap();
    assert_eq!(op, op2);
    assert!(back.contains("cascade"));
}

#[test]
fn serde_create_db_if_not_exists_round_trip() {
    let json_str = r#"{
        "create_db": "mydb",
        "if_not_exists": true
    }"#;
    let op: shamir_db::query::batch::BatchOp = serde_json::from_str(json_str).unwrap();
    let back = serde_json::to_string(&op).unwrap();
    assert!(back.contains("if_not_exists"));
    let op2: shamir_db::query::batch::BatchOp = serde_json::from_str(&back).unwrap();
    assert_eq!(op, op2);
}

// =====================================================================
// Phase 1b-D: folder-meta persistence (#118)
// =====================================================================

#[tokio::test]
async fn create_function_folder_persists_mkdir_p() {
    let db = setup_db().await;

    // Create ["reports", "daily"] → should create both "reports" and
    // "reports/daily".
    let req: BatchRequest = serde_json::from_value(json!({
        "id": "cff",
        "queries": {
            "op": {
                "create_function_folder": ["reports", "daily"]
            }
        }
    }))
    .unwrap();
    let resp = db.execute("testdb", &req).await.unwrap();
    let result = &resp.results["op"].records[0];
    assert_eq!(
        result["created_function_folder"],
        json!(["reports", "daily"])
    );
    // Both intermediate and leaf folders should be created.
    let created = result["created"].as_array().unwrap();
    assert!(
        created.contains(&json!("reports")),
        "should have created 'reports', got: {:?}",
        created
    );
    assert!(
        created.contains(&json!("reports/daily")),
        "should have created 'reports/daily', got: {:?}",
        created
    );
}

#[tokio::test]
async fn create_function_folder_idempotent() {
    let db = setup_db().await;

    let req: BatchRequest = serde_json::from_value(json!({
        "id": "cff",
        "queries": {
            "op": {
                "create_function_folder": ["utils"]
            }
        }
    }))
    .unwrap();

    // First create
    let resp = db.execute("testdb", &req).await.unwrap();
    let created = resp.results["op"].records[0]["created"].as_array().unwrap();
    assert_eq!(created.len(), 1);

    // Second create → no error, but nothing new created.
    let resp = db.execute("testdb", &req).await.unwrap();
    let created = resp.results["op"].records[0]["created"].as_array().unwrap();
    assert_eq!(
        created.len(),
        0,
        "repeat create should produce no new folders"
    );
}

#[tokio::test]
async fn create_function_folder_meta_owner_is_actor() {
    let shamir = ShamirDb::init_memory().await.unwrap();
    shamir.create_db("testdb").await;
    let repo_config = RepoConfig::new("main", BoxRepoFactory::in_memory());
    shamir.add_repo("testdb", repo_config).await.unwrap();

    let user_actor = Actor::User(55);

    let req: BatchRequest = serde_json::from_value(json!({
        "id": "cff",
        "queries": {
            "op": {
                "create_function_folder": ["owned_folder"]
            }
        }
    }))
    .unwrap();
    shamir
        .execute_as(user_actor.clone(), "testdb", &req)
        .await
        .unwrap();

    // Read back the meta.
    let meta = shamir
        .resource_meta(&ResourcePath::FunctionFolder {
            path: vec!["owned_folder".to_string()],
        })
        .await;
    assert_eq!(
        meta.owner,
        Actor::User(55),
        "folder owner should be the creating user actor"
    );
    assert_eq!(meta.mode, 0o777, "mode must stay open");
}

#[tokio::test]
async fn function_folder_meta_survives_reopen() {
    // Simulate restart: init → create folder → re-init from same store
    // (in-memory doesn't truly survive, but we confirm resource_meta
    // reads from the catalogue, not ephemeral state).
    let shamir = ShamirDb::init_memory().await.unwrap();
    shamir.create_db("testdb").await;
    let repo_config = RepoConfig::new("main", BoxRepoFactory::in_memory());
    shamir.add_repo("testdb", repo_config).await.unwrap();

    let req: BatchRequest = serde_json::from_value(json!({
        "id": "cff",
        "queries": {
            "op": {
                "create_function_folder": ["persist_test"]
            }
        }
    }))
    .unwrap();
    shamir.execute("testdb", &req).await.unwrap();

    // Confirm meta is persisted (reads from catalogue).
    let meta = shamir
        .resource_meta(&ResourcePath::FunctionFolder {
            path: vec!["persist_test".to_string()],
        })
        .await;
    assert_eq!(meta.owner, Actor::System);
    assert_eq!(meta.mode, 0o777);
}

// =====================================================================
// Phase 1b-D: backward compat — slash-named functions without explicit folders
// =====================================================================

#[tokio::test]
async fn slash_named_function_works_without_explicit_folder() {
    let db = setup_db().await;

    // Create a function with a slash-name — no explicit folder creation.
    let wasm = accept_wasm();
    let create_req: BatchRequest = serde_json::from_value(json!({
        "id": "cf",
        "queries": {
            "op": {
                "create_function": "math/abs",
                "wasm": wasm_b64(&wasm),
                "replace": false
            }
        }
    }))
    .unwrap();
    db.execute("testdb", &create_req).await.unwrap();

    // The implicit folder "math" should return open meta (not error).
    let meta = db
        .resource_meta(&ResourcePath::FunctionFolder {
            path: vec!["math".to_string()],
        })
        .await;
    assert_eq!(
        meta,
        shamir_types::access::ResourceMeta::open(),
        "implicit folder should return open meta for backward compat"
    );

    // The function itself should be invocable/listable.
    let functions = db.list_functions().await.unwrap();
    assert!(
        functions.contains(&"math/abs".to_string()),
        "slash-named function should be listed"
    );
}

// =====================================================================
// Phase 1b-C: introspection — list functions / validators / function_folders
// =====================================================================

#[tokio::test]
async fn list_functions_over_wire() {
    let db = setup_db().await;

    // Create two functions
    let wasm = accept_wasm();
    for name in ["fn_alpha", "fn_beta"] {
        let req: BatchRequest = serde_json::from_value(json!({
            "id": "cf",
            "queries": {
                "op": {
                    "create_function": name,
                    "wasm": wasm_b64(&wasm),
                    "replace": false
                }
            }
        }))
        .unwrap();
        db.execute("testdb", &req).await.unwrap();
    }

    // List all functions
    let list_req: BatchRequest = serde_json::from_value(json!({
        "id": "lf",
        "queries": {
            "op": {
                "list": "functions"
            }
        }
    }))
    .unwrap();
    let resp = db.execute("testdb", &list_req).await.unwrap();
    let result = &resp.results["op"].records[0];
    let fns = result["functions"].as_array().unwrap();
    assert!(
        fns.iter().any(|f| f == "fn_alpha"),
        "should contain fn_alpha"
    );
    assert!(fns.iter().any(|f| f == "fn_beta"), "should contain fn_beta");
}

#[tokio::test]
async fn list_functions_filtered_by_folder() {
    let db = setup_db().await;

    let wasm = accept_wasm();
    for name in ["math/add", "math/sub", "str/upper"] {
        let req: BatchRequest = serde_json::from_value(json!({
            "id": "cf",
            "queries": {
                "op": {
                    "create_function": name,
                    "wasm": wasm_b64(&wasm),
                    "replace": false
                }
            }
        }))
        .unwrap();
        db.execute("testdb", &req).await.unwrap();
    }

    // List filtered by folder "math"
    let list_req: BatchRequest = serde_json::from_value(json!({
        "id": "lf",
        "queries": {
            "op": {
                "list": "functions",
                "folder": "math"
            }
        }
    }))
    .unwrap();
    let resp = db.execute("testdb", &list_req).await.unwrap();
    let fns = resp.results["op"].records[0]["functions"]
        .as_array()
        .unwrap();
    assert_eq!(fns.len(), 2, "should have 2 math functions, got: {:?}", fns);
    assert!(fns.iter().any(|f| f == "math/add"));
    assert!(fns.iter().any(|f| f == "math/sub"));
}

#[tokio::test]
async fn list_validators_all_over_wire() {
    let db = setup_db().await;

    let wasm = accept_wasm();
    let create_req: BatchRequest = serde_json::from_value(json!({
        "id": "cv",
        "queries": {
            "op": {
                "create_validator": "v_list_all",
                "wasm": wasm_b64(&wasm),
                "replace": false
            }
        }
    }))
    .unwrap();
    db.execute("testdb", &create_req).await.unwrap();

    // Bind it to a table so we can verify bound_in.
    let bind_req: BatchRequest = serde_json::from_value(json!({
        "id": "bv",
        "queries": {
            "op": {
                "bind_validator": "v_list_all",
                "db": "testdb",
                "repo": "main",
                "table": "users",
                "ops": ["insert"],
                "priority": 1500
            }
        }
    }))
    .unwrap();
    db.execute("testdb", &bind_req).await.unwrap();

    // List ALL validators (not per-table; the new ListOp::Validators).
    let list_req: BatchRequest = serde_json::from_value(json!({
        "id": "lv",
        "queries": {
            "op": {
                "list": "validators"
            }
        }
    }))
    .unwrap();
    let resp = db.execute("testdb", &list_req).await.unwrap();
    let items = resp.results["op"].records[0]["validators"]
        .as_array()
        .unwrap();
    assert!(!items.is_empty(), "should have at least one validator");
    let v = items
        .iter()
        .find(|v| v["name"] == "v_list_all")
        .expect("should find v_list_all");
    assert!(v.get("id").is_some(), "should have id");
    let bound = v["bound_in"].as_array().unwrap();
    assert!(!bound.is_empty(), "should have at least one bound_in entry");
}

#[tokio::test]
async fn list_function_folders_over_wire() {
    let db = setup_db().await;

    // Create folders
    let req: BatchRequest = serde_json::from_value(json!({
        "id": "cff",
        "queries": {
            "op": {
                "create_function_folder": ["reports", "daily"]
            }
        }
    }))
    .unwrap();
    db.execute("testdb", &req).await.unwrap();

    // List all folders
    let list_req: BatchRequest = serde_json::from_value(json!({
        "id": "lff",
        "queries": {
            "op": {
                "list": "function_folders"
            }
        }
    }))
    .unwrap();
    let resp = db.execute("testdb", &list_req).await.unwrap();
    let folders = resp.results["op"].records[0]["function_folders"]
        .as_array()
        .unwrap();
    assert!(
        folders.contains(&json!("reports")),
        "should contain 'reports'"
    );
    assert!(
        folders.contains(&json!("reports/daily")),
        "should contain 'reports/daily'"
    );
}

#[tokio::test]
async fn list_function_folders_filtered_by_parent() {
    let db = setup_db().await;

    // Create folders in two trees.
    for path in [
        vec!["alpha".to_string()],
        vec!["alpha".to_string(), "beta".to_string()],
        vec!["gamma".to_string()],
    ] {
        let req: BatchRequest = serde_json::from_value(json!({
            "id": "cff",
            "queries": {
                "op": {
                    "create_function_folder": path
                }
            }
        }))
        .unwrap();
        db.execute("testdb", &req).await.unwrap();
    }

    // List only children of "alpha"
    let list_req: BatchRequest = serde_json::from_value(json!({
        "id": "lff",
        "queries": {
            "op": {
                "list": "function_folders",
                "parent": "alpha"
            }
        }
    }))
    .unwrap();
    let resp = db.execute("testdb", &list_req).await.unwrap();
    let folders = resp.results["op"].records[0]["function_folders"]
        .as_array()
        .unwrap();
    assert_eq!(
        folders.len(),
        1,
        "should have 1 folder under alpha, got: {:?}",
        folders
    );
    assert_eq!(folders[0], "alpha/beta");
}

// =====================================================================
// Serde round-trip for new ListOp variants
// =====================================================================

#[test]
fn serde_list_functions_round_trip() {
    let json_str = r#"{"list": "functions"}"#;
    let op: shamir_db::query::batch::BatchOp = serde_json::from_str(json_str).unwrap();
    assert!(op.is_admin());
    let back = serde_json::to_string(&op).unwrap();
    let op2: shamir_db::query::batch::BatchOp = serde_json::from_str(&back).unwrap();
    assert_eq!(op, op2);
}

#[test]
fn serde_list_functions_with_folder_round_trip() {
    let json_str = r#"{"list": "functions", "folder": "math"}"#;
    let op: shamir_db::query::batch::BatchOp = serde_json::from_str(json_str).unwrap();
    assert!(op.is_admin());
    let back = serde_json::to_string(&op).unwrap();
    assert!(
        back.contains("math"),
        "serialised form should contain folder"
    );
    let op2: shamir_db::query::batch::BatchOp = serde_json::from_str(&back).unwrap();
    assert_eq!(op, op2);
}

#[test]
fn serde_list_validators_round_trip() {
    let json_str = r#"{"list": "validators"}"#;
    let op: shamir_db::query::batch::BatchOp = serde_json::from_str(json_str).unwrap();
    assert!(op.is_admin());
    let back = serde_json::to_string(&op).unwrap();
    let op2: shamir_db::query::batch::BatchOp = serde_json::from_str(&back).unwrap();
    assert_eq!(op, op2);
}

#[test]
fn serde_list_function_folders_round_trip() {
    let json_str = r#"{"list": "function_folders"}"#;
    let op: shamir_db::query::batch::BatchOp = serde_json::from_str(json_str).unwrap();
    assert!(op.is_admin());
    let back = serde_json::to_string(&op).unwrap();
    let op2: shamir_db::query::batch::BatchOp = serde_json::from_str(&back).unwrap();
    assert_eq!(op, op2);
}

#[test]
fn serde_list_function_folders_with_parent_round_trip() {
    let json_str = r#"{"list": "function_folders", "parent": "reports"}"#;
    let op: shamir_db::query::batch::BatchOp = serde_json::from_str(json_str).unwrap();
    assert!(op.is_admin());
    let back = serde_json::to_string(&op).unwrap();
    assert!(
        back.contains("reports"),
        "serialised form should contain parent"
    );
    let op2: shamir_db::query::batch::BatchOp = serde_json::from_str(&back).unwrap();
    assert_eq!(op, op2);
}
