//! Integration tests for TableManager::filter_stream.
//!
//! Each test: JSON filter → compile → filter_stream over real table → check results.

#![allow(deprecated)]

use shamir_types::codecs::transform;
use crate::db_instance::db_instance::DbInstance;
use crate::repo::repo_types::BoxRepoFactory;
use crate::repo::RepoConfig;
use crate::table::tests::stream_utils::collect_filter_stream;
use crate::table::TableConfig;
use crate::query::common::filter_from_value;
use crate::query::filter::eval_context::FilterContext;
use crate::query::read::QueryResult;
use shamir_types::types::common::new_map;
use shamir_types::types::value::{QueryValue, UserValue};

/// Create a DbInstance with one "users" table, insert test records, return the table manager.
async fn setup_table_with_users() -> crate::table::TableManager {
    let repo_config = RepoConfig {
        name: "default".to_string(),
        factory: BoxRepoFactory::in_memory(),
        tables: vec![TableConfig::new("users")],
    };
    let db = DbInstance::with_repos(vec![repo_config]).await.unwrap();
    let table = db.get_table("default", "users").await.unwrap();

    // Insert 5 users with different attributes
    let users = vec![
        vec![
            ("name", UserValue::Str("Alice".into())),
            ("age", UserValue::Int(30)),
            ("status", UserValue::Str("active".into())),
            ("score", UserValue::Int(95)),
        ],
        vec![
            ("name", UserValue::Str("Bob".into())),
            ("age", UserValue::Int(25)),
            ("status", UserValue::Str("active".into())),
            ("score", UserValue::Int(60)),
        ],
        vec![
            ("name", UserValue::Str("Carol".into())),
            ("age", UserValue::Int(35)),
            ("status", UserValue::Str("inactive".into())),
            ("score", UserValue::Int(80)),
        ],
        vec![
            ("name", UserValue::Str("Dave".into())),
            ("age", UserValue::Int(22)),
            ("status", UserValue::Str("active".into())),
            ("score", UserValue::Int(45)),
        ],
        vec![
            ("name", UserValue::Str("Eve".into())),
            ("age", UserValue::Int(28)),
            ("status", UserValue::Str("deleted".into())),
            ("score", UserValue::Int(70)),
        ],
    ];

    let interner = table.interner().get().await.unwrap();
    for fields in &users {
        let mut map = new_map();
        for (k, v) in fields {
            map.insert(k.to_string(), v.clone());
        }
        let user_val = UserValue::Map(map);
        let result = transform::user_to_inner(&user_val, &interner);
        if let Some(ref new_keys) = result.new_keys {
            table.interner().save_new_keys(new_keys).await.unwrap();
        }
        table.insert(&result.inner_value).await.unwrap();
    }

    table
}

/// Parse JSON string → Filter
fn parse_filter(json: &str) -> crate::query::filter::Filter {
    let value: QueryValue = serde_json::from_str(json).expect("Invalid JSON");
    filter_from_value(&value).expect("Invalid filter")
}

/// Extract name strings from filtered results using the interner
async fn extract_names(
    table: &crate::table::TableManager,
    results: &[(shamir_types::types::record_id::RecordId, shamir_types::types::value::InnerValue)],
) -> Vec<String> {
    let interner = table.interner().get().await.unwrap();
    let name_key = interner.get_ind("name").unwrap();
    let mut names = Vec::new();
    for (_id, record) in results {
        if let shamir_types::types::value::InnerValue::Map(map) = record {
            if let Some(shamir_types::types::value::InnerValue::Str(name)) = map.get(&name_key) {
                names.push(name.clone());
            }
        }
    }
    names.sort();
    names
}

// ============================================================================
// Eq
// ============================================================================

