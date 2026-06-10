use crate::query::filter::eval::compile_filter;
use crate::query::filter::eval_context::FilterContext;
use crate::query::filter::Filter;
use shamir_types::core::interner::Interner;

use super::helpers::{empty_refs, make_alice_record, make_nullable_record};

// ============================================================================
// IsNull / IsNotNull
// ============================================================================

#[test]
fn test_is_null_on_null_field() {
    let interner = Interner::new();
    let record = make_nullable_record(&interner);
    let refs = empty_refs();
    let ctx = FilterContext::new(&interner, &refs);

    let filter = Filter::IsNull {
        field: vec!["deleted_at".to_string()],
    };
    let cb = compile_filter(&filter, &interner);
    assert!(cb.matches(&record, &ctx));
}

#[test]
fn test_is_null_on_existing_field() {
    let interner = Interner::new();
    let record = make_nullable_record(&interner);
    let refs = empty_refs();
    let ctx = FilterContext::new(&interner, &refs);

    let filter = Filter::IsNull {
        field: vec!["name".to_string()],
    };
    let cb = compile_filter(&filter, &interner);
    assert!(!cb.matches(&record, &ctx)); // name = "Carol", not null
}

#[test]
fn test_is_null_on_missing_field() {
    let interner = Interner::new();
    let record = make_nullable_record(&interner);
    let refs = empty_refs();
    let ctx = FilterContext::new(&interner, &refs);

    let filter = Filter::IsNull {
        field: vec!["nonexistent".to_string()],
    };
    // "nonexistent" not in interner yet, so compile treats as always-null
    let cb = compile_filter(&filter, &interner);
    assert!(cb.matches(&record, &ctx));
}

#[test]
fn test_is_not_null_on_existing_field() {
    let interner = Interner::new();
    let record = make_nullable_record(&interner);
    let refs = empty_refs();
    let ctx = FilterContext::new(&interner, &refs);

    let filter = Filter::IsNotNull {
        field: vec!["name".to_string()],
    };
    let cb = compile_filter(&filter, &interner);
    assert!(cb.matches(&record, &ctx));
}

#[test]
fn test_is_not_null_on_null_field() {
    let interner = Interner::new();
    let record = make_nullable_record(&interner);
    let refs = empty_refs();
    let ctx = FilterContext::new(&interner, &refs);

    let filter = Filter::IsNotNull {
        field: vec!["deleted_at".to_string()],
    };
    let cb = compile_filter(&filter, &interner);
    assert!(!cb.matches(&record, &ctx));
}

// ============================================================================
// Exists / NotExists
// ============================================================================

#[test]
fn test_exists_present_field() {
    let interner = Interner::new();
    let record = make_alice_record(&interner);
    let refs = empty_refs();
    let ctx = FilterContext::new(&interner, &refs);

    let filter = Filter::Exists {
        field: vec!["name".to_string()],
    };
    let cb = compile_filter(&filter, &interner);
    assert!(cb.matches(&record, &ctx));
}

#[test]
fn test_exists_null_field() {
    let interner = Interner::new();
    let record = make_nullable_record(&interner);
    let refs = empty_refs();
    let ctx = FilterContext::new(&interner, &refs);

    // deleted_at exists in the record (value is Null but key is present)
    let filter = Filter::Exists {
        field: vec!["deleted_at".to_string()],
    };
    let cb = compile_filter(&filter, &interner);
    assert!(cb.matches(&record, &ctx)); // Exists checks presence, not value
}

#[test]
fn test_exists_missing_field() {
    let interner = Interner::new();
    let record = make_alice_record(&interner);
    let refs = empty_refs();
    let ctx = FilterContext::new(&interner, &refs);

    // "email" doesn't exist in alice record, but also not in interner
    let filter = Filter::Exists {
        field: vec!["email".to_string()],
    };
    let cb = compile_filter(&filter, &interner);
    assert!(!cb.matches(&record, &ctx));
}

#[test]
fn test_exists_missing_field_in_record_but_in_interner() {
    let interner = Interner::new();
    let record = make_alice_record(&interner);
    // Intern "email" so the path resolves, but it's not in the record
    interner.touch_ind("email").unwrap();
    let refs = empty_refs();
    let ctx = FilterContext::new(&interner, &refs);

    let filter = Filter::Exists {
        field: vec!["email".to_string()],
    };
    let cb = compile_filter(&filter, &interner);
    assert!(!cb.matches(&record, &ctx)); // field not in record
}

#[test]
fn test_not_exists_missing_field() {
    let interner = Interner::new();
    let record = make_alice_record(&interner);
    let refs = empty_refs();
    let ctx = FilterContext::new(&interner, &refs);

    let filter = Filter::NotExists {
        field: vec!["email".to_string()],
    };
    let cb = compile_filter(&filter, &interner);
    assert!(cb.matches(&record, &ctx)); // field not interned => TrueCallback
}

#[test]
fn test_not_exists_present_field() {
    let interner = Interner::new();
    let record = make_alice_record(&interner);
    let refs = empty_refs();
    let ctx = FilterContext::new(&interner, &refs);

    let filter = Filter::NotExists {
        field: vec!["name".to_string()],
    };
    let cb = compile_filter(&filter, &interner);
    assert!(!cb.matches(&record, &ctx)); // "name" exists
}

#[test]
fn test_not_exists_null_field() {
    let interner = Interner::new();
    let record = make_nullable_record(&interner);
    let refs = empty_refs();
    let ctx = FilterContext::new(&interner, &refs);

    // deleted_at has Null value but IS present in the record
    let filter = Filter::NotExists {
        field: vec!["deleted_at".to_string()],
    };
    let cb = compile_filter(&filter, &interner);
    assert!(!cb.matches(&record, &ctx)); // key exists (even though value is Null)
}

#[test]
fn test_not_exists_field_in_interner_but_not_record() {
    let interner = Interner::new();
    let record = make_alice_record(&interner);
    interner.touch_ind("email").unwrap();
    let refs = empty_refs();
    let ctx = FilterContext::new(&interner, &refs);

    let filter = Filter::NotExists {
        field: vec!["email".to_string()],
    };
    let cb = compile_filter(&filter, &interner);
    assert!(cb.matches(&record, &ctx)); // "email" not in record
}
