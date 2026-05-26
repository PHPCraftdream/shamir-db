//! Tests for filter evaluation (callback network).

use crate::query::filter::eval::{compare_values, compile_filter, resolve_field};
use crate::query::filter::eval_context::FilterContext;
use crate::query::filter::{Filter, FilterValue};
use crate::query::read::QueryResult;
use shamir_types::core::interner::Interner;
use shamir_types::types::common::{new_map, new_set, TMap};
use shamir_types::types::value::InnerValue;

// ============================================================================
// Helpers
// ============================================================================

/// Build a test record: {name: "Alice", age: 30, status: "active"}
fn make_alice_record(interner: &Interner) -> InnerValue {
    let mut map = new_map();
    let k_name = interner.touch_ind("name").unwrap().key().clone();
    let k_age = interner.touch_ind("age").unwrap().key().clone();
    let k_status = interner.touch_ind("status").unwrap().key().clone();
    map.insert(k_name, InnerValue::Str("Alice".to_string()));
    map.insert(k_age, InnerValue::Int(30));
    map.insert(k_status, InnerValue::Str("active".to_string()));
    InnerValue::Map(map)
}

/// Build a record with nested fields: {user: {name: "Bob", score: 85}}
fn make_nested_record(interner: &Interner) -> InnerValue {
    let mut inner = new_map();
    let k_name = interner.touch_ind("name").unwrap().key().clone();
    let k_score = interner.touch_ind("score").unwrap().key().clone();
    inner.insert(k_name, InnerValue::Str("Bob".to_string()));
    inner.insert(k_score, InnerValue::Int(85));

    let mut outer = new_map();
    let k_user = interner.touch_ind("user").unwrap().key().clone();
    outer.insert(k_user, InnerValue::Map(inner));
    InnerValue::Map(outer)
}

/// Build a record with nullable field: {name: "Carol", deleted_at: Null}
fn make_nullable_record(interner: &Interner) -> InnerValue {
    let mut map = new_map();
    let k_name = interner.touch_ind("name").unwrap().key().clone();
    let k_deleted = interner.touch_ind("deleted_at").unwrap().key().clone();
    map.insert(k_name, InnerValue::Str("Carol".to_string()));
    map.insert(k_deleted, InnerValue::Null);
    InnerValue::Map(map)
}

/// Build a record with start/end dates: {start_date: 100, end_date: 200}
fn make_date_record(interner: &Interner) -> InnerValue {
    let mut map = new_map();
    let k_start = interner.touch_ind("start_date").unwrap().key().clone();
    let k_end = interner.touch_ind("end_date").unwrap().key().clone();
    map.insert(k_start, InnerValue::Int(100));
    map.insert(k_end, InnerValue::Int(200));
    InnerValue::Map(map)
}

fn empty_refs() -> TMap<String, QueryResult> {
    new_map()
}

// ============================================================================
// Step 1: resolve_field + compare_values
// ============================================================================

#[test]
fn test_resolve_field_simple() {
    let interner = Interner::new();
    let record = make_alice_record(&interner);
    let k_name = interner.get_ind("name").unwrap().id();

    let val = resolve_field(&record, &[k_name]);
    assert_eq!(val, Some(InnerValue::Str("Alice".to_string())));
}

#[test]
fn test_resolve_field_nested() {
    let interner = Interner::new();
    let record = make_nested_record(&interner);
    let k_user = interner.get_ind("user").unwrap().id();
    let k_name = interner.get_ind("name").unwrap().id();

    let val = resolve_field(&record, &[k_user, k_name]);
    assert_eq!(val, Some(InnerValue::Str("Bob".to_string())));
}

#[test]
fn test_resolve_field_missing() {
    let interner = Interner::new();
    let record = make_alice_record(&interner);
    let k_missing = interner
        .touch_ind("nonexistent")
        .unwrap()
        .key()
        .clone()
        .id();

    let val = resolve_field(&record, &[k_missing]);
    assert_eq!(val, None);
}

#[test]
fn test_resolve_field_empty_path() {
    let interner = Interner::new();
    let record = make_alice_record(&interner);
    let val = resolve_field(&record, &[]);
    assert!(val.is_some());
}

#[test]
fn test_compare_values_int() {
    use std::cmp::Ordering;
    assert_eq!(
        compare_values(&InnerValue::Int(10), &InnerValue::Int(20)),
        Some(Ordering::Less)
    );
    assert_eq!(
        compare_values(&InnerValue::Int(20), &InnerValue::Int(20)),
        Some(Ordering::Equal)
    );
    assert_eq!(
        compare_values(&InnerValue::Int(30), &InnerValue::Int(20)),
        Some(Ordering::Greater)
    );
}

#[test]
fn test_compare_values_str() {
    use std::cmp::Ordering;
    assert_eq!(
        compare_values(
            &InnerValue::Str("abc".into()),
            &InnerValue::Str("def".into())
        ),
        Some(Ordering::Less)
    );
    assert_eq!(
        compare_values(
            &InnerValue::Str("abc".into()),
            &InnerValue::Str("abc".into())
        ),
        Some(Ordering::Equal)
    );
}

#[test]
fn test_compare_values_float() {
    use std::cmp::Ordering;
    assert_eq!(
        compare_values(&InnerValue::F64(1.0), &InnerValue::F64(2.0)),
        Some(Ordering::Less)
    );
}

#[test]
fn test_compare_values_int_float_cross() {
    use std::cmp::Ordering;
    assert_eq!(
        compare_values(&InnerValue::Int(10), &InnerValue::F64(10.5)),
        Some(Ordering::Less)
    );
}

#[test]
fn test_compare_values_null() {
    use std::cmp::Ordering;
    assert_eq!(
        compare_values(&InnerValue::Null, &InnerValue::Null),
        Some(Ordering::Equal)
    );
}

#[test]
fn test_compare_values_incompatible() {
    assert_eq!(
        compare_values(&InnerValue::Int(1), &InnerValue::Str("a".into())),
        None
    );
}

// ============================================================================
// Step 2: Basic comparisons (Eq, Ne, Gt, Gte, Lt, Lte)
// ============================================================================

#[test]
fn test_eq_string_match() {
    let interner = Interner::new();
    let record = make_alice_record(&interner);
    let refs = empty_refs();
    let ctx = FilterContext::new(&interner, &refs);

    let filter = Filter::Eq {
        field: vec!["status".to_string()],
        value: FilterValue::String("active".to_string()),
    };
    let cb = compile_filter(&filter, &interner);
    assert!(cb.matches(&record, &ctx));
}

