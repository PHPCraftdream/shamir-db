use crate::table::tests::write::record_mutation::execution::fixtures::write_exec_tests::*;
use shamir_query_builder::filter;
use shamir_query_builder::write::{self, doc, UpdateReturnMode};
use shamir_types::types::common::new_map;

// ============================================================================
// DELETE RETURNING (Phase E.5 #245)
// ============================================================================

#[tokio::test]
async fn test_execute_delete_returning_returns_deleted_rows() {
    let (table, repo) = setup_table_with_users().await;
    let refs = new_map();

    // Delete active users (Alice, Bob) with RETURNING all fields.
    let op = write::delete("users")
        .where_(filter::eq("status", "active"))
        .returning()
        .build()
        .unwrap();

    let result = delete_via_tx(&repo, &table, &op, &refs).await.unwrap();
    assert_eq!(result.affected, 2);
    // RETURNING: one record per matched-and-deleted row.
    assert_eq!(result.records.len(), 2);

    // Each returned row should carry the deleted record's fields.
    let names: Vec<String> = result
        .records
        .iter()
        .filter_map(|r| {
            r.get_value_owned("name")
                .and_then(|v| v.as_str().map(String::from))
        })
        .collect();
    assert!(names.contains(&"Alice".to_string()), "names={:?}", names);
    assert!(names.contains(&"Bob".to_string()), "names={:?}", names);

    // Rows are actually gone from the table.
    assert_eq!(table.count().await.unwrap(), 1);
}

#[tokio::test]
async fn test_execute_delete_returning_fields_projects() {
    let (table, repo) = setup_table_with_users().await;
    let refs = new_map();

    // Delete inactive user (Carol) with RETURNING only the "name" field.
    let op = write::delete("users")
        .where_(filter::eq("status", "inactive"))
        .returning_fields(["name"])
        .build()
        .unwrap();

    let result = delete_via_tx(&repo, &table, &op, &refs).await.unwrap();
    assert_eq!(result.affected, 1);
    assert_eq!(result.records.len(), 1);

    let rec = &result.records[0];
    // Projection kept "name".
    assert_eq!(
        rec.get_value_owned("name"),
        Some(shamir_types::types::value::QueryValue::Str("Carol".into()))
    );
    // Projection dropped "age" and "status".
    assert!(rec.get_value_owned("age").is_none());
    assert!(rec.get_value_owned("status").is_none());
}

#[tokio::test]
async fn test_execute_delete_without_returning_has_empty_records() {
    let (table, repo) = setup_table_with_users().await;
    let refs = new_map();

    // Vanilla delete (no .returning) must keep records empty — backward
    // compatibility with every existing caller.
    let op = write::delete("users")
        .where_(filter::eq("status", "active"))
        .build()
        .unwrap();

    let result = delete_via_tx(&repo, &table, &op, &refs).await.unwrap();
    assert_eq!(result.affected, 2);
    assert!(result.records.is_empty());
}

// ============================================================================
// INSERT RETURNING projection (Phase E.5 #245)
// ============================================================================

#[tokio::test]
async fn test_execute_insert_returning_fields_projects() {
    let (table, repo) = setup_empty_table().await;

    let op = write::insert("users")
        .row(
            doc()
                .set("name", "Alice")
                .set("age", 30_i64)
                .set("city", "NYC"),
        )
        .returning_fields(["name", "city"])
        .build();

    let result = insert_via_tx(&repo, &table, &op, true).await.unwrap();
    assert_eq!(result.affected, 1);
    assert_eq!(result.records.len(), 1);

    let rec = &result.records[0];
    // Projection kept the requested fields.
    assert_eq!(
        rec.get_value_owned("name"),
        Some(shamir_types::types::value::QueryValue::Str("Alice".into()))
    );
    assert_eq!(
        rec.get_value_owned("city"),
        Some(shamir_types::types::value::QueryValue::Str("NYC".into()))
    );
    // Projection dropped "age".
    assert!(rec.get_value_owned("age").is_none());
    // _id is still injected (it's separate from the projected fields map).
    assert!(rec.get_value_owned("_id").is_some());
}

#[tokio::test]
async fn test_execute_insert_without_projection_keeps_all_fields() {
    let (table, repo) = setup_empty_table().await;

    let op = write::insert("users")
        .row(doc().set("name", "Alice").set("age", 30_i64))
        .build();

    let result = insert_via_tx(&repo, &table, &op, true).await.unwrap();
    assert_eq!(result.records.len(), 1);
    let rec = &result.records[0];
    assert_eq!(
        rec.get_value_owned("name"),
        Some(shamir_types::types::value::QueryValue::Str("Alice".into()))
    );
    assert_eq!(
        rec.get_value_owned("age"),
        Some(shamir_types::types::value::QueryValue::Int(30))
    );
}

// ============================================================================
// UPDATE RETURNING fields projection (Phase E.5 #245 — closes the
// declaration↔implementation gap on UpdateSelect.fields)
// ============================================================================

#[tokio::test]
async fn test_execute_update_returning_fields_projects() {
    let (table, repo) = setup_table_with_users().await;
    let refs = new_map();

    let op = write::update("users")
        .where_(filter::eq("status", "active"))
        .set(doc().set("status", "premium"))
        .returning_fields(UpdateReturnMode::Changed, ["name", "status"])
        .build()
        .unwrap();

    let result = update_via_tx(&repo, &table, &op, &refs).await.unwrap();
    assert_eq!(result.affected, 2);
    assert_eq!(result.records.len(), 2);
    for rec in &result.records {
        // Projection kept name + status.
        assert!(rec.get_value_owned("name").is_some());
        assert_eq!(
            rec.get_value_owned("status"),
            Some(shamir_types::types::value::QueryValue::Str(
                "premium".into()
            ))
        );
        // Projection dropped "age".
        assert!(rec.get_value_owned("age").is_none());
    }
}