#[tokio::test]
async fn test_filter_stream_eq_status_active() {
    let table = setup_table_with_users().await;
    let refs = new_map();
    let interner = table.interner().get().await.unwrap();
    let ctx = FilterContext::new(&interner, &refs);

    let filter = parse_filter(r#"{"op": "eq", "field": "status", "value": "active"}"#);
    let results = collect_filter_stream(table.filter_stream(100, &filter, &ctx).await.unwrap()).await.unwrap();

    assert_eq!(results.len(), 3);
    let names = extract_names(&table, &results).await;
    assert_eq!(names, vec!["Alice", "Bob", "Dave"]);
}

#[tokio::test]
async fn test_filter_stream_eq_no_match() {
    let table = setup_table_with_users().await;
    let refs = new_map();
    let interner = table.interner().get().await.unwrap();
    let ctx = FilterContext::new(&interner, &refs);

    let filter = parse_filter(r#"{"op": "eq", "field": "status", "value": "banned"}"#);
    let results = collect_filter_stream(table.filter_stream(100, &filter, &ctx).await.unwrap()).await.unwrap();

    assert_eq!(results.len(), 0);
}

// ============================================================================
// Gt, Lt, Gte, Lte
// ============================================================================

#[tokio::test]
async fn test_filter_stream_gt_age() {
    let table = setup_table_with_users().await;
    let refs = new_map();
    let interner = table.interner().get().await.unwrap();
    let ctx = FilterContext::new(&interner, &refs);

    let filter = parse_filter(r#"{"op": "gt", "field": "age", "value": 28}"#);
    let results = collect_filter_stream(table.filter_stream(100, &filter, &ctx).await.unwrap()).await.unwrap();

    // Alice(30), Carol(35)
    assert_eq!(results.len(), 2);
    let names = extract_names(&table, &results).await;
    assert_eq!(names, vec!["Alice", "Carol"]);
}

#[tokio::test]
async fn test_filter_stream_lte_age() {
    let table = setup_table_with_users().await;
    let refs = new_map();
    let interner = table.interner().get().await.unwrap();
    let ctx = FilterContext::new(&interner, &refs);

    let filter = parse_filter(r#"{"op": "lte", "field": "age", "value": 25}"#);
    let results = collect_filter_stream(table.filter_stream(100, &filter, &ctx).await.unwrap()).await.unwrap();

    // Bob(25), Dave(22)
    assert_eq!(results.len(), 2);
    let names = extract_names(&table, &results).await;
    assert_eq!(names, vec!["Bob", "Dave"]);
}

// ============================================================================
// And
// ============================================================================

#[tokio::test]
async fn test_filter_stream_and() {
    let table = setup_table_with_users().await;
    let refs = new_map();
    let interner = table.interner().get().await.unwrap();
    let ctx = FilterContext::new(&interner, &refs);

    let json = r#"{
        "op": "and",
        "filters": [
            {"op": "eq", "field": "status", "value": "active"},
            {"op": "gte", "field": "score", "value": 60}
        ]
    }"#;
    let filter = parse_filter(json);
    let results = collect_filter_stream(table.filter_stream(100, &filter, &ctx).await.unwrap()).await.unwrap();

    // Alice(active, 95), Bob(active, 60) — Dave(active, 45) excluded by score
    assert_eq!(results.len(), 2);
    let names = extract_names(&table, &results).await;
    assert_eq!(names, vec!["Alice", "Bob"]);
}

// ============================================================================
// Or
// ============================================================================

#[tokio::test]
async fn test_filter_stream_or() {
    let table = setup_table_with_users().await;
    let refs = new_map();
    let interner = table.interner().get().await.unwrap();
    let ctx = FilterContext::new(&interner, &refs);

    let json = r#"{
        "op": "or",
        "filters": [
            {"op": "eq", "field": "status", "value": "deleted"},
            {"op": "gt", "field": "age", "value": 34}
        ]
    }"#;
    let filter = parse_filter(json);
    let results = collect_filter_stream(table.filter_stream(100, &filter, &ctx).await.unwrap()).await.unwrap();

    // Eve(deleted), Carol(35)
    assert_eq!(results.len(), 2);
    let names = extract_names(&table, &results).await;
    assert_eq!(names, vec!["Carol", "Eve"]);
}

// ============================================================================
// Not
// ============================================================================

#[tokio::test]
async fn test_filter_stream_not() {
    let table = setup_table_with_users().await;
    let refs = new_map();
    let interner = table.interner().get().await.unwrap();
    let ctx = FilterContext::new(&interner, &refs);

    let json = r#"{
        "op": "not",
        "filter": {"op": "eq", "field": "status", "value": "active"}
    }"#;
    let filter = parse_filter(json);
    let results = collect_filter_stream(table.filter_stream(100, &filter, &ctx).await.unwrap()).await.unwrap();

    // Carol(inactive), Eve(deleted)
    assert_eq!(results.len(), 2);
    let names = extract_names(&table, &results).await;
    assert_eq!(names, vec!["Carol", "Eve"]);
}

// ============================================================================
// Ne
// ============================================================================

#[tokio::test]
async fn test_filter_stream_ne() {
    let table = setup_table_with_users().await;
    let refs = new_map();
    let interner = table.interner().get().await.unwrap();
    let ctx = FilterContext::new(&interner, &refs);

    let filter = parse_filter(r#"{"op": "ne", "field": "status", "value": "active"}"#);
    let results = collect_filter_stream(table.filter_stream(100, &filter, &ctx).await.unwrap()).await.unwrap();

    // Carol(inactive), Eve(deleted)
    assert_eq!(results.len(), 2);
    let names = extract_names(&table, &results).await;
    assert_eq!(names, vec!["Carol", "Eve"]);
}

// ============================================================================
// Nested: And + Or
// ============================================================================

#[tokio::test]
async fn test_filter_stream_nested_and_or() {
    let table = setup_table_with_users().await;
    let refs = new_map();
    let interner = table.interner().get().await.unwrap();
    let ctx = FilterContext::new(&interner, &refs);

    // (status == "active" AND score >= 60) OR (status == "inactive")
    let json = r#"{
        "op": "or",
        "filters": [
            {
                "op": "and",
                "filters": [
                    {"op": "eq", "field": "status", "value": "active"},
                    {"op": "gte", "field": "score", "value": 60}
                ]
            },
            {"op": "eq", "field": "status", "value": "inactive"}
        ]
    }"#;
    let filter = parse_filter(json);
    let results = collect_filter_stream(table.filter_stream(100, &filter, &ctx).await.unwrap()).await.unwrap();

    // Alice(active,95), Bob(active,60), Carol(inactive)
    assert_eq!(results.len(), 3);
    let names = extract_names(&table, &results).await;
    assert_eq!(names, vec!["Alice", "Bob", "Carol"]);
}

// ============================================================================
// Triple nesting: Not + Or + And
// ============================================================================

#[tokio::test]
async fn test_filter_stream_triple_nesting() {
    let table = setup_table_with_users().await;
    let refs = new_map();
    let interner = table.interner().get().await.unwrap();
    let ctx = FilterContext::new(&interner, &refs);

    // NOT ( (status == "deleted") OR (status == "inactive" AND score < 90) )
    // Excluded: Eve(deleted), Carol(inactive, score 80 < 90)
    // Remaining: Alice, Bob, Dave
    let json = r#"{
        "op": "not",
        "filter": {
            "op": "or",
            "filters": [
                {"op": "eq", "field": "status", "value": "deleted"},
                {
                    "op": "and",
                    "filters": [
                        {"op": "eq", "field": "status", "value": "inactive"},
                        {"op": "lt", "field": "score", "value": 90}
                    ]
                }
            ]
        }
    }"#;
    let filter = parse_filter(json);
    let results = collect_filter_stream(table.filter_stream(100, &filter, &ctx).await.unwrap()).await.unwrap();

    assert_eq!(results.len(), 3);
    let names = extract_names(&table, &results).await;
    assert_eq!(names, vec!["Alice", "Bob", "Dave"]);
}