#[test]
fn test_eq_string_no_match() {
    let interner = Interner::new();
    let record = make_alice_record(&interner);
    let refs = empty_refs();
    let ctx = FilterContext::new(&interner, &refs);

    let filter = Filter::Eq {
        field: vec!["status".to_string()],
        value: FilterValue::String("deleted".to_string()),
    };
    let cb = compile_filter(&filter, &interner);
    assert!(!cb.matches(&record, &ctx));
}

#[test]
fn test_gt_int() {
    let interner = Interner::new();
    let record = make_alice_record(&interner);
    let refs = empty_refs();
    let ctx = FilterContext::new(&interner, &refs);

    let filter = Filter::Gt {
        field: vec!["age".to_string()],
        value: FilterValue::Int(25),
    };
    let cb = compile_filter(&filter, &interner);
    assert!(cb.matches(&record, &ctx)); // 30 > 25
}

#[test]
fn test_lt_int_no_match() {
    let interner = Interner::new();
    let record = make_alice_record(&interner);
    let refs = empty_refs();
    let ctx = FilterContext::new(&interner, &refs);

    let filter = Filter::Lt {
        field: vec!["age".to_string()],
        value: FilterValue::Int(25),
    };
    let cb = compile_filter(&filter, &interner);
    assert!(!cb.matches(&record, &ctx)); // 30 < 25 == false
}

#[test]
fn test_gte_int_equal() {
    let interner = Interner::new();
    let record = make_alice_record(&interner);
    let refs = empty_refs();
    let ctx = FilterContext::new(&interner, &refs);

    let filter = Filter::Gte {
        field: vec!["age".to_string()],
        value: FilterValue::Int(30),
    };
    let cb = compile_filter(&filter, &interner);
    assert!(cb.matches(&record, &ctx)); // 30 >= 30
}

#[test]
fn test_lte_int() {
    let interner = Interner::new();
    let record = make_alice_record(&interner);
    let refs = empty_refs();
    let ctx = FilterContext::new(&interner, &refs);

    let filter = Filter::Lte {
        field: vec!["age".to_string()],
        value: FilterValue::Int(30),
    };
    let cb = compile_filter(&filter, &interner);
    assert!(cb.matches(&record, &ctx)); // 30 <= 30
}

#[test]
fn test_ne_string() {
    let interner = Interner::new();
    let record = make_alice_record(&interner);
    let refs = empty_refs();
    let ctx = FilterContext::new(&interner, &refs);

    let filter = Filter::Ne {
        field: vec!["status".to_string()],
        value: FilterValue::String("deleted".to_string()),
    };
    let cb = compile_filter(&filter, &interner);
    assert!(cb.matches(&record, &ctx)); // "active" != "deleted"
}

// ============================================================================
// Step 3: Logical operators (And, Or, Not)
// ============================================================================

#[test]
fn test_and_both_true() {
    let interner = Interner::new();
    let record = make_alice_record(&interner);
    let refs = empty_refs();
    let ctx = FilterContext::new(&interner, &refs);

    let filter = Filter::And {
        filters: vec![
            Filter::Eq {
                field: vec!["status".to_string()],
                value: FilterValue::String("active".to_string()),
            },
            Filter::Gt {
                field: vec!["age".to_string()],
                value: FilterValue::Int(25),
            },
        ],
    };
    let cb = compile_filter(&filter, &interner);
    assert!(cb.matches(&record, &ctx));
}

#[test]
fn test_and_one_false() {
    let interner = Interner::new();
    let record = make_alice_record(&interner);
    let refs = empty_refs();
    let ctx = FilterContext::new(&interner, &refs);

    let filter = Filter::And {
        filters: vec![
            Filter::Eq {
                field: vec!["status".to_string()],
                value: FilterValue::String("deleted".to_string()),
            },
            Filter::Gt {
                field: vec!["age".to_string()],
                value: FilterValue::Int(25),
            },
        ],
    };
    let cb = compile_filter(&filter, &interner);
    assert!(!cb.matches(&record, &ctx));
}

#[test]
fn test_or_one_true() {
    let interner = Interner::new();
    let record = make_alice_record(&interner);
    let refs = empty_refs();
    let ctx = FilterContext::new(&interner, &refs);

    let filter = Filter::Or {
        filters: vec![
            Filter::Eq {
                field: vec!["status".to_string()],
                value: FilterValue::String("deleted".to_string()),
            },
            Filter::Gt {
                field: vec!["age".to_string()],
                value: FilterValue::Int(25),
            },
        ],
    };
    let cb = compile_filter(&filter, &interner);
    assert!(cb.matches(&record, &ctx));
}

#[test]
fn test_or_both_false() {
    let interner = Interner::new();
    let record = make_alice_record(&interner);
    let refs = empty_refs();
    let ctx = FilterContext::new(&interner, &refs);

    let filter = Filter::Or {
        filters: vec![
            Filter::Eq {
                field: vec!["status".to_string()],
                value: FilterValue::String("deleted".to_string()),
            },
            Filter::Lt {
                field: vec!["age".to_string()],
                value: FilterValue::Int(25),
            },
        ],
    };
    let cb = compile_filter(&filter, &interner);
    assert!(!cb.matches(&record, &ctx));
}

#[test]
fn test_not_inverts() {
    let interner = Interner::new();
    let record = make_alice_record(&interner);
    let refs = empty_refs();
    let ctx = FilterContext::new(&interner, &refs);

    let filter = Filter::Not {
        filter: Box::new(Filter::Eq {
            field: vec!["status".to_string()],
            value: FilterValue::String("deleted".to_string()),
        }),
    };
    let cb = compile_filter(&filter, &interner);
    assert!(cb.matches(&record, &ctx)); // NOT (active == deleted) => true
}

// ============================================================================
// Step 4: IsNull, IsNotNull
// ============================================================================

#[test]
fn test_is_null_on_null_field() {
    let interner = Interner::new();
    let record = make_nullable_record(&interner);
    let refs = empty_refs();
    let ctx = FilterContext::new(&interner, &refs);

    let filter = Filter::IsNull {
        field: vec!["deleted_at".to_string()],
    };
    let cb = compile_filter(&filter, &interner);
    assert!(cb.matches(&record, &ctx));
}

#[test]
fn test_is_null_on_existing_field() {
    let interner = Interner::new();
    let record = make_nullable_record(&interner);
    let refs = empty_refs();
    let ctx = FilterContext::new(&interner, &refs);

    let filter = Filter::IsNull {
        field: vec!["name".to_string()],
    };
    let cb = compile_filter(&filter, &interner);
    assert!(!cb.matches(&record, &ctx)); // name = "Carol", not null
}

