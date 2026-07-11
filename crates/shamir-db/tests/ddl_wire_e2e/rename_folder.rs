//! Rename function-folder DDL: rename, nested rekey, and guards.

use shamir_db::engine::repo::repo_types::BoxRepoFactory;
use shamir_db::engine::repo::RepoConfig;
use shamir_db::engine::table::TableConfig;
use shamir_db::ShamirDb;
use shamir_query_builder::batch::Batch;
use shamir_query_builder::ddl;
use shamir_types::access::ResourcePath;

use super::helpers::*;

// =====================================================================
// rename_function_folder — basic rename + readback
// =====================================================================

#[tokio::test]
async fn rename_function_folder_basic() {
    let db = setup_db().await;

    // Create ["a","b"].
    let mut b = Batch::new();
    b.id("cff");
    b.create_function_folder("op", ddl::create_function_folder(["a", "b"]));
    db.execute("testdb", &b.to_request_via_msgpack())
        .await
        .unwrap();

    // Rename ["a","b"] → ["a","c"].
    let mut b = Batch::new();
    b.id("rff");
    b.rename_function_folder("op", ddl::rename_function_folder(["a", "b"], ["a", "c"]));
    let resp = db
        .execute("testdb", &b.to_request_via_msgpack())
        .await
        .unwrap();
    let rec = resp.results["op"].records[0].as_value().into_owned();
    assert_eq!(
        rec["renamed_function_folder"].as_array().unwrap(),
        &[
            shamir_types::types::value::QueryValue::Str("a".to_string()),
            shamir_types::types::value::QueryValue::Str("b".to_string()),
        ]
    );
    assert_eq!(
        rec["to"].as_array().unwrap(),
        &[
            shamir_types::types::value::QueryValue::Str("a".to_string()),
            shamir_types::types::value::QueryValue::Str("c".to_string()),
        ]
    );

    // Readback via list_function_folders(): a/c present, a/b absent, a present.
    let folders = db.list_function_folders().await.unwrap();
    assert!(
        folders.iter().any(|p| p == "a"),
        "'a' should remain, got: {:?}",
        folders
    );
    assert!(
        folders.iter().any(|p| p == "a/c"),
        "'a/c' should be present, got: {:?}",
        folders
    );
    assert!(
        !folders.iter().any(|p| p == "a/b"),
        "'a/b' should be gone, got: {:?}",
        folders
    );
}

// =====================================================================
// rename_function_folder — nested subtree rekey
// =====================================================================

#[tokio::test]
async fn rename_function_folder_nested_rekeys_descendants() {
    let db = setup_db().await;

    // Create ["a","b","c"] (mkdir -p creates a, a/b, a/b/c).
    let mut b = Batch::new();
    b.id("cff");
    b.create_function_folder("op", ddl::create_function_folder(["a", "b", "c"]));
    db.execute("testdb", &b.to_request_via_msgpack())
        .await
        .unwrap();

    // Rename ["a","b"] → ["a","x"].
    let mut b = Batch::new();
    b.id("rff");
    b.rename_function_folder("op", ddl::rename_function_folder(["a", "b"], ["a", "x"]));
    db.execute("testdb", &b.to_request_via_msgpack())
        .await
        .unwrap();

    // a/x and a/x/c present, all a/b* gone, a remains.
    let folders = db.list_function_folders().await.unwrap();
    assert!(folders.iter().any(|p| p == "a"), "a should remain");
    assert!(
        folders.iter().any(|p| p == "a/x"),
        "a/x should be present, got: {:?}",
        folders
    );
    assert!(
        folders.iter().any(|p| p == "a/x/c"),
        "a/x/c should be present, got: {:?}",
        folders
    );
    assert!(
        !folders.iter().any(|p| p == "a/b" || p.starts_with("a/b/")),
        "no a/b* should remain, got: {:?}",
        folders
    );
}

// =====================================================================
// rename_function_folder — ResourceMeta preserved (owner)
// =====================================================================

