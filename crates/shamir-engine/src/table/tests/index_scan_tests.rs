//! Integration tests for index-aware read query execution.
//!
//! Tests that `TableManager::read()` uses indexes when the WHERE clause
//! contains Eq conditions on indexed fields, and falls through to full
//! scan otherwise.

#![allow(deprecated)]

use serde_json::json;

use shamir_types::codecs::transform;
use crate::db_instance::db_instance::DbInstance;
use crate::repo::repo_types::BoxRepoFactory;
use crate::repo::RepoConfig;
use crate::table::TableConfig;
use crate::query::filter::eval_context::FilterContext;
use crate::query::read::ReadQuery;
use shamir_types::types::common::new_map;
use shamir_types::types::value::UserValue;

/// Create a table with 5 users and a regular index on "status".
async fn setup_table_with_index() -> crate::table::TableManager {
    let repo_config = RepoConfig {
        name: "default".to_string(),
        factory: BoxRepoFactory::in_memory(),
        tables: vec![TableConfig::new("users")],
    };
    let db = DbInstance::with_repos(vec![repo_config]).await.unwrap();
    let table = db.get_table("default", "users").await.unwrap();

    let users = vec![
        vec![
            ("name", UserValue::Str("Alice".into())),
            ("age", UserValue::Int(30)),
            ("status", UserValue::Str("active".into())),
            ("city", UserValue::Str("NYC".into())),
        ],
        vec![
            ("name", UserValue::Str("Bob".into())),
            ("age", UserValue::Int(25)),
            ("status", UserValue::Str("active".into())),
            ("city", UserValue::Str("LA".into())),
        ],
        vec![
            ("name", UserValue::Str("Carol".into())),
            ("age", UserValue::Int(35)),
            ("status", UserValue::Str("inactive".into())),
            ("city", UserValue::Str("NYC".into())),
        ],
        vec![
            ("name", UserValue::Str("Dave".into())),
            ("age", UserValue::Int(22)),
            ("status", UserValue::Str("active".into())),
            ("city", UserValue::Str("LA".into())),
        ],
        vec![
            ("name", UserValue::Str("Eve".into())),
            ("age", UserValue::Int(28)),
            ("status", UserValue::Str("deleted".into())),
            ("city", UserValue::Str("NYC".into())),
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

    // Create index on "status"
    table.create_index("status_idx", &["status"]).await.unwrap();

    table
}

/// Extract sorted name strings from a QueryResult
fn extract_names_from_result(result: &crate::query::read::QueryResult) -> Vec<String> {
    let mut names: Vec<String> = result
        .records
        .iter()
        .filter_map(|r| r.get("name").and_then(|v| v.as_str()).map(|s| s.to_string()))
        .collect();
    names.sort();
    names
}

// ============================================================================
// Index used for simple Eq
// ============================================================================

#[tokio::test]
async fn test_read_uses_index_for_eq_filter() {
    let table = setup_table_with_index().await;
    let interner = table.interner().get().await.unwrap();
    let refs = new_map();
    let ctx = FilterContext::new(interner, &refs);

    let query: ReadQuery = serde_json::from_value(json!({
        "from": "users",
        "where": {"op": "eq", "field": ["status"], "value": "active"}
    })).unwrap();

    let result = table.read(&query, &ctx).await.unwrap();

    assert_eq!(extract_names_from_result(&result), vec!["Alice", "Bob", "Dave"]);
    assert_eq!(
        result.stats.as_ref().unwrap().index_used,
        Some("status_idx".to_string())
    );
}

// ============================================================================
// Index used for And with Eq + residual post-filter
// ============================================================================

#[tokio::test]
async fn test_read_uses_index_for_and_with_eq() {
    let table = setup_table_with_index().await;
    let interner = table.interner().get().await.unwrap();
    let refs = new_map();
    let ctx = FilterContext::new(interner, &refs);

    // status == "active" AND age > 25
    let query: ReadQuery = serde_json::from_value(json!({
        "from": "users",
        "where": {
            "op": "and",
            "filters": [
                {"op": "eq", "field": ["status"], "value": "active"},
                {"op": "gt", "field": ["age"], "value": 25}
            ]
        }
    })).unwrap();

    let result = table.read(&query, &ctx).await.unwrap();

    // active: Alice(30), Bob(25), Dave(22)
    // age > 25: Alice(30)
    assert_eq!(extract_names_from_result(&result), vec!["Alice"]);
    assert_eq!(
        result.stats.as_ref().unwrap().index_used,
        Some("status_idx".to_string())
    );
}

// ============================================================================
// Composite index
// ============================================================================

#[tokio::test]
async fn test_read_composite_index() {
    let table = setup_table_with_index().await;

    // Create composite index on ["status", "city"]
    table
        .create_index("status_city_idx", &["status", "city"])
        .await
        .unwrap();

    let interner = table.interner().get().await.unwrap();
    let refs = new_map();
    let ctx = FilterContext::new(interner, &refs);

    let query: ReadQuery = serde_json::from_value(json!({
        "from": "users",
        "where": {
            "op": "and",
            "filters": [
                {"op": "eq", "field": ["status"], "value": "active"},
                {"op": "eq", "field": ["city"], "value": "LA"}
            ]
        }
    })).unwrap();

    let result = table.read(&query, &ctx).await.unwrap();

    assert_eq!(extract_names_from_result(&result), vec!["Bob", "Dave"]);
    // Should use the composite index
    assert_eq!(
        result.stats.as_ref().unwrap().index_used,
        Some("status_city_idx".to_string())
    );
}

// ============================================================================
// No index for Gt (not equality)
// ============================================================================

#[tokio::test]
async fn test_read_no_index_for_gt() {
    let table = setup_table_with_index().await;
    let interner = table.interner().get().await.unwrap();
    let refs = new_map();
    let ctx = FilterContext::new(interner, &refs);

    let query: ReadQuery = serde_json::from_value(json!({
        "from": "users",
        "where": {"op": "gt", "field": ["age"], "value": 25}
    })).unwrap();

    let result = table.read(&query, &ctx).await.unwrap();

    assert_eq!(extract_names_from_result(&result), vec!["Alice", "Carol", "Eve"]);
    assert_eq!(result.stats.as_ref().unwrap().index_used, None);
}

// ============================================================================
// No index for Or
// ============================================================================

#[tokio::test]
async fn test_read_no_index_for_or() {
    let table = setup_table_with_index().await;
    let interner = table.interner().get().await.unwrap();
    let refs = new_map();
    let ctx = FilterContext::new(interner, &refs);

    let query: ReadQuery = serde_json::from_value(json!({
        "from": "users",
        "where": {
            "op": "or",
            "filters": [
                {"op": "eq", "field": ["status"], "value": "active"},
                {"op": "eq", "field": ["status"], "value": "deleted"}
            ]
        }
    })).unwrap();

    let result = table.read(&query, &ctx).await.unwrap();
    assert_eq!(result.stats.as_ref().unwrap().index_used, None);
}

// ============================================================================
// Index with no results
// ============================================================================

#[tokio::test]
async fn test_read_index_with_no_results() {
    let table = setup_table_with_index().await;
    let interner = table.interner().get().await.unwrap();
    let refs = new_map();
    let ctx = FilterContext::new(interner, &refs);

    let query: ReadQuery = serde_json::from_value(json!({
        "from": "users",
        "where": {"op": "eq", "field": ["status"], "value": "banned"}
    })).unwrap();

    let result = table.read(&query, &ctx).await.unwrap();

    assert!(result.records.is_empty());
    assert_eq!(
        result.stats.as_ref().unwrap().index_used,
        Some("status_idx".to_string())
    );
}

// ============================================================================
// Index + pagination
// ============================================================================

#[tokio::test]
async fn test_read_index_with_pagination() {
    let table = setup_table_with_index().await;
    let interner = table.interner().get().await.unwrap();
    let refs = new_map();
    let ctx = FilterContext::new(interner, &refs);

    let query: ReadQuery = serde_json::from_value(json!({
        "from": "users",
        "where": {"op": "eq", "field": ["status"], "value": "active"},
        "pagination": {"mode": "LimitOffset", "limit": 2},
        "count_total": true
    })).unwrap();

    let result = table.read(&query, &ctx).await.unwrap();

    assert_eq!(result.records.len(), 2);
    assert_eq!(
        result.stats.as_ref().unwrap().index_used,
        Some("status_idx".to_string())
    );
    // count_total should reflect all 3 active users
    let pagination = result.pagination.as_ref().unwrap();
    assert_eq!(pagination.total_count, Some(3));
    assert!(pagination.has_next);
}

// ============================================================================
// Index + order_by
// ============================================================================

#[tokio::test]
async fn test_read_index_with_order_by() {
    let table = setup_table_with_index().await;
    let interner = table.interner().get().await.unwrap();
    let refs = new_map();
    let ctx = FilterContext::new(interner, &refs);

    let query: ReadQuery = serde_json::from_value(json!({
        "from": "users",
        "where": {"op": "eq", "field": ["status"], "value": "active"},
        "order_by": {"items": [{"field": ["age"], "direction": "desc"}]}
    })).unwrap();

    let result = table.read(&query, &ctx).await.unwrap();

    let names: Vec<String> = result
        .records
        .iter()
        .filter_map(|r| r.get("name").and_then(|v| v.as_str()).map(|s| s.to_string()))
        .collect();
    // Active sorted by age desc: Alice(30), Bob(25), Dave(22)
    assert_eq!(names, vec!["Alice", "Bob", "Dave"]);
    assert_eq!(
        result.stats.as_ref().unwrap().index_used,
        Some("status_idx".to_string())
    );
}

// ============================================================================
// FieldRef value falls through (not a literal)
// ============================================================================

#[tokio::test]
async fn test_read_index_with_field_ref_falls_through() {
    let table = setup_table_with_index().await;
    let interner = table.interner().get().await.unwrap();
    let refs = new_map();
    let ctx = FilterContext::new(interner, &refs);

    let query: ReadQuery = serde_json::from_value(json!({
        "from": "users",
        "where": {"op": "eq", "field": ["status"], "value": {"$ref": ["name"]}}
    })).unwrap();

    let result = table.read(&query, &ctx).await.unwrap();
    // FieldRef can't be used for index lookup -> full scan
    assert_eq!(result.stats.as_ref().unwrap().index_used, None);
}

// ============================================================================
// In: multiple index lookups, union results
// ============================================================================

#[tokio::test]
async fn test_read_uses_index_for_in() {
    let table = setup_table_with_index().await;
    let interner = table.interner().get().await.unwrap();
    let refs = new_map();
    let ctx = FilterContext::new(interner, &refs);

    let query: ReadQuery = serde_json::from_value(json!({
        "from": "users",
        "where": {"op": "in", "field": ["status"], "values": ["active", "deleted"]}
    })).unwrap();

    let result = table.read(&query, &ctx).await.unwrap();

    // active: Alice, Bob, Dave; deleted: Eve
    assert_eq!(
        extract_names_from_result(&result),
        vec!["Alice", "Bob", "Dave", "Eve"]
    );
    assert_eq!(
        result.stats.as_ref().unwrap().index_used,
        Some("status_idx".to_string())
    );
}

#[tokio::test]
async fn test_read_uses_index_for_in_single_value() {
    let table = setup_table_with_index().await;
    let interner = table.interner().get().await.unwrap();
    let refs = new_map();
    let ctx = FilterContext::new(interner, &refs);

    let query: ReadQuery = serde_json::from_value(json!({
        "from": "users",
        "where": {"op": "in", "field": ["status"], "values": ["inactive"]}
    })).unwrap();

    let result = table.read(&query, &ctx).await.unwrap();

    assert_eq!(extract_names_from_result(&result), vec!["Carol"]);
    assert_eq!(
        result.stats.as_ref().unwrap().index_used,
        Some("status_idx".to_string())
    );
}

#[tokio::test]
async fn test_read_uses_index_for_in_no_match() {
    let table = setup_table_with_index().await;
    let interner = table.interner().get().await.unwrap();
    let refs = new_map();
    let ctx = FilterContext::new(interner, &refs);

    let query: ReadQuery = serde_json::from_value(json!({
        "from": "users",
        "where": {"op": "in", "field": ["status"], "values": ["banned", "suspended"]}
    })).unwrap();

    let result = table.read(&query, &ctx).await.unwrap();

    assert!(result.records.is_empty());
    assert_eq!(
        result.stats.as_ref().unwrap().index_used,
        Some("status_idx".to_string())
    );
}

// ============================================================================
// And with In + residual
// ============================================================================

#[tokio::test]
async fn test_read_uses_index_for_and_with_in() {
    let table = setup_table_with_index().await;
    let interner = table.interner().get().await.unwrap();
    let refs = new_map();
    let ctx = FilterContext::new(interner, &refs);

    // status IN ["active", "inactive"] AND age > 25
    let query: ReadQuery = serde_json::from_value(json!({
        "from": "users",
        "where": {
            "op": "and",
            "filters": [
                {"op": "in", "field": ["status"], "values": ["active", "inactive"]},
                {"op": "gt", "field": ["age"], "value": 25}
            ]
        }
    })).unwrap();

    let result = table.read(&query, &ctx).await.unwrap();

    // active+inactive: Alice(30), Bob(25), Carol(35), Dave(22)
    // age > 25: Alice(30), Carol(35)
    assert_eq!(
        extract_names_from_result(&result),
        vec!["Alice", "Carol"]
    );
    assert_eq!(
        result.stats.as_ref().unwrap().index_used,
        Some("status_idx".to_string())
    );
}

// ============================================================================
// Sorted-index ORDER BY ASC + LIMIT K fast path
// ============================================================================

/// Build a fresh table with sorted index on `score` and N records of
/// varying scores. Returns (table, expected_sorted_scores_asc).
async fn setup_sorted_score(n: usize) -> (crate::table::TableManager, Vec<i64>) {
    let repo_config = RepoConfig {
        name: "default".to_string(),
        factory: BoxRepoFactory::in_memory(),
        tables: vec![TableConfig::new("users")],
    };
    let db = DbInstance::with_repos(vec![repo_config]).await.unwrap();
    let table = db.get_table("default", "users").await.unwrap();

    // Insert N records with score = (i * 7919) % 1000 — same pseudo-
    // random pattern as the bench, so this exercises the realistic
    // distribution where order_by ≠ insert_order.
    let interner = table.interner().get().await.unwrap();
    let mut scores: Vec<i64> = Vec::with_capacity(n);
    for i in 0..n {
        let s = ((i * 7919) % 1000) as i64;
        scores.push(s);
        let mut map = new_map();
        map.insert("idx".to_string(), UserValue::Int(i as i64));
        map.insert("score".to_string(), UserValue::Int(s));
        let user = UserValue::Map(map);
        let result = transform::user_to_inner(&user, interner);
        if let Some(ref new_keys) = result.new_keys {
            table.interner().save_new_keys(new_keys).await.unwrap();
        }
        table.insert(&result.inner_value).await.unwrap();
    }

    // Sorted index on `score`. `create_sorted_index` registers the
    // definition AND backfills entries from existing records.
    table
        .create_sorted_index("by_score", &["score"])
        .await
        .unwrap();

    let mut expected = scores.clone();
    expected.sort();
    (table, expected)
}

/// Regression / opt: ORDER BY field ASC LIMIT K with a sorted index
/// on `field` must skip the "collect all + sort + truncate" pipeline
/// and go straight through `lookup_first_k`. The signal: stats
/// `index_used` carries the sorted-index marker.
#[tokio::test]
async fn test_order_by_asc_limit_uses_sorted_index_fast_path() {
    let (table, expected) = setup_sorted_score(200).await;
    let interner = table.interner().get().await.unwrap();
    let refs = new_map();
    let ctx = FilterContext::new(interner, &refs);

    let query: ReadQuery = serde_json::from_value(json!({
        "from": "users",
        "order_by": {"items": [{"field": ["score"], "direction": "asc"}]},
        "pagination": {"mode": "LimitOffset", "limit": 5, "offset": 0}
    })).unwrap();

    let result = table.read(&query, &ctx).await.unwrap();

    // Returned exactly 5 records in ascending score order.
    assert_eq!(result.records.len(), 5, "expected 5 records, got {}", result.records.len());
    let got_scores: Vec<i64> = result
        .records
        .iter()
        .map(|r| r.get("score").and_then(|v| v.as_i64()).expect("score field"))
        .collect();
    assert_eq!(got_scores, expected[..5], "wrong records returned for ORDER BY score ASC LIMIT 5");

    // Fast path marker. Without the fast path, `index_used` is None
    // because the fall-back full-scan path doesn't set it for an
    // order-by-only query.
    let used = result
        .stats
        .as_ref()
        .expect("stats")
        .index_used
        .as_deref()
        .unwrap_or("");
    assert!(
        used.starts_with("sorted_idx_") && used.contains("first_k"),
        "ORDER BY ASC LIMIT K did not take the sorted-index fast path \
         (index_used = {:?})",
        used
    );
}