#[test]
fn test_is_null_on_missing_field() {
    let interner = Interner::new();
    let record = make_nullable_record(&interner);
    let refs = empty_refs();
    let ctx = FilterContext::new(&interner, &refs);

    let filter = Filter::IsNull {
        field: vec!["nonexistent".to_string()],
    };
    // "nonexistent" not in interner yet, so compile treats as always-null
    let cb = compile_filter(&filter, &interner);
    assert!(cb.matches(&record, &ctx));
}

#[test]
fn test_is_not_null_on_existing_field() {
    let interner = Interner::new();
    let record = make_nullable_record(&interner);
    let refs = empty_refs();
    let ctx = FilterContext::new(&interner, &refs);

    let filter = Filter::IsNotNull {
        field: vec!["name".to_string()],
    };
    let cb = compile_filter(&filter, &interner);
    assert!(cb.matches(&record, &ctx));
}

#[test]
fn test_is_not_null_on_null_field() {
    let interner = Interner::new();
    let record = make_nullable_record(&interner);
    let refs = empty_refs();
    let ctx = FilterContext::new(&interner, &refs);

    let filter = Filter::IsNotNull {
        field: vec!["deleted_at".to_string()],
    };
    let cb = compile_filter(&filter, &interner);
    assert!(!cb.matches(&record, &ctx));
}

// ============================================================================
// Step 5: FieldRef
// ============================================================================

#[test]
fn test_field_ref_gt() {
    let interner = Interner::new();
    let record = make_date_record(&interner);
    let refs = empty_refs();
    let ctx = FilterContext::new(&interner, &refs);

    // end_date (200) > start_date (100) => true
    let filter = Filter::Gt {
        field: vec!["end_date".to_string()],
        value: FilterValue::FieldRef {
            path: vec!["start_date".to_string()],
        },
    };
    let cb = compile_filter(&filter, &interner);
    assert!(cb.matches(&record, &ctx));
}

#[test]
fn test_field_ref_lt() {
    let interner = Interner::new();
    let record = make_date_record(&interner);
    let refs = empty_refs();
    let ctx = FilterContext::new(&interner, &refs);

    // start_date (100) < end_date (200) => true
    let filter = Filter::Lt {
        field: vec!["start_date".to_string()],
        value: FilterValue::FieldRef {
            path: vec!["end_date".to_string()],
        },
    };
    let cb = compile_filter(&filter, &interner);
    assert!(cb.matches(&record, &ctx));
}

#[test]
fn test_field_ref_eq_same() {
    let interner = Interner::new();
    let record = make_date_record(&interner);
    let refs = empty_refs();
    let ctx = FilterContext::new(&interner, &refs);

    // start_date == start_date => true
    let filter = Filter::Eq {
        field: vec!["start_date".to_string()],
        value: FilterValue::FieldRef {
            path: vec!["start_date".to_string()],
        },
    };
    let cb = compile_filter(&filter, &interner);
    assert!(cb.matches(&record, &ctx));
}

// ============================================================================
// Step 6: QueryRef
// ============================================================================

#[test]
fn test_query_ref_eq() {
    let interner = Interner::new();

    // Record: {user_id: 42}
    let mut map = new_map();
    let k_user_id = interner.touch_ind("user_id").unwrap().key().clone();
    map.insert(k_user_id, InnerValue::Int(42));
    let record = InnerValue::Map(map);

    // QueryResult: users => [{id: 42, name: "Alice"}]
    let mut refs: TMap<String, QueryResult> = new_map();
    refs.insert(
        "users".to_string(),
        QueryResult {
            records: vec![serde_json::json!({"id": 42, "name": "Alice"})],
            stats: None,
            pagination: None,
        },
    );

    let ctx = FilterContext::new(&interner, &refs);

    let filter = Filter::Eq {
        field: vec!["user_id".to_string()],
        value: FilterValue::QueryRef {
            alias: "users".to_string(),
            path: Some("[0].id".to_string()),
        },
    };
    let cb = compile_filter(&filter, &interner);
    assert!(cb.matches(&record, &ctx));
}

#[test]
fn test_query_ref_no_match() {
    let interner = Interner::new();

    let mut map = new_map();
    let k_user_id = interner.touch_ind("user_id").unwrap().key().clone();
    map.insert(k_user_id, InnerValue::Int(99));
    let record = InnerValue::Map(map);

    let mut refs: TMap<String, QueryResult> = new_map();
    refs.insert(
        "users".to_string(),
        QueryResult {
            records: vec![serde_json::json!({"id": 42})],
            stats: None,
            pagination: None,
        },
    );

    let ctx = FilterContext::new(&interner, &refs);

    let filter = Filter::Eq {
        field: vec!["user_id".to_string()],
        value: FilterValue::QueryRef {
            alias: "users".to_string(),
            path: Some("[0].id".to_string()),
        },
    };
    let cb = compile_filter(&filter, &interner);
    assert!(!cb.matches(&record, &ctx)); // 99 != 42
}

#[test]
fn test_query_ref_missing_alias() {
    let interner = Interner::new();

    let mut map = new_map();
    let k_user_id = interner.touch_ind("user_id").unwrap().key().clone();
    map.insert(k_user_id, InnerValue::Int(42));
    let record = InnerValue::Map(map);

    let refs = empty_refs();
    let ctx = FilterContext::new(&interner, &refs);

    let filter = Filter::Eq {
        field: vec!["user_id".to_string()],
        value: FilterValue::QueryRef {
            alias: "nonexistent".to_string(),
            path: Some("[0].id".to_string()),
        },
    };
    let cb = compile_filter(&filter, &interner);
    // Missing alias => resolve returns None => Eq fails
    assert!(!cb.matches(&record, &ctx));
}

// ============================================================================
// Complex / integration
// ============================================================================

#[test]
fn test_complex_nested_filter() {
    let interner = Interner::new();
    let record = make_alice_record(&interner);
    let refs = empty_refs();
    let ctx = FilterContext::new(&interner, &refs);

    // (status == "active" AND age > 25) OR (status == "vip")
    let filter = Filter::Or {
        filters: vec![
            Filter::And {
                filters: vec![
                    Filter::Eq {
                        field: vec!["status".to_string()],
                        value: FilterValue::String("active".to_string()),
                    },
                    Filter::Gt {
                        field: vec!["age".to_string()],
                        value: FilterValue::Int(25),
                    },
                ],
            },
            Filter::Eq {
                field: vec!["status".to_string()],
                value: FilterValue::String("vip".to_string()),
            },
        ],
    };
    let cb = compile_filter(&filter, &interner);
    assert!(cb.matches(&record, &ctx));
}

