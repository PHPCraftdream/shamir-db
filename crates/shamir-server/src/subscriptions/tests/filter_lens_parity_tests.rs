//! Parity tests: subscription filter evaluation via the zero-copy `RecordView`
//! lens produces IDENTICAL match results to the legacy `InnerValue` tree walk
//! for a battery of record shapes and filter kinds.
//!
//! These tests exercise `filter_matches_bytes` (lens path with InnerValue
//! fallback) against `filter_matches_inner` (compiled engine evaluator on the
//! InnerValue tree) and assert bit-for-bit identity of the boolean match
//! result.

use std::sync::Arc;

use shamir_collections::TMap;
use shamir_db::core::interner::Interner;
use shamir_db::types::value::InnerValue;
use shamir_query_types::filter::{Filter, FilterValue};
use tokio::sync::OnceCell;

use crate::subscriptions::filter_eval::{filter_matches_bytes, filter_matches_inner};

// ---------------------------------------------------------------------------
// helpers
// ---------------------------------------------------------------------------

/// Build a record from flat `(field_name, value)` pairs.
/// Returns `(bytes, InnerValue, Arc<OnceCell<Interner>>)`.
fn make_record(fields: &[(&str, InnerValue)]) -> (Vec<u8>, InnerValue, Arc<OnceCell<Interner>>) {
    let interner = Interner::new();
    let mut map: TMap<_, InnerValue> = TMap::default();
    for (field, val) in fields {
        let key = interner.touch_ind(*field).expect("intern field").into_key();
        map.insert(key, val.clone());
    }
    let inner = InnerValue::Map(map);
    let bytes = Vec::from(inner.to_bytes().expect("serialize").as_ref());
    let cell = OnceCell::new();
    cell.set(interner).unwrap();
    (bytes, inner, Arc::new(cell))
}

/// Build a nested record: `{ outer: { inner_field: inner_val } }`.
fn make_nested_record(
    outer: &str,
    inner_field: &str,
    inner_val: InnerValue,
) -> (Vec<u8>, InnerValue, Arc<OnceCell<Interner>>) {
    let interner = Interner::new();
    let inner_key = interner.touch_ind(inner_field).unwrap().into_key();
    let outer_key = interner.touch_ind(outer).unwrap().into_key();
    let mut inner_map: TMap<_, InnerValue> = TMap::default();
    inner_map.insert(inner_key, inner_val);
    let mut root: TMap<_, InnerValue> = TMap::default();
    root.insert(outer_key, InnerValue::Map(inner_map));
    let inner = InnerValue::Map(root);
    let bytes = Vec::from(inner.to_bytes().unwrap().as_ref());
    let cell = OnceCell::new();
    cell.set(interner).unwrap();
    (bytes, inner, Arc::new(cell))
}

/// Assert that lens-based and InnerValue-based evaluation agree for a given
/// filter + record.
fn assert_parity(
    label: &str,
    filter: &Filter,
    bytes: &[u8],
    inner: &InnerValue,
    cell: &OnceCell<Interner>,
) {
    let lens_result = filter_matches_bytes(filter, bytes, cell);
    let inner_result = filter_matches_inner(filter, inner, cell);
    assert_eq!(
        lens_result, inner_result,
        "PARITY FAILURE [{label}]: lens={lens_result}, inner={inner_result}"
    );
}

// ---------------------------------------------------------------------------
// Eq / Ne
// ---------------------------------------------------------------------------

#[test]
fn parity_eq_string_match() {
    let filter = Filter::Eq {
        field: vec!["name".into()],
        value: FilterValue::String("alice".into()),
    };
    let (b, iv, c) = make_record(&[("name", InnerValue::Str("alice".into()))]);
    assert_parity("eq_str_match", &filter, &b, &iv, &c);
}

#[test]
fn parity_eq_string_mismatch() {
    let filter = Filter::Eq {
        field: vec!["name".into()],
        value: FilterValue::String("alice".into()),
    };
    let (b, iv, c) = make_record(&[("name", InnerValue::Str("bob".into()))]);
    assert_parity("eq_str_mismatch", &filter, &b, &iv, &c);
}

