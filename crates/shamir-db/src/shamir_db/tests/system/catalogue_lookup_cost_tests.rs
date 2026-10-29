use crate::access::{Actor, ResourceMeta, ResourcePath};
use crate::engine::function::Params;
use crate::engine::repo::{BoxRepoFactory, RepoConfig};
use crate::engine::table::TableConfig;
use crate::query::admin::GroupRef;
use crate::shamir_db::shamir_db::ShamirDb;
use crate::shamir_db::system_store::{SystemStore, SystemStoreConfig};
use shamir_query_builder::batch::Batch;
use shamir_query_builder::Query;

#[tokio::test]
async fn invocation_reads_one_function_and_one_settings_snapshot() {
    let db = ShamirDb::init_memory().await.unwrap();
    db.register_fn("sample", false, |_, _, _| async {
        Ok(shamir_types::types::value::QueryValue::Null)
    })
    .unwrap();
    let store = db.system_store();
    let batch = db.new_batch_context();
    for variant in 0..4 {
        store.reset_function_catalogue_reads();
        store.reset_invocation_settings_reads();
        match variant {
            0 => {
                db.invoke_function_as("sample", Params::new(), Actor::User(7))
                    .await
            }
            1 => {
                db.invoke_function_with_batch_as("sample", Params::new(), &batch, Actor::User(7))
                    .await
            }
            2 => {
                db.invoke_function_in_db_as("db", "main", "sample", Params::new(), Actor::User(7))
                    .await
            }
            _ => {
                db.invoke_function_in_db_with_batch_as(
                    "db",
                    "main",
                    "sample",
                    Params::new(),
                    &batch,
                    Actor::User(7),
                )
                .await
            }
        }
        .unwrap();
        assert_eq!(store.function_catalogue_read_count(), 1);
        assert_eq!(store.invocation_settings_read_count(), 1);
    }
    db.set_resource_meta(
        &ResourcePath::FunctionNamespace,
        &ResourceMeta {
            owner: Actor::System,
            group: None,
            mode: 0o700,
        },
    )
    .await
    .unwrap();
    assert!(db
        .invoke_function_as("sample", Params::new(), Actor::User(7))
        .await
        .is_err());
}

#[tokio::test]
async fn rv_db_5977a782_group_name_resolution_is_keyed() {
    let shamir = ShamirDb::init_memory().await.expect("db");
    shamir
        .system_store()
        .save_group(17, "ops", &[], 0)
        .await
        .expect("save group");
    let store = shamir.system_store();
    store.reset_catalogue_counts();
    store
        .save_group(17, "ops", &[], 0)
        .await
        .expect("same-name save");
    assert_eq!(store.full_catalogue_load_count(), 0);
    store.reset_catalogue_counts();
    assert_eq!(
        shamir
            .resolve_group_id(&GroupRef::Name { name: "ops".into() })
            .await
            .expect("id"),
        17
    );
    assert_eq!(store.catalogue_access_count(), 0);
    shamir
        .system_store()
        .save_group(17, "operations", &[], 0)
        .await
        .expect("rename");
    assert!(shamir
        .resolve_group_id(&GroupRef::Name { name: "ops".into() })
        .await
        .is_err());
    assert_eq!(
        shamir
            .resolve_group_id(&GroupRef::Name {
                name: "operations".into()
            })
            .await
            .expect("renamed id"),
        17
    );
    shamir.system_store().remove_group(17).await.expect("drop");
    assert!(shamir
        .resolve_group_id(&GroupRef::Name {
            name: "operations".into()
        })
        .await
        .is_err());
}

#[tokio::test]
async fn rv_db_c58706f9_chmod_between_requests_is_not_memoized() {
    let db = ShamirDb::init_memory().await.expect("db");
    db.create_db("authdb").await.expect("database");
    db.set_resource_meta(&ResourcePath::database("authdb"), &ResourceMeta::open())
        .await
        .expect("open database");
    db.add_repo(
        "authdb",
        RepoConfig::new("main", BoxRepoFactory::in_memory()).add_table(TableConfig::new("items")),
    )
    .await
    .expect("repository");
    db.set_resource_meta(
        &ResourcePath::store("authdb", "main"),
        &ResourceMeta::open(),
    )
    .await
    .expect("open repository");
    let path = ResourcePath::table("authdb", "main", "items");
    db.set_resource_meta(
        &path,
        &ResourceMeta {
            owner: Actor::System,
            group: None,
            mode: 0o755,
        },
    )
    .await
    .expect("open table");
    let mut batch = Batch::new();
    batch.id(1);
    batch.query("first", Query::with_repo("main", "items"));
    let request = batch.to_request_via_msgpack().expect("request");
    assert!(db
        .execute_as(Actor::User(7), "authdb", &request)
        .await
        .is_ok());
    db.set_resource_meta(
        &path,
        &ResourceMeta {
            owner: Actor::System,
            group: None,
            mode: 0o700,
        },
    )
    .await
    .expect("restrict table");
    let error = db
        .execute_as(Actor::User(7), "authdb", &request)
        .await
        .unwrap_err();
    assert!(error.to_string().contains("items"));
    assert!(error.to_string().to_ascii_lowercase().contains("denied"));
}