// ============================================================================
// Small batch size (tests multi-batch streaming)
// ============================================================================

#[tokio::test]
async fn test_filter_stream_small_batches() {
    let table = setup_table_with_users().await;
    let refs = new_map();
    let interner = table.interner().get().await.unwrap();
    let ctx = FilterContext::new(&interner, &refs);

    let filter = parse_filter(r#"{"op": "eq", "field": "status", "value": "active"}"#);
    // batch_size=2 forces multiple iterations over 5 records
    let results = collect_filter_stream(table.filter_stream(2, &filter, &ctx).await.unwrap()).await.unwrap();

    assert_eq!(results.len(), 3);
    let names = extract_names(&table, &results).await;
    assert_eq!(names, vec!["Alice", "Bob", "Dave"]);
}

// ============================================================================
// QueryRef in filter_stream
// ============================================================================

#[tokio::test]
async fn test_filter_stream_with_query_ref() {
    let table = setup_table_with_users().await;

    // Simulated resolved query result: a query "threshold" returned [{min_score: 70}]
    let mut refs = new_map();
    refs.insert(
        "threshold".to_string(),
        QueryResult {
            records: vec![serde_json::json!({"min_score": 70})],
            stats: None,
            pagination: None,
        },
    );

    let interner = table.interner().get().await.unwrap();
    let ctx = FilterContext::new(&interner, &refs);

    // score >= $query("@threshold[0].min_score")
    let json = r#"{
        "op": "gte",
        "field": "score",
        "value": {"$query": "@threshold[0].min_score"}
    }"#;
    let filter = parse_filter(json);
    let results = collect_filter_stream(table.filter_stream(100, &filter, &ctx).await.unwrap()).await.unwrap();

    // Alice(95), Carol(80), Eve(70)
    assert_eq!(results.len(), 3);
    let names = extract_names(&table, &results).await;
    assert_eq!(names, vec!["Alice", "Carol", "Eve"]);
}

