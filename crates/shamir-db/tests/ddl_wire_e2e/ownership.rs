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

    // Bootstrap a db to dispatch admin ops through. G.4c: create_db defaults to
    // enforced (0o700, System), so open the bootstrap db so the user actor can
    // traverse it to reach the Create path. The bootstrap db is setup-only;
    // the SUBJECT is the user-created "owned_db".
    shamir.create_db("bootstrap").await;
    shamir
        .set_resource_meta(
            &ResourcePath::database("bootstrap"),
            &shamir_types::access::ResourceMeta::open(),
        )
        .await
        .unwrap();

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
        .await
        .unwrap();
    assert_eq!(
        meta.owner,
        Actor::User(42),
        "db owner should be the user actor"
    );
    // G.4c: new objects default to enforced owner-rwx (0o700).
    assert_eq!(meta.mode, 0o700, "mode must be enforced (0o700)");
    assert!(meta.group.is_none(), "group must stay None");
}

#[tokio::test]
async fn owner_on_create_db_system_actor() {
    let shamir = ShamirDb::init_memory().await.unwrap();

    // System-initiated create → owner stays System.
    shamir.create_db("sys_db").await;

    let meta = shamir
        .resource_meta(&ResourcePath::database("sys_db"))
        .await
        .unwrap();
    assert_eq!(
        meta.owner,
        Actor::System,
        "system db owner should be System"
    );
    // G.4c: new objects default to enforced owner-rwx (0o700).
    assert_eq!(meta.mode, 0o700);
}

#[tokio::test]
async fn owner_on_create_repo_user_actor() {
    let shamir = ShamirDb::init_memory().await.unwrap();
    let user_actor = Actor::User(99);

    // Create db first (as system — we only care about the repo). G.4c: open
    // the db so the user actor can traverse it to reach the Create path.
    shamir.create_db("testdb").await;
    shamir
        .set_resource_meta(
            &ResourcePath::database("testdb"),
            &shamir_types::access::ResourceMeta::open(),
        )
        .await
        .unwrap();

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
        .await
        .unwrap();
    assert_eq!(
        meta.owner,
        Actor::User(99),
        "repo owner should be the user actor"
    );
    // G.4c: new objects default to enforced owner-rwx (0o700).
    assert_eq!(meta.mode, 0o700);
}

#[tokio::test]
async fn owner_on_create_table_user_actor() {
    let shamir = ShamirDb::init_memory().await.unwrap();
    let user_actor = Actor::User(7);

    shamir.create_db("testdb").await;
    let repo_config = RepoConfig::new("main", BoxRepoFactory::in_memory());
    shamir.add_repo("testdb", repo_config).await.unwrap();
    // G.4c: open db + repo ancestors so the user actor can traverse them to
    // reach the Create path. The SUBJECT is the user-created table.
    let open = shamir_types::access::ResourceMeta::open();
    shamir
        .set_resource_meta(&ResourcePath::database("testdb"), &open)
        .await
        .unwrap();
    shamir
        .set_resource_meta(&ResourcePath::store("testdb", "main"), &open)
        .await
        .unwrap();

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
        .await
        .unwrap();
    assert_eq!(
        meta.owner,
        Actor::User(7),
        "table owner should be the user actor"
    );
    // G.4c: new objects default to enforced owner-rwx (0o700).
    assert_eq!(meta.mode, 0o700);
}

#[tokio::test]
async fn owner_on_create_function_user_actor() {
    let shamir = ShamirDb::init_memory().await.unwrap();
    let user_actor = Actor::User(13);

    shamir.create_db("testdb").await;
    let repo_config = RepoConfig::new("main", BoxRepoFactory::in_memory());
    shamir.add_repo("testdb", repo_config).await.unwrap();
    // G.4c: open db + repo ancestors so the user actor can traverse them.
    let open = shamir_types::access::ResourceMeta::open();
    shamir
        .set_resource_meta(&ResourcePath::database("testdb"), &open)
        .await
        .unwrap();
    shamir
        .set_resource_meta(&ResourcePath::store("testdb", "main"), &open)
        .await
        .unwrap();

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
        .await
        .unwrap();
    assert_eq!(
        meta.owner,
        Actor::User(13),
        "function owner should be the user actor"
    );
    // G.4c: new objects default to enforced owner-rwx (0o700).
    assert_eq!(meta.mode, 0o700);
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
        .await
        .unwrap();
    assert_eq!(
        meta.owner,
        Actor::System,
        "system-created function owner should be System"
    );
    // G.4c: new objects default to enforced owner-rwx (0o700).
    assert_eq!(meta.mode, 0o700);
}
