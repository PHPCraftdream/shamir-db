use crate::table::tests::write::record_mutation::execution::shared::write_exec_tests::*;
use shamir_query_builder::write::{self, doc};

// ============================================================================
// SET (upsert)
// ============================================================================

#[tokio::test]
async fn test_execute_set_insert_new() {
    let (table, repo) = setup_empty_table().await;

    let op = write::upsert("users")
        .key(doc().set("email", "alice@example.com").unwrap())
        .value(
            doc()
                .set("email", "alice@example.com")
                .unwrap()
                .set("name", "Alice")
                .unwrap(),
        )
        .build()
        .unwrap();

    let result = set_via_tx(&repo, &table, &op).await.unwrap();

    assert_eq!(result.affected, 1);
    assert_eq!(
        result.records[0].get_value_owned("_created"),
        Some(shamir_types::types::value::QueryValue::Bool(true))
    );
    assert_eq!(
        result.records[0].get_value_owned("name"),
        Some(shamir_types::types::value::QueryValue::Str(
            "Alice".to_string()
        ))
    );
    assert_eq!(table.count().await.unwrap(), 1);
}

#[tokio::test]
async fn test_execute_set_update_existing() {
    let (table, repo) = setup_table_with_users().await;
    let _interner = table.interner().get().await.unwrap();

    // Alice exists with status=active. Upsert by name.
    let op = write::upsert("users")
        .key(doc().set("name", "Alice").unwrap())
        .value(
            doc()
                .set("name", "Alice")
                .unwrap()
                .set("status", "vip")
                .unwrap()
                .set("score", 100_i64)
                .unwrap(),
        )
        .build()
        .unwrap();

    let result = set_via_tx(&repo, &table, &op).await.unwrap();

    assert_eq!(result.affected, 1);
    assert_eq!(
        result.records[0].get_value_owned("_created"),
        Some(shamir_types::types::value::QueryValue::Bool(false))
    );
    assert_eq!(
        result.records[0].get_value_owned("status"),
        Some(shamir_types::types::value::QueryValue::Str(
            "vip".to_string()
        ))
    );
    // Original field "age" should be preserved (merge)
    assert_eq!(
        result.records[0].get_value_owned("age"),
        Some(shamir_types::types::value::QueryValue::Int(30))
    );
    assert_eq!(table.count().await.unwrap(), 3); // no new record
}

#[tokio::test]
async fn test_execute_set_no_match_inserts() {
    let (table, repo) = setup_table_with_users().await;

    let op = write::upsert("users")
        .key(doc().set("name", "Zara").unwrap())
        .value(
            doc()
                .set("name", "Zara")
                .unwrap()
                .set("age", 22_i64)
                .unwrap(),
        )
        .build()
        .unwrap();

    let result = set_via_tx(&repo, &table, &op).await.unwrap();

    assert_eq!(
        result.records[0].get_value_owned("_created"),
        Some(shamir_types::types::value::QueryValue::Bool(true))
    );
    assert_eq!(table.count().await.unwrap(), 4); // new record added
}

// ============================================================================
// Interner persistence after writes
// ============================================================================