#[test]
fn parity_ne_int() {
    let filter = Filter::Ne {
        field: vec!["age".into()],
        value: FilterValue::Int(30),
    };
    let (b, iv, c) = make_record(&[("age", InnerValue::Int(25))]);
    assert_parity("ne_int", &filter, &b, &iv, &c);
}

// ---------------------------------------------------------------------------
// Gt / Gte / Lt / Lte (range)
// ---------------------------------------------------------------------------

#[test]
fn parity_gt_int() {
    let filter = Filter::Gt {
        field: vec!["score".into()],
        value: FilterValue::Int(50),
    };
    let (b1, iv1, c1) = make_record(&[("score", InnerValue::Int(75))]);
    assert_parity("gt_pass", &filter, &b1, &iv1, &c1);
    let (b2, iv2, c2) = make_record(&[("score", InnerValue::Int(50))]);
    assert_parity("gt_eq_fail", &filter, &b2, &iv2, &c2);
    let (b3, iv3, c3) = make_record(&[("score", InnerValue::Int(25))]);
    assert_parity("gt_fail", &filter, &b3, &iv3, &c3);
}

#[test]
fn parity_gte_float() {
    let filter = Filter::Gte {
        field: vec!["temp".into()],
        value: FilterValue::Float(36.6),
    };
    let (b1, iv1, c1) = make_record(&[("temp", InnerValue::F64(36.6))]);
    assert_parity("gte_eq", &filter, &b1, &iv1, &c1);
    let (b2, iv2, c2) = make_record(&[("temp", InnerValue::F64(36.5))]);
    assert_parity("gte_fail", &filter, &b2, &iv2, &c2);
}

#[test]
fn parity_lt_lte_string() {
    let filter_lt = Filter::Lt {
        field: vec!["name".into()],
        value: FilterValue::String("m".into()),
    };
    let filter_lte = Filter::Lte {
        field: vec!["name".into()],
        value: FilterValue::String("m".into()),
    };
    let (b, iv, c) = make_record(&[("name", InnerValue::Str("alice".into()))]);
    assert_parity("lt_str", &filter_lt, &b, &iv, &c);
    assert_parity("lte_str", &filter_lte, &b, &iv, &c);

    let (b2, iv2, c2) = make_record(&[("name", InnerValue::Str("zara".into()))]);
    assert_parity("lt_str_fail", &filter_lt, &b2, &iv2, &c2);
}

// ---------------------------------------------------------------------------
// Cross-type: Int field vs Float filter, Float field vs Int filter
// ---------------------------------------------------------------------------

#[test]
fn parity_cross_int_float() {
    let filter = Filter::Eq {
        field: vec!["x".into()],
        value: FilterValue::Float(42.0),
    };
    let (b, iv, c) = make_record(&[("x", InnerValue::Int(42))]);
    assert_parity("int_vs_float_eq", &filter, &b, &iv, &c);

    let filter2 = Filter::Gt {
        field: vec!["y".into()],
        value: FilterValue::Int(10),
    };
    let (b2, iv2, c2) = make_record(&[("y", InnerValue::F64(10.5))]);
    assert_parity("float_vs_int_gt", &filter2, &b2, &iv2, &c2);
}

// ---------------------------------------------------------------------------
// In / NotIn
// ---------------------------------------------------------------------------

#[test]
fn parity_in_set() {
    let filter = Filter::In {
        field: vec!["status".into()],
        values: vec![
            FilterValue::String("active".into()),
            FilterValue::String("pending".into()),
        ],
    };
    let (b1, iv1, c1) = make_record(&[("status", InnerValue::Str("active".into()))]);
    assert_parity("in_match", &filter, &b1, &iv1, &c1);
    let (b2, iv2, c2) = make_record(&[("status", InnerValue::Str("deleted".into()))]);
    assert_parity("in_miss", &filter, &b2, &iv2, &c2);
}

#[test]
fn parity_not_in() {
    let filter = Filter::NotIn {
        field: vec!["code".into()],
        values: vec![
            FilterValue::Int(1),
            FilterValue::Int(2),
            FilterValue::Int(3),
        ],
    };
    let (b1, iv1, c1) = make_record(&[("code", InnerValue::Int(4))]);
    assert_parity("nin_pass", &filter, &b1, &iv1, &c1);
    let (b2, iv2, c2) = make_record(&[("code", InnerValue::Int(2))]);
    assert_parity("nin_fail", &filter, &b2, &iv2, &c2);
}

