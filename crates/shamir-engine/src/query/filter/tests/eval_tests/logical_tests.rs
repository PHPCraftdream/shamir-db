use crate::query::filter::eval::compile_filter;
use crate::query::filter::eval_context::FilterContext;
use crate::query::filter::{Filter, FilterValue};
use shamir_types::core::interner::Interner;

use super::helpers::{empty_refs, make_alice_record, make_nested_record};

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