#[test]
fn test_nested_field_path_in_filter() {
    let interner = Interner::new();
    let record = make_nested_record(&interner);
    let refs = empty_refs();
    let ctx = FilterContext::new(&interner, &refs);

    let filter = Filter::Eq {
        field: vec!["user".to_string(), "name".to_string()],
        value: FilterValue::String("Bob".to_string()),
    };
    let cb = compile_filter(&filter, &interner);
    assert!(cb.matches(&record, &ctx));
}

#[test]
fn test_nested_field_path_gt() {
    let interner = Interner::new();
    let record = make_nested_record(&interner);
    let refs = empty_refs();
    let ctx = FilterContext::new(&interner, &refs);

    let filter = Filter::Gt {
        field: vec!["user".to_string(), "score".to_string()],
        value: FilterValue::Int(80),
    };
    let cb = compile_filter(&filter, &interner);
    assert!(cb.matches(&record, &ctx)); // 85 > 80
}

// ============================================================================
// Step 7: In / NotIn
// ============================================================================

#[test]
fn test_in_literal_match() {
    let interner = Interner::new();
    let record = make_alice_record(&interner);
    let refs = empty_refs();
    let ctx = FilterContext::new(&interner, &refs);

    let filter = Filter::In {
        field: vec!["status".to_string()],
        values: vec![
            FilterValue::String("active".to_string()),
            FilterValue::String("pending".to_string()),
        ],
    };
    let cb = compile_filter(&filter, &interner);
    assert!(cb.matches(&record, &ctx));
}

#[test]
fn test_in_literal_no_match() {
    let interner = Interner::new();
    let record = make_alice_record(&interner);
    let refs = empty_refs();
    let ctx = FilterContext::new(&interner, &refs);

    let filter = Filter::In {
        field: vec!["status".to_string()],
        values: vec![
            FilterValue::String("deleted".to_string()),
            FilterValue::String("banned".to_string()),
        ],
    };
    let cb = compile_filter(&filter, &interner);
    assert!(!cb.matches(&record, &ctx));
}

#[test]
fn test_not_in_literal() {
    let interner = Interner::new();
    let record = make_alice_record(&interner);
    let refs = empty_refs();
    let ctx = FilterContext::new(&interner, &refs);

    let filter = Filter::NotIn {
        field: vec!["status".to_string()],
        values: vec![
            FilterValue::String("deleted".to_string()),
            FilterValue::String("banned".to_string()),
        ],
    };
    let cb = compile_filter(&filter, &interner);
    assert!(cb.matches(&record, &ctx)); // "active" not in ["deleted", "banned"]
}

#[test]
fn test_in_query_ref_column() {
    let interner = Interner::new();

    // Record: {user_id: 2}
    let mut map = new_map();
    let k = interner.touch_ind("user_id").unwrap().key().clone();
    map.insert(k, InnerValue::Int(2));
    let record = InnerValue::Map(map);

    // Query result: "allowed_users" => [{id: 1}, {id: 2}, {id: 5}]
    let mut refs: TMap<String, QueryResult> = new_map();
    refs.insert(
        "allowed_users".to_string(),
        QueryResult {
            records: vec![
                serde_json::json!({"id": 1}),
                serde_json::json!({"id": 2}),
                serde_json::json!({"id": 5}),
            ],
            stats: None,
            pagination: None,
        },
    );

    let ctx = FilterContext::new(&interner, &refs);

    // user_id IN @allowed_users[].id
    let filter = Filter::In {
        field: vec!["user_id".to_string()],
        values: vec![FilterValue::QueryRef {
            alias: "allowed_users".to_string(),
            path: Some("[].id".to_string()),
        }],
    };
    let cb = compile_filter(&filter, &interner);
    assert!(cb.matches(&record, &ctx)); // 2 is in [1, 2, 5]
}

#[test]
fn test_in_query_ref_column_no_match() {
    let interner = Interner::new();

    let mut map = new_map();
    let k = interner.touch_ind("user_id").unwrap().key().clone();
    map.insert(k, InnerValue::Int(99));
    let record = InnerValue::Map(map);

    let mut refs: TMap<String, QueryResult> = new_map();
    refs.insert(
        "allowed_users".to_string(),
        QueryResult {
            records: vec![serde_json::json!({"id": 1}), serde_json::json!({"id": 2})],
            stats: None,
            pagination: None,
        },
    );

    let ctx = FilterContext::new(&interner, &refs);

    let filter = Filter::In {
        field: vec!["user_id".to_string()],
        values: vec![FilterValue::QueryRef {
            alias: "allowed_users".to_string(),
            path: Some("[].id".to_string()),
        }],
    };
    let cb = compile_filter(&filter, &interner);
    assert!(!cb.matches(&record, &ctx)); // 99 not in [1, 2]
}

#[test]
fn test_not_in_query_ref_column() {
    let interner = Interner::new();

    let mut map = new_map();
    let k = interner.touch_ind("user_id").unwrap().key().clone();
    map.insert(k, InnerValue::Int(99));
    let record = InnerValue::Map(map);

    let mut refs: TMap<String, QueryResult> = new_map();
    refs.insert(
        "blocked".to_string(),
        QueryResult {
            records: vec![serde_json::json!({"id": 1}), serde_json::json!({"id": 2})],
            stats: None,
            pagination: None,
        },
    );

    let ctx = FilterContext::new(&interner, &refs);

    let filter = Filter::NotIn {
        field: vec!["user_id".to_string()],
        values: vec![FilterValue::QueryRef {
            alias: "blocked".to_string(),
            path: Some("[].id".to_string()),
        }],
    };
    let cb = compile_filter(&filter, &interner);
    assert!(cb.matches(&record, &ctx)); // 99 not in [1, 2]
}

// ============================================================================
// Step 8: Like / ILike
// ============================================================================

#[test]
fn test_like_prefix_match() {
    let interner = Interner::new();
    let record = make_alice_record(&interner);
    let refs = empty_refs();
    let ctx = FilterContext::new(&interner, &refs);

    let filter = Filter::Like {
        field: vec!["name".to_string()],
        pattern: "Ali%".to_string(),
    };
    let cb = compile_filter(&filter, &interner);
    assert!(cb.matches(&record, &ctx)); // "Alice" matches "Ali%"
}

