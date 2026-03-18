//! Integration tests for write operation execution.

#![allow(deprecated)]

use serde_json::json;

use crate::codecs::transform;
use crate::db::engine::db_instance::db_instance::DbInstance;
use crate::db::engine::repo::repo_types::BoxRepoFactory;
use crate::db::engine::repo::RepoConfig;
use crate::db::engine::table::TableConfig;
use crate::db::query::filter::eval_context::FilterContext;
use crate::db::query::write::{DeleteOp, InsertOp, SetOp, UpdateOp};
use crate::types::common::new_map;
use crate::types::value::UserValue;

/// Create a DbInstance with one "users" table, return the table manager.
async fn setup_empty_table() -> crate::db::engine::table::TableManager {
    let repo_config = RepoConfig {
        name: "default".to_string(),
        factory: BoxRepoFactory::in_memory(),
        tables: vec![TableConfig::new("users")],
    };
    let db = DbInstance::with_repos(vec![repo_config]).await.unwrap();
    db.get_table("default", "users").await.unwrap()
}

/// Setup table with pre-inserted users.
async fn setup_table_with_users() -> crate::db::engine::table::TableManager {
    let table = setup_empty_table().await;

    let users = vec![
        vec![
            ("name", UserValue::Str("Alice".into())),
            ("age", UserValue::Int(30)),
            ("status", UserValue::Str("active".into())),
        ],
        vec![
            ("name", UserValue::Str("Bob".into())),
            ("age", UserValue::Int(25)),
            ("status", UserValue::Str("active".into())),
        ],
        vec![
            ("name", UserValue::Str("Carol".into())),
            ("age", UserValue::Int(35)),
            ("status", UserValue::Str("inactive".into())),
        ],
    ];

    let interner = table.interner().get().await.unwrap();
    for fields in &users {
        let mut map = new_map();
        for (k, v) in fields {
            map.insert(k.to_string(), v.clone());
        }
        let user_val = UserValue::Map(map);
        let result = transform::user_to_inner(&user_val, interner);
        if let Some(ref new_keys) = result.new_keys {
            table.interner().save_new_keys(new_keys).await.unwrap();
        }
        table.insert(&result.inner_value).await.unwrap();
    }

    table
}

// ============================================================================
// INSERT
// ============================================================================

#[tokio::test]
async fn test_execute_insert_single() {
    let table = setup_empty_table().await;

    let op: InsertOp = serde_json::from_value(json!({
        "insert_into": "users",
        "values": [{"name": "Alice", "age": 30}]
    })).unwrap();

    let result = table.execute_insert(&op).await.unwrap();

    assert_eq!(result.affected, 1);
    assert_eq!(result.records.len(), 1);
    assert_eq!(result.records[0]["name"], "Alice");
    assert_eq!(result.records[0]["age"], 30);
    assert!(result.records[0].get("_id").is_some());

    // Verify record count
    assert_eq!(table.count().await.unwrap(), 1);
}

#[tokio::test]
async fn test_execute_insert_multiple() {
    let table = setup_empty_table().await;

    let op: InsertOp = serde_json::from_value(json!({
        "insert_into": "users",
        "values": [
            {"name": "Alice", "age": 30},
            {"name": "Bob", "age": 25},
            {"name": "Carol", "age": 35}
        ]
    })).unwrap();

    let result = table.execute_insert(&op).await.unwrap();

    assert_eq!(result.affected, 3);
    assert_eq!(result.records.len(), 3);
    assert_eq!(table.count().await.unwrap(), 3);
}

#[tokio::test]
async fn test_execute_insert_empty() {
    let table = setup_empty_table().await;

    let op: InsertOp = serde_json::from_value(json!({
        "insert_into": "users",
        "values": []
    })).unwrap();

    let result = table.execute_insert(&op).await.unwrap();

    assert_eq!(result.affected, 0);
    assert_eq!(table.count().await.unwrap(), 0);
}

// ============================================================================
// UPDATE
// ============================================================================

