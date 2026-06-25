//! Integration tests for EXPLAIN / dry-run plan preview.
//!
//! Verifies that `ReadQuery::explain = true` runs only the planner,
//! returns an `ExplainPlan` with the chosen plan type and index, and
//! does NOT materialise any rows.

use crate::db_instance::db_instance::DbInstance;
use crate::query::filter::eval_context::FilterContext;
use crate::query::read::{PlanType, ReadQuery};
use crate::repo::repo_types::BoxRepoFactory;
use crate::repo::RepoConfig;
use crate::table::tests::test_helpers::query_value_to_inner_tracked;
use crate::table::TableConfig;
use shamir_query_builder::Query;
use shamir_types::types::common::new_map;
use shamir_types::types::value::QueryValue;

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
            ("name", QueryValue::Str("Alice".into())),
            ("status", QueryValue::Str("active".into())),
            ("age", QueryValue::Int(30)),
        ],
        vec![
            ("name", QueryValue::Str("Bob".into())),
            ("status", QueryValue::Str("active".into())),
            ("age", QueryValue::Int(25)),
        ],
        vec![
            ("name", QueryValue::Str("Carol".into())),
            ("status", QueryValue::Str("inactive".into())),
            ("age", QueryValue::Int(35)),
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

    // Create a btree index on "status".
    table.create_index("status_idx", &["status"]).await.unwrap();

    table
}

// ============================================================================
// EXPLAIN with index scan — shows IndexScan plan + index name, no rows
// ============================================================================

#[tokio::test]
async fn explain_index_scan_returns_plan_no_rows() {
    let table = setup_table_with_index().await;
    let interner = table.interner().get().await.unwrap();
    let refs = new_map();
    let ctx = FilterContext::new(interner, &refs);

    let query: ReadQuery = Query::from("users")
        .where_eq("status", "active")
        .explain()
        .build();

    let result = table.read(&query, &ctx).await.unwrap();

    // No rows materialised.
    assert!(
        result.records.is_empty(),
        "explain must not materialise rows"
    );

    // ExplainPlan present.
    let plan = result
        .explain
        .as_ref()
        .expect("explain plan must be present");
    assert_eq!(plan.plan_type, PlanType::IndexScan);
    assert!(
        plan.index_used.is_some(),
        "index_used must be set for index scan"
    );
}

// ============================================================================
// EXPLAIN full scan — no index, shows FullScan plan
// ============================================================================

#[tokio::test]
async fn explain_full_scan_returns_plan_no_rows() {
    let table = setup_table_with_index().await;
    let interner = table.interner().get().await.unwrap();
    let refs = new_map();
    let ctx = FilterContext::new(interner, &refs);

    // Filter on "name" which has no index.
    let query: ReadQuery = Query::from("users")
        .where_eq("name", "Alice")
        .explain()
        .build();

    let result = table.read(&query, &ctx).await.unwrap();

    assert!(
        result.records.is_empty(),
        "explain must not materialise rows"
    );

    let plan = result
        .explain
        .as_ref()
        .expect("explain plan must be present");
    assert_eq!(plan.plan_type, PlanType::FullScan);
    assert!(plan.index_used.is_none());
}

// ============================================================================
// EXPLAIN counter shortcut — count(*) without WHERE
// ============================================================================

#[tokio::test]
async fn explain_count_star_shows_counter_shortcut() {
    let table = setup_table_with_index().await;
    let interner = table.interner().get().await.unwrap();
    let refs = new_map();
    let ctx = FilterContext::new(interner, &refs);

    let query: ReadQuery = Query::from("users")
        .select(vec![shamir_query_types::read::SelectItem::CountAll {
            alias: None,
        }])
        .explain()
        .build();

    let result = table.read(&query, &ctx).await.unwrap();

    assert!(result.records.is_empty());

    let plan = result
        .explain
        .as_ref()
        .expect("explain plan must be present");
    assert_eq!(plan.plan_type, PlanType::CounterShortcut);
    assert_eq!(plan.index_used.as_deref(), Some("__record_counter__"));
}

// ============================================================================
// Non-explain query returns no explain field
// ============================================================================

#[tokio::test]
async fn non_explain_query_has_no_explain_field() {
    let table = setup_table_with_index().await;
    let interner = table.interner().get().await.unwrap();
    let refs = new_map();
    let ctx = FilterContext::new(interner, &refs);

    let query: ReadQuery = Query::from("users").where_eq("status", "active").build();

    let result = table.read(&query, &ctx).await.unwrap();

    // Normal query returns rows, no explain.
    assert!(!result.records.is_empty());
    assert!(result.explain.is_none());
}
