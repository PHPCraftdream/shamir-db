//! Tests for filter evaluation (callback network).

use crate::core::interner::Interner;
use crate::db::query::filter::eval::{compare_values, compile_filter, resolve_field};
use crate::db::query::filter::eval_context::FilterContext;
use crate::db::query::filter::{Filter, FilterValue};
use crate::db::query::read::QueryResult;
use crate::types::common::{new_map, TMap};
use crate::types::value::InnerValue;

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
    let k_missing = interner.touch_ind("nonexistent").unwrap().key().clone().id();

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
        field: "status".to_string(),
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
        field: "status".to_string(),
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
        field: "age".to_string(),
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
        field: "age".to_string(),
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
        field: "age".to_string(),
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
        field: "age".to_string(),
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
        field: "status".to_string(),
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
                field: "status".to_string(),
                value: FilterValue::String("active".to_string()),
            },
            Filter::Gt {
                field: "age".to_string(),
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
                field: "status".to_string(),
                value: FilterValue::String("deleted".to_string()),
            },
            Filter::Gt {
                field: "age".to_string(),
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
                field: "status".to_string(),
                value: FilterValue::String("deleted".to_string()),
            },
            Filter::Gt {
                field: "age".to_string(),
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
                field: "status".to_string(),
                value: FilterValue::String("deleted".to_string()),
            },
            Filter::Lt {
                field: "age".to_string(),
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
            field: "status".to_string(),
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
        field: "deleted_at".to_string(),
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
        field: "name".to_string(),
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
        field: "nonexistent".to_string(),
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
        field: "name".to_string(),
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
        field: "deleted_at".to_string(),
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
        field: "end_date".to_string(),
        value: FilterValue::FieldRef {
            path: "start_date".to_string(),
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
        field: "start_date".to_string(),
        value: FilterValue::FieldRef {
            path: "end_date".to_string(),
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
        field: "start_date".to_string(),
        value: FilterValue::FieldRef {
            path: "start_date".to_string(),
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
        field: "user_id".to_string(),
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
        field: "user_id".to_string(),
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
        field: "user_id".to_string(),
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
                        field: "status".to_string(),
                        value: FilterValue::String("active".to_string()),
                    },
                    Filter::Gt {
                        field: "age".to_string(),
                        value: FilterValue::Int(25),
                    },
                ],
            },
            Filter::Eq {
                field: "status".to_string(),
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
        field: "user.name".to_string(),
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
        field: "user.score".to_string(),
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
        field: "status".to_string(),
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
        field: "status".to_string(),
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
        field: "status".to_string(),
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
        field: "user_id".to_string(),
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
            records: vec![
                serde_json::json!({"id": 1}),
                serde_json::json!({"id": 2}),
            ],
            stats: None,
            pagination: None,
        },
    );

    let ctx = FilterContext::new(&interner, &refs);

    let filter = Filter::In {
        field: "user_id".to_string(),
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
            records: vec![
                serde_json::json!({"id": 1}),
                serde_json::json!({"id": 2}),
            ],
            stats: None,
            pagination: None,
        },
    );

    let ctx = FilterContext::new(&interner, &refs);

    let filter = Filter::NotIn {
        field: "user_id".to_string(),
        values: vec![FilterValue::QueryRef {
            alias: "blocked".to_string(),
            path: Some("[].id".to_string()),
        }],
    };
    let cb = compile_filter(&filter, &interner);
    assert!(cb.matches(&record, &ctx)); // 99 not in [1, 2]
}
