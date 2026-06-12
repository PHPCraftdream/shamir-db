//! Correctness tests for `FilterNode::matches_msgpack_bytes`.
//!
//! Each test serialises an `InnerValue::Map` to msgpack bytes (exactly the same
//! path as `table.rs::insert_many`), then:
//!   (a) runs the bytes-level pre-filter, and
//!   (b) runs the compiled normal filter on the decoded `InnerValue`.
//!
//! The two results MUST agree on every test case.

use shamir_types::core::interner::{Interner, TouchInd};
use shamir_types::types::common::new_map_wc;
use shamir_types::types::value::InnerValue;

use crate::query::filter::eval_context::FilterContext;
use crate::query::filter::{compile_filter, Filter, FilterValue};

// ── helpers ──────────────────────────────────────────────────────────────────

fn touch(i: &Interner, s: &str) -> shamir_types::core::interner::InternerKey {
    match i.touch_ind(s).unwrap() {
        TouchInd::Exists(k) | TouchInd::New(k) => k,
    }
}

fn make_record(interner: &Interner) -> InnerValue {
    let mut m = new_map_wc(6);
    m.insert(touch(interner, "name"), InnerValue::Str("Alice".into()));
    m.insert(touch(interner, "age"), InnerValue::Int(30));
    m.insert(touch(interner, "score"), InnerValue::F64(95.5));
    m.insert(touch(interner, "active"), InnerValue::Bool(true));
    m.insert(touch(interner, "tag"), InnerValue::Null);
    m.insert(touch(interner, "bin"), InnerValue::Bin(vec![1, 2, 3]));
    InnerValue::Map(m)
}

/// Encode a record to bytes the same way `insert_many` does.
fn encode(record: &InnerValue) -> bytes::Bytes {
    record.to_bytes().expect("encode failed")
}

/// Assert that bytes-eval and normal filter agree for a given filter.
fn assert_agree(interner: &Interner, record: &InnerValue, filter: &Filter) {
    let bytes = encode(record);
    let compiled = compile_filter(filter, interner);

    let empty_refs: shamir_types::types::common::TMap<String, _> = new_map_wc(0);
    let ctx = FilterContext::new(interner, &empty_refs);

    let normal_result = compiled.matches(record, &ctx);
    let bytes_result = compiled.matches_msgpack_bytes(&bytes);

    // Semantic identity: bytes-eval must not contradict normal eval.
    // `None` from bytes-eval is a fallback, not a disagreement.
    if let Some(br) = bytes_result {
        assert_eq!(
            br, normal_result,
            "bytes-eval ({br:?}) disagrees with normal eval ({normal_result:?}) for {filter:?}"
        );
    }
}

// ── correctness tests ─────────────────────────────────────────────────────────

#[test]
fn test_eq_string_match() {
    let i = Interner::new();
    let rec = make_record(&i);
    let filter = Filter::Eq {
        field: vec!["name".into()],
        value: FilterValue::String("Alice".into()),
    };
    let bytes = encode(&rec);
    let compiled = compile_filter(&filter, &i);
    assert_eq!(compiled.matches_msgpack_bytes(&bytes), Some(true));
    assert_agree(&i, &rec, &filter);
}

#[test]
fn test_eq_string_no_match() {
    let i = Interner::new();
    let rec = make_record(&i);
    let filter = Filter::Eq {
        field: vec!["name".into()],
        value: FilterValue::String("Bob".into()),
    };
    let bytes = encode(&rec);
    let compiled = compile_filter(&filter, &i);
    assert_eq!(compiled.matches_msgpack_bytes(&bytes), Some(false));
    assert_agree(&i, &rec, &filter);
}

#[test]
fn test_eq_int_match() {
    let i = Interner::new();
    let rec = make_record(&i);
    let filter = Filter::Eq {
        field: vec!["age".into()],
        value: FilterValue::Int(30),
    };
    assert_agree(&i, &rec, &filter);
    let bytes = encode(&rec);
    let compiled = compile_filter(&filter, &i);
    assert_eq!(compiled.matches_msgpack_bytes(&bytes), Some(true));
}

#[test]
fn test_gt_int_match() {
    let i = Interner::new();
    let rec = make_record(&i);
    let filter = Filter::Gt {
        field: vec!["age".into()],
        value: FilterValue::Int(20),
    };
    assert_agree(&i, &rec, &filter);
    let bytes = encode(&rec);
    let compiled = compile_filter(&filter, &i);
    assert_eq!(compiled.matches_msgpack_bytes(&bytes), Some(true));
}

