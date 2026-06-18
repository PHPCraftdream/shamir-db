//! Tests for SelectProjection::project_value.
//!
//! Verifies that `project_value` produces the correct key-value pairs
//! for select-all and explicit field projections.
//!
//! The old JSON-twin parity tests (comparing `project` against `project_value`)
//! have been replaced with concrete expected-value assertions after `project`
//! was removed in J1 JSON elimination.

use std::sync::Arc;

use shamir_types::core::interner::Interner;
use shamir_types::types::common::new_map_wc;
use shamir_types::types::value::{InnerValue, QueryValue};

use crate::query::read::select_projection::SelectProjection;
use crate::query::read::{Select, SelectItem};

/// Build an InnerValue::Map with the given string keys, interning them into
/// `interner`, and associate the provided values.
fn make_record(interner: &Interner, fields: Vec<(&str, InnerValue)>) -> InnerValue {
    let mut m = new_map_wc(fields.len());
    for (k, v) in fields {
        let key = interner.touch_ind(k).expect("intern key").into_key();
        m.insert(key, v);
    }
    InnerValue::Map(m)
}

/// SELECT * via project_value returns all fields.
#[test]
fn project_value_select_all_returns_all_fields() {
    let interner = Arc::new(Interner::new());
    let record = make_record(
        &interner,
        vec![
            ("name", InnerValue::Str("Alice".to_string())),
            ("age", InnerValue::Int(30)),
            ("active", InnerValue::Bool(true)),
        ],
    );

    let select = Select::all();
    let proj = SelectProjection::new(&select, &interner);
    let qval = proj.project_value(&record, &interner);

    match &qval {
        QueryValue::Map(m) => {
            assert_eq!(m.get("name"), Some(&QueryValue::Str("Alice".to_string())));
            assert_eq!(m.get("age"), Some(&QueryValue::Int(30)));
            assert_eq!(m.get("active"), Some(&QueryValue::Bool(true)));
            assert_eq!(m.len(), 3);
        }
        _ => panic!("expected QueryValue::Map, got {:?}", qval),
    }
}

/// Explicit field projection returns only the named fields.
#[test]
fn project_value_field_projection_returns_named_fields_only() {
    let interner = Arc::new(Interner::new());
    let record = make_record(
        &interner,
        vec![
            ("name", InnerValue::Str("Bob".to_string())),
            ("age", InnerValue::Int(25)),
            ("score", InnerValue::F64(9.5)),
        ],
    );

    let select = Select {
        items: vec![
            SelectItem::Field {
                path: vec!["name".to_string()],
                alias: None,
            },
            SelectItem::Field {
                path: vec!["age".to_string()],
                alias: Some("years".to_string()),
            },
        ],
        distinct: false,
    };
    let proj = SelectProjection::new(&select, &interner);
    let qval = proj.project_value(&record, &interner);

    match &qval {
        QueryValue::Map(m) => {
            // "name" is projected as-is
            assert_eq!(m.get("name"), Some(&QueryValue::Str("Bob".to_string())));
            // "age" is projected with alias "years"
            assert_eq!(m.get("years"), Some(&QueryValue::Int(25)));
            // "age" key itself is absent (aliased)
            assert!(
                !m.contains_key("age"),
                "original key should not appear when aliased"
            );
            // "score" is not in the select list
            assert!(
                !m.contains_key("score"),
                "non-selected field should be absent"
            );
            assert_eq!(m.len(), 2);
        }
        _ => panic!("expected QueryValue::Map, got {:?}", qval),
    }
}

/// Missing field in projection results in QueryValue::Null.
#[test]
fn project_value_missing_field_is_null() {
    let interner = Arc::new(Interner::new());
    let record = make_record(
        &interner,
        vec![("name", InnerValue::Str("Carol".to_string()))],
    );

    let select = Select {
        items: vec![
            SelectItem::Field {
                path: vec!["name".to_string()],
                alias: None,
            },
            SelectItem::Field {
                path: vec!["nonexistent".to_string()],
                alias: None,
            },
        ],
        distinct: false,
    };
    let proj = SelectProjection::new(&select, &interner);
    let qval = proj.project_value(&record, &interner);

    match &qval {
        QueryValue::Map(m) => {
            assert_eq!(m.get("name"), Some(&QueryValue::Str("Carol".to_string())));
            assert_eq!(m.get("nonexistent"), Some(&QueryValue::Null));
        }
        _ => panic!("expected QueryValue::Map"),
    }
}

/// Empty select (no items) returns QueryValue::Map with all fields (is_all path).
#[test]
fn project_value_empty_items_returns_all() {
    let interner = Arc::new(Interner::new());
    let record = make_record(
        &interner,
        vec![("x", InnerValue::Int(1)), ("y", InnerValue::Int(2))],
    );

    let select = Select {
        items: vec![],
        distinct: false,
    };
    let proj = SelectProjection::new(&select, &interner);
    let qval = proj.project_value(&record, &interner);

    match &qval {
        QueryValue::Map(m) => {
            assert_eq!(m.get("x"), Some(&QueryValue::Int(1)));
            assert_eq!(m.get("y"), Some(&QueryValue::Int(2)));
            assert_eq!(m.len(), 2);
        }
        _ => panic!("expected QueryValue::Map"),
    }
}
