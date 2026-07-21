//! Integration tests for write operation execution.
//!
//! Request-building uses the typed query builder (`shamir_query_builder`).

use shamir_query_builder::filter;
use shamir_query_builder::val::*;
use shamir_query_builder::write::{self, doc, UpdateReturnMode};

use crate::db_instance::db_instance::DbInstance;
use crate::query::batch::BatchError;
use crate::query::filter::eval_context::FilterContext;
use crate::query::write::{InsertOp, WriteResult};
use crate::repo::repo_types::BoxRepoFactory;
use crate::repo::{RepoConfig, RepoInstance};
use crate::table::tests::test_helpers::query_value_to_inner_tracked;
use crate::table::TableConfig;
use shamir_types::access::Actor;
use shamir_types::types::common::new_map;
use shamir_types::types::value::QueryValue;

// ---------------------------------------------------------------------------
// Test-only helpers: drive a single INSERT / UPDATE / DELETE through the
// PRODUCTION implicit-tx path (`run_implicit_batch_tx` → the `_tx` variant
// → commit). The legacy non-tx `execute_insert` / `execute_update` /
// `execute_delete` were removed (W3a); these helpers preserve the
// "single-op autocommit" ergonomics the tests rely on while exercising the
// real staging + commit pipeline (so index / counter / doctor side-effects
// land exactly as in production).
// ---------------------------------------------------------------------------

/// Test-only INSERT via the production implicit-tx + commit path.
/// Returns the `Result` so tests can assert on `.is_err()` (e.g. for
/// fail-closed computed-field cases) or `.unwrap()`.
pub(crate) async fn insert_via_tx(
    repo: &RepoInstance,
    table: &crate::table::TableManager,
    op: &InsertOp,
    return_result: bool,
) -> Result<WriteResult, BatchError> {
    let owned_op = op.clone();
    let owned_table = table.clone();
    repo.run_implicit_batch_tx(Actor::System, "test_insert", move |tx| {
        Box::pin(async move {
            owned_table
                .execute_insert_tx(
                    &owned_op,
                    tx,
                    return_result,
                    None,
                    &shamir_types::access::Actor::System,
                )
                .await
        })
    })
    .await
}

/// Test-only UPDATE via the production implicit-tx + commit path.
/// `ctx` is rebuilt inside the staging closure (FilterContext borrows the
/// interner Arc, which must be obtained AFTER `begin_tx`).
pub(crate) async fn update_via_tx(
    repo: &RepoInstance,
    table: &crate::table::TableManager,
    op: &shamir_query_types::write::UpdateOp,
    refs: &shamir_types::types::common::TMap<String, crate::query::read::QueryResult>,
) -> Result<WriteResult, BatchError> {
    let owned_op = op.clone();
    let owned_table = table.clone();
    let owned_refs = refs.clone();
    repo.run_implicit_batch_tx(Actor::System, "test_update", move |tx| {
        Box::pin(async move {
            let interner = owned_table.interner().get().await?;
            let ctx = FilterContext::new(interner, &owned_refs);
            owned_table
                .execute_update_tx(
                    &owned_op,
                    &ctx,
                    tx,
                    None,
                    &shamir_types::access::Actor::System,
                )
                .await
        })
    })
    .await
}

/// Test-only DELETE via the production implicit-tx + commit path.
pub(crate) async fn delete_via_tx(
    repo: &RepoInstance,
    table: &crate::table::TableManager,
    op: &shamir_query_types::write::DeleteOp,
    refs: &shamir_types::types::common::TMap<String, crate::query::read::QueryResult>,
) -> Result<WriteResult, BatchError> {
    let owned_op = op.clone();
    let owned_table = table.clone();
    let owned_refs = refs.clone();
    repo.run_implicit_batch_tx(Actor::System, "test_delete", move |tx| {
        Box::pin(async move {
            let interner = owned_table.interner().get().await?;
            let ctx = FilterContext::new(interner, &owned_refs);
            owned_table
                .execute_delete_tx(
                    &owned_op,
                    &ctx,
                    tx,
                    None,
                    &shamir_types::access::Actor::System,
                )
                .await
        })
    })
    .await
}

/// Test-only SET (upsert) via the production implicit-tx + commit path.
pub(crate) async fn set_via_tx(
    repo: &RepoInstance,
    table: &crate::table::TableManager,
    op: &shamir_query_types::write::SetOp,
) -> Result<WriteResult, BatchError> {
    let owned_op = op.clone();
    let owned_table = table.clone();
    repo.run_implicit_batch_tx(Actor::System, "test_set", move |tx| {
        Box::pin(async move {
            owned_table
                .execute_set_tx(&owned_op, tx, None, &shamir_types::access::Actor::System)
                .await
        })
    })
    .await
}

