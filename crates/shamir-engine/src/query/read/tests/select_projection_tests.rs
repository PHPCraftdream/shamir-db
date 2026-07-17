//! Tests for SelectProjection::project_value.
//!
//! Verifies that `project_value` produces the correct key-value pairs
//! for select-all and explicit field projections.
//!
//! The old parity tests (comparing `project` against `project_value`)
//! have been replaced with concrete expected-value assertions after `project`
//! was removed in J1 elimination.

use std::sync::Arc;

use shamir_types::core::interner::Interner;
use shamir_types::types::common::new_map_wc;
use shamir_types::types::value::{InnerValue, QueryValue};

use crate::query::filter::{Cond, Filter, FilterValue};
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

/// #665 (#643 gap) — engine-level integration test through the real
/// production seam: `SelectProjection::new` is the one production call site
/// that populates `CondCache` (via `prescan_cond_cache` walking `funcs`
/// once, at query-compile time). This test builds a `Select` with a
/// `SelectItem::Function` (`strings/upper`) whose sole arg embeds a
/// `FilterValue::Cond` (`score > 50 ? "high" : "low"`), builds
/// `SelectProjection::new` ONCE (populating `funcs_cond_cache` internally),
/// then calls `project_value` for TWO records with differing `score`
/// values on that SAME projection instance. If the cache-hit branch inside
/// `resolve_filter_query`'s `Cond` arm ever regressed to a frozen/stale
/// answer (baked in at whichever record first hit the cache), the SECOND
/// record's assertion below would incorrectly still see "HIGH" — proving
/// the SAME internal `funcs_cond_cache`, built once, correctly serves both
/// calls with per-record-correct answers.
#[test]
fn project_value_cond_function_projection_caches_and_evaluates_per_record() {
    let interner = Arc::new(Interner::new());
    // `SelectProjection::new`'s prescan compiles the $cond's condition
    // immediately via `compile_filter`, whose field-path resolution
    // (`intern_field_path_compact` → `Interner::get_ind`) is lookup-only —
    // it never inserts. "score" must already be interned before `new()`
    // runs, or the compiled field path fails to resolve and the node folds
    // to `FilterNode::False` (always "low"), independent of any record.
    // Mirrors production: field names are interned by the write path before
    // a query ever compiles a filter that references them.
    interner.touch_ind("score").unwrap();

    let cond_arg = FilterValue::Cond {
        cond: Box::new(Cond::new(
            Filter::Gt {
                field: vec!["score".to_string()],
                value: FilterValue::Int(50),
            },
            FilterValue::String("high".to_string()),
            FilterValue::String("low".to_string()),
        )),
    };

    let select = Select {
        items: vec![SelectItem::Function {
            name: "strings/upper".to_string(),
            args: vec![cond_arg],
            alias: Some("bucket".to_string()),
        }],
        distinct: false,
    };

    // Built ONCE — this is what populates `funcs_cond_cache` internally via
    // `prescan_cond_cache`, the real production call site under test.
    let proj = SelectProjection::new(&select, &interner);

    let record_high = make_record(&interner, vec![("score", InnerValue::Int(80))]);
    let record_low = make_record(&interner, vec![("score", InnerValue::Int(20))]);

    // SAME `proj` (SAME internal cond cache) serves both calls.
    let qval_high = proj.project_value(&record_high, &interner);
    let qval_low = proj.project_value(&record_low, &interner);

    match &qval_high {
        QueryValue::Map(m) => {
            assert_eq!(
                m.get("bucket"),
                Some(&QueryValue::Str("HIGH".to_string())),
                "score=80 must project through the cached $cond to \"high\" \
                 (upper-cased by strings/upper)"
            );
        }
        _ => panic!("expected QueryValue::Map, got {:?}", qval_high),
    }
    match &qval_low {
        QueryValue::Map(m) => {
            assert_eq!(
                m.get("bucket"),
                Some(&QueryValue::Str("LOW".to_string())),
                "score=20 must project through the SAME cached $cond to \"low\" — \
                 proving the SAME funcs_cond_cache, built once, is genuinely \
                 re-evaluated per record rather than frozen to the first call's answer"
            );
        }
        _ => panic!("expected QueryValue::Map, got {:?}", qval_low),
    }
}
