//! Semantic tests for `$in @ref[].field` — the column query-ref arm of
//! `FilterNode::In`.
//!
//! These lock the exact semantics (duplicates in the ref column, type
//! coercion across Int/F64, empty ref column, `negate` / NOT IN, and a
//! multi-segment path) so the O(N²)→O(N) memoisation refactor cannot
//! regress behaviour.

use crate::query::filter::eval::compile_filter;
use crate::query::filter::eval_context::FilterContext;
use crate::query::filter::{Filter, FilterValue};
use crate::query::read::{QueryRecord, QueryResult};
use shamir_types::core::interner::Interner;
use shamir_types::mpack;
use shamir_types::types::common::{new_map, TMap};
use shamir_types::types::value::InnerValue;

use super::helpers::empty_refs;

// ── helper: build a record {age: <i64>} ──────────────────────────
fn age_record(interner: &Interner, age: i64) -> InnerValue {
    let mut map = new_map();
    let k = interner.touch_ind("age").unwrap().into_key();
    map.insert(k, InnerValue::Int(age));
    InnerValue::Map(map)
}

// ── helper: build a ref map with one alias → QueryResult ─────────
fn ref_map(alias: &str, records: Vec<QueryRecord>) -> TMap<String, QueryResult> {
    let mut refs = new_map();
    refs.insert(
        alias.to_string(),
        QueryResult {
            records,
            stats: None,
            pagination: None,
            value: None,
        },
    );
    refs
}

// ── helper: build a record {cat: <str>} for string-column tests ──
fn cat_record(interner: &Interner, cat: &str) -> InnerValue {
    let mut map = new_map();
    let k = interner.touch_ind("cat").unwrap().into_key();
    map.insert(k, InnerValue::Str(cat.to_string()));
    InnerValue::Map(map)
}

/// Basic match: `age` is in the ref column `[].val`.
#[test]
fn test_in_ref_column_match() {
    let interner = Interner::new();
    let record = age_record(&interner, 42);

    let refs = ref_map(
        "ref",
        vec![
            QueryRecord::Direct(mpack!({"val": 10})),
            QueryRecord::Direct(mpack!({"val": 42})),
            QueryRecord::Direct(mpack!({"val": 99})),
        ],
    );
    let ctx = FilterContext::new(&interner, &refs);

    let filter = Filter::In {
        field: vec!["age".to_string()],
        values: vec![FilterValue::QueryRef {
            alias: "ref".to_string(),
            path: Some("[].val".to_string()),
        }],
    };
    let cb = compile_filter(&filter, &interner);
    assert!(cb.matches(&record, &ctx));
}

/// No match: `age` not in the ref column.
#[test]
fn test_in_ref_column_no_match() {
    let interner = Interner::new();
    let record = age_record(&interner, 77);

    let refs = ref_map(
        "ref",
        vec![
            QueryRecord::Direct(mpack!({"val": 10})),
            QueryRecord::Direct(mpack!({"val": 42})),
        ],
    );
    let ctx = FilterContext::new(&interner, &refs);

    let filter = Filter::In {
        field: vec!["age".to_string()],
        values: vec![FilterValue::QueryRef {
            alias: "ref".to_string(),
            path: Some("[].val".to_string()),
        }],
    };
    let cb = compile_filter(&filter, &interner);
    assert!(!cb.matches(&record, &ctx));
}

/// Duplicates in the ref column: must still match (duplicates don't
/// break membership).
#[test]
fn test_in_ref_column_duplicates() {
    let interner = Interner::new();
    let record = age_record(&interner, 42);

    let refs = ref_map(
        "ref",
        vec![
            QueryRecord::Direct(mpack!({"val": 42})),
            QueryRecord::Direct(mpack!({"val": 42})),
            QueryRecord::Direct(mpack!({"val": 42})),
        ],
    );
    let ctx = FilterContext::new(&interner, &refs);

    let filter = Filter::In {
        field: vec!["age".to_string()],
        values: vec![FilterValue::QueryRef {
            alias: "ref".to_string(),
            path: Some("[].val".to_string()),
        }],
    };
    let cb = compile_filter(&filter, &interner);
    assert!(cb.matches(&record, &ctx));
}