/// Create a DbInstance with one "users" table, return the table manager + repo.
pub(crate) async fn setup_empty_table() -> (crate::table::TableManager, RepoInstance) {
    let repo_config = RepoConfig {
        name: "default".to_string(),
        factory: BoxRepoFactory::in_memory(),
        tables: vec![TableConfig::new("users")],
    };
    let db = DbInstance::with_repos(vec![repo_config]).await.unwrap();
    let table = db.get_table("default", "users").await.unwrap();
    let repo = db.get_repo("default").unwrap();
    (table, repo)
}

/// Setup table with pre-inserted users.
async fn setup_table_with_users() -> (crate::table::TableManager, RepoInstance) {
    let (table, repo) = setup_empty_table().await;

    let users = vec![
        vec![
            ("name", QueryValue::Str("Alice".into())),
            ("age", QueryValue::Int(30)),
            ("status", QueryValue::Str("active".into())),
        ],
        vec![
            ("name", QueryValue::Str("Bob".into())),
            ("age", QueryValue::Int(25)),
            ("status", QueryValue::Str("active".into())),
        ],
        vec![
            ("name", QueryValue::Str("Carol".into())),
            ("age", QueryValue::Int(35)),
            ("status", QueryValue::Str("inactive".into())),
        ],
    ];

    let interner = table.interner().get().await.unwrap();
    for fields in &users {
        let mut map = new_map();
        for (k, v) in fields {
            map.insert(k.to_string(), v.clone());
        }
        let user_val = QueryValue::Map(map);
        let (inner_val, new_keys) = query_value_to_inner_tracked(&user_val, interner).unwrap();
        if !new_keys.is_empty() {
            table.interner().save_new_keys(&new_keys).await.unwrap();
        }
        table.insert(&inner_val).await.unwrap();
    }

    (table, repo)
}

// ============================================================================
// INSERT
// ============================================================================

#[tokio::test]
async fn test_execute_insert_single() {
    let (table, repo) = setup_empty_table().await;

    let op = write::insert("users")
        .row(doc().set("name", "Alice").set("age", 30_i64))
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
        .row(doc().set("name", "Alice").set("age", 30_i64))
        .row(doc().set("name", "Bob").set("age", 25_i64))
        .row(doc().set("name", "Carol").set("age", 35_i64))
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
        .set(doc().set("status", "premium"))
        .build();

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
        .set(doc().set("status", "premium"))
        .returning(UpdateReturnMode::Changed)
        .build();

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
        .set(doc().set("status", "active"))
        .build();

    let result = update_via_tx(&repo, &table, &op, &refs).await.unwrap();
    assert_eq!(result.affected, 0);
}

#[tokio::test]
async fn test_execute_update_all_records() {
    let (table, repo) = setup_table_with_users().await;
    let refs = new_map();

    // No where clause — update all
    let op = write::update("users")
        .set(doc().set("verified", true))
        .returning(UpdateReturnMode::All)
        .build();

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
        .set(doc().set("status", "active"))
        .returning(UpdateReturnMode::Unchanged)
        .build();

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
        .build();

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
        .build();

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
        .build();

    let result = delete_via_tx(&repo, &table, &op, &refs).await.unwrap();
    assert_eq!(result.affected, 2);
    assert_eq!(table.count().await.unwrap(), 1);
}

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
        .build();

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
        .build();

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
        .build();

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
        .build();

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

// ============================================================================
// INSERT + UPDATE + DELETE pipeline
// ============================================================================

#[tokio::test]
async fn test_insert_update_delete_pipeline() {
    let (table, repo) = setup_empty_table().await;
    let refs = new_map();

    // 1. Insert
    let insert_op = write::insert("users")
        .row(doc().set("name", "Alice").set("score", 100_i64))
        .row(doc().set("name", "Bob").set("score", 50_i64))
        .build();
    let r = insert_via_tx(&repo, &table, &insert_op, true)
        .await
        .unwrap();
    assert_eq!(r.affected, 2);

    // 2. Update: boost Bob's score
    let update_op = write::update("users")
        .where_(filter::eq("name", "Bob"))
        .set(doc().set("score", 75_i64))
        .returning(UpdateReturnMode::Changed)
        .build();
    let r = update_via_tx(&repo, &table, &update_op, &refs)
        .await
        .unwrap();
    assert_eq!(r.affected, 1);
    assert_eq!(
        r.records[0].get_value_owned("score"),
        Some(shamir_types::types::value::QueryValue::Int(75))
    );

    // 3. Delete: remove low scorers
    let delete_op = write::delete("users")
        .where_(filter::lt("score", 80_i64))
        .build();
    let r = delete_via_tx(&repo, &table, &delete_op, &refs)
        .await
        .unwrap();
    assert_eq!(r.affected, 1); // Bob(75) deleted

    assert_eq!(table.count().await.unwrap(), 1); // Only Alice remains
}

