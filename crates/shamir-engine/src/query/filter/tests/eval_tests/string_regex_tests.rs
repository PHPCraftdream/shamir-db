use crate::query::filter::eval::compile_filter;
use crate::query::filter::eval_context::FilterContext;
use crate::query::filter::Filter;
use shamir_types::core::interner::Interner;

use super::helpers::{empty_refs, make_alice_record};

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
