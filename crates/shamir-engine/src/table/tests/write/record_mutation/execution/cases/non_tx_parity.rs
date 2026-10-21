use shamir_query_builder::write::{self, doc};
use crate::query::filter::eval_context::FilterContext;
use shamir_types::types::common::new_map;
use crate::table::tests::write::record_mutation::execution::fixtures::write_exec_tests::*;

/// INSERT path: non-tx SET (via `set_via_tx`) creates a new record, stores
/// bytes that round-trip correctly, and returns `_created = true` with the
/// expected fields including `_id`.
#[tokio::test]
async fn test_set_nontx_insert_parity() {
    let (table, repo) = setup_empty_table().await;

    let op = write::upsert("users")
        .key(doc().set("email", "parity@test.com"))
        .value(
            doc()
                .set("email", "parity@test.com")
                .set("name", "Parity")
                .set("score", 42_i64),
        )
        .build()
        .unwrap();

    let result = set_via_tx(&repo, &table, &op).await.unwrap();

    assert_eq!(result.affected, 1);
    let rec = &result.records[0];
    assert_eq!(
        rec.get_value_owned("_created"),
        Some(shamir_types::types::value::QueryValue::Bool(true))
    );
    assert_eq!(
        rec.get_value_owned("email"),
        Some(shamir_types::types::value::QueryValue::Str(
            "parity@test.com".to_string()
        ))
    );
    assert_eq!(
        rec.get_value_owned("name"),
        Some(shamir_types::types::value::QueryValue::Str(
            "Parity".to_string()
        ))
    );
    assert_eq!(
        rec.get_value_owned("score"),
        Some(shamir_types::types::value::QueryValue::Int(42))
    );
    assert!(rec.get_value_owned("_id").is_some(), "_id must be present");
    assert_eq!(table.count().await.unwrap(), 1);

    // Read back and verify the stored bytes round-trip to the same values.
    let interner = table.interner().get().await.unwrap();
    let refs = new_map();
    let ctx = FilterContext::new(interner, &refs);
    let query = crate::query::read::ReadQuery::new("users");
    let read = table.read(&query, &ctx).await.unwrap();
    assert_eq!(read.records.len(), 1);
    let stored = &read.records[0];
    assert_eq!(stored.get_value_str("email"), Some("parity@test.com"));
    assert_eq!(stored.get_value_str("name"), Some("Parity"));
    assert_eq!(stored.get_value_i64("score"), Some(42));
}

/// UPDATE path: non-tx SET (via `set_via_tx`) merges into an existing
/// record, preserves untouched fields, and returns `_created = false`.
#[tokio::test]
async fn test_set_nontx_update_parity() {
    let (table, repo) = setup_table_with_users().await;

    // Alice exists with name, age, status. Upsert by name, adding a new field.
    let op = write::upsert("users")
        .key(doc().set("name", "Alice"))
        .value(
            doc()
                .set("name", "Alice")
                .set("status", "vip")
                .set("badge", "gold"),
        )
        .build()
        .unwrap();

    let result = set_via_tx(&repo, &table, &op).await.unwrap();

    assert_eq!(result.affected, 1);
    let rec = &result.records[0];
    assert_eq!(
        rec.get_value_owned("_created"),
        Some(shamir_types::types::value::QueryValue::Bool(false))
    );
    assert_eq!(
        rec.get_value_owned("status"),
        Some(shamir_types::types::value::QueryValue::Str(
            "vip".to_string()
        ))
    );
    assert_eq!(
        rec.get_value_owned("badge"),
        Some(shamir_types::types::value::QueryValue::Str(
            "gold".to_string()
        ))
    );
    // Original field "age" preserved by merge.
    assert_eq!(
        rec.get_value_owned("age"),
        Some(shamir_types::types::value::QueryValue::Int(30))
    );
    // No new record created.
    assert_eq!(table.count().await.unwrap(), 3);

    // Read back: stored bytes match expectations.
    let interner = table.interner().get().await.unwrap();
    let refs = new_map();
    let ctx = FilterContext::new(interner, &refs);
    let query =
        crate::query::read::ReadQuery::new("users").filter(crate::query::filter::Filter::Eq {
            field: vec!["name".to_string()],
            value: crate::query::filter::FilterValue::String("Alice".to_string()),
        });
    let read = table.read(&query, &ctx).await.unwrap();
    assert_eq!(read.records.len(), 1);
    let stored = &read.records[0];
    assert_eq!(stored.get_value_str("status"), Some("vip"));
    assert_eq!(stored.get_value_str("badge"), Some("gold"));
    assert_eq!(stored.get_value_i64("age"), Some(30));
}
