//! Owner-on-create tests: user-initiated creates stamp the acting actor;
//! system-initiated creates keep System.

use shamir_db::engine::repo::repo_types::BoxRepoFactory;
use shamir_db::engine::repo::RepoConfig;
use shamir_db::ShamirDb;
use shamir_query_builder::batch::Batch;
use shamir_query_builder::ddl;
use shamir_types::access::{Actor, ResourcePath};

use super::helpers::*;

#[tokio::test]
async fn owner_on_create_db_user_actor() {
    let shamir = ShamirDb::init_memory().await.unwrap();
    let user_actor = Actor::User(42);

    // Bootstrap a db to dispatch admin ops through.
    shamir.create_db("bootstrap").await;

    // Create a NEW database via execute_as with a user actor.
    let mut b = Batch::new();
    b.id(1);
    b.create_db("op", ddl::create_db("owned_db"));
    let req = b.to_request_via_msgpack();
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
    let mut b = Batch::new();
    b.id(1);
    b.create_repo("op", ddl::create_repo("user_repo").engine("in_memory"));
    let req = b.to_request_via_msgpack();
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
    let mut b = Batch::new();
    b.id(1);
    b.create_table("op", ddl::create_table("owned_table").repo("main"));
    let req = b.to_request_via_msgpack();
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
    let mut b = Batch::new();
    b.id(1);
    b.create_function("op", ddl::create_function("user_fn").wasm(wasm_b64(&wasm)));
    let req = b.to_request_via_msgpack();
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
    let mut b = Batch::new();
    b.id(1);
    b.create_function("op", ddl::create_function("sys_fn").wasm(wasm_b64(&wasm)));
    let req = b.to_request_via_msgpack();
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
