use crate::access::ResourceMeta;
use crate::engine::repo::{BoxRepoFactory, RepoConfig};
use crate::engine::table::TableConfig;
use crate::query::batch::BatchRequest;
use crate::shamir_db::ShamirDb;
use crate::types::value::QueryValue;
use shamir_query_builder::batch::Batch;
use shamir_query_builder::ddl;

async fn fixture() -> ShamirDb {
    ShamirDb::init_memory().await.unwrap()
}

async fn schema_db() -> ShamirDb {
    let db = fixture().await;
    db.create_db("testdb").await.unwrap();
    db.add_repo(
        "testdb",
        RepoConfig::new("main", BoxRepoFactory::in_memory()).add_table(TableConfig::new("users")),
    )
    .await
    .unwrap();
    db
}

fn schema_request(kind: &str) -> BatchRequest {
    let mut batch = Batch::new();
    batch.id(1);
    match kind {
        "set" => batch.set_table_schema(
            "schema",
            ddl::set_table_schema("users").rules(vec![ddl::field(["email"]).string().build()]),
        ),
        "add" => batch.add_schema_rule(
            "schema",
            ddl::add_schema_rule("users")
                .rule(ddl::field(["email"]).string().build())
                .build()
                .unwrap(),
        ),
        _ => batch.remove_schema_rule("schema", ddl::remove_schema_rule("users", ["email"])),
    };
    batch.to_request_via_msgpack().expect("build request")
}

#[tokio::test]
async fn schema_handlers_take_namespace_before_repository_admission() {
    let db = schema_db().await;
    let repository = db.get_db("testdb").unwrap().get_repo("main").unwrap();
    for kind in ["set", "add", "remove"] {
        let namespace_guard = db.lock_replication_control().await;
        let request = schema_request(kind);
        let mut operation = Box::pin(db.execute("testdb", &request));
        assert!(futures::poll!(&mut operation).is_pending());
        let mut admission = Box::pin(repository.write_domain().admit());
        assert!(futures::poll!(&mut admission).is_ready());
        drop(admission);
        drop(operation);
        drop(namespace_guard);
        let response = db.execute("testdb", &request).await.unwrap();
        assert!(response.results.contains_key("schema"));
    }
}

#[tokio::test]
async fn table_resource_meta_update_waits_for_namespace_before_catalogue_rmw() {
    let db = fixture().await;
    db.create_db("testdb").await.unwrap();
    db.add_repo(
        "testdb",
        RepoConfig::new("main", BoxRepoFactory::in_memory()).add_table(TableConfig::new("users")),
    )
    .await
    .unwrap();
    let path = crate::access::ResourcePath::table("testdb", "main", "users");
    let namespace_guard = db.lock_replication_control().await;
    let meta = ResourceMeta::open();
    let mut update = Box::pin(db.set_resource_meta(&path, &meta));
    assert!(futures::poll!(&mut update).is_pending());
    drop(namespace_guard);
    update.await.unwrap();
    assert_eq!(db.resource_meta(&path).await.unwrap(), ResourceMeta::open());
}

#[tokio::test]
async fn concurrent_create_table_serializes_duplicate_and_if_not_exists() {
    let db = schema_db().await;
    let create_request = |if_not_exists: bool| {
        let mut batch = Batch::new();
        batch.id(1);
        let name = if if_not_exists { "dupe_ignore" } else { "dupe" };
        let op = if if_not_exists {
            ddl::create_table(name).repo("main").if_not_exists().build()
        } else {
            ddl::create_table(name).repo("main").build()
        };
        batch.create_table("create", op);
        batch.to_request_via_msgpack().expect("build request")
    };
    let request = create_request(false);
    let namespace_guard = db.lock_replication_control().await;
    let mut create = Box::pin(db.execute("testdb", &request));
    assert!(futures::poll!(&mut create).is_pending());
    let table_created = db
        .add_table_as_locked(
            "testdb",
            "main",
            "dupe",
            false,
            crate::access::Actor::System,
            &namespace_guard,
        )
        .await;
    assert!(table_created.is_ok());
    drop(namespace_guard);
    assert!(create.await.is_err());
    let duplicate = db.execute("testdb", &request).await;
    assert!(duplicate.is_err());

    let request = create_request(true);
    let namespace_guard = db.lock_replication_control().await;
    let mut create = Box::pin(db.execute("testdb", &request));
    assert!(futures::poll!(&mut create).is_pending());
    db.add_table_as_locked(
        "testdb",
        "main",
        "dupe_ignore",
        false,
        crate::access::Actor::System,
        &namespace_guard,
    )
    .await
    .unwrap();
    drop(namespace_guard);
    let ignored = create.await.unwrap();
    assert_eq!(
        ignored.results["create"].records[0].get_value_owned("created"),
        Some(QueryValue::Bool(false))
    );
    let already_exists = db.execute("testdb", &request).await.unwrap();
    assert_eq!(
        already_exists.results["create"].records[0].get_value_owned("created"),
        Some(QueryValue::Bool(false))
    );
}