// ---------------------------------------------------------------------------
// IsNull / IsNotNull / Exists / NotExists
// ---------------------------------------------------------------------------

#[test]
fn parity_null_checks() {
    let is_null = Filter::IsNull {
        field: vec!["x".into()],
    };
    let is_not_null = Filter::IsNotNull {
        field: vec!["x".into()],
    };
    let exists = Filter::Exists {
        field: vec!["x".into()],
    };
    let not_exists = Filter::NotExists {
        field: vec!["x".into()],
    };

    // Field is Null
    let (b1, iv1, c1) = make_record(&[("x", InnerValue::Null)]);
    assert_parity("is_null_with_null", &is_null, &b1, &iv1, &c1);
    assert_parity("is_not_null_with_null", &is_not_null, &b1, &iv1, &c1);
    assert_parity("exists_with_null", &exists, &b1, &iv1, &c1);

    // Field is present (non-null)
    let (b2, iv2, c2) = make_record(&[("x", InnerValue::Int(42))]);
    assert_parity("is_null_with_int", &is_null, &b2, &iv2, &c2);
    assert_parity("is_not_null_with_int", &is_not_null, &b2, &iv2, &c2);
    assert_parity("exists_with_int", &exists, &b2, &iv2, &c2);

    // Field absent (record has other fields)
    let (b3, iv3, c3) = make_record(&[("y", InnerValue::Int(1))]);
    assert_parity("is_null_absent", &is_null, &b3, &iv3, &c3);
    assert_parity("is_not_null_absent", &is_not_null, &b3, &iv3, &c3);
    assert_parity("not_exists_absent", &not_exists, &b3, &iv3, &c3);
}

// ---------------------------------------------------------------------------
// Nested field paths
// ---------------------------------------------------------------------------

#[test]
fn parity_nested_eq() {
    let filter = Filter::Eq {
        field: vec!["address".into(), "city".into()],
        value: FilterValue::String("Jerusalem".into()),
    };
    let (b1, iv1, c1) = make_nested_record("address", "city", InnerValue::Str("Jerusalem".into()));
    assert_parity("nested_eq_match", &filter, &b1, &iv1, &c1);
    let (b2, iv2, c2) = make_nested_record("address", "city", InnerValue::Str("Tel Aviv".into()));
    assert_parity("nested_eq_miss", &filter, &b2, &iv2, &c2);
}

// ---------------------------------------------------------------------------
// And / Or / Not (logical)
// ---------------------------------------------------------------------------

#[test]
fn parity_and_or_not() {
    let and_filter = Filter::And {
        filters: vec![
            Filter::Eq {
                field: vec!["a".into()],
                value: FilterValue::Int(1),
            },
            Filter::Eq {
                field: vec!["b".into()],
                value: FilterValue::Int(2),
            },
        ],
    };
    let or_filter = Filter::Or {
        filters: vec![
            Filter::Eq {
                field: vec!["a".into()],
                value: FilterValue::Int(1),
            },
            Filter::Eq {
                field: vec!["b".into()],
                value: FilterValue::Int(999),
            },
        ],
    };
    let not_filter = Filter::Not {
        filter: Box::new(Filter::Eq {
            field: vec!["a".into()],
            value: FilterValue::Int(1),
        }),
    };

    let (b, iv, c) = make_record(&[("a", InnerValue::Int(1)), ("b", InnerValue::Int(2))]);
    assert_parity("and_both_match", &and_filter, &b, &iv, &c);
    assert_parity("or_first_match", &or_filter, &b, &iv, &c);
    assert_parity("not_match", &not_filter, &b, &iv, &c);

    let (b2, iv2, c2) = make_record(&[("a", InnerValue::Int(999)), ("b", InnerValue::Int(2))]);
    assert_parity("and_first_mismatch", &and_filter, &b2, &iv2, &c2);
    assert_parity("not_mismatch", &not_filter, &b2, &iv2, &c2);
}

// ---------------------------------------------------------------------------
// Binary field (Bin)
// ---------------------------------------------------------------------------