#[test]
fn test_like_suffix_match() {
    let interner = Interner::new();
    let record = make_alice_record(&interner);
    let refs = empty_refs();
    let ctx = FilterContext::new(&interner, &refs);

    let filter = Filter::Like {
        field: vec!["name".to_string()],
        pattern: "%ice".to_string(),
    };
    let cb = compile_filter(&filter, &interner);
    assert!(cb.matches(&record, &ctx)); // "Alice" matches "%ice"
}

#[test]
fn test_like_no_match() {
    let interner = Interner::new();
    let record = make_alice_record(&interner);
    let refs = empty_refs();
    let ctx = FilterContext::new(&interner, &refs);

    let filter = Filter::Like {
        field: vec!["name".to_string()],
        pattern: "Bob%".to_string(),
    };
    let cb = compile_filter(&filter, &interner);
    assert!(!cb.matches(&record, &ctx));
}

#[test]
fn test_like_underscore_single_char() {
    let interner = Interner::new();
    let record = make_alice_record(&interner);
    let refs = empty_refs();
    let ctx = FilterContext::new(&interner, &refs);

    let filter = Filter::Like {
        field: vec!["name".to_string()],
        pattern: "Alic_".to_string(),
    };
    let cb = compile_filter(&filter, &interner);
    assert!(cb.matches(&record, &ctx)); // "Alice" matches "Alic_"
}

#[test]
fn test_like_exact_match() {
    let interner = Interner::new();
    let record = make_alice_record(&interner);
    let refs = empty_refs();
    let ctx = FilterContext::new(&interner, &refs);

    let filter = Filter::Like {
        field: vec!["name".to_string()],
        pattern: "Alice".to_string(),
    };
    let cb = compile_filter(&filter, &interner);
    assert!(cb.matches(&record, &ctx));
}

#[test]
fn test_like_case_sensitive() {
    let interner = Interner::new();
    let record = make_alice_record(&interner);
    let refs = empty_refs();
    let ctx = FilterContext::new(&interner, &refs);

    let filter = Filter::Like {
        field: vec!["name".to_string()],
        pattern: "ali%".to_string(),
    };
    let cb = compile_filter(&filter, &interner);
    assert!(!cb.matches(&record, &ctx)); // case-sensitive: "Alice" doesn't match "ali%"
}

#[test]
fn test_ilike_case_insensitive() {
    let interner = Interner::new();
    let record = make_alice_record(&interner);
    let refs = empty_refs();
    let ctx = FilterContext::new(&interner, &refs);

    let filter = Filter::ILike {
        field: vec!["name".to_string()],
        pattern: "ali%".to_string(),
    };
    let cb = compile_filter(&filter, &interner);
    assert!(cb.matches(&record, &ctx)); // case-insensitive: "Alice" matches "ali%"
}

#[test]
fn test_ilike_no_match() {
    let interner = Interner::new();
    let record = make_alice_record(&interner);
    let refs = empty_refs();
    let ctx = FilterContext::new(&interner, &refs);

    let filter = Filter::ILike {
        field: vec!["name".to_string()],
        pattern: "bob%".to_string(),
    };
    let cb = compile_filter(&filter, &interner);
    assert!(!cb.matches(&record, &ctx));
}

// ============================================================================
// Step 9: Regex
// ============================================================================

#[test]
fn test_regex_match() {
    let interner = Interner::new();
    let record = make_alice_record(&interner);
    let refs = empty_refs();
    let ctx = FilterContext::new(&interner, &refs);

    let filter = Filter::Regex {
        field: vec!["name".to_string()],
        pattern: "^A[a-z]+e$".to_string(),
    };
    let cb = compile_filter(&filter, &interner);
    assert!(cb.matches(&record, &ctx)); // "Alice" matches "^A[a-z]+e$"
}

#[test]
fn test_regex_no_match() {
    let interner = Interner::new();
    let record = make_alice_record(&interner);
    let refs = empty_refs();
    let ctx = FilterContext::new(&interner, &refs);

    let filter = Filter::Regex {
        field: vec!["name".to_string()],
        pattern: "^[0-9]+$".to_string(),
    };
    let cb = compile_filter(&filter, &interner);
    assert!(!cb.matches(&record, &ctx));
}

#[test]
fn test_regex_partial_match() {
    let interner = Interner::new();
    let record = make_alice_record(&interner);
    let refs = empty_refs();
    let ctx = FilterContext::new(&interner, &refs);

    // Without anchors, regex matches partially
    let filter = Filter::Regex {
        field: vec!["name".to_string()],
        pattern: "lic".to_string(),
    };
    let cb = compile_filter(&filter, &interner);
    assert!(cb.matches(&record, &ctx)); // "Alice" contains "lic"
}

#[test]
fn test_regex_on_non_string_field() {
    let interner = Interner::new();
    let record = make_alice_record(&interner);
    let refs = empty_refs();
    let ctx = FilterContext::new(&interner, &refs);

    let filter = Filter::Regex {
        field: vec!["age".to_string()],
        pattern: "\\d+".to_string(),
    };
    let cb = compile_filter(&filter, &interner);
    assert!(!cb.matches(&record, &ctx)); // age is Int, not Str
}

// ============================================================================
// Step 10: Contains
// ============================================================================

/// Build a record with a list field: {name: "Test", tags: ["rust", "db", "query"]}
fn make_list_record(interner: &Interner) -> InnerValue {
    let mut map = new_map();
    let k_name = interner.touch_ind("name").unwrap().key().clone();
    let k_tags = interner.touch_ind("tags").unwrap().key().clone();
    map.insert(k_name, InnerValue::Str("Test".to_string()));
    map.insert(
        k_tags,
        InnerValue::List(vec![
            InnerValue::Str("rust".to_string()),
            InnerValue::Str("db".to_string()),
            InnerValue::Str("query".to_string()),
        ]),
    );
    InnerValue::Map(map)
}

/// Build a record with a set field: {name: "Test", roles: {"admin", "user"}}
fn make_set_record(interner: &Interner) -> InnerValue {
    let mut map = new_map();
    let k_name = interner.touch_ind("name").unwrap().key().clone();
    let k_roles = interner.touch_ind("roles").unwrap().key().clone();
    let mut roles = new_set();
    roles.insert(InnerValue::Str("admin".to_string()));
    roles.insert(InnerValue::Str("user".to_string()));
    map.insert(k_name, InnerValue::Str("Test".to_string()));
    map.insert(k_roles, InnerValue::Set(roles));
    InnerValue::Map(map)
}

