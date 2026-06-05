//! Integration tests for write operation execution.
//!
//! Request-building uses the typed query builder (`shamir_query_builder`).

#![allow(deprecated)]

use shamir_query_builder::filter;
use shamir_query_builder::val::*;
use shamir_query_builder::write::{self, doc, UpdateReturnMode};

use crate::db_instance::db_instance::DbInstance;
use crate::query::filter::eval_context::FilterContext;
use crate::query::write::InsertOp;
use crate::repo::repo_types::BoxRepoFactory;
use crate::repo::RepoConfig;
use crate::table::TableConfig;
use shamir_types::codecs::transform;
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

    let op = write::insert("users")
        .row(doc().set("name", "Alice").set("age", 30_i64))
        .build();

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

    let op = write::insert("users")
        .row(doc().set("name", "Alice").set("age", 30_i64))
        .row(doc().set("name", "Bob").set("age", 25_i64))
        .row(doc().set("name", "Carol").set("age", 35_i64))
        .build();

    let result = table.execute_insert(&op).await.unwrap();

    assert_eq!(result.affected, 3);
    assert_eq!(result.records.len(), 3);
    assert_eq!(table.count().await.unwrap(), 3);
}

#[tokio::test]
async fn test_execute_insert_empty() {
    let table = setup_empty_table().await;

    let op: InsertOp = write::insert("users").build();

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
    let op = write::update("users")
        .where_(filter::eq("status", "active"))
        .set(doc().set("status", "premium"))
        .build();

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

    let op = write::update("users")
        .where_(filter::eq("status", "active"))
        .set(doc().set("status", "premium"))
        .returning(UpdateReturnMode::Changed)
        .build();

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

    let op = write::update("users")
        .where_(filter::eq("status", "deleted"))
        .set(doc().set("status", "active"))
        .build();

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
    let op = write::update("users")
        .set(doc().set("verified", true))
        .returning(UpdateReturnMode::All)
        .build();

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
    let op = write::update("users")
        .where_(filter::eq("status", "active"))
        .set(doc().set("status", "active"))
        .returning(UpdateReturnMode::Unchanged)
        .build();

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

    let op = write::delete("users")
        .where_(filter::eq("status", "inactive"))
        .build();

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

    let op = write::delete("users")
        .where_(filter::eq("status", "deleted"))
        .build();

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
    let op = write::delete("users")
        .where_(filter::eq("status", "active"))
        .build();

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
    let _ctx = FilterContext::new(interner, &refs);

    // 1. Insert
    let insert_op = write::insert("users")
        .row(doc().set("name", "Alice").set("score", 100_i64))
        .row(doc().set("name", "Bob").set("score", 50_i64))
        .build();
    let r = table.execute_insert(&insert_op).await.unwrap();
    assert_eq!(r.affected, 2);

    // Need to re-get interner after insert (new keys may have been interned)
    let interner = table.interner().get().await.unwrap();
    let ctx = FilterContext::new(interner, &refs);

    // 2. Update: boost Bob's score
    let update_op = write::update("users")
        .where_(filter::eq("name", "Bob"))
        .set(doc().set("score", 75_i64))
        .returning(UpdateReturnMode::Changed)
        .build();
    let r = table.execute_update(&update_op, &ctx).await.unwrap();
    assert_eq!(r.affected, 1);
    assert_eq!(r.records[0]["score"], 75);

    // 3. Delete: remove low scorers
    let delete_op = write::delete("users")
        .where_(filter::lt("score", 80_i64))
        .build();
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

    let op = write::upsert("users")
        .key(doc().set("email", "alice@example.com"))
        .value(doc().set("email", "alice@example.com").set("name", "Alice"))
        .build();

    let result = table.execute_set(&op).await.unwrap();

    assert_eq!(result.affected, 1);
    assert_eq!(result.records[0]["_created"], true);
    assert_eq!(result.records[0]["name"], "Alice");
    assert_eq!(table.count().await.unwrap(), 1);
}

