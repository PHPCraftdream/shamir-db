use crate::query::filter::eval::compile_filter;
use crate::query::filter::eval_context::FilterContext;
use crate::query::filter::Filter;
use shamir_types::core::interner::Interner;

use crate::query::filter::compile::{
    compile_filter_with_context, try_compile_filter, try_compile_filter_with_context,
};
use crate::query::filter::tests::eval_tests::shared::helpers::{empty_refs, make_alice_record};
use crate::query::filter::FilterNode;

// ============================================================================
// Like / ILike
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
// Regex
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

#[test]
fn rv_engine_2a18b647_try_compile_rejects_nested_invalid_and_oversized_regex() {
    let interner = Interner::new();
    interner.touch_ind("name").unwrap();
    let record = make_alice_record(&interner);
    let oversized_name = "a".repeat(crate::query::filter::MAX_FILTER_PATTERN_LENGTH + 1);
    let mut oversized_map = shamir_types::types::common::new_map();
    oversized_map.insert(
        interner.touch_ind("name").unwrap().into_key(),
        shamir_types::types::value::InnerValue::Str(oversized_name.clone()),
    );
    let oversized_record = shamir_types::types::value::InnerValue::Map(oversized_map);
    let refs = empty_refs();
    let ctx = FilterContext::new(&interner, &refs);

    let invalid = Filter::Not {
        filter: Box::new(Filter::Regex {
            field: vec!["name".to_string()],
            pattern: "(unbalanced".to_string(),
        }),
    };
    assert!(try_compile_filter(&invalid, &interner).is_err());
    assert!(!compile_filter(&invalid, &interner).matches(&record, &ctx));

    let oversized = Filter::Regex {
        field: vec!["name".to_string()],
        pattern: oversized_name,
    };
    assert!(try_compile_filter(&oversized, &interner).is_err());
    assert!(!compile_filter(&oversized, &interner).matches(&oversized_record, &ctx));
}

#[test]
fn unknown_field_patterns_are_validated_before_folding() {
    let interner = Interner::new();
    let record = make_alice_record(&interner);
    let refs = empty_refs();
    let ctx = FilterContext::new(&interner, &refs)
        .with_overlay_ids_snapshot(Some(std::sync::Arc::new(shamir_collections::new_fx_map())));
    let oversized = "a".repeat(crate::query::filter::MAX_FILTER_PATTERN_LENGTH + 1);
    // LIKE treats regex metacharacters as literals, so its rejection case is the cap.
    let patterns = [
        (0, "("),
        (0, oversized.as_str()),
        (1, oversized.as_str()),
        (2, oversized.as_str()),
    ];
    for (kind, pattern) in patterns {
        for field in ["missing", "name"] {
            let field = vec![field.to_string()];
            let pattern = pattern.to_string();
            let leaf = match kind {
                0 => Filter::Regex { field, pattern },
                1 => Filter::Like { field, pattern },
                _ => Filter::ILike { field, pattern },
            };
            let wrappers = [
                leaf.clone(),
                Filter::Not {
                    filter: Box::new(leaf.clone()),
                },
                Filter::And {
                    filters: vec![
                        Filter::IsNull {
                            field: vec!["missing".into()],
                        },
                        leaf.clone(),
                    ],
                },
                Filter::Or {
                    filters: vec![
                        Filter::IsNull {
                            field: vec!["missing".into()],
                        },
                        leaf,
                    ],
                },
            ];
            for filter in wrappers {
                let direct_error = try_compile_filter(&filter, &interner).err();
                let overlay_error = try_compile_filter_with_context(&filter, &ctx).err();
                assert!(direct_error.is_some(), "expected rejection for {filter:?}");
                assert_eq!(direct_error, overlay_error);
                assert!(!compile_filter(&filter, &interner).matches(&record, &ctx));
                assert!(!compile_filter_with_context(&filter, &ctx).matches(&record, &ctx));
            }
        }
    }
}

#[test]
fn valid_unknown_field_patterns_fold_to_false() {
    let interner = Interner::new();
    let refs = empty_refs();
    let ctx = FilterContext::new(&interner, &refs)
        .with_overlay_ids_snapshot(Some(std::sync::Arc::new(shamir_collections::new_fx_map())));
    let field = vec!["missing".to_string()];
    for filter in [
        Filter::Regex {
            field: field.clone(),
            pattern: "abc".into(),
        },
        Filter::Like {
            field: field.clone(),
            pattern: "abc%".into(),
        },
        Filter::ILike {
            field: field.clone(),
            pattern: "abc%".into(),
        },
    ] {
        assert!(matches!(
            try_compile_filter(&filter, &interner),
            Ok(FilterNode::False)
        ));
        assert!(matches!(
            try_compile_filter_with_context(&filter, &ctx),
            Ok(FilterNode::False)
        ));
    }
}
