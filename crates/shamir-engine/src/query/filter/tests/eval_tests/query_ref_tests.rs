use crate::query::filter::eval::compile_filter;
use crate::query::filter::eval_context::FilterContext;
use crate::query::filter::{Filter, FilterValue};
use crate::query::read::{QueryRecord, QueryResult};
use shamir_types::core::interner::Interner;
use shamir_types::mpack;
use shamir_types::types::common::{new_map, TMap};
use shamir_types::types::value::InnerValue;

use super::helpers::empty_refs;

#[test]
fn test_query_ref_eq() {
    let interner = Interner::new();

    // Record: {user_id: 42}
    let mut map = new_map();
    let k_user_id = interner.touch_ind("user_id").unwrap().into_key();
    map.insert(k_user_id, InnerValue::Int(42));
    let record = InnerValue::Map(map);

    // QueryResult: users => [{id: 42, name: "Alice"}]
    let mut refs: TMap<String, QueryResult> = new_map();
    refs.insert(
        "users".to_string(),
        QueryResult {
            records: vec![QueryRecord::Direct(mpack!({"id": 42, "name": "Alice"}))],
            stats: None,
            pagination: None,
            value: None,
            explain: None,
            skipped: false,
            versions: None,
        },
    );

    let ctx = FilterContext::new(&interner, &refs);

    let filter = Filter::Eq {
        field: vec!["user_id".to_string()],
        value: FilterValue::QueryRef {
            alias: "users".to_string(),
            path: Some("[0].id".to_string()),
        },
    };
    let cb = compile_filter(&filter, &interner);
    assert!(cb.matches(&record, &ctx));
}

#[test]
fn test_query_ref_no_match() {
    let interner = Interner::new();

    let mut map = new_map();
    let k_user_id = interner.touch_ind("user_id").unwrap().into_key();
    map.insert(k_user_id, InnerValue::Int(99));
    let record = InnerValue::Map(map);

    let mut refs: TMap<String, QueryResult> = new_map();
    refs.insert(
        "users".to_string(),
        QueryResult {
            records: vec![QueryRecord::Direct(mpack!({"id": 42}))],
            stats: None,
            pagination: None,
            value: None,
            explain: None,
            skipped: false,
            versions: None,
        },
    );

    let ctx = FilterContext::new(&interner, &refs);

    let filter = Filter::Eq {
        field: vec!["user_id".to_string()],
        value: FilterValue::QueryRef {
            alias: "users".to_string(),
            path: Some("[0].id".to_string()),
        },
    };
    let cb = compile_filter(&filter, &interner);
    assert!(!cb.matches(&record, &ctx)); // 99 != 42
}

#[test]
fn test_query_ref_missing_alias() {
    let interner = Interner::new();

    let mut map = new_map();
    let k_user_id = interner.touch_ind("user_id").unwrap().into_key();
    map.insert(k_user_id, InnerValue::Int(42));
    let record = InnerValue::Map(map);

    let refs = empty_refs();
    let ctx = FilterContext::new(&interner, &refs);

    let filter = Filter::Eq {
        field: vec!["user_id".to_string()],
        value: FilterValue::QueryRef {
            alias: "nonexistent".to_string(),
            path: Some("[0].id".to_string()),
        },
    };
    let cb = compile_filter(&filter, &interner);
    // Missing alias => resolve returns None => Eq fails
    assert!(!cb.matches(&record, &ctx));
}
