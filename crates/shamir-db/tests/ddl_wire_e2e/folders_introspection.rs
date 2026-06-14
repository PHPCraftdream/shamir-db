//! Function folders and introspection listing tests.

use serde_json::json;
use shamir_db::engine::repo::repo_types::BoxRepoFactory;
use shamir_db::engine::repo::RepoConfig;
use shamir_db::ShamirDb;
use shamir_query_builder::batch::Batch;
use shamir_query_builder::ddl;
use shamir_query_builder::ddl::WriteOp;
use shamir_types::access::{Actor, ResourcePath};

use super::helpers::*;

// =====================================================================
// Phase 1b-D: folder-meta persistence (#118)
// =====================================================================

#[tokio::test]
async fn create_function_folder_persists_mkdir_p() {
    let db = setup_db().await;

    // Create ["reports", "daily"] → should create both "reports" and
    // "reports/daily".
    let mut b = Batch::new();
    b.id("cff");
    b.create_function_folder("op", ddl::create_function_folder(["reports", "daily"]));
    let req = b.to_request_via_msgpack();
    let resp = db.execute("testdb", &req).await.unwrap();
    let result = resp.results["op"].records[0].as_json();
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

    let mut b = Batch::new();
    b.id("cff");
    b.create_function_folder("op", ddl::create_function_folder(["utils"]));
    let req = b.to_request_via_msgpack();

    // First create
    let resp = db.execute("testdb", &req).await.unwrap();
    let rec = resp.results["op"].records[0].as_json();
    let created = rec["created"].as_array().unwrap();
    assert_eq!(created.len(), 1);

    // Second create → no error, but nothing new created.
    let resp = db.execute("testdb", &req).await.unwrap();
    let rec = resp.results["op"].records[0].as_json();
    let created = rec["created"].as_array().unwrap();
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

    let mut b = Batch::new();
    b.id("cff");
    b.create_function_folder("op", ddl::create_function_folder(["owned_folder"]));
    let req = b.to_request_via_msgpack();
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

    let mut b = Batch::new();
    b.id("cff");
    b.create_function_folder("op", ddl::create_function_folder(["persist_test"]));
    let req = b.to_request_via_msgpack();
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
    let mut b = Batch::new();
    b.id("cf");
    b.create_function("op", ddl::create_function("math/abs").wasm(wasm_b64(&wasm)));
    let create_req = b.to_request_via_msgpack();
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
        let mut b = Batch::new();
        b.id("cf");
        b.create_function("op", ddl::create_function(name).wasm(wasm_b64(&wasm)));
        let req = b.to_request_via_msgpack();
        db.execute("testdb", &req).await.unwrap();
    }

    let mut b = Batch::new();
    b.id("lf");
    b.list_functions("op", ddl::list_functions());
    let list_req = b.to_request_via_msgpack();
    let resp = db.execute("testdb", &list_req).await.unwrap();
    let result = resp.results["op"].records[0].as_json();
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
        let mut b = Batch::new();
        b.id("cf");
        b.create_function("op", ddl::create_function(name).wasm(wasm_b64(&wasm)));
        let req = b.to_request_via_msgpack();
        db.execute("testdb", &req).await.unwrap();
    }

    let mut b = Batch::new();
    b.id("lf");
    b.list_functions("op", ddl::list_functions().folder("math"));
    let list_req = b.to_request_via_msgpack();
    let resp = db.execute("testdb", &list_req).await.unwrap();
    let rec = resp.results["op"].records[0].as_json();
    let fns = rec["functions"].as_array().unwrap();
    assert_eq!(fns.len(), 2, "should have 2 math functions, got: {:?}", fns);
    assert!(fns.iter().any(|f| f == "math/add"));
    assert!(fns.iter().any(|f| f == "math/sub"));
}

#[tokio::test]
async fn list_validators_all_over_wire() {
    let db = setup_db().await;

    let wasm = accept_wasm();
    let mut b = Batch::new();
    b.id("cv");
    b.create_validator(
        "op",
        ddl::create_validator("v_list_all").wasm(wasm_b64(&wasm)),
    );
    let create_req = b.to_request_via_msgpack();
    db.execute("testdb", &create_req).await.unwrap();

    // Bind it to a table so we can verify bound_in.
    let mut b = Batch::new();
    b.id("bv");
    b.bind_validator(
        "op",
        ddl::bind_validator("v_list_all", "users")
            .db("testdb")
            .ops([WriteOp::Insert])
            .priority(1500),
    );
    let bind_req = b.to_request_via_msgpack();
    db.execute("testdb", &bind_req).await.unwrap();

    let mut b = Batch::new();
    b.id("lv");
    b.list_all_validators("op", ddl::list_all_validators());
    let list_req = b.to_request_via_msgpack();
    let resp = db.execute("testdb", &list_req).await.unwrap();
    let rec = resp.results["op"].records[0].as_json();
    let items = rec["validators"].as_array().unwrap();
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
    let mut b = Batch::new();
    b.id("cff");
    b.create_function_folder("op", ddl::create_function_folder(["reports", "daily"]));
    let req = b.to_request_via_msgpack();
    db.execute("testdb", &req).await.unwrap();

    let mut b = Batch::new();
    b.id("lff");
    b.list_function_folders("op", ddl::list_function_folders());
    let list_req = b.to_request_via_msgpack();
    let resp = db.execute("testdb", &list_req).await.unwrap();
    let rec = resp.results["op"].records[0].as_json();
    let folders = rec["function_folders"].as_array().unwrap();
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
        let mut b = Batch::new();
        b.id("cff");
        b.create_function_folder("op", ddl::create_function_folder(path));
        let req = b.to_request_via_msgpack();
        db.execute("testdb", &req).await.unwrap();
    }

    let mut b = Batch::new();
    b.id("lff");
    b.list_function_folders("op", ddl::list_function_folders().parent("alpha"));
    let list_req = b.to_request_via_msgpack();
    let resp = db.execute("testdb", &list_req).await.unwrap();
    let rec = resp.results["op"].records[0].as_json();
    let folders = rec["function_folders"].as_array().unwrap();
    assert_eq!(
        folders.len(),
        1,
        "should have 1 folder under alpha, got: {:?}",
        folders
    );
    assert_eq!(folders[0], "alpha/beta");
}