// ============================================================================
// All records (no filter match → empty)
// ============================================================================

#[tokio::test]
async fn test_filter_stream_all_excluded() {
    let table = setup_table_with_users().await;
    let refs = new_map();
    let interner = table.interner().get().await.unwrap();
    let ctx = FilterContext::new(&interner, &refs);

    // age > 100 — nobody
    let filter = parse_filter(r#"{"op": "gt", "field": "age", "value": 100}"#);
    let results = collect_filter_stream(table.filter_stream(100, &filter, &ctx).await.unwrap()).await.unwrap();

    assert_eq!(results.len(), 0);
}

// ============================================================================
// All records match
// ============================================================================

#[tokio::test]
async fn test_filter_stream_all_match() {
    let table = setup_table_with_users().await;
    let refs = new_map();
    let interner = table.interner().get().await.unwrap();
    let ctx = FilterContext::new(&interner, &refs);

    // age > 0 — everyone
    let filter = parse_filter(r#"{"op": "gt", "field": "age", "value": 0}"#);
    let results = collect_filter_stream(table.filter_stream(100, &filter, &ctx).await.unwrap()).await.unwrap();

    assert_eq!(results.len(), 5);
}

// ============================================================================
// Complex nested filters with QueryRef
// ============================================================================

#[tokio::test]
async fn test_filter_stream_query_ref_in_and() {
    let table = setup_table_with_users().await;

    // Two resolved queries:
    //   "config"  -> [{min_age: 25}]
    //   "scoring" -> [{cutoff: 60}]
    let mut refs = new_map();
    refs.insert(
        "config".to_string(),
        QueryResult {
            records: vec![serde_json::json!({"min_age": 25})],
            stats: None,
            pagination: None,
        },
    );
    refs.insert(
        "scoring".to_string(),
        QueryResult {
            records: vec![serde_json::json!({"cutoff": 60})],
            stats: None,
            pagination: None,
        },
    );

    let interner = table.interner().get().await.unwrap();
    let ctx = FilterContext::new(&interner, &refs);

    // age >= @config[0].min_age AND score >= @scoring[0].cutoff
    let json = r#"{
        "op": "and",
        "filters": [
            {"op": "gte", "field": "age", "value": {"$query": "@config[0].min_age"}},
            {"op": "gte", "field": "score", "value": {"$query": "@scoring[0].cutoff"}}
        ]
    }"#;
    let filter = parse_filter(json);
    let results = collect_filter_stream(table.filter_stream(100, &filter, &ctx).await.unwrap()).await.unwrap();

    // Alice(30,95), Bob(25,60), Carol(35,80), Eve(28,70)
    // Dave excluded: age 22 < 25
    assert_eq!(results.len(), 4);
    let names = extract_names(&table, &results).await;
    assert_eq!(names, vec!["Alice", "Bob", "Carol", "Eve"]);
}

#[tokio::test]
async fn test_filter_stream_query_ref_in_or_with_literal() {
    let table = setup_table_with_users().await;

    // "vip_list" -> [{name: "Carol"}]
    let mut refs = new_map();
    refs.insert(
        "vip_list".to_string(),
        QueryResult {
            records: vec![serde_json::json!({"score_threshold": 90})],
            stats: None,
            pagination: None,
        },
    );

    let interner = table.interner().get().await.unwrap();
    let ctx = FilterContext::new(&interner, &refs);

    // (score >= @vip_list[0].score_threshold) OR (status == "deleted")
    let json = r#"{
        "op": "or",
        "filters": [
            {"op": "gte", "field": "score", "value": {"$query": "@vip_list[0].score_threshold"}},
            {"op": "eq", "field": "status", "value": "deleted"}
        ]
    }"#;
    let filter = parse_filter(json);
    let results = collect_filter_stream(table.filter_stream(100, &filter, &ctx).await.unwrap()).await.unwrap();

    // Alice(score 95 >= 90), Eve(deleted)
    assert_eq!(results.len(), 2);
    let names = extract_names(&table, &results).await;
    assert_eq!(names, vec!["Alice", "Eve"]);
}

