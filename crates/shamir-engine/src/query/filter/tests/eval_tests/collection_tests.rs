use crate::query::filter::eval::compile_filter;
use crate::query::filter::eval_context::FilterContext;
use crate::query::filter::{Filter, FilterValue};
use crate::query::read::{QueryRecord, QueryResult};
use shamir_types::core::interner::Interner;
use shamir_types::mpack;
use shamir_types::types::common::{new_map, new_set, TMap};
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
// $contains_all — duplicate-element correctness (ContainsAllSet fast path)
// ============================================================================

#[test]
fn test_contains_all_list_duplicate_stands_in_for_missing_value() {
    // Fast-path closure of the duplicate-counting bug: `tags = ["a", "a"]`
    // must NOT satisfy `$contains_all: ["a", "b"]` — "b" is genuinely absent,
    // even though both "a" elements are members of the required set. The
    // all-literal value list selects ContainsAllSet; the old raw-hit count
    // wrongly returned a match here.
    let interner = Interner::new();
    let refs = empty_refs();
    let ctx = FilterContext::new(&interner, &refs);

    let mut map = new_map();
    let k_tags = interner.touch_ind("tags").unwrap().into_key();
    map.insert(
        k_tags,
        InnerValue::List(vec![
            InnerValue::Str("a".to_string()),
            InnerValue::Str("a".to_string()),
        ]),
    );
    let record = InnerValue::Map(map);

    let filter = Filter::ContainsAll {
        field: vec!["tags".to_string()],
        values: vec![
            FilterValue::String("a".to_string()),
            FilterValue::String("b".to_string()),
        ],
    };
    let cb = compile_filter(&filter, &interner);
    assert!(
        !cb.matches(&record, &ctx),
        "duplicate 'a' must not stand in for the absent required value 'b'"
    );
}

#[test]
fn test_contains_all_list_duplicate_with_all_values_present() {
    // Positive counterpart: a field that duplicates a required value AND
    // genuinely contains every required value still matches.
    let interner = Interner::new();
    let refs = empty_refs();
    let ctx = FilterContext::new(&interner, &refs);

    let mut map = new_map();
    let k_tags = interner.touch_ind("tags").unwrap().into_key();
    map.insert(
        k_tags,
        InnerValue::List(vec![
            InnerValue::Str("a".to_string()),
            InnerValue::Str("b".to_string()),
            InnerValue::Str("a".to_string()),
        ]),
    );
    let record = InnerValue::Map(map);

    let filter = Filter::ContainsAll {
        field: vec!["tags".to_string()],
        values: vec![
            FilterValue::String("a".to_string()),
            FilterValue::String("b".to_string()),
        ],
    };
    let cb = compile_filter(&filter, &interner);
    assert!(cb.matches(&record, &ctx));
}

#[test]
fn test_contains_all_fast_slow_parity() {
    // The all-literal form compiles to ContainsAllSet (the O(field_len) fast
    // path). Replacing one value with a `$ref` forces the ContainsAll slow
    // path (its value resolves at match time to the same literal). Both paths
    // must agree on every input — in particular the duplicate-elements case
    // that previously diverged.
    let interner = Interner::new();
    let refs = empty_refs();
    let ctx = FilterContext::new(&interner, &refs);

    let k_tags = interner.touch_ind("tags").unwrap().into_key();
    let k_needle = interner.touch_ind("needle").unwrap().into_key();

    // Fast path: all-literal `["a", "b"]` -> ContainsAllSet.
    let fast_filter = Filter::ContainsAll {
        field: vec!["tags".to_string()],
        values: vec![
            FilterValue::String("a".to_string()),
            FilterValue::String("b".to_string()),
        ],
    };
    let fast_cb = compile_filter(&fast_filter, &interner);

    // Slow path: `["a", {"$ref": "needle"}]` -> ContainsAll. The record carries
    // `needle: "b"`, so the resolved value set is identical to the fast path's.
    let slow_filter = Filter::ContainsAll {
        field: vec!["tags".to_string()],
        values: vec![
            FilterValue::String("a".to_string()),
            FilterValue::field_ref("needle"),
        ],
    };
    let slow_cb = compile_filter(&slow_filter, &interner);

    // (tags, expected_match). The required set is always {"a", "b"}.
    let cases: &[(&[&str], bool)] = &[
        (&["a", "a"], false),     // duplicate, missing "b" — the original bug
        (&["a", "b", "a"], true), // duplicate, all present
        (&["a", "b"], true),      // exact
        (&["a", "b", "c"], true), // superset
        (&["a"], false),          // subset, missing "b"
        (&[], false),             // empty field
    ];

    for (tags, expected) in cases {
        let mut map = new_map();
        map.insert(
            k_tags.clone(),
            InnerValue::List(
                tags.iter()
                    .map(|t| InnerValue::Str((*t).to_string()))
                    .collect(),
            ),
        );
        map.insert(k_needle.clone(), InnerValue::Str("b".to_string()));
        let record = InnerValue::Map(map);

        let fast = fast_cb.matches(&record, &ctx);
        let slow = slow_cb.matches(&record, &ctx);
        assert_eq!(
            fast, slow,
            "fast/slow divergence for tags={:?}: fast={} slow={}",
            tags, fast, slow
        );
        assert_eq!(
            fast, *expected,
            "unexpected result for tags={:?}: got {} want {}",
            tags, fast, expected
        );
    }
}