#[tokio::test]
async fn test_execute_update_with_filter() {
    let table = setup_table_with_users().await;
    let interner = table.interner().get().await.unwrap();
    let refs = new_map();
    let ctx = FilterContext::new(interner, &refs);

    // Update active users: set status = "premium"
    let op: UpdateOp = serde_json::from_value(json!({
        "update": "users",
        "where": {"op": "eq", "field": ["status"], "value": "active"},
        "set": {"status": "premium"}
    })).unwrap();

    let result = table.execute_update(&op, &ctx).await.unwrap();

    // Alice and Bob are active
    assert_eq!(result.affected, 2);
    assert!(result.records.is_empty()); // no select requested

    // Verify: read all and check statuses
    assert_eq!(table.count().await.unwrap(), 3);
}

#[tokio::test]
async fn test_execute_update_returns_changed() {
    let table = setup_table_with_users().await;
    let interner = table.interner().get().await.unwrap();
    let refs = new_map();
    let ctx = FilterContext::new(interner, &refs);

    let op: UpdateOp = serde_json::from_value(json!({
        "update": "users",
        "where": {"op": "eq", "field": ["status"], "value": "active"},
        "set": {"status": "premium"},
        "select": {"return_mode": "changed"}
    })).unwrap();

    let result = table.execute_update(&op, &ctx).await.unwrap();

    assert_eq!(result.affected, 2);
    assert_eq!(result.records.len(), 2);
    // All returned records should have status = "premium"
    for record in &result.records {
        assert_eq!(record["status"], "premium");
    }
}

#[tokio::test]
async fn test_execute_update_no_match() {
    let table = setup_table_with_users().await;
    let interner = table.interner().get().await.unwrap();
    let refs = new_map();
    let ctx = FilterContext::new(interner, &refs);

    let op: UpdateOp = serde_json::from_value(json!({
        "update": "users",
        "where": {"op": "eq", "field": ["status"], "value": "deleted"},
        "set": {"status": "active"}
    })).unwrap();

    let result = table.execute_update(&op, &ctx).await.unwrap();
    assert_eq!(result.affected, 0);
}

#[tokio::test]
async fn test_execute_update_all_records() {
    let table = setup_table_with_users().await;
    let interner = table.interner().get().await.unwrap();
    let refs = new_map();
    let ctx = FilterContext::new(interner, &refs);

    // No where clause — update all
    let op: UpdateOp = serde_json::from_value(json!({
        "update": "users",
        "set": {"verified": true},
        "select": {"return_mode": "all"}
    })).unwrap();

    let result = table.execute_update(&op, &ctx).await.unwrap();
    assert_eq!(result.affected, 3);
    assert_eq!(result.records.len(), 3);
    for record in &result.records {
        assert_eq!(record["verified"], true);
    }
}

#[tokio::test]
async fn test_execute_update_unchanged_mode() {
    let table = setup_table_with_users().await;
    let interner = table.interner().get().await.unwrap();
    let refs = new_map();
    let ctx = FilterContext::new(interner, &refs);

    // Set status = "active" on active users — no actual change
    let op: UpdateOp = serde_json::from_value(json!({
        "update": "users",
        "where": {"op": "eq", "field": ["status"], "value": "active"},
        "set": {"status": "active"},
        "select": {"return_mode": "unchanged"}
    })).unwrap();

    let result = table.execute_update(&op, &ctx).await.unwrap();
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
    let table = setup_table_with_users().await;
    let interner = table.interner().get().await.unwrap();
    let refs = new_map();
    let ctx = FilterContext::new(interner, &refs);

    let op: DeleteOp = serde_json::from_value(json!({
        "delete_from": "users",
        "where": {"op": "eq", "field": ["status"], "value": "inactive"}
    })).unwrap();

    let result = table.execute_delete(&op, &ctx).await.unwrap();

    // Carol is inactive
    assert_eq!(result.affected, 1);
    assert_eq!(table.count().await.unwrap(), 2);
}

#[tokio::test]
async fn test_execute_delete_no_match() {
    let table = setup_table_with_users().await;
    let interner = table.interner().get().await.unwrap();
    let refs = new_map();
    let ctx = FilterContext::new(interner, &refs);

    let op: DeleteOp = serde_json::from_value(json!({
        "delete_from": "users",
        "where": {"op": "eq", "field": ["status"], "value": "deleted"}
    })).unwrap();

    let result = table.execute_delete(&op, &ctx).await.unwrap();
    assert_eq!(result.affected, 0);
    assert_eq!(table.count().await.unwrap(), 3);
}