#[tokio::test]
async fn rv_db_c58706f9_batch_denial_names_table() {
    let db = ShamirDb::init_memory().await.expect("db");
    db.create_db("authdb").await.expect("database");
    db.set_resource_meta(&ResourcePath::database("authdb"), &ResourceMeta::open())
        .await
        .expect("open database");
    db.add_repo(
        "authdb",
        RepoConfig::new("main", BoxRepoFactory::in_memory())
            .add_table(TableConfig::new("allowed"))
            .add_table(TableConfig::new("private")),
    )
    .await
    .expect("repository");
    db.set_resource_meta(
        &ResourcePath::store("authdb", "main"),
        &ResourceMeta::open(),
    )
    .await
    .expect("open repository");
    for (name, mode) in [("allowed", 0o755), ("private", 0o700)] {
        db.set_resource_meta(
            &ResourcePath::table("authdb", "main", name),
            &ResourceMeta {
                owner: Actor::System,
                group: None,
                mode,
            },
        )
        .await
        .expect("table mode");
    }
    let mut batch = Batch::new();
    batch.id(1);
    batch.query("public-read", Query::with_repo("main", "allowed"));
    batch.query("private-read", Query::with_repo("main", "private"));
    let request = batch.to_request_via_msgpack().expect("request");
    let error = db
        .execute_as(Actor::User(7), "authdb", &request)
        .await
        .unwrap_err();
    assert!(error.to_string().contains("private"));
    assert!(error.to_string().to_ascii_lowercase().contains("denied"));
}

#[tokio::test]
async fn rv_db_23a03103_load_repository_record_is_not_a_full_load() {
    let store = SystemStore::init(SystemStoreConfig::InMemory)
        .await
        .expect("store");
    store.reset_catalogue_counts();
    let result = store
        .load_repository_record("db", "repo")
        .await
        .expect("lookup");
    assert!(result.is_none());
    assert_eq!(store.full_catalogue_load_count(), 0);
}

#[tokio::test]
async fn rv_db_c58706f9_batch_auth_reads_shared_ancestors_once() {
    let db = ShamirDb::init_memory().await.expect("db");
    db.create_db("authdb").await.expect("database");
    db.set_resource_meta(&ResourcePath::database("authdb"), &ResourceMeta::open())
        .await
        .expect("open database");
    let config = RepoConfig::new("main", BoxRepoFactory::in_memory())
        .add_table(TableConfig::new("t0"))
        .add_table(TableConfig::new("t1"))
        .add_table(TableConfig::new("t2"))
        .add_table(TableConfig::new("t3"))
        .add_table(TableConfig::new("t4"))
        .add_table(TableConfig::new("t5"))
        .add_table(TableConfig::new("t6"))
        .add_table(TableConfig::new("t7"));
    db.add_repo("authdb", config).await.expect("repository");
    db.set_resource_meta(
        &ResourcePath::store("authdb", "main"),
        &ResourceMeta::open(),
    )
    .await
    .expect("open repository");
    for i in 0..8 {
        db.set_resource_meta(
            &ResourcePath::table("authdb", "main", format!("t{i}")),
            &ResourceMeta {
                owner: Actor::System,
                group: None,
                mode: 0o755,
            },
        )
        .await
        .expect("table mode");
    }
    let store = db.system_store();
    store.reset_catalogue_counts();
    let mut batch = Batch::new();
    batch.id(1);
    for i in 0..8 {
        batch.query(format!("q{i}"), Query::with_repo("main", format!("t{i}")));
    }
    let request = batch.to_request_via_msgpack().expect("build batch");
    let result = db.execute_as(Actor::User(7), "authdb", &request).await;
    assert!(result.is_ok());
    assert_eq!(store.catalogue_access_count(), 11);
}