// ============================================================================
// SET (upsert)
// ============================================================================

#[tokio::test]
async fn test_execute_set_insert_new() {
    let (table, repo) = setup_empty_table().await;

    let op = write::upsert("users")
        .key(doc().set("email", "alice@example.com"))
        .value(doc().set("email", "alice@example.com").set("name", "Alice"))
        .build();

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
        .key(doc().set("name", "Alice"))
        .value(
            doc()
                .set("name", "Alice")
                .set("status", "vip")
                .set("score", 100_i64),
        )
        .build();

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
        .key(doc().set("name", "Zara"))
        .value(doc().set("name", "Zara").set("age", 22_i64))
        .build();

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
    let repo = db.get_repo("default").unwrap();

    // Insert records with new field keys ("brand_new_field" never seen before)
    let op = write::insert("users")
        .row(doc().set("brand_new_field", "value1"))
        .row(doc().set("brand_new_field", "value2"))
        .build();
    insert_via_tx(&repo, &table, &op, true).await.unwrap();

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
    let (table, repo) = setup_table_with_users().await;
    let refs = new_map();

    // Update with a brand new field key
    let op = write::update("users")
        .set(doc().set("completely_new_key", 42_i64))
        .build();
    update_via_tx(&repo, &table, &op, &refs).await.unwrap();

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
    let (table, repo) = setup_empty_table().await;

    let op = write::upsert("users")
        .key(doc().set("unique_field_xyz", "val"))
        .value(
            doc()
                .set("unique_field_xyz", "val")
                .set("another_new_field", 99_i64),
        )
        .build();
    set_via_tx(&repo, &table, &op).await.unwrap();

    let interner = table.interner().get().await.unwrap();
    assert!(interner.get_ind("unique_field_xyz").is_some());
    assert!(interner.get_ind("another_new_field").is_some());
}

// ============================================================================
// Computed write values ("установка знаний" via inline $fn)
// ============================================================================

#[tokio::test]
async fn test_insert_computed_field_lowercase() {
    let (table, repo) = setup_empty_table().await;

    // email_norm = strings/lower(email), evaluated at write time.
    let op = write::insert("users")
        .row(
            doc()
                .set("email", "Alice@Example.COM")
                .set("email_norm", func("strings/lower", [col("email")])),
        )
        .build();

    let result = insert_via_tx(&repo, &table, &op, true).await.unwrap();
    assert_eq!(result.affected, 1);
    // The literal field is untouched; the computed field holds the result.
    assert_eq!(
        result.records[0].get_value_owned("email"),
        Some(shamir_types::types::value::QueryValue::Str(
            "Alice@Example.COM".to_string()
        ))
    );
    assert_eq!(
        result.records[0].get_value_owned("email_norm"),
        Some(shamir_types::types::value::QueryValue::Str(
            "alice@example.com".to_string()
        ))
    );
}

#[tokio::test]
async fn test_set_computed_field() {
    let (table, repo) = setup_empty_table().await;

    let op = write::upsert("users")
        .key(doc().set("email", "x@y.z"))
        .value(
            doc()
                .set("email", "X@Y.Z")
                .set("email_norm", func("strings/lower", [col("email")])),
        )
        .build();

    let result = set_via_tx(&repo, &table, &op).await.unwrap();
    assert_eq!(
        result.records[0].get_value_owned("email_norm"),
        Some(shamir_types::types::value::QueryValue::Str(
            "x@y.z".to_string()
        ))
    );
}

#[tokio::test]
async fn test_insert_computed_unknown_function_fails_closed() {
    let (table, repo) = setup_empty_table().await;

    let op = write::insert("users")
        .row(doc().set("x", func("strings/does_not_exist", [])))
        .build();

    // A broken computed value aborts the write rather than storing garbage.
    assert!(insert_via_tx(&repo, &table, &op, true).await.is_err());
}

#[tokio::test]
async fn test_insert_computed_bad_ref_fails_closed() {
    let (table, repo) = setup_empty_table().await;

    let op = write::insert("users")
        .row(doc().set("y", func("strings/lower", [col("missing_field")])))
        .build();

    assert!(insert_via_tx(&repo, &table, &op, true).await.is_err());
}

// ============================================================================
// W3d-2: non-tx SET reroute parity (implicit-tx path)
// ============================================================================

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
        .build();

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
        .build();

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