// ============================================================================
// Int↔F64 coercion in all-literal fast-path nodes (InSet/ContainsAnySet/ContainsAllSet)
//
// The slow-path nodes (In, ContainsAny, ContainsAll) go through
// scalar_ref_cmp_qv / compare_values which treat Int(1) and F64(1.0) as equal.
// The all-literal fast-path nodes previously used exact TSet::contains /
// swap_remove, so the same logical filter gave different answers depending on
// whether its value list happened to be fully literal.
// ============================================================================

fn make_int_field_record(interner: &Interner, field: &str, val: i64) -> InnerValue {
    let mut map = new_map();
    let k = interner.touch_ind(field).unwrap().into_key();
    map.insert(k, InnerValue::Int(val));
    InnerValue::Map(map)
}

fn make_f64_field_record(interner: &Interner, field: &str, val: f64) -> InnerValue {
    let mut map = new_map();
    let k = interner.touch_ind(field).unwrap().into_key();
    map.insert(k, InnerValue::F64(val));
    InnerValue::Map(map)
}

#[test]
fn test_inset_int_field_f64_literal_coerces() {
    // InSet: field n = Int(1), filter {$in: [1.0]} — must MATCH.
    let interner = Interner::new();
    let record = make_int_field_record(&interner, "n", 1);
    let refs = empty_refs();
    let ctx = FilterContext::new(&interner, &refs);

    let filter = Filter::In {
        field: vec!["n".to_string()],
        values: vec![FilterValue::Float(1.0)],
    };
    let cb = compile_filter(&filter, &interner);
    assert!(
        cb.matches(&record, &ctx),
        "Int(1) field must match $in [1.0]"
    );
}

#[test]
fn test_inset_f64_field_int_literal_coerces() {
    // InSet: field n = F64(1.0), filter {$in: [1]} — must MATCH.
    let interner = Interner::new();
    let record = make_f64_field_record(&interner, "n", 1.0);
    let refs = empty_refs();
    let ctx = FilterContext::new(&interner, &refs);

    let filter = Filter::In {
        field: vec!["n".to_string()],
        values: vec![FilterValue::Int(1)],
    };
    let cb = compile_filter(&filter, &interner);
    assert!(
        cb.matches(&record, &ctx),
        "F64(1.0) field must match $in [1]"
    );
}

#[test]
fn test_inset_no_false_positive_on_non_integer_f64() {
    // InSet: field n = Int(1), filter {$in: [1.5]} — must NOT match.
    let interner = Interner::new();
    let record = make_int_field_record(&interner, "n", 1);
    let refs = empty_refs();
    let ctx = FilterContext::new(&interner, &refs);

    let filter = Filter::In {
        field: vec!["n".to_string()],
        values: vec![FilterValue::Float(1.5)],
    };
    let cb = compile_filter(&filter, &interner);
    assert!(
        !cb.matches(&record, &ctx),
        "Int(1) must NOT match $in [1.5]"
    );
}

#[test]
fn test_contains_any_set_int_list_f64_literal_coerces() {
    // ContainsAnySet: field is List containing Int(1), required {1.0} — MATCH.
    let interner = Interner::new();
    let k = interner.touch_ind("items").unwrap().into_key();
    let mut map = new_map();
    map.insert(
        k,
        InnerValue::List(vec![InnerValue::Int(1), InnerValue::Int(2)]),
    );
    let record = InnerValue::Map(map);
    let refs = empty_refs();
    let ctx = FilterContext::new(&interner, &refs);

    let filter = Filter::ContainsAny {
        field: vec!["items".to_string()],
        values: vec![FilterValue::Float(1.0)],
    };
    let cb = compile_filter(&filter, &interner);
    assert!(
        cb.matches(&record, &ctx),
        "List[Int(1),Int(2)] must match $contains_any [1.0]"
    );
}