/// Type coercion: `scalar_ref_cmp_qv` treats `Int(a)` as equal to
/// `F64(b)` when `(a as f64) == b`. The `$in @ref` column-ref arm
/// preserves this coercion — `Int(42)` DOES match a ref column
/// containing `F64(42.0)`.
#[test]
fn test_in_ref_column_int_field_f64_column_coercion() {
    let interner = Interner::new();
    // Record has Int(42)
    let record = age_record(&interner, 42);

    // Ref column has F64(42.0)
    let refs = ref_map("ref", vec![QueryRecord::Direct(mpack!({"val": 42.0f64}))]);
    let ctx = FilterContext::new(&interner, &refs);

    let filter = Filter::In {
        field: vec!["age".to_string()],
        values: vec![FilterValue::QueryRef {
            alias: "ref".to_string(),
            path: Some("[].val".to_string()),
        }],
    };
    let cb = compile_filter(&filter, &interner);
    // Int(42) coerces to F64(42.0) → match.
    assert!(cb.matches(&record, &ctx));
}

/// Reverse coercion: `F64(f)` field matches `Int(b)` column entry when
/// `f == (b as f64)`. Record has F64(42.0), column has Int(42) → match.
#[test]
fn test_in_ref_column_f64_field_int_column_coercion() {
    let interner = Interner::new();
    let mut map = new_map();
    let k_age = interner.touch_ind("age").unwrap().into_key();
    map.insert(k_age, InnerValue::F64(42.0));
    let record = InnerValue::Map(map);

    let refs = ref_map("ref", vec![QueryRecord::Direct(mpack!({"val": 42}))]);
    let ctx = FilterContext::new(&interner, &refs);

    let filter = Filter::In {
        field: vec!["age".to_string()],
        values: vec![FilterValue::QueryRef {
            alias: "ref".to_string(),
            path: Some("[].val".to_string()),
        }],
    };
    let cb = compile_filter(&filter, &interner);
    // F64(42.0) coerces to Int(42) → match.
    assert!(cb.matches(&record, &ctx));
}

/// Same-type match within the ref column: Int(42) matches Int(42).
#[test]
fn test_in_ref_column_int_match_same_type() {
    let interner = Interner::new();
    let record = age_record(&interner, 42);

    let refs = ref_map("ref", vec![QueryRecord::Direct(mpack!({"val": 42}))]);
    let ctx = FilterContext::new(&interner, &refs);

    let filter = Filter::In {
        field: vec!["age".to_string()],
        values: vec![FilterValue::QueryRef {
            alias: "ref".to_string(),
            path: Some("[].val".to_string()),
        }],
    };
    let cb = compile_filter(&filter, &interner);
    assert!(cb.matches(&record, &ctx));
}

/// Empty ref column: nothing matches.
#[test]
fn test_in_ref_column_empty() {
    let interner = Interner::new();
    let record = age_record(&interner, 42);

    let refs = ref_map("ref", vec![]);
    let ctx = FilterContext::new(&interner, &refs);

    let filter = Filter::In {
        field: vec!["age".to_string()],
        values: vec![FilterValue::QueryRef {
            alias: "ref".to_string(),
            path: Some("[].val".to_string()),
        }],
    };
    let cb = compile_filter(&filter, &interner);
    assert!(!cb.matches(&record, &ctx));
}

/// NOT IN (`negate`): record NOT in the ref column → true.
#[test]
fn test_not_in_ref_column() {
    let interner = Interner::new();
    let record = age_record(&interner, 77);

    let refs = ref_map(
        "ref",
        vec![
            QueryRecord::Direct(mpack!({"val": 10})),
            QueryRecord::Direct(mpack!({"val": 42})),
        ],
    );
    let ctx = FilterContext::new(&interner, &refs);

    let filter = Filter::NotIn {
        field: vec!["age".to_string()],
        values: vec![FilterValue::QueryRef {
            alias: "ref".to_string(),
            path: Some("[].val".to_string()),
        }],
    };
    let cb = compile_filter(&filter, &interner);
    assert!(cb.matches(&record, &ctx)); // 77 is NOT in {10,42}
}