#[tokio::test]
async fn rename_function_folder_preserves_resource_meta() {
    let shamir = ShamirDb::init_memory().await.unwrap();
    shamir.create_db("testdb").await;
    let repo_config =
        RepoConfig::new("main", BoxRepoFactory::in_memory()).add_table(TableConfig::new("users"));
    shamir.add_repo("testdb", repo_config).await.unwrap();
    // Open the db so a user actor can traverse to the function-namespace.
    shamir
        .set_resource_meta(
            &ResourcePath::database("testdb"),
            &shamir_types::access::ResourceMeta::open(),
        )
        .await
        .unwrap();

    let user_actor = shamir_types::access::Actor::User(77);

    // Create ["meta","src"] as user 77.
    let mut b = Batch::new();
    b.id("cff");
    b.create_function_folder("op", ddl::create_function_folder(["meta", "src"]));
    shamir
        .execute_as(user_actor.clone(), "testdb", &b.to_request_via_msgpack())
        .await
        .unwrap();

    // Rename ["meta","src"] → ["meta","dst"] as the same user.
    let mut b = Batch::new();
    b.id("rff");
    b.rename_function_folder(
        "op",
        ddl::rename_function_folder(["meta", "src"], ["meta", "dst"]),
    );
    shamir
        .execute_as(user_actor.clone(), "testdb", &b.to_request_via_msgpack())
        .await
        .unwrap();

    // Owner should be preserved on the renamed folder.
    let meta = shamir
        .resource_meta(&ResourcePath::FunctionFolder {
            path: vec!["meta".to_string(), "dst".to_string()],
        })
        .await
        .unwrap();
    assert_eq!(
        meta.owner,
        shamir_types::access::Actor::User(77),
        "owner must be preserved across rename"
    );
    assert_eq!(meta.mode, 0o700, "mode must be preserved across rename");
}

// =====================================================================
// rename_function_folder — guards
// =====================================================================

#[tokio::test]
async fn rename_function_folder_source_missing_errors() {
    let db = setup_db().await;

    let mut b = Batch::new();
    b.id("rff");
    b.rename_function_folder("op", ddl::rename_function_folder(["nope"], ["renamed"]));
    let resp = db.execute("testdb", &b.to_request_via_msgpack()).await;
    assert!(resp.is_err(), "renaming a non-existent folder must error");
    let err = resp.unwrap_err().to_string();
    assert!(
        err.contains("not found"),
        "error should mention 'not found', got: {}",
        err
    );
}

#[tokio::test]
async fn rename_function_folder_destination_occupied_errors() {
    let db = setup_db().await;

    // Create both ["a","b"] and ["a","c"].
    for path in [["a", "b"], ["a", "c"]] {
        let mut b = Batch::new();
        b.id("cff");
        b.create_function_folder("op", ddl::create_function_folder(path));
        db.execute("testdb", &b.to_request_via_msgpack())
            .await
            .unwrap();
    }

    // Rename ["a","b"] → ["a","c"] — destination is taken.
    let mut b = Batch::new();
    b.id("rff");
    b.rename_function_folder("op", ddl::rename_function_folder(["a", "b"], ["a", "c"]));
    let resp = db.execute("testdb", &b.to_request_via_msgpack()).await;
    assert!(
        resp.is_err(),
        "renaming into an occupied destination must error"
    );
    let err = resp.unwrap_err().to_string();
    assert!(
        err.contains("already exists"),
        "error should mention 'already exists', got: {}",
        err
    );
}

#[tokio::test]
async fn rename_function_folder_destination_descendant_occupied_errors() {
    let db = setup_db().await;

    // Create ["a","b","c"] and ["a","x","y"].
    for path in [["a", "b", "c"], ["a", "x", "y"]] {
        let mut b = Batch::new();
        b.id("cff");
        b.create_function_folder("op", ddl::create_function_folder(path));
        db.execute("testdb", &b.to_request_via_msgpack())
            .await
            .unwrap();
    }

    // Rename ["a","b"] → ["a","x"]: destination ["a","x"] already exists.
    let mut b = Batch::new();
    b.id("rff");
    b.rename_function_folder("op", ddl::rename_function_folder(["a", "b"], ["a", "x"]));
    let resp = db.execute("testdb", &b.to_request_via_msgpack()).await;
    assert!(
        resp.is_err(),
        "renaming into an occupied subtree must error"
    );
}