#[test]
fn parity_bin_eq() {
    let filter = Filter::Eq {
        field: vec!["data".into()],
        value: FilterValue::Binary(vec![0xDE, 0xAD, 0xBE, 0xEF]),
    };
    let (b1, iv1, c1) = make_record(&[("data", InnerValue::Bin(vec![0xDE, 0xAD, 0xBE, 0xEF]))]);
    assert_parity("bin_eq_match", &filter, &b1, &iv1, &c1);
    let (b2, iv2, c2) = make_record(&[("data", InnerValue::Bin(vec![0x00, 0x01]))]);
    assert_parity("bin_eq_mismatch", &filter, &b2, &iv2, &c2);
}

// ---------------------------------------------------------------------------
// Bool field
// ---------------------------------------------------------------------------

#[test]
fn parity_bool() {
    let filter = Filter::Eq {
        field: vec!["active".into()],
        value: FilterValue::Bool(true),
    };
    let (b1, iv1, c1) = make_record(&[("active", InnerValue::Bool(true))]);
    assert_parity("bool_match", &filter, &b1, &iv1, &c1);
    let (b2, iv2, c2) = make_record(&[("active", InnerValue::Bool(false))]);
    assert_parity("bool_mismatch", &filter, &b2, &iv2, &c2);
}

// ---------------------------------------------------------------------------
// Contains (string substring, list membership)
// ---------------------------------------------------------------------------

#[test]
fn parity_contains_string() {
    let filter = Filter::Contains {
        field: vec!["desc".into()],
        value: FilterValue::String("world".into()),
    };
    let (b1, iv1, c1) = make_record(&[("desc", InnerValue::Str("hello world".into()))]);
    assert_parity("contains_str_match", &filter, &b1, &iv1, &c1);
    let (b2, iv2, c2) = make_record(&[("desc", InnerValue::Str("goodbye".into()))]);
    assert_parity("contains_str_miss", &filter, &b2, &iv2, &c2);
}

#[test]
fn parity_contains_list() {
    let filter = Filter::Contains {
        field: vec!["tags".into()],
        value: FilterValue::String("rust".into()),
    };
    let (b1, iv1, c1) = make_record(&[(
        "tags",
        InnerValue::List(vec![
            InnerValue::Str("rust".into()),
            InnerValue::Str("db".into()),
        ]),
    )]);
    assert_parity("contains_list_match", &filter, &b1, &iv1, &c1);
    let (b2, iv2, c2) =
        make_record(&[("tags", InnerValue::List(vec![InnerValue::Str("go".into())]))]);
    assert_parity("contains_list_miss", &filter, &b2, &iv2, &c2);
}

// ---------------------------------------------------------------------------
// ContainsAny / ContainsAll
// ---------------------------------------------------------------------------

#[test]
fn parity_contains_any() {
    let filter = Filter::ContainsAny {
        field: vec!["tags".into()],
        values: vec![
            FilterValue::String("rust".into()),
            FilterValue::String("python".into()),
        ],
    };
    let (b1, iv1, c1) = make_record(&[(
        "tags",
        InnerValue::List(vec![
            InnerValue::Str("rust".into()),
            InnerValue::Str("db".into()),
        ]),
    )]);
    assert_parity("contains_any_match", &filter, &b1, &iv1, &c1);
    let (b2, iv2, c2) =
        make_record(&[("tags", InnerValue::List(vec![InnerValue::Str("go".into())]))]);
    assert_parity("contains_any_miss", &filter, &b2, &iv2, &c2);
}

#[test]
fn parity_contains_all() {
    let filter = Filter::ContainsAll {
        field: vec!["tags".into()],
        values: vec![
            FilterValue::String("rust".into()),
            FilterValue::String("db".into()),
        ],
    };
    let (b1, iv1, c1) = make_record(&[(
        "tags",
        InnerValue::List(vec![
            InnerValue::Str("rust".into()),
            InnerValue::Str("db".into()),
            InnerValue::Str("fast".into()),
        ]),
    )]);
    assert_parity("contains_all_match", &filter, &b1, &iv1, &c1);
    let (b2, iv2, c2) = make_record(&[(
        "tags",
        InnerValue::List(vec![InnerValue::Str("rust".into())]),
    )]);
    assert_parity("contains_all_miss", &filter, &b2, &iv2, &c2);
}