#[tokio::test]
async fn test_execute_delete_multiple() {
    let table = setup_table_with_users().await;
    let interner = table.interner().get().await.unwrap();
    let refs = new_map();
    let ctx = FilterContext::new(interner, &refs);

    // Delete all active users (Alice, Bob)
    let op: DeleteOp = serde_json::from_value(json!({
        "delete_from": "users",
        "where": {"op": "eq", "field": ["status"], "value": "active"}
    })).unwrap();

    let result = table.execute_delete(&op, &ctx).await.unwrap();
    assert_eq!(result.affected, 2);
    assert_eq!(table.count().await.unwrap(), 1);
}

// ============================================================================
// INSERT + UPDATE + DELETE pipeline
// ============================================================================

#[tokio::test]
async fn test_insert_update_delete_pipeline() {
    let table = setup_empty_table().await;
    let interner = table.interner().get().await.unwrap();
    let refs = new_map();
    let ctx = FilterContext::new(interner, &refs);

    // 1. Insert
    let insert_op: InsertOp = serde_json::from_value(json!({
        "insert_into": "users",
        "values": [
            {"name": "Alice", "score": 100},
            {"name": "Bob", "score": 50}
        ]
    })).unwrap();
    let r = table.execute_insert(&insert_op).await.unwrap();
    assert_eq!(r.affected, 2);

    // Need to re-get interner after insert (new keys may have been interned)
    let interner = table.interner().get().await.unwrap();
    let ctx = FilterContext::new(interner, &refs);

    // 2. Update: boost Bob's score
    let update_op: UpdateOp = serde_json::from_value(json!({
        "update": "users",
        "where": {"op": "eq", "field": ["name"], "value": "Bob"},
        "set": {"score": 75},
        "select": {"return_mode": "changed"}
    })).unwrap();
    let r = table.execute_update(&update_op, &ctx).await.unwrap();
    assert_eq!(r.affected, 1);
    assert_eq!(r.records[0]["score"], 75);

    // 3. Delete: remove low scorers
    let delete_op: DeleteOp = serde_json::from_value(json!({
        "delete_from": "users",
        "where": {"op": "lt", "field": ["score"], "value": 80}
    })).unwrap();
    let r = table.execute_delete(&delete_op, &ctx).await.unwrap();
    assert_eq!(r.affected, 1); // Bob(75) deleted

    assert_eq!(table.count().await.unwrap(), 1); // Only Alice remains
}

// ============================================================================
// SET (upsert)
// ============================================================================

#[tokio::test]
async fn test_execute_set_insert_new() {
    let table = setup_empty_table().await;

    let op: SetOp = serde_json::from_value(json!({
        "set": "users",
        "key": {"email": "alice@example.com"},
        "value": {"email": "alice@example.com", "name": "Alice"}
    })).unwrap();

    let result = table.execute_set(&op).await.unwrap();

    assert_eq!(result.affected, 1);
    assert_eq!(result.records[0]["_created"], true);
    assert_eq!(result.records[0]["name"], "Alice");
    assert_eq!(table.count().await.unwrap(), 1);
}

#[tokio::test]
async fn test_execute_set_update_existing() {
    let table = setup_table_with_users().await;
    let interner = table.interner().get().await.unwrap();

    // Alice exists with status=active. Upsert by name.
    let op: SetOp = serde_json::from_value(json!({
        "set": "users",
        "key": {"name": "Alice"},
        "value": {"name": "Alice", "status": "vip", "score": 100}
    })).unwrap();

    let result = table.execute_set(&op).await.unwrap();

    assert_eq!(result.affected, 1);
    assert_eq!(result.records[0]["_created"], false);
    assert_eq!(result.records[0]["status"], "vip");
    // Original field "age" should be preserved (merge)
    assert_eq!(result.records[0]["age"], 30);
    assert_eq!(table.count().await.unwrap(), 3); // no new record
}

#[tokio::test]
async fn test_execute_set_no_match_inserts() {
    let table = setup_table_with_users().await;

    let op: SetOp = serde_json::from_value(json!({
        "set": "users",
        "key": {"name": "Zara"},
        "value": {"name": "Zara", "age": 22}
    })).unwrap();

    let result = table.execute_set(&op).await.unwrap();

    assert_eq!(result.records[0]["_created"], true);
    assert_eq!(table.count().await.unwrap(), 4); // new record added
}