#[tokio::test]
async fn test_filter_stream_not_query_ref() {
    let table = setup_table_with_users().await;

    // "limits" -> [{max_age: 30}]
    let mut refs = new_map();
    refs.insert(
        "limits".to_string(),
        QueryResult {
            records: vec![serde_json::json!({"max_age": 30})],
            stats: None,
            pagination: None,
        },
    );

    let interner = table.interner().get().await.unwrap();
    let ctx = FilterContext::new(&interner, &refs);

    // NOT (age > @limits[0].max_age)
    // age > 30 is only Carol(35), so NOT gives everyone except Carol
    let json = r#"{
        "op": "not",
        "filter": {"op": "gt", "field": "age", "value": {"$query": "@limits[0].max_age"}}
    }"#;
    let filter = parse_filter(json);
    let results = collect_filter_stream(table.filter_stream(100, &filter, &ctx).await.unwrap()).await.unwrap();

    assert_eq!(results.len(), 4);
    let names = extract_names(&table, &results).await;
    assert_eq!(names, vec!["Alice", "Bob", "Dave", "Eve"]);
}

#[tokio::test]
async fn test_filter_stream_deep_nesting_with_multiple_query_refs() {
    let table = setup_table_with_users().await;

    // "age_range" -> [{min: 24, max: 31}]
    // "status_config" -> [{allowed: "active"}]  (used as literal comparison)
    let mut refs = new_map();
    refs.insert(
        "age_range".to_string(),
        QueryResult {
            records: vec![serde_json::json!({"min": 24, "max": 31})],
            stats: None,
            pagination: None,
        },
    );
    refs.insert(
        "status_config".to_string(),
        QueryResult {
            records: vec![serde_json::json!({"allowed": "active"})],
            stats: None,
            pagination: None,
        },
    );

    let interner = table.interner().get().await.unwrap();
    let ctx = FilterContext::new(&interner, &refs);

    // AND [
    //   status == @status_config[0].allowed,      -- must be "active"
    //   OR [
    //     age >= @age_range[0].min AND age <= @age_range[0].max,  -- 24..31
    //     NOT(score < 90)                          -- score >= 90
    //   ]
    // ]
    let json = r#"{
        "op": "and",
        "filters": [
            {
                "op": "eq",
                "field": "status",
                "value": {"$query": "@status_config[0].allowed"}
            },
            {
                "op": "or",
                "filters": [
                    {
                        "op": "and",
                        "filters": [
                            {"op": "gte", "field": "age", "value": {"$query": "@age_range[0].min"}},
                            {"op": "lte", "field": "age", "value": {"$query": "@age_range[0].max"}}
                        ]
                    },
                    {
                        "op": "not",
                        "filter": {"op": "lt", "field": "score", "value": 90}
                    }
                ]
            }
        ]
    }"#;
    let filter = parse_filter(json);
    let results = collect_filter_stream(table.filter_stream(100, &filter, &ctx).await.unwrap()).await.unwrap();

    // Must be active: Alice(30), Bob(25), Dave(22)
    // Then OR branch:
    //   age in 24..31: Alice(30), Bob(25) — Dave(22) excluded
    //   OR score >= 90: Alice(95) — already in, Bob(60) no, Dave(45) no
    // Result: Alice(both branches), Bob(age branch)
    assert_eq!(results.len(), 2);
    let names = extract_names(&table, &results).await;
    assert_eq!(names, vec!["Alice", "Bob"]);
}

#[tokio::test]
async fn test_filter_stream_query_ref_missing_graceful() {
    let table = setup_table_with_users().await;

    // Empty refs — query ref can't resolve
    let refs = new_map();
    let interner = table.interner().get().await.unwrap();
    let ctx = FilterContext::new(&interner, &refs);

    // age >= @nonexistent[0].value — should not match anything (unresolvable ref)
    let json = r#"{
        "op": "gte",
        "field": "age",
        "value": {"$query": "@nonexistent[0].value"}
    }"#;
    let filter = parse_filter(json);
    let results = collect_filter_stream(table.filter_stream(100, &filter, &ctx).await.unwrap()).await.unwrap();

    assert_eq!(results.len(), 0);
}