#[test]
fn test_gt_int_no_match() {
    let i = Interner::new();
    let rec = make_record(&i);
    let filter = Filter::Gt {
        field: vec!["age".into()],
        value: FilterValue::Int(50),
    };
    assert_agree(&i, &rec, &filter);
    let bytes = encode(&rec);
    let compiled = compile_filter(&filter, &i);
    assert_eq!(compiled.matches_msgpack_bytes(&bytes), Some(false));
}

#[test]
fn test_is_null_on_null_field() {
    let i = Interner::new();
    let rec = make_record(&i);
    let filter = Filter::IsNull {
        field: vec!["tag".into()],
    };
    assert_agree(&i, &rec, &filter);
    let bytes = encode(&rec);
    let compiled = compile_filter(&filter, &i);
    assert_eq!(compiled.matches_msgpack_bytes(&bytes), Some(true));
}

#[test]
fn test_is_null_on_non_null_field() {
    let i = Interner::new();
    let rec = make_record(&i);
    let filter = Filter::IsNull {
        field: vec!["name".into()],
    };
    assert_agree(&i, &rec, &filter);
    let bytes = encode(&rec);
    let compiled = compile_filter(&filter, &i);
    assert_eq!(compiled.matches_msgpack_bytes(&bytes), Some(false));
}

#[test]
fn test_is_not_null_on_str_field() {
    let i = Interner::new();
    let rec = make_record(&i);
    let filter = Filter::IsNotNull {
        field: vec!["name".into()],
    };
    assert_agree(&i, &rec, &filter);
    let bytes = encode(&rec);
    let compiled = compile_filter(&filter, &i);
    assert_eq!(compiled.matches_msgpack_bytes(&bytes), Some(true));
}

#[test]
fn test_exists_on_present_field() {
    let i = Interner::new();
    let rec = make_record(&i);
    let filter = Filter::Exists {
        field: vec!["age".into()],
    };
    assert_agree(&i, &rec, &filter);
    let bytes = encode(&rec);
    let compiled = compile_filter(&filter, &i);
    assert_eq!(compiled.matches_msgpack_bytes(&bytes), Some(true));
}

#[test]
fn test_not_exists_on_absent_field() {
    let i = Interner::new();
    let rec = make_record(&i);
    let filter = Filter::NotExists {
        field: vec!["missing".into()],
    };
    assert_agree(&i, &rec, &filter);
    let bytes = encode(&rec);
    let compiled = compile_filter(&filter, &i);
    assert_eq!(compiled.matches_msgpack_bytes(&bytes), Some(true));
}

#[test]
fn test_and_both_match() {
    let i = Interner::new();
    let rec = make_record(&i);
    let filter = Filter::And {
        filters: vec![
            Filter::Eq {
                field: vec!["name".into()],
                value: FilterValue::String("Alice".into()),
            },
            Filter::Eq {
                field: vec!["age".into()],
                value: FilterValue::Int(30),
            },
        ],
    };
    assert_agree(&i, &rec, &filter);
    let bytes = encode(&rec);
    let compiled = compile_filter(&filter, &i);
    assert_eq!(compiled.matches_msgpack_bytes(&bytes), Some(true));
}

#[test]
fn test_and_one_fails() {
    let i = Interner::new();
    let rec = make_record(&i);
    let filter = Filter::And {
        filters: vec![
            Filter::Eq {
                field: vec!["name".into()],
                value: FilterValue::String("Alice".into()),
            },
            Filter::Eq {
                field: vec!["age".into()],
                value: FilterValue::Int(99),
            },
        ],
    };
    assert_agree(&i, &rec, &filter);
    let bytes = encode(&rec);
    let compiled = compile_filter(&filter, &i);
    assert_eq!(compiled.matches_msgpack_bytes(&bytes), Some(false));
}

#[test]
fn test_or_one_matches() {
    let i = Interner::new();
    let rec = make_record(&i);
    let filter = Filter::Or {
        filters: vec![
            Filter::Eq {
                field: vec!["name".into()],
                value: FilterValue::String("Bob".into()),
            },
            Filter::Eq {
                field: vec!["age".into()],
                value: FilterValue::Int(30),
            },
        ],
    };
    assert_agree(&i, &rec, &filter);
    let bytes = encode(&rec);
    let compiled = compile_filter(&filter, &i);
    assert_eq!(compiled.matches_msgpack_bytes(&bytes), Some(true));
}

