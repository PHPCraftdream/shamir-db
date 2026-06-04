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