#[test]
fn test_contains_any_set_int_set_f64_literal_coerces() {
    // ContainsAnySet: field is Set containing Int(1), required {1.0} — MATCH.
    let interner = Interner::new();
    let k = interner.touch_ind("items").unwrap().into_key();
    let mut s = new_set();
    s.insert(InnerValue::Int(1));
    s.insert(InnerValue::Int(2));
    let mut map = new_map();
    map.insert(k, InnerValue::Set(s));
    let record = InnerValue::Map(map);
    let refs = empty_refs();
    let ctx = FilterContext::new(&interner, &refs);

    let filter = Filter::ContainsAny {
        field: vec!["items".to_string()],
        values: vec![FilterValue::Float(1.0)],
    };
    let cb = compile_filter(&filter, &interner);
    assert!(
        cb.matches(&record, &ctx),
        "Set{{Int(1),Int(2)}} must match $contains_any [1.0]"
    );
}

#[test]
fn test_contains_all_set_int_list_f64_literals_coerces() {
    // ContainsAllSet: field contains [Int(1), Int(2)], required {1.0, 2.0} — MATCH.
    // Exercises the swap_remove-must-actually-remove coercion path.
    let interner = Interner::new();
    let k = interner.touch_ind("items").unwrap().into_key();
    let mut map = new_map();
    map.insert(
        k,
        InnerValue::List(vec![InnerValue::Int(1), InnerValue::Int(2)]),
    );
    let record = InnerValue::Map(map);
    let refs = empty_refs();
    let ctx = FilterContext::new(&interner, &refs);

    let filter = Filter::ContainsAll {
        field: vec!["items".to_string()],
        values: vec![FilterValue::Float(1.0), FilterValue::Float(2.0)],
    };
    let cb = compile_filter(&filter, &interner);
    assert!(
        cb.matches(&record, &ctx),
        "List[Int(1),Int(2)] must match $contains_all [1.0, 2.0]"
    );
}

#[test]
fn test_contains_all_set_int_list_f64_literals_partial_no_false_positive() {
    // ContainsAllSet: field contains [Int(1), Int(2)], required {1.0, 3.0} —
    // must NOT match (3 absent). Verifies the coercing swap_remove correctly
    // tracks remaining count.
    let interner = Interner::new();
    let k = interner.touch_ind("items").unwrap().into_key();
    let mut map = new_map();
    map.insert(
        k,
        InnerValue::List(vec![InnerValue::Int(1), InnerValue::Int(2)]),
    );
    let record = InnerValue::Map(map);
    let refs = empty_refs();
    let ctx = FilterContext::new(&interner, &refs);

    let filter = Filter::ContainsAll {
        field: vec!["items".to_string()],
        values: vec![FilterValue::Float(1.0), FilterValue::Float(3.0)],
    };
    let cb = compile_filter(&filter, &interner);
    assert!(
        !cb.matches(&record, &ctx),
        "List[Int(1),Int(2)] must NOT match $contains_all [1.0, 3.0]"
    );
}