#[test]
fn test_or_none_matches() {
    let i = Interner::new();
    let rec = make_record(&i);
    let filter = Filter::Or {
        filters: vec![
            Filter::Eq {
                field: vec!["name".into()],
                value: FilterValue::String("Bob".into()),
            },
            Filter::Eq {
                field: vec!["age".into()],
                value: FilterValue::Int(99),
            },
        ],
    };
    assert_agree(&i, &rec, &filter);
    let bytes = encode(&rec);
    let compiled = compile_filter(&filter, &i);
    assert_eq!(compiled.matches_msgpack_bytes(&bytes), Some(false));
}

#[test]
fn test_not_negates_match() {
    let i = Interner::new();
    let rec = make_record(&i);
    let filter = Filter::Not {
        filter: Box::new(Filter::Eq {
            field: vec!["name".into()],
            value: FilterValue::String("Alice".into()),
        }),
    };
    assert_agree(&i, &rec, &filter);
    let bytes = encode(&rec);
    let compiled = compile_filter(&filter, &i);
    assert_eq!(compiled.matches_msgpack_bytes(&bytes), Some(false));
}

#[test]
fn test_regex_returns_none_fallback() {
    let i = Interner::new();
    let rec = make_record(&i);
    let filter = Filter::Regex {
        field: vec!["name".into()],
        pattern: "Ali.*".into(),
    };
    let bytes = encode(&rec);
    let compiled = compile_filter(&filter, &i);
    // Regex is unsupported in bytes-eval — must return None (not false).
    assert_eq!(
        compiled.matches_msgpack_bytes(&bytes),
        None,
        "Regex should return None from bytes-eval (fall through to full decode)"
    );
    // But the normal filter agrees it's a match.
    let empty_refs: shamir_types::types::common::TMap<String, _> = new_map_wc(0);
    let ctx = FilterContext::new(&i, &empty_refs);
    assert!(compiled.matches(&rec, &ctx));
}

#[test]
fn test_eq_bool_match() {
    let i = Interner::new();
    let rec = make_record(&i);
    let filter = Filter::Eq {
        field: vec!["active".into()],
        value: FilterValue::Bool(true),
    };
    assert_agree(&i, &rec, &filter);
    let bytes = encode(&rec);
    let compiled = compile_filter(&filter, &i);
    assert_eq!(compiled.matches_msgpack_bytes(&bytes), Some(true));
}

#[test]
fn test_ne_int_match() {
    let i = Interner::new();
    let rec = make_record(&i);
    let filter = Filter::Ne {
        field: vec!["age".into()],
        value: FilterValue::Int(99),
    };
    assert_agree(&i, &rec, &filter);
    let bytes = encode(&rec);
    let compiled = compile_filter(&filter, &i);
    assert_eq!(compiled.matches_msgpack_bytes(&bytes), Some(true));
}

#[test]
fn test_missing_field_ne_returns_true() {
    // Ne on a missing field should match (same as normal filter semantics).
    let i = Interner::new();
    let rec = make_record(&i);
    let filter = Filter::Ne {
        field: vec!["nonexistent".into()],
        value: FilterValue::Int(1),
    };
    assert_agree(&i, &rec, &filter);
}

#[test]
fn test_field_ref_returns_none_fallback() {
    // FieldRef values cannot be evaluated in bytes-eval — must return None.
    let i = Interner::new();
    let rec = make_record(&i);
    let filter = Filter::Eq {
        field: vec!["age".into()],
        value: FilterValue::FieldRef {
            path: vec!["score".into()],
        },
    };
    let bytes = encode(&rec);
    let compiled = compile_filter(&filter, &i);
    assert_eq!(
        compiled.matches_msgpack_bytes(&bytes),
        None,
        "FieldRef must return None from bytes-eval"
    );
}

#[test]
fn test_lte_int_boundary() {
    let i = Interner::new();
    let rec = make_record(&i);
    // age == 30, lte 30 → true; lte 29 → false
    let filter_true = Filter::Lte {
        field: vec!["age".into()],
        value: FilterValue::Int(30),
    };
    let filter_false = Filter::Lte {
        field: vec!["age".into()],
        value: FilterValue::Int(29),
    };
    assert_agree(&i, &rec, &filter_true);
    assert_agree(&i, &rec, &filter_false);
    let bytes = encode(&rec);
    let compiled_true = compile_filter(&filter_true, &i);
    let compiled_false = compile_filter(&filter_false, &i);
    assert_eq!(compiled_true.matches_msgpack_bytes(&bytes), Some(true));
    assert_eq!(compiled_false.matches_msgpack_bytes(&bytes), Some(false));
}