#[test]
fn test_contains_string_substring() {
    let interner = Interner::new();
    let record = make_alice_record(&interner);
    let refs = empty_refs();
    let ctx = FilterContext::new(&interner, &refs);

    let filter = Filter::Contains {
        field: vec!["name".to_string()],
        value: FilterValue::String("lic".to_string()),
    };
    let cb = compile_filter(&filter, &interner);
    assert!(cb.matches(&record, &ctx)); // "Alice" contains "lic"
}

#[test]
fn test_contains_string_no_match() {
    let interner = Interner::new();
    let record = make_alice_record(&interner);
    let refs = empty_refs();
    let ctx = FilterContext::new(&interner, &refs);

    let filter = Filter::Contains {
        field: vec!["name".to_string()],
        value: FilterValue::String("xyz".to_string()),
    };
    let cb = compile_filter(&filter, &interner);
    assert!(!cb.matches(&record, &ctx));
}

#[test]
fn test_contains_list() {
    let interner = Interner::new();
    let record = make_list_record(&interner);
    let refs = empty_refs();
    let ctx = FilterContext::new(&interner, &refs);

    let filter = Filter::Contains {
        field: vec!["tags".to_string()],
        value: FilterValue::String("rust".to_string()),
    };
    let cb = compile_filter(&filter, &interner);
    assert!(cb.matches(&record, &ctx));
}

#[test]
fn test_contains_list_no_match() {
    let interner = Interner::new();
    let record = make_list_record(&interner);
    let refs = empty_refs();
    let ctx = FilterContext::new(&interner, &refs);

    let filter = Filter::Contains {
        field: vec!["tags".to_string()],
        value: FilterValue::String("python".to_string()),
    };
    let cb = compile_filter(&filter, &interner);
    assert!(!cb.matches(&record, &ctx));
}

#[test]
fn test_contains_set() {
    let interner = Interner::new();
    let record = make_set_record(&interner);
    let refs = empty_refs();
    let ctx = FilterContext::new(&interner, &refs);

    let filter = Filter::Contains {
        field: vec!["roles".to_string()],
        value: FilterValue::String("admin".to_string()),
    };
    let cb = compile_filter(&filter, &interner);
    assert!(cb.matches(&record, &ctx));
}

#[test]
fn test_contains_set_no_match() {
    let interner = Interner::new();
    let record = make_set_record(&interner);
    let refs = empty_refs();
    let ctx = FilterContext::new(&interner, &refs);

    let filter = Filter::Contains {
        field: vec!["roles".to_string()],
        value: FilterValue::String("superadmin".to_string()),
    };
    let cb = compile_filter(&filter, &interner);
    assert!(!cb.matches(&record, &ctx));
}

// ============================================================================
// Step 11: ContainsAny / ContainsAll
// ============================================================================

#[test]
fn test_contains_any_list_match() {
    let interner = Interner::new();
    let record = make_list_record(&interner);
    let refs = empty_refs();
    let ctx = FilterContext::new(&interner, &refs);

    let filter = Filter::ContainsAny {
        field: vec!["tags".to_string()],
        values: vec![
            FilterValue::String("python".to_string()),
            FilterValue::String("rust".to_string()),
        ],
    };
    let cb = compile_filter(&filter, &interner);
    assert!(cb.matches(&record, &ctx)); // tags contains "rust"
}

#[test]
fn test_contains_any_list_no_match() {
    let interner = Interner::new();
    let record = make_list_record(&interner);
    let refs = empty_refs();
    let ctx = FilterContext::new(&interner, &refs);

    let filter = Filter::ContainsAny {
        field: vec!["tags".to_string()],
        values: vec![
            FilterValue::String("python".to_string()),
            FilterValue::String("java".to_string()),
        ],
    };
    let cb = compile_filter(&filter, &interner);
    assert!(!cb.matches(&record, &ctx));
}

#[test]
fn test_contains_all_list_match() {
    let interner = Interner::new();
    let record = make_list_record(&interner);
    let refs = empty_refs();
    let ctx = FilterContext::new(&interner, &refs);

    let filter = Filter::ContainsAll {
        field: vec!["tags".to_string()],
        values: vec![
            FilterValue::String("rust".to_string()),
            FilterValue::String("db".to_string()),
        ],
    };
    let cb = compile_filter(&filter, &interner);
    assert!(cb.matches(&record, &ctx)); // tags contains both "rust" and "db"
}

#[test]
fn test_contains_all_list_partial_match() {
    let interner = Interner::new();
    let record = make_list_record(&interner);
    let refs = empty_refs();
    let ctx = FilterContext::new(&interner, &refs);

    let filter = Filter::ContainsAll {
        field: vec!["tags".to_string()],
        values: vec![
            FilterValue::String("rust".to_string()),
            FilterValue::String("python".to_string()),
        ],
    };
    let cb = compile_filter(&filter, &interner);
    assert!(!cb.matches(&record, &ctx)); // tags has "rust" but not "python"
}

#[test]
fn test_contains_any_set_match() {
    let interner = Interner::new();
    let record = make_set_record(&interner);
    let refs = empty_refs();
    let ctx = FilterContext::new(&interner, &refs);

    let filter = Filter::ContainsAny {
        field: vec!["roles".to_string()],
        values: vec![
            FilterValue::String("admin".to_string()),
            FilterValue::String("superadmin".to_string()),
        ],
    };
    let cb = compile_filter(&filter, &interner);
    assert!(cb.matches(&record, &ctx));
}

#[test]
fn test_contains_all_set_match() {
    let interner = Interner::new();
    let record = make_set_record(&interner);
    let refs = empty_refs();
    let ctx = FilterContext::new(&interner, &refs);

    let filter = Filter::ContainsAll {
        field: vec!["roles".to_string()],
        values: vec![
            FilterValue::String("admin".to_string()),
            FilterValue::String("user".to_string()),
        ],
    };
    let cb = compile_filter(&filter, &interner);
    assert!(cb.matches(&record, &ctx));
}

// ============================================================================
// Step 12: Between
// ============================================================================

#[test]
fn test_between_in_range() {
    let interner = Interner::new();
    let record = make_alice_record(&interner);
    let refs = empty_refs();
    let ctx = FilterContext::new(&interner, &refs);

    let filter = Filter::Between {
        field: vec!["age".to_string()],
        from: FilterValue::Int(25),
        to: FilterValue::Int(35),
    };
    let cb = compile_filter(&filter, &interner);
    assert!(cb.matches(&record, &ctx)); // 30 is between 25 and 35
}

