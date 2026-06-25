//! Function folders and introspection listing tests.

use shamir_db::engine::repo::repo_types::BoxRepoFactory;
use shamir_db::engine::repo::RepoConfig;
use shamir_db::ShamirDb;
use shamir_query_builder::batch::Batch;
use shamir_query_builder::ddl;
use shamir_query_builder::ddl::WriteOp;
use shamir_types::access::{Actor, ResourcePath};
use shamir_types::types::value::QueryValue;

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
    let result = resp.results["op"].records[0].as_value().into_owned();
    assert_eq!(
        result["created_function_folder"].as_array().unwrap(),
        &[
            QueryValue::Str("reports".to_string()),
            QueryValue::Str("daily".to_string()),
        ]
    );
    // Both intermediate and leaf folders should be created.
    let created = result["created"].as_array().unwrap();
    assert!(
        created.iter().any(|v| v == "reports"),
        "should have created 'reports', got: {:?}",
        created
    );
    assert!(
        created.iter().any(|v| v == "reports/daily"),
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
    let rec = resp.results["op"].records[0].as_value().into_owned();
    let created = rec["created"].as_array().unwrap();
    assert_eq!(created.len(), 1);

    // Second create → no error, but nothing new created.
    let resp = db.execute("testdb", &req).await.unwrap();
    let rec = resp.results["op"].records[0].as_value().into_owned();
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
    // G.4c: open the db so the user actor can traverse it to reach the
    // function-namespace Create path.
    shamir
        .set_resource_meta(
            &ResourcePath::database("testdb"),
            &shamir_types::access::ResourceMeta::open(),
        )
        .await
        .unwrap();

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
    // G.4c: new objects default to enforced owner-rwx (0o700).
    assert_eq!(meta.mode, 0o700, "mode must be enforced (0o700)");
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
    // G.4c: new objects default to enforced owner-rwx (0o700).
    assert_eq!(meta.mode, 0o700);
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
    let result = resp.results["op"].records[0].as_value().into_owned();
    let fns = result["functions"].as_array().unwrap();
    // Phase 4: each entry is now a {name, kind} object. WASM functions
    // created via create_function report kind = "wasm".
    let names: Vec<&str> = fns.iter().map(|f| f["name"].as_str().unwrap()).collect();
    assert!(names.contains(&"fn_alpha"), "should contain fn_alpha");
    assert!(names.contains(&"fn_beta"), "should contain fn_beta");
    for f in fns {
        assert_eq!(
            f["kind"].as_str(),
            Some("wasm"),
            "wasm fn should report kind=wasm"
        );
    }
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
    let rec = resp.results["op"].records[0].as_value().into_owned();
    let fns = rec["functions"].as_array().unwrap();
    // Phase 4: entries are {name, kind} objects.
    let names: Vec<&str> = fns.iter().map(|f| f["name"].as_str().unwrap()).collect();
    assert_eq!(
        fns.len(),
        2,
        "should have 2 math functions, got: {:?}",
        names
    );
    assert!(names.contains(&"math/add"));
    assert!(names.contains(&"math/sub"));
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
    let rec = resp.results["op"].records[0].as_value().into_owned();
    let items = rec["validators"].as_array().unwrap();
    assert!(!items.is_empty(), "should have at least one validator");
    let v = items
        .iter()
        .find(|v| v["name"] == "v_list_all")
        .expect("should find v_list_all");
    assert!(v.get("id").is_some(), "should have id");
    // Phase 4: kind is surfaced on the validator list response.
    assert_eq!(
        v["kind"].as_str(),
        Some("wasm"),
        "wasm validator should report kind=wasm"
    );
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
    let rec = resp.results["op"].records[0].as_value().into_owned();
    let folders = rec["function_folders"].as_array().unwrap();
    assert!(
        folders.iter().any(|v| v == "reports"),
        "should contain 'reports'"
    );
    assert!(
        folders.iter().any(|v| v == "reports/daily"),
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
    let rec = resp.results["op"].records[0].as_value().into_owned();
    let folders = rec["function_folders"].as_array().unwrap();
    assert_eq!(
        folders.len(),
        1,
        "should have 1 folder under alpha, got: {:?}",
        folders
    );
    assert_eq!(folders[0], "alpha/beta");
}