/// NOT IN with a matching record → false.
#[test]
fn test_not_in_ref_column_match_is_false() {
    let interner = Interner::new();
    let record = age_record(&interner, 42);

    let refs = ref_map("ref", vec![QueryRecord::Direct(mpack!({"val": 42}))]);
    let ctx = FilterContext::new(&interner, &refs);

    let filter = Filter::NotIn {
        field: vec!["age".to_string()],
        values: vec![FilterValue::QueryRef {
            alias: "ref".to_string(),
            path: Some("[].val".to_string()),
        }],
    };
    let cb = compile_filter(&filter, &interner);
    assert!(!cb.matches(&record, &ctx)); // 42 IS in {42}
}

/// NOT IN with empty ref column → true (vacuously, nothing is in the
/// empty set, so NOT IN holds).
#[test]
fn test_not_in_ref_column_empty() {
    let interner = Interner::new();
    let record = age_record(&interner, 42);

    let refs = ref_map("ref", vec![]);
    let ctx = FilterContext::new(&interner, &refs);

    let filter = Filter::NotIn {
        field: vec!["age".to_string()],
        values: vec![FilterValue::QueryRef {
            alias: "ref".to_string(),
            path: Some("[].val".to_string()),
        }],
    };
    let cb = compile_filter(&filter, &interner);
    assert!(cb.matches(&record, &ctx));
}

/// String column: `cat` in ref column `[].tag`.
#[test]
fn test_in_ref_column_string() {
    let interner = Interner::new();
    let record = cat_record(&interner, "beta");

    let refs = ref_map(
        "ref",
        vec![
            QueryRecord::Direct(mpack!({"tag": "alpha"})),
            QueryRecord::Direct(mpack!({"tag": "beta"})),
            QueryRecord::Direct(mpack!({"tag": "gamma"})),
        ],
    );
    let ctx = FilterContext::new(&interner, &refs);

    let filter = Filter::In {
        field: vec!["cat".to_string()],
        values: vec![FilterValue::QueryRef {
            alias: "ref".to_string(),
            path: Some("[].tag".to_string()),
        }],
    };
    let cb = compile_filter(&filter, &interner);
    assert!(cb.matches(&record, &ctx));
}

/// Missing alias: ref not in `resolved_refs` → nothing matches (In→false).
#[test]
fn test_in_ref_column_missing_alias() {
    let interner = Interner::new();
    let record = age_record(&interner, 42);

    let refs = empty_refs();
    let ctx = FilterContext::new(&interner, &refs);

    let filter = Filter::In {
        field: vec!["age".to_string()],
        values: vec![FilterValue::QueryRef {
            alias: "nonexistent".to_string(),
            path: Some("[].val".to_string()),
        }],
    };
    let cb = compile_filter(&filter, &interner);
    assert!(!cb.matches(&record, &ctx));
}

/// Mixed values: a literal AND a QueryRef column in the same `$in`.
/// The literal matches first (short-circuit), then the ref column is
/// checked. This exercises the per-value loop with both `pre_resolved`
/// and the column-ref sub-case.
#[test]
fn test_in_mixed_literal_and_ref_column() {
    let interner = Interner::new();

    // Record 1: age=5 → matches the literal.
    let rec1 = age_record(&interner, 5);
    // Record 2: age=42 → matches the ref column.
    let rec2 = age_record(&interner, 42);
    // Record 3: age=99 → matches neither.
    let rec3 = age_record(&interner, 99);

    let refs = ref_map("ref", vec![QueryRecord::Direct(mpack!({"val": 42}))]);
    let ctx = FilterContext::new(&interner, &refs);

    let filter = Filter::In {
        field: vec!["age".to_string()],
        values: vec![
            FilterValue::Int(5),
            FilterValue::QueryRef {
                alias: "ref".to_string(),
                path: Some("[].val".to_string()),
            },
        ],
    };
    let cb = compile_filter(&filter, &interner);
    assert!(cb.matches(&rec1, &ctx)); // literal 5
    assert!(cb.matches(&rec2, &ctx)); // ref column {42}
    assert!(!cb.matches(&rec3, &ctx)); // neither
}
