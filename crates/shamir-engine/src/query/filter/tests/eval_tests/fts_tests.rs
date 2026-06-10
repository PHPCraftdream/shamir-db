use crate::query::filter::eval::compile_filter;
use crate::query::filter::eval_context::FilterContext;
use crate::query::filter::Filter;
use shamir_types::core::interner::Interner;

use super::helpers::{empty_refs, make_alice_record, make_body_record};

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
