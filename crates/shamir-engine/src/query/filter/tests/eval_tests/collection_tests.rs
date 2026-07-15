use crate::query::filter::eval::compile_filter;
use crate::query::filter::eval_context::FilterContext;
use crate::query::filter::{Filter, FilterValue};
use crate::query::read::{QueryRecord, QueryResult};
use shamir_types::core::interner::Interner;
use shamir_types::mpack;
use shamir_types::types::common::{new_map, TMap};
use shamir_types::types::value::InnerValue;

use super::helpers::{
    empty_refs, make_alice_record, make_date_record, make_list_record, make_set_record,
};

// ============================================================================
// In / NotIn — literal values
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

// ============================================================================
// In / NotIn — QueryRef column
// ============================================================================

#[test]
fn test_in_query_ref_column() {
    let interner = Interner::new();

    // Record: {user_id: 2}
    let mut map = new_map();
    let k = interner.touch_ind("user_id").unwrap().into_key();
    map.insert(k, InnerValue::Int(2));
    let record = InnerValue::Map(map);

    // Query result: "allowed_users" => [{id: 1}, {id: 2}, {id: 5}]
    let mut refs: TMap<String, QueryResult> = new_map();
    refs.insert(
        "allowed_users".to_string(),
        QueryResult {
            records: vec![
                QueryRecord::Direct(mpack!({"id": 1})),
                QueryRecord::Direct(mpack!({"id": 2})),
                QueryRecord::Direct(mpack!({"id": 5})),
            ],
            stats: None,
            pagination: None,
            value: None,
            explain: None,
            skipped: false,
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
    let k = interner.touch_ind("user_id").unwrap().into_key();
    map.insert(k, InnerValue::Int(99));
    let record = InnerValue::Map(map);

    let mut refs: TMap<String, QueryResult> = new_map();
    refs.insert(
        "allowed_users".to_string(),
        QueryResult {
            records: vec![
                QueryRecord::Direct(mpack!({"id": 1})),
                QueryRecord::Direct(mpack!({"id": 2})),
            ],
            stats: None,
            pagination: None,
            value: None,
            explain: None,
            skipped: false,
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
    let k = interner.touch_ind("user_id").unwrap().into_key();
    map.insert(k, InnerValue::Int(99));
    let record = InnerValue::Map(map);

    let mut refs: TMap<String, QueryResult> = new_map();
    refs.insert(
        "blocked".to_string(),
        QueryResult {
            records: vec![
                QueryRecord::Direct(mpack!({"id": 1})),
                QueryRecord::Direct(mpack!({"id": 2})),
            ],
            stats: None,
            pagination: None,
            value: None,
            explain: None,
            skipped: false,
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
// InSet fast path — compile-time HashSet when all values are literals
// ============================================================================

#[test]
fn test_in_all_literals_compiles_to_inset() {
    use crate::query::filter::filter_node::FilterNode;
    let interner = Interner::new();
    // Touch the field so the interner knows it; otherwise compile_filter returns False.
    interner.touch_ind("status").unwrap();
    let filter = Filter::In {
        field: vec!["status".to_string()],
        values: vec![
            FilterValue::String("a".to_string()),
            FilterValue::String("b".to_string()),
            FilterValue::String("c".to_string()),
        ],
    };
    let node = compile_filter(&filter, &interner);
    assert!(
        matches!(node, FilterNode::InSet { .. }),
        "all-literal $in must compile to InSet"
    );
}

#[test]
fn test_in_with_query_ref_compiles_to_in_vec() {
    use crate::query::filter::filter_node::FilterNode;
    let interner = Interner::new();
    interner.touch_ind("user_id").unwrap();
    let filter = Filter::In {
        field: vec!["user_id".to_string()],
        values: vec![FilterValue::QueryRef {
            alias: "@users".to_string(),
            path: Some("[].id".to_string()),
        }],
    };
    let node = compile_filter(&filter, &interner);
    assert!(
        matches!(node, FilterNode::In { .. }),
        "non-literal value must fall back to In (linear scan)"
    );
}

#[test]
fn test_inset_match_present() {
    let interner = Interner::new();
    let record = make_alice_record(&interner);
    let refs = empty_refs();
    let ctx = FilterContext::new(&interner, &refs);

    let filter = Filter::In {
        field: vec!["status".to_string()],
        values: vec![
            FilterValue::String("active".to_string()),
            FilterValue::String("pending".to_string()),
            FilterValue::String("suspended".to_string()),
        ],
    };
    let cb = compile_filter(&filter, &interner);
    // record.status == "active" — must match
    assert!(cb.matches(&record, &ctx));
}

#[test]
fn test_inset_match_absent() {
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
    // record.status == "active" — must NOT match
    assert!(!cb.matches(&record, &ctx));
}

#[test]
fn test_not_inset_match() {
    let interner = Interner::new();
    let record = make_alice_record(&interner);
    let refs = empty_refs();
    let ctx = FilterContext::new(&interner, &refs);

    // $nin with all literals → InSet, negate=true
    let filter = Filter::NotIn {
        field: vec!["status".to_string()],
        values: vec![
            FilterValue::String("deleted".to_string()),
            FilterValue::String("banned".to_string()),
        ],
    };
    let cb = compile_filter(&filter, &interner);
    // record.status == "active" — not in ["deleted","banned"] → true
    assert!(cb.matches(&record, &ctx));
}

// ============================================================================
// Contains / ContainsAny / ContainsAll
// ============================================================================

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
// Between
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