#[tokio::test]
async fn test_filter_stream_mixed_query_ref_field_ref_literal() {
    let table = setup_table_with_users().await;

    // "bonus" -> [{threshold: 50}]
    let mut refs = new_map();
    refs.insert(
        "bonus".to_string(),
        QueryResult {
            records: vec![serde_json::json!({"threshold": 50})],
            stats: None,
            pagination: None,
        },
    );

    let interner = table.interner().get().await.unwrap();
    let ctx = FilterContext::new(&interner, &refs);

    // AND [
    //   score >= @bonus[0].threshold,   -- QueryRef: score >= 50
    //   status != "deleted",            -- literal
    //   age > 23                        -- literal
    // ]
    let json = r#"{
        "op": "and",
        "filters": [
            {"op": "gte", "field": "score", "value": {"$query": "@bonus[0].threshold"}},
            {"op": "ne", "field": "status", "value": "deleted"},
            {"op": "gt", "field": "age", "value": 23}
        ]
    }"#;
    let filter = parse_filter(json);
    let results = collect_filter_stream(table.filter_stream(100, &filter, &ctx).await.unwrap()).await.unwrap();

    // score>=50: Alice(95), Bob(60), Carol(80), Eve(70) — Dave(45) out
    // status!="deleted": Alice, Bob, Carol — Eve out
    // age>23: Alice(30), Bob(25), Carol(35) — all pass
    assert_eq!(results.len(), 3);
    let names = extract_names(&table, &results).await;
    assert_eq!(names, vec!["Alice", "Bob", "Carol"]);
}

// ============================================================================
// In / NotIn with literals
// ============================================================================

#[tokio::test]
async fn test_filter_stream_in_literals() {
    let table = setup_table_with_users().await;
    let refs = new_map();
    let interner = table.interner().get().await.unwrap();
    let ctx = FilterContext::new(&interner, &refs);

    let json = r#"{
        "op": "in",
        "field": "status",
        "values": ["active", "inactive"]
    }"#;
    let filter = parse_filter(json);
    let results = collect_filter_stream(table.filter_stream(100, &filter, &ctx).await.unwrap()).await.unwrap();

    // Alice(active), Bob(active), Carol(inactive), Dave(active) — Eve(deleted) excluded
    assert_eq!(results.len(), 4);
    let names = extract_names(&table, &results).await;
    assert_eq!(names, vec!["Alice", "Bob", "Carol", "Dave"]);
}

#[tokio::test]
async fn test_filter_stream_not_in_literals() {
    let table = setup_table_with_users().await;
    let refs = new_map();
    let interner = table.interner().get().await.unwrap();
    let ctx = FilterContext::new(&interner, &refs);

    let json = r#"{
        "op": "not_in",
        "field": "status",
        "values": ["deleted", "inactive"]
    }"#;
    let filter = parse_filter(json);
    let results = collect_filter_stream(table.filter_stream(100, &filter, &ctx).await.unwrap()).await.unwrap();

    // Alice(active), Bob(active), Dave(active)
    assert_eq!(results.len(), 3);
    let names = extract_names(&table, &results).await;
    assert_eq!(names, vec!["Alice", "Bob", "Dave"]);
}

// ============================================================================
// In with QueryRef column selector [].field
// ============================================================================

#[tokio::test]
async fn test_filter_stream_in_query_ref_column() {
    let table = setup_table_with_users().await;

    // "whitelist" query returned [{status: "active"}, {status: "inactive"}]
    let mut refs = new_map();
    refs.insert(
        "whitelist".to_string(),
        QueryResult {
            records: vec![
                serde_json::json!({"status": "active"}),
                serde_json::json!({"status": "inactive"}),
            ],
            stats: None,
            pagination: None,
        },
    );

    let interner = table.interner().get().await.unwrap();
    let ctx = FilterContext::new(&interner, &refs);

    // status IN @whitelist[].status
    let json = r#"{
        "op": "in",
        "field": "status",
        "values": {"$query": "@whitelist[].status"}
    }"#;
    let filter = parse_filter(json);
    let results = collect_filter_stream(table.filter_stream(100, &filter, &ctx).await.unwrap()).await.unwrap();

    // active: Alice, Bob, Dave; inactive: Carol; deleted Eve excluded
    assert_eq!(results.len(), 4);
    let names = extract_names(&table, &results).await;
    assert_eq!(names, vec!["Alice", "Bob", "Carol", "Dave"]);
}