#[test]
fn test_between_at_lower_bound() {
    let interner = Interner::new();
    let record = make_alice_record(&interner);
    let refs = empty_refs();
    let ctx = FilterContext::new(&interner, &refs);

    let filter = Filter::Between {
        field: vec!["age".to_string()],
        from: FilterValue::Int(30),
        to: FilterValue::Int(40),
    };
    let cb = compile_filter(&filter, &interner);
    assert!(cb.matches(&record, &ctx)); // 30 >= 30
}

#[test]
fn test_between_at_upper_bound() {
    let interner = Interner::new();
    let record = make_alice_record(&interner);
    let refs = empty_refs();
    let ctx = FilterContext::new(&interner, &refs);

    let filter = Filter::Between {
        field: vec!["age".to_string()],
        from: FilterValue::Int(20),
        to: FilterValue::Int(30),
    };
    let cb = compile_filter(&filter, &interner);
    assert!(cb.matches(&record, &ctx)); // 30 <= 30
}

#[test]
fn test_between_out_of_range() {
    let interner = Interner::new();
    let record = make_alice_record(&interner);
    let refs = empty_refs();
    let ctx = FilterContext::new(&interner, &refs);

    let filter = Filter::Between {
        field: vec!["age".to_string()],
        from: FilterValue::Int(31),
        to: FilterValue::Int(40),
    };
    let cb = compile_filter(&filter, &interner);
    assert!(!cb.matches(&record, &ctx)); // 30 < 31
}

#[test]
fn test_between_with_field_ref() {
    let interner = Interner::new();
    let record = make_date_record(&interner);
    let refs = empty_refs();
    let ctx = FilterContext::new(&interner, &refs);

    // start_date (100) between 50 and end_date (200)
    let filter = Filter::Between {
        field: vec!["start_date".to_string()],
        from: FilterValue::Int(50),
        to: FilterValue::FieldRef {
            path: vec!["end_date".to_string()],
        },
    };
    let cb = compile_filter(&filter, &interner);
    assert!(cb.matches(&record, &ctx)); // 100 between 50 and 200
}

#[test]
fn test_between_string() {
    let interner = Interner::new();
    let record = make_alice_record(&interner);
    let refs = empty_refs();
    let ctx = FilterContext::new(&interner, &refs);

    let filter = Filter::Between {
        field: vec!["name".to_string()],
        from: FilterValue::String("A".to_string()),
        to: FilterValue::String("B".to_string()),
    };
    let cb = compile_filter(&filter, &interner);
    assert!(cb.matches(&record, &ctx)); // "Alice" between "A" and "B"
}

// ============================================================================
// Step 13: Exists / NotExists
// ============================================================================

#[test]
fn test_exists_present_field() {
    let interner = Interner::new();
    let record = make_alice_record(&interner);
    let refs = empty_refs();
    let ctx = FilterContext::new(&interner, &refs);

    let filter = Filter::Exists {
        field: vec!["name".to_string()],
    };
    let cb = compile_filter(&filter, &interner);
    assert!(cb.matches(&record, &ctx));
}

#[test]
fn test_exists_null_field() {
    let interner = Interner::new();
    let record = make_nullable_record(&interner);
    let refs = empty_refs();
    let ctx = FilterContext::new(&interner, &refs);

    // deleted_at exists in the record (value is Null but key is present)
    let filter = Filter::Exists {
        field: vec!["deleted_at".to_string()],
    };
    let cb = compile_filter(&filter, &interner);
    assert!(cb.matches(&record, &ctx)); // Exists checks presence, not value
}

#[test]
fn test_exists_missing_field() {
    let interner = Interner::new();
    let record = make_alice_record(&interner);
    let refs = empty_refs();
    let ctx = FilterContext::new(&interner, &refs);

    // "email" doesn't exist in alice record, but also not in interner
    let filter = Filter::Exists {
        field: vec!["email".to_string()],
    };
    let cb = compile_filter(&filter, &interner);
    assert!(!cb.matches(&record, &ctx));
}

#[test]
fn test_exists_missing_field_in_record_but_in_interner() {
    let interner = Interner::new();
    let record = make_alice_record(&interner);
    // Intern "email" so the path resolves, but it's not in the record
    interner.touch_ind("email").unwrap();
    let refs = empty_refs();
    let ctx = FilterContext::new(&interner, &refs);

    let filter = Filter::Exists {
        field: vec!["email".to_string()],
    };
    let cb = compile_filter(&filter, &interner);
    assert!(!cb.matches(&record, &ctx)); // field not in record
}

#[test]
fn test_not_exists_missing_field() {
    let interner = Interner::new();
    let record = make_alice_record(&interner);
    let refs = empty_refs();
    let ctx = FilterContext::new(&interner, &refs);

    let filter = Filter::NotExists {
        field: vec!["email".to_string()],
    };
    let cb = compile_filter(&filter, &interner);
    assert!(cb.matches(&record, &ctx)); // field not interned => TrueCallback
}

#[test]
fn test_not_exists_present_field() {
    let interner = Interner::new();
    let record = make_alice_record(&interner);
    let refs = empty_refs();
    let ctx = FilterContext::new(&interner, &refs);

    let filter = Filter::NotExists {
        field: vec!["name".to_string()],
    };
    let cb = compile_filter(&filter, &interner);
    assert!(!cb.matches(&record, &ctx)); // "name" exists
}

#[test]
fn test_not_exists_null_field() {
    let interner = Interner::new();
    let record = make_nullable_record(&interner);
    let refs = empty_refs();
    let ctx = FilterContext::new(&interner, &refs);

    // deleted_at has Null value but IS present in the record
    let filter = Filter::NotExists {
        field: vec!["deleted_at".to_string()],
    };
    let cb = compile_filter(&filter, &interner);
    assert!(!cb.matches(&record, &ctx)); // key exists (even though value is Null)
}

#[test]
fn test_not_exists_field_in_interner_but_not_record() {
    let interner = Interner::new();
    let record = make_alice_record(&interner);
    interner.touch_ind("email").unwrap();
    let refs = empty_refs();
    let ctx = FilterContext::new(&interner, &refs);

    let filter = Filter::NotExists {
        field: vec!["email".to_string()],
    };
    let cb = compile_filter(&filter, &interner);
    assert!(cb.matches(&record, &ctx)); // "email" not in record
}