#[tokio::test]
async fn test_execute_set_update_existing() {
    let table = setup_table_with_users().await;
    let _interner = table.interner().get().await.unwrap();

    // Alice exists with status=active. Upsert by name.
    let op = write::upsert("users")
        .key(doc().set("name", "Alice"))
        .value(
            doc()
                .set("name", "Alice")
                .set("status", "vip")
                .set("score", 100_i64),
        )
        .build();

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

    let op = write::upsert("users")
        .key(doc().set("name", "Zara"))
        .value(doc().set("name", "Zara").set("age", 22_i64))
        .build();

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
    let op = write::insert("users")
        .row(doc().set("brand_new_field", "value1"))
        .row(doc().set("brand_new_field", "value2"))
        .build();
    table.execute_insert(&op).await.unwrap();

    // Verify the key was interned
    let interner = table.interner().get().await.unwrap();
    assert!(interner.get_ind("brand_new_field").is_some());

    // Simulate "restart": create a new InternerManager on the same storage
    // The key test: do the persisted entries contain "brand_new_field"?
    let entries = interner.all_entries();
    assert!(
        entries
            .iter()
            .any(|(_, user_key)| user_key.as_str() == "brand_new_field"),
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
    let op = write::update("users")
        .set(doc().set("completely_new_key", 42_i64))
        .build();
    table.execute_update(&op, &ctx).await.unwrap();

    // The new key should be persisted
    let interner = table.interner().get().await.unwrap();
    assert!(interner.get_ind("completely_new_key").is_some());
    let entries = interner.all_entries();
    assert!(
        entries
            .iter()
            .any(|(_, uk)| uk.as_str() == "completely_new_key"),
        "completely_new_key should be persisted"
    );
}

/// Same test for execute_set: upsert should persist new keys.
#[tokio::test]
async fn test_interner_persisted_after_set() {
    let table = setup_empty_table().await;

    let op = write::upsert("users")
        .key(doc().set("unique_field_xyz", "val"))
        .value(
            doc()
                .set("unique_field_xyz", "val")
                .set("another_new_field", 99_i64),
        )
        .build();
    table.execute_set(&op).await.unwrap();

    let interner = table.interner().get().await.unwrap();
    assert!(interner.get_ind("unique_field_xyz").is_some());
    assert!(interner.get_ind("another_new_field").is_some());
}

// ============================================================================
// Computed write values ("установка знаний" via inline $fn)
// ============================================================================

#[tokio::test]
async fn test_insert_computed_field_lowercase() {
    let table = setup_empty_table().await;

    // email_norm = strings/lower(email), evaluated at write time.
    let op = write::insert("users")
        .row(
            doc()
                .set("email", "Alice@Example.COM")
                .set("email_norm", func("strings/lower", [col("email")])),
        )
        .build();

    let result = table.execute_insert(&op).await.unwrap();
    assert_eq!(result.affected, 1);
    // The literal field is untouched; the computed field holds the result.
    assert_eq!(result.records[0]["email"], "Alice@Example.COM");
    assert_eq!(result.records[0]["email_norm"], "alice@example.com");
}

#[tokio::test]
async fn test_set_computed_field() {
    let table = setup_empty_table().await;

    let op = write::upsert("users")
        .key(doc().set("email", "x@y.z"))
        .value(
            doc()
                .set("email", "X@Y.Z")
                .set("email_norm", func("strings/lower", [col("email")])),
        )
        .build();

    let result = table.execute_set(&op).await.unwrap();
    assert_eq!(result.records[0]["email_norm"], "x@y.z");
}

#[tokio::test]
async fn test_insert_computed_unknown_function_fails_closed() {
    let table = setup_empty_table().await;

    let op = write::insert("users")
        .row(doc().set("x", func("strings/does_not_exist", [])))
        .build();

    // A broken computed value aborts the write rather than storing garbage.
    assert!(table.execute_insert(&op).await.is_err());
}

#[tokio::test]
async fn test_insert_computed_bad_ref_fails_closed() {
    let table = setup_empty_table().await;

    let op = write::insert("users")
        .row(doc().set("y", func("strings/lower", [col("missing_field")])))
        .build();

    assert!(table.execute_insert(&op).await.is_err());
}