#[tokio::test]
async fn test_filter_stream_not_in_query_ref_column() {
    let table = setup_table_with_users().await;

    // "exclude_scores" query returned scores to exclude
    let mut refs = new_map();
    refs.insert(
        "exclude_scores".to_string(),
        QueryResult {
            records: vec![
                serde_json::json!({"val": 45}),
                serde_json::json!({"val": 70}),
            ],
            stats: None,
            pagination: None,
        },
    );

    let interner = table.interner().get().await.unwrap();
    let ctx = FilterContext::new(&interner, &refs);

    // score NOT IN @exclude_scores[].val
    let json = r#"{
        "op": "not_in",
        "field": "score",
        "values": {"$query": "@exclude_scores[].val"}
    }"#;
    let filter = parse_filter(json);
    let results = collect_filter_stream(table.filter_stream(100, &filter, &ctx).await.unwrap()).await.unwrap();

    // Excluded: Dave(45), Eve(70). Remaining: Alice(95), Bob(60), Carol(80)
    assert_eq!(results.len(), 3);
    let names = extract_names(&table, &results).await;
    assert_eq!(names, vec!["Alice", "Bob", "Carol"]);
}

// ============================================================================
// In with QueryRef column + nested logic
// ============================================================================

#[tokio::test]
async fn test_filter_stream_in_query_ref_nested_and() {
    let table = setup_table_with_users().await;

    // "allowed_statuses" -> [{s: "active"}, {s: "inactive"}]
    // "min_scores" -> [{threshold: 60}]
    let mut refs = new_map();
    refs.insert(
        "allowed_statuses".to_string(),
        QueryResult {
            records: vec![
                serde_json::json!({"s": "active"}),
                serde_json::json!({"s": "inactive"}),
            ],
            stats: None,
            pagination: None,
        },
    );
    refs.insert(
        "min_scores".to_string(),
        QueryResult {
            records: vec![serde_json::json!({"threshold": 60})],
            stats: None,
            pagination: None,
        },
    );

    let interner = table.interner().get().await.unwrap();
    let ctx = FilterContext::new(&interner, &refs);

    // AND [
    //   status IN @allowed_statuses[].s,
    //   score >= @min_scores[0].threshold
    // ]
    let json = r#"{
        "op": "and",
        "filters": [
            {"op": "in", "field": "status", "values": {"$query": "@allowed_statuses[].s"}},
            {"op": "gte", "field": "score", "value": {"$query": "@min_scores[0].threshold"}}
        ]
    }"#;
    let filter = parse_filter(json);
    let results = collect_filter_stream(table.filter_stream(100, &filter, &ctx).await.unwrap()).await.unwrap();

    // status in [active, inactive]: Alice(95), Bob(60), Carol(80), Dave(45)
    // score >= 60: Alice(95), Bob(60), Carol(80)
    assert_eq!(results.len(), 3);
    let names = extract_names(&table, &results).await;
    assert_eq!(names, vec!["Alice", "Bob", "Carol"]);
}

#[tokio::test]
async fn test_filter_stream_not_in_query_ref_with_or() {
    let table = setup_table_with_users().await;

    // "blacklist" -> [{n: "Dave"}, {n: "Eve"}]
    let mut refs = new_map();
    refs.insert(
        "blacklist".to_string(),
        QueryResult {
            records: vec![
                serde_json::json!({"n": "Dave"}),
                serde_json::json!({"n": "Eve"}),
            ],
            stats: None,
            pagination: None,
        },
    );

    let interner = table.interner().get().await.unwrap();
    let ctx = FilterContext::new(&interner, &refs);

    // OR [
    //   name NOT IN @blacklist[].n,      -- excludes Dave and Eve
    //   score > 90                        -- only Alice(95)
    // ]
    // NOT IN branch: Alice, Bob, Carol
    // score > 90 branch: Alice
    // Union: Alice, Bob, Carol
    let json = r#"{
        "op": "or",
        "filters": [
            {"op": "not_in", "field": "name", "values": {"$query": "@blacklist[].n"}},
            {"op": "gt", "field": "score", "value": 90}
        ]
    }"#;
    let filter = parse_filter(json);
    let results = collect_filter_stream(table.filter_stream(100, &filter, &ctx).await.unwrap()).await.unwrap();

    assert_eq!(results.len(), 3);
    let names = extract_names(&table, &results).await;
    assert_eq!(names, vec!["Alice", "Bob", "Carol"]);
}
