use crate::query::filter::eval::compile_filter;
use crate::query::filter::eval_context::FilterContext;
use crate::query::filter::{Filter, FilterValue};
use shamir_types::core::interner::Interner;

use super::helpers::{empty_refs, make_date_record};

#[test]
fn test_field_ref_gt() {
    let interner = Interner::new();
    let record = make_date_record(&interner);
    let refs = empty_refs();
    let ctx = FilterContext::new(&interner, &refs);

    // end_date (200) > start_date (100) => true
    let filter = Filter::Gt {
        field: vec!["end_date".to_string()],
        value: FilterValue::FieldRef {
            path: vec!["start_date".to_string()],
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
        field: vec!["start_date".to_string()],
        value: FilterValue::FieldRef {
            path: vec!["end_date".to_string()],
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
        field: vec!["start_date".to_string()],
        value: FilterValue::FieldRef {
            path: vec!["start_date".to_string()],
        },
    };
    let cb = compile_filter(&filter, &interner);
    assert!(cb.matches(&record, &ctx));
}
