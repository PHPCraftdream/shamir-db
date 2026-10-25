use crate::query::write::InsertOp;
use crate::table::tests::write::record_mutation::execution::shared::write_exec_tests::*;
use shamir_query_builder::filter;
use shamir_query_builder::write::{self, doc, UpdateReturnMode};
use shamir_types::types::common::new_map;

// ============================================================================
// INSERT
// ============================================================================

#[tokio::test]
async fn test_execute_insert_single() {
    let (table, repo) = setup_empty_table().await;

    let op = write::insert("users")
        .row(
            doc()
                .set("name", "Alice")
                .unwrap()
                .set("age", 30_i64)
                .unwrap(),
        )
        .build();

    let result = insert_via_tx(&repo, &table, &op, true).await.unwrap();

    assert_eq!(result.affected, 1);
    assert_eq!(result.records.len(), 1);
    assert_eq!(
        result.records[0].get_value_owned("name"),
        Some(shamir_types::types::value::QueryValue::Str(
            "Alice".to_string()
        ))
    );
    assert_eq!(
        result.records[0].get_value_owned("age"),
        Some(shamir_types::types::value::QueryValue::Int(30))
    );
    assert!(result.records[0].get_value_owned("_id").is_some());

    // Verify record count
    assert_eq!(table.count().await.unwrap(), 1);
}

#[tokio::test]
async fn test_execute_insert_multiple() {
    let (table, repo) = setup_empty_table().await;

    let op = write::insert("users")
        .row(
            doc()
                .set("name", "Alice")
                .unwrap()
                .set("age", 30_i64)
                .unwrap(),
        )
        .row(
            doc()
                .set("name", "Bob")
                .unwrap()
                .set("age", 25_i64)
                .unwrap(),
        )
        .row(
            doc()
                .set("name", "Carol")
                .unwrap()
                .set("age", 35_i64)
                .unwrap(),
        )
        .build();

    let result = insert_via_tx(&repo, &table, &op, true).await.unwrap();

    assert_eq!(result.affected, 3);
    assert_eq!(result.records.len(), 3);
    assert_eq!(table.count().await.unwrap(), 3);
}

#[tokio::test]
async fn test_execute_insert_empty() {
    let (table, repo) = setup_empty_table().await;

    let op: InsertOp = write::insert("users").build();

    let result = insert_via_tx(&repo, &table, &op, true).await.unwrap();

    assert_eq!(result.affected, 0);
    assert_eq!(table.count().await.unwrap(), 0);
}

// ============================================================================
// UPDATE
// ============================================================================

#[tokio::test]
async fn test_execute_update_with_filter() {
    let (table, repo) = setup_table_with_users().await;
    let refs = new_map();

    // Update active users: set status = "premium"
    let op = write::update("users")
        .where_(filter::eq("status", "active"))
        .set(doc().set("status", "premium").unwrap())
        .build()
        .unwrap();

    let result = update_via_tx(&repo, &table, &op, &refs).await.unwrap();

    // Alice and Bob are active
    assert_eq!(result.affected, 2);
    assert!(result.records.is_empty()); // no select requested

    // Verify: read all and check statuses
    assert_eq!(table.count().await.unwrap(), 3);
}

#[tokio::test]
async fn test_execute_update_returns_changed() {
    let (table, repo) = setup_table_with_users().await;
    let refs = new_map();

    let op = write::update("users")
        .where_(filter::eq("status", "active"))
        .set(doc().set("status", "premium").unwrap())
        .returning(UpdateReturnMode::Changed)
        .build()
        .unwrap();

    let result = update_via_tx(&repo, &table, &op, &refs).await.unwrap();

    assert_eq!(result.affected, 2);
    assert_eq!(result.records.len(), 2);
    // All returned records should have status = "premium"
    for record in &result.records {
        assert_eq!(
            record.get_value_owned("status"),
            Some(shamir_types::types::value::QueryValue::Str(
                "premium".to_string()
            ))
        );
    }
}

#[tokio::test]
async fn test_execute_update_no_match() {
    let (table, repo) = setup_table_with_users().await;
    let refs = new_map();

    let op = write::update("users")
        .where_(filter::eq("status", "deleted"))
        .set(doc().set("status", "active").unwrap())
        .build()
        .unwrap();

    let result = update_via_tx(&repo, &table, &op, &refs).await.unwrap();
    assert_eq!(result.affected, 0);
}

#[tokio::test]
async fn test_execute_update_all_records() {
    let (table, repo) = setup_table_with_users().await;
    let refs = new_map();

    // No where clause — update all
    let op = write::update("users")
        .set(doc().set("verified", true).unwrap())
        .returning(UpdateReturnMode::All)
        .build()
        .unwrap();

    let result = update_via_tx(&repo, &table, &op, &refs).await.unwrap();
    assert_eq!(result.affected, 3);
    assert_eq!(result.records.len(), 3);
    for record in &result.records {
        assert_eq!(
            record.get_value_owned("verified"),
            Some(shamir_types::types::value::QueryValue::Bool(true))
        );
    }
}

#[tokio::test]
async fn test_execute_update_unchanged_mode() {
    let (table, repo) = setup_table_with_users().await;
    let refs = new_map();

    // Set status = "active" on active users — no actual change
    let op = write::update("users")
        .where_(filter::eq("status", "active"))
        .set(doc().set("status", "active").unwrap())
        .returning(UpdateReturnMode::Unchanged)
        .build()
        .unwrap();

    let result = update_via_tx(&repo, &table, &op, &refs).await.unwrap();
    // Nothing actually changed
    assert_eq!(result.affected, 0);
    // But Unchanged mode returns records that matched but didn't change
    assert_eq!(result.records.len(), 2);
}

// ============================================================================
// DELETE
// ============================================================================

#[tokio::test]
async fn test_execute_delete_with_filter() {
    let (table, repo) = setup_table_with_users().await;
    let refs = new_map();

    let op = write::delete("users")
        .where_(filter::eq("status", "inactive"))
        .build()
        .unwrap();

    let result = delete_via_tx(&repo, &table, &op, &refs).await.unwrap();

    // Carol is inactive
    assert_eq!(result.affected, 1);
    assert_eq!(table.count().await.unwrap(), 2);
}

#[tokio::test]
async fn test_execute_delete_no_match() {
    let (table, repo) = setup_table_with_users().await;
    let refs = new_map();

    let op = write::delete("users")
        .where_(filter::eq("status", "deleted"))
        .build()
        .unwrap();

    let result = delete_via_tx(&repo, &table, &op, &refs).await.unwrap();
    assert_eq!(result.affected, 0);
    assert_eq!(table.count().await.unwrap(), 3);
}

#[tokio::test]
async fn test_execute_delete_multiple() {
    let (table, repo) = setup_table_with_users().await;
    let refs = new_map();

    // Delete all active users (Alice, Bob)
    let op = write::delete("users")
        .where_(filter::eq("status", "active"))
        .build()
        .unwrap();

    let result = delete_via_tx(&repo, &table, &op, &refs).await.unwrap();
    assert_eq!(result.affected, 2);
    assert_eq!(table.count().await.unwrap(), 1);
}
