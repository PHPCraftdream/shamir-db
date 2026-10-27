use shamir_query_builder::batch::Batch;
use shamir_query_builder::ddl;

use crate::engine::repo::RepoConfig;
use crate::engine::table::TableConfig;
use crate::ShamirDb;

#[tokio::test]
async fn rv_db_9257b8e1_removing_missing_rule_keeps_schema_version() {
    let shamir = ShamirDb::init_memory().await.unwrap();
    shamir.create_db("testdb").await.unwrap();
    shamir
        .add_repo(
            "testdb",
            RepoConfig::new("main", crate::engine::repo::BoxRepoFactory::in_memory())
                .add_table(TableConfig::new("items")),
        )
        .await
        .unwrap();
    let mut persisted = shamir_types::types::common::new_map();
    persisted.insert(
        "db_name".into(),
        crate::types::value::QueryValue::Str("testdb".into()),
    );
    persisted.insert(
        "repo_name".into(),
        crate::types::value::QueryValue::Str("main".into()),
    );
    persisted.insert(
        "table_name".into(),
        crate::types::value::QueryValue::Str("items".into()),
    );
    persisted.insert(
        "enable_indexes".into(),
        crate::types::value::QueryValue::Bool(false),
    );
    persisted.insert(
        "schema_version".into(),
        crate::types::value::QueryValue::Int(0),
    );
    let persisted_value = crate::types::value::QueryValue::Map(persisted);
    shamir
        .system_store()
        .save_table_meta(&persisted_value)
        .await
        .unwrap();
    let initial = shamir
        .system_store()
        .load_table_record("testdb", "main", "items")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        initial
            .get("schema_version")
            .and_then(|value| value.as_i64()),
        Some(0)
    );

    let mut batch = Batch::new();
    batch.id("remove");
    batch.remove_schema_rule(
        "remove",
        ddl::remove_schema_rule("items", ["does_not_exist"]).repo("main"),
    );
    let request = batch.to_request_via_msgpack();
    let response = shamir.execute("testdb", &request).await.unwrap();
    let result = &response.results["remove"];
    let record = &result.records[0];
    let crate::query::read::QueryRecord::Direct(record) = record else {
        panic!("DDL result should be a direct record");
    };
    assert_eq!(
        record.get("removed").and_then(|value| value.as_bool()),
        Some(false)
    );
    assert_eq!(
        record
            .get("schema_version")
            .and_then(|value| value.as_i64()),
        Some(0)
    );

    let schema = shamir
        .system_store()
        .load_table_record("testdb", "main", "items")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        schema
            .get("schema_version")
            .and_then(|value| value.as_i64()),
        Some(0)
    );
}