#[tokio::test]
async fn rv_db_30ac7f3a_rename_fails_closed_on_malformed_sibling_schema() {
    let db = fixture().await;
    db.create_db("testdb").await.unwrap();
    db.add_repo(
        "testdb",
        RepoConfig::new("repo", BoxRepoFactory::in_memory())
            .add_table(TableConfig::new("parent"))
            .add_table(TableConfig::new("child")),
    )
    .await
    .unwrap();

    let mut child = db
        .system_store()
        .load_table_record("testdb", "repo", "child")
        .await
        .unwrap()
        .unwrap();
    let QueryValue::Map(fields) = &mut child else {
        panic!("table catalogue record must be a map");
    };
    fields.insert(
        "schema".to_string(),
        QueryValue::List(vec![QueryValue::Int(1)]),
    );
    db.system_store().save_table_meta(&child).await.unwrap();

    let renamed = db.rename_table("testdb", "repo", "parent", "renamed").await;
    assert!(matches!(renamed, Err(crate::DbError::Validation(_))));
    let instance = db.get_db("testdb").unwrap();
    assert!(instance.has_table("repo", "parent"));
    assert!(!instance.has_table("repo", "renamed"));
}

#[tokio::test]
async fn rv_db_72e8717c_rename_reads_table_catalogue_once_for_many_siblings() {
    let db = fixture().await;
    db.create_db("testdb").await.unwrap();
    let mut config = RepoConfig::new("repo", BoxRepoFactory::in_memory());
    for index in 0..8 {
        config = config.add_table(TableConfig::new(format!("sibling-{index}")));
    }
    config = config.add_table(TableConfig::new("target"));
    db.add_repo("testdb", config).await.unwrap();

    let store = db.system_store();
    store
        .load_table_record("testdb", "repo", "target")
        .await
        .unwrap()
        .unwrap();
    store.reset_table_catalogue_reads();
    db.rename_table("testdb", "repo", "target", "renamed")
        .await
        .unwrap();
    assert_eq!(store.table_catalogue_read_count(), 2);
}

#[tokio::test]
async fn rv_db_370d8ac1_table_facades_wait_for_namespace_guard() {
    let db = fixture().await;
    let guard = db.lock_replication_control().await;

    let mut add = Box::pin(db.add_table("missing-db", "repo", "table", false));
    assert!(futures::poll!(&mut add).is_pending());
    drop(guard);
    assert!(matches!(add.await, Err(crate::DbError::NotFound(_))));

    let guard = db.lock_replication_control().await;
    let mut drop_table = Box::pin(db.drop_table("missing-db", "repo", "table"));
    assert!(futures::poll!(&mut drop_table).is_pending());
    drop(guard);
    assert!(matches!(drop_table.await, Err(crate::DbError::NotFound(_))));

    let guard = db.lock_replication_control().await;
    let mut rename = Box::pin(db.rename_table("missing-db", "repo", "old", "new"));
    assert!(futures::poll!(&mut rename).is_pending());
    drop(guard);
    assert!(matches!(rename.await, Err(crate::DbError::NotFound(_))));
}

#[tokio::test]
async fn rv_db_c57ea68c_repo_creation_waits_for_namespace_guard() {
    let db = fixture().await;
    let config = RepoConfig::new("repo", BoxRepoFactory::in_memory());
    let guard = db.lock_replication_control().await;
    let mut create = Box::pin(db.add_repo_as_with_durability(
        "missing-db",
        config,
        crate::access::Actor::System,
        None,
    ));
    assert!(futures::poll!(&mut create).is_pending());
    drop(guard);
    assert!(matches!(create.await, Err(crate::DbError::NotFound(_))));
}
