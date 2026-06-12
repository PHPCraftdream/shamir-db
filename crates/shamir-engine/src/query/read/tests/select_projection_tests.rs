//! Tests for SelectProjection — verifies that `project` and `project_value`
//! produce semantically equivalent output (same key-value pairs).
//!
//! Note: `serde_json::Map` uses a `BTreeMap` internally (sorted keys), while
//! `QueryValue::Map` uses an insertion-ordered `TMap`.  Byte equality of
//! msgpack is therefore not guaranteed for the select-all case.  We compare
//! by converting both outputs to a canonical sorted-JSON string instead.

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

/// Serialize QueryValue to a canonically-sorted JSON string for comparison.
/// Sorting is done by serializing to serde_json::Value (which uses BTreeMap
/// internally) and then serializing back to a string.
fn canonical_json(qv: &QueryValue) -> String {
    let j: serde_json::Value = serde_json::to_value(qv).expect("to_value");
    serde_json::to_string(&j).expect("to_string")
}

/// Canonical "select all" — project and project_value must have the same
/// key-value content.
#[test]
fn project_and_project_value_select_all_are_wire_equivalent() {
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

    let json_val = proj.project(&record, &interner);
    let qval = proj.project_value(&record, &interner);

    // Canonicalize both through serde_json sorted-key serialization.
    let canonical_json_val = serde_json::to_string(&json_val).expect("json_val to_string");
    let canonical_qval = canonical_json(&qval);

    assert_eq!(
        canonical_json_val, canonical_qval,
        "project and project_value differ for select-all"
    );
}

/// Explicit field projection — only named fields are returned.
#[test]
fn project_and_project_value_field_projection_are_wire_equivalent() {
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

    let json_val = proj.project(&record, &interner);
    let qval = proj.project_value(&record, &interner);

    // Field projection output keys are pre-ordered (same order both paths),
    // so byte-level msgpack equality holds here.
    let json_bytes = serde_json::to_vec(&json_val).expect("json serialize");
    let qval_from_json: QueryValue = serde_json::from_slice(&json_bytes).expect("json deserialize");

    let bytes_a = rmp_serde::to_vec_named(&qval_from_json).expect("msgpack a");
    let bytes_b = rmp_serde::to_vec_named(&qval).expect("msgpack b");

    assert_eq!(
        bytes_a, bytes_b,
        "project and project_value differ for field projection"
    );
}