// ---------------------------------------------------------------------------
// Between
// ---------------------------------------------------------------------------

#[test]
fn parity_between() {
    let filter = Filter::Between {
        field: vec!["score".into()],
        from: FilterValue::Int(10),
        to: FilterValue::Int(20),
    };
    let (b1, iv1, c1) = make_record(&[("score", InnerValue::Int(15))]);
    assert_parity("between_in", &filter, &b1, &iv1, &c1);
    let (b2, iv2, c2) = make_record(&[("score", InnerValue::Int(10))]);
    assert_parity("between_lower_bound", &filter, &b2, &iv2, &c2);
    let (b3, iv3, c3) = make_record(&[("score", InnerValue::Int(20))]);
    assert_parity("between_upper_bound", &filter, &b3, &iv3, &c3);
    let (b4, iv4, c4) = make_record(&[("score", InnerValue::Int(5))]);
    assert_parity("between_below", &filter, &b4, &iv4, &c4);
    let (b5, iv5, c5) = make_record(&[("score", InnerValue::Int(25))]);
    assert_parity("between_above", &filter, &b5, &iv5, &c5);
}

// ---------------------------------------------------------------------------
// Set field (InnerValue::Set) -- exercises the lens Set→Arr mapping
// ---------------------------------------------------------------------------

#[test]
fn parity_contains_set() {
    let filter = Filter::Contains {
        field: vec!["roles".into()],
        value: FilterValue::String("admin".into()),
    };
    let (b1, iv1, c1) = make_record(&[(
        "roles",
        InnerValue::Set(
            vec![
                InnerValue::Str("admin".into()),
                InnerValue::Str("user".into()),
            ]
            .into_iter()
            .collect(),
        ),
    )]);
    assert_parity("contains_set_match", &filter, &b1, &iv1, &c1);
    let (b2, iv2, c2) = make_record(&[(
        "roles",
        InnerValue::Set(vec![InnerValue::Str("user".into())].into_iter().collect()),
    )]);
    assert_parity("contains_set_miss", &filter, &b2, &iv2, &c2);
}

// ---------------------------------------------------------------------------
// Bare-scalar fallback: non-map records should be handled gracefully
// ---------------------------------------------------------------------------

#[test]
fn parity_bare_scalar_filter_fail_closed() {
    // A bare integer (not a map) -- RecordView::new will fail.
    // filter_matches_bytes falls back to InnerValue.
    // The filter references a field path, which won't resolve on a non-map.
    let filter = Filter::Eq {
        field: vec!["name".into()],
        value: FilterValue::String("alice".into()),
    };
    let inner = InnerValue::Int(42);
    let bytes = Vec::from(inner.to_bytes().unwrap().as_ref());
    let interner = Interner::new();
    let _ = interner.touch_ind("name");
    let cell = OnceCell::new();
    cell.set(interner).unwrap();
    let cell = Arc::new(cell);

    let lens_result = filter_matches_bytes(&filter, &bytes, &cell);
    let inner_result = filter_matches_inner(&filter, &inner, &cell);
    // Both should fail-closed (false) since you can't walk a field path
    // on a bare scalar.
    assert!(!lens_result, "bare scalar should not match via lens");
    assert!(!inner_result, "bare scalar should not match via inner");
}

// ---------------------------------------------------------------------------
// Multi-field record with mixed types
// ---------------------------------------------------------------------------

#[test]
fn parity_multi_field_mixed_types() {
    let filter = Filter::And {
        filters: vec![
            Filter::Eq {
                field: vec!["name".into()],
                value: FilterValue::String("test".into()),
            },
            Filter::Gt {
                field: vec!["count".into()],
                value: FilterValue::Int(0),
            },
            Filter::Eq {
                field: vec!["active".into()],
                value: FilterValue::Bool(true),
            },
        ],
    };
    let (b, iv, c) = make_record(&[
        ("name", InnerValue::Str("test".into())),
        ("count", InnerValue::Int(5)),
        ("active", InnerValue::Bool(true)),
        ("data", InnerValue::Bin(vec![1, 2, 3])),
        ("nothing", InnerValue::Null),
    ]);
    assert_parity("multi_field_pass", &filter, &b, &iv, &c);
}