// ============================================================================
// FTS brute-force (FtsMatch FilterNode)
// ============================================================================

fn make_body_record(interner: &Interner, body: &str) -> InnerValue {
    let mut map = new_map();
    let k_body = interner.touch_ind("body").unwrap().key().clone();
    map.insert(k_body, InnerValue::Str(body.to_string()));
    InnerValue::Map(map)
}

#[test]
fn test_fts_and_match() {
    let interner = Interner::new();
    let rec = make_body_record(&interner, "Hello World foo bar");
    let refs = empty_refs();
    let ctx = FilterContext::new(&interner, &refs);
    let filter = Filter::Fts {
        field: vec!["body".into()],
        query: "hello world".into(),
        mode: "and".into(),
    };
    let cb = compile_filter(&filter, &interner);
    assert!(cb.matches(&rec, &ctx));
}

#[test]
fn test_fts_and_no_match() {
    let interner = Interner::new();
    let rec = make_body_record(&interner, "Hello bar");
    let refs = empty_refs();
    let ctx = FilterContext::new(&interner, &refs);
    let filter = Filter::Fts {
        field: vec!["body".into()],
        query: "hello world".into(),
        mode: "and".into(),
    };
    let cb = compile_filter(&filter, &interner);
    assert!(!cb.matches(&rec, &ctx));
}

#[test]
fn test_fts_or_match() {
    let interner = Interner::new();
    let rec = make_body_record(&interner, "baz qux");
    let refs = empty_refs();
    let ctx = FilterContext::new(&interner, &refs);
    let filter = Filter::Fts {
        field: vec!["body".into()],
        query: "hello baz".into(),
        mode: "or".into(),
    };
    let cb = compile_filter(&filter, &interner);
    assert!(cb.matches(&rec, &ctx));
}

#[test]
fn test_fts_case_insensitive() {
    let interner = Interner::new();
    let rec = make_body_record(&interner, "HELLO world");
    let refs = empty_refs();
    let ctx = FilterContext::new(&interner, &refs);
    let filter = Filter::Fts {
        field: vec!["body".into()],
        query: "hello WORLD".into(),
        mode: "and".into(),
    };
    let cb = compile_filter(&filter, &interner);
    assert!(cb.matches(&rec, &ctx));
}

#[test]
fn test_fts_missing_field() {
    let interner = Interner::new();
    let rec = make_alice_record(&interner);
    interner.touch_ind("body").unwrap();
    let refs = empty_refs();
    let ctx = FilterContext::new(&interner, &refs);
    let filter = Filter::Fts {
        field: vec!["body".into()],
        query: "hello".into(),
        mode: "and".into(),
    };
    let cb = compile_filter(&filter, &interner);
    assert!(!cb.matches(&rec, &ctx));
}

// ============================================================================
// Computed expression comparison (ComputedCompare FilterNode)
// ============================================================================

fn make_email_record(interner: &Interner, email: &str) -> InnerValue {
    let mut map = new_map();
    let k_email = interner.touch_ind("email").unwrap().key().clone();
    map.insert(k_email, InnerValue::Str(email.to_string()));
    InnerValue::Map(map)
}

#[test]
fn test_computed_lower_eq() {
    let interner = Interner::new();
    let rec = make_email_record(&interner, "ALICE@FOO.COM");
    let refs = empty_refs();
    let ctx = FilterContext::new(&interner, &refs);
    let filter = Filter::Computed {
        expr_op: "lower".into(),
        field: vec!["email".into()],
        expr_args: None,
        cmp: "eq".into(),
        value: FilterValue::String("alice@foo.com".into()),
    };
    let cb = compile_filter(&filter, &interner);
    assert!(cb.matches(&rec, &ctx));
}

#[test]
fn test_computed_lower_eq_no_match() {
    let interner = Interner::new();
    let rec = make_email_record(&interner, "Bob@bar.com");
    let refs = empty_refs();
    let ctx = FilterContext::new(&interner, &refs);
    let filter = Filter::Computed {
        expr_op: "lower".into(),
        field: vec!["email".into()],
        expr_args: None,
        cmp: "eq".into(),
        value: FilterValue::String("alice@foo.com".into()),
    };
    let cb = compile_filter(&filter, &interner);
    assert!(!cb.matches(&rec, &ctx));
}

#[test]
fn test_computed_upper_eq() {
    let interner = Interner::new();
    let rec = make_email_record(&interner, "alice@foo.com");
    let refs = empty_refs();
    let ctx = FilterContext::new(&interner, &refs);
    let filter = Filter::Computed {
        expr_op: "upper".into(),
        field: vec!["email".into()],
        expr_args: None,
        cmp: "eq".into(),
        value: FilterValue::String("ALICE@FOO.COM".into()),
    };
    let cb = compile_filter(&filter, &interner);
    assert!(cb.matches(&rec, &ctx));
}

#[test]
fn test_computed_trim_eq() {
    let interner = Interner::new();
    let rec = make_email_record(&interner, "  alice  ");
    let refs = empty_refs();
    let ctx = FilterContext::new(&interner, &refs);
    let filter = Filter::Computed {
        expr_op: "trim".into(),
        field: vec!["email".into()],
        expr_args: None,
        cmp: "eq".into(),
        value: FilterValue::String("alice".into()),
    };
    let cb = compile_filter(&filter, &interner);
    assert!(cb.matches(&rec, &ctx));
}

#[test]
fn test_computed_length_gt() {
    let interner = Interner::new();
    let rec = make_email_record(&interner, "alexander@example.com");
    let refs = empty_refs();
    let ctx = FilterContext::new(&interner, &refs);
    let filter = Filter::Computed {
        expr_op: "length".into(),
        field: vec!["email".into()],
        expr_args: None,
        cmp: "gt".into(),
        value: FilterValue::Int(10),
    };
    let cb = compile_filter(&filter, &interner);
    assert!(cb.matches(&rec, &ctx));
}

#[test]
fn test_computed_unknown_op_is_false() {
    let interner = Interner::new();
    interner.touch_ind("email").unwrap();
    let rec = make_email_record(&interner, "alice");
    let refs = empty_refs();
    let ctx = FilterContext::new(&interner, &refs);
    let filter = Filter::Computed {
        expr_op: "nonexistent".into(),
        field: vec!["email".into()],
        expr_args: None,
        cmp: "eq".into(),
        value: FilterValue::String("alice".into()),
    };
    let cb = compile_filter(&filter, &interner);
    assert!(!cb.matches(&rec, &ctx));
}
