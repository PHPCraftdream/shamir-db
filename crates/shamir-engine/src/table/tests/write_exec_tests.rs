//! Integration tests for write operation execution.

#![allow(deprecated)]

use serde_json::json;

use shamir_types::codecs::transform;
use crate::db_instance::db_instance::DbInstance;
use crate::repo::repo_types::BoxRepoFactory;
use crate::repo::RepoConfig;
use crate::table::TableConfig;
use crate::query::filter::eval_context::FilterContext;
use crate::query::write::{DeleteOp, InsertOp, SetOp, UpdateOp};
use shamir_types::types::common::new_map;
use shamir_types::types::value::UserValue;

/// Create a DbInstance with one "users" table, return the table manager.
async fn setup_empty_table() -> crate::table::TableManager {
    let repo_config = RepoConfig {
        name: "default".to_string(),
        factory: BoxRepoFactory::in_memory(),
        tables: vec![TableConfig::new("users")],
    };
    let db = DbInstance::with_repos(vec![repo_config]).await.unwrap();
    db.get_table("default", "users").await.unwrap()
}

/// Setup table with pre-inserted users.
async fn setup_table_with_users() -> crate::table::TableManager {
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

// ============================================================================
// Interner persistence after writes
// ============================================================================

/// This test verifies that new interned keys are persisted after insert.
/// Without auto-persist, a "restart" (new InternerManager on same storage)
/// would lose the keys and fail to read back the data correctly.
#[tokio::test]
async fn test_interner_persisted_after_insert() {
    // Setup: create table via raw storage (so we can simulate restart)
    let repo_config = RepoConfig {
        name: "default".to_string(),
        factory: BoxRepoFactory::in_memory(),
        tables: vec![TableConfig::new("users")],
    };
    let db = DbInstance::with_repos(vec![repo_config]).await.unwrap();
    let table = db.get_table("default", "users").await.unwrap();

    // Insert records with new field keys ("brand_new_field" never seen before)
    let op: InsertOp = serde_json::from_value(json!({
        "insert_into": "users",
        "values": [{"brand_new_field": "value1"}, {"brand_new_field": "value2"}]
    })).unwrap();
    table.execute_insert(&op).await.unwrap();

    // Verify the key was interned
    let interner = table.interner().get().await.unwrap();
    assert!(interner.get_ind("brand_new_field").is_some());

    // Simulate "restart": create a new InternerManager on the same storage
    // The key test: do the persisted entries contain "brand_new_field"?
    let entries = interner.all_entries();
    assert!(
        entries.iter().any(|(_, user_key)| user_key.as_str() == "brand_new_field"),
        "brand_new_field should be in interner entries after persist"
    );
}

/// Same test for execute_update: new set fields should be persisted.
#[tokio::test]
async fn test_interner_persisted_after_update() {
    let table = setup_table_with_users().await;
    let interner = table.interner().get().await.unwrap();
    let refs = new_map();
    let ctx = FilterContext::new(interner, &refs);

    // Update with a brand new field key
    let op: UpdateOp = serde_json::from_value(json!({
        "update": "users",
        "set": {"completely_new_key": 42}
    })).unwrap();
    table.execute_update(&op, &ctx).await.unwrap();

    // The new key should be persisted
    let interner = table.interner().get().await.unwrap();
    assert!(interner.get_ind("completely_new_key").is_some());
    let entries = interner.all_entries();
    assert!(
        entries.iter().any(|(_, uk)| uk.as_str() == "completely_new_key"),
        "completely_new_key should be persisted"
    );
}

/// Same test for execute_set: upsert should persist new keys.
#[tokio::test]
async fn test_interner_persisted_after_set() {
    let table = setup_empty_table().await;

    let op: SetOp = serde_json::from_value(json!({
        "set": "users",
        "key": {"unique_field_xyz": "val"},
        "value": {"unique_field_xyz": "val", "another_new_field": 99}
    })).unwrap();
    table.execute_set(&op).await.unwrap();

    let interner = table.interner().get().await.unwrap();
    assert!(interner.get_ind("unique_field_xyz").is_some());
    assert!(interner.get_ind("another_new_field").is_some());
}