#[test]
fn test_inset_non_numeric_exact_match_unchanged() {
    // Regression: non-numeric types continue to use exact matching — no
    // accidental coercion introduced for Str/Bool.
    let interner = Interner::new();
    let mut map = new_map();
    let k = interner.touch_ind("s").unwrap().into_key();
    map.insert(k, InnerValue::Str("hello".to_string()));
    let record = InnerValue::Map(map);
    let refs = empty_refs();
    let ctx = FilterContext::new(&interner, &refs);

    // Exact match
    let f1 = Filter::In {
        field: vec!["s".to_string()],
        values: vec![FilterValue::String("hello".to_string())],
    };
    assert!(compile_filter(&f1, &interner).matches(&record, &ctx));

    // No false cross-type match
    let f2 = Filter::In {
        field: vec!["s".to_string()],
        values: vec![FilterValue::String("world".to_string())],
    };
    assert!(!compile_filter(&f2, &interner).matches(&record, &ctx));
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

// ============================================================================
// F6 regression: $in / $nin against a NON-scalar (container) field.
//
// `FilterNode::InSet` (the all-literal fast path for `$in`/`$nin`) now uses
// `record.scalar_at` + `set_contains_coercing` — the SAME borrow-based probe
// the sibling `FilterNode::In` (the dynamic / mixed-literal path) uses. A
// non-scalar field (List / Set / Map / Bin) returns `None` from `scalar_at`,
// so both nodes treat it as ABSENT: `$in` → false, `$nin` → true. These
// tests pin that the two sibling nodes agree on this edge case (the
// inconsistency F6 fixed was that `InSet` previously walked INTO the
// container via `materialize_at`, while `In` already treated it as absent).
// ============================================================================

#[test]
fn inset_against_container_field_list_is_false() {
    // Field `tags` is a List; `$in` against a container is non-sensical and
    // must be false (the field is "absent" from a scalar-probe POV). Both
    // the InSet fast path and the In slow path agree.
    let interner = Interner::new();
    let record = make_list_record(&interner); // {name: "Test", tags: ["rust","db","query"]}
    let refs = empty_refs();
    let ctx = FilterContext::new(&interner, &refs);

    // All-literal values → compiles to InSet (fast path).
    let filter_in = Filter::In {
        field: vec!["tags".to_string()],
        values: vec![FilterValue::String("rust".to_string())],
    };
    let node = compile_filter(&filter_in, &interner);
    assert!(
        matches!(
            node,
            crate::query::filter::filter_node::FilterNode::InSet { .. }
        ),
        "all-literal $in must compile to InSet"
    );
    assert!(
        !node.matches(&record, &ctx),
        "$in against a List field must be false (InSet treats non-scalar as absent)"
    );
}

#[test]
fn inset_against_container_field_list_nin_is_true() {
    // Negated form: `$nin` against a List field → true (negate of absent).
    let interner = Interner::new();
    let record = make_list_record(&interner);
    let refs = empty_refs();
    let ctx = FilterContext::new(&interner, &refs);

    let filter_nin = Filter::NotIn {
        field: vec!["tags".to_string()],
        values: vec![FilterValue::String("rust".to_string())],
    };
    let node = compile_filter(&filter_nin, &interner);
    assert!(
        matches!(
            node,
            crate::query::filter::filter_node::FilterNode::InSet { .. }
        ),
        "all-literal $nin must compile to InSet"
    );
    assert!(
        node.matches(&record, &ctx),
        "$nin against a List field must be true (InSet treats non-scalar as absent → negate)"
    );
}

#[test]
fn inset_against_container_field_set_is_false() {
    // Field `roles` is a Set — same absent semantics as List.
    let interner = Interner::new();
    let record = make_set_record(&interner); // {name: "Test", roles: {admin, user}}
    let refs = empty_refs();
    let ctx = FilterContext::new(&interner, &refs);

    let filter_in = Filter::In {
        field: vec!["roles".to_string()],
        values: vec![FilterValue::String("admin".to_string())],
    };
    let node = compile_filter(&filter_in, &interner);
    assert!(
        matches!(
            node,
            crate::query::filter::filter_node::FilterNode::InSet { .. }
        ),
        "all-literal $in must compile to InSet"
    );
    assert!(
        !node.matches(&record, &ctx),
        "$in against a Set field must be false (InSet treats non-scalar as absent)"
    );
}

#[test]
fn inset_and_in_agree_on_container_field() {
    // The WHOLE point of F6: InSet (all-literal fast path) and In (dynamic
    // path with at least one non-literal) must agree on a container field.
    // We force the In slow path by mixing a literal with a FieldRef that
    // resolves to the same literal value; against a List field, both nodes
    // return false for `$in`.
    let interner = Interner::new();
    let record = make_list_record(&interner); // {name: "Test", tags: [...]}
    let refs = empty_refs();
    let ctx = FilterContext::new(&interner, &refs);

    // InSet fast path (all literals).
    let inset_node = compile_filter(
        &Filter::In {
            field: vec!["tags".to_string()],
            values: vec![FilterValue::String("rust".to_string())],
        },
        &interner,
    );
    // In slow path (one non-literal FieldRef → does not collapse to InSet).
    // The FieldRef resolves to `name` = "Test" (a scalar), which won't be in
    // the literal set anyway; what matters is the FIELD side (`tags`) is a
    // List, so scalar_at returns None and both nodes treat it as absent.
    let in_node = compile_filter(
        &Filter::In {
            field: vec!["tags".to_string()],
            values: vec![
                FilterValue::String("rust".to_string()),
                FilterValue::FieldRef {
                    path: vec!["name".to_string()],
                },
            ],
        },
        &interner,
    );
    assert!(
        matches!(
            inset_node,
            crate::query::filter::filter_node::FilterNode::InSet { .. }
        ),
        "all-literal $in must compile to InSet"
    );
    assert!(
        matches!(
            in_node,
            crate::query::filter::filter_node::FilterNode::In { .. }
        ),
        "mixed $in must compile to In (slow path)"
    );
    // Both must agree: a List field is absent for scalar probing → no match.
    assert_eq!(
        inset_node.matches(&record, &ctx),
        in_node.matches(&record, &ctx),
        "InSet and In must agree on a container (List) field"
    );
    assert!(
        !inset_node.matches(&record, &ctx),
        "both must be false for $in against a List field"
    );
}
