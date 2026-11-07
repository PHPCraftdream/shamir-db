//! apply_select_value QueryValue assertions and the QV aggregate fixtures
//! (former `qv_postprocess_tests.rs`, Stage D).

use shamir_funclib::scalar_resolver::ScalarResolver;
use shamir_types::types::value::QueryValue;

use crate::query::read::exec::{apply_distinct_qv, apply_select_value};
use crate::query::read::order::apply_order_by_qv;
use crate::query::read::{OrderBy, Select};
use shamir_types::core::interner::{Interner, InternerKey, TouchInd};
use shamir_types::types::common::new_map;
use shamir_types::types::record_id::RecordId;
use shamir_types::types::value::InnerValue;

// ============================================================================
// Stage D: apply_select_value explicit assertions
// ============================================================================

/// Intern a string, returning its u64 id.
pub(in crate::query::read::tests) fn intern(interner: &Interner, s: &str) -> u64 {
    match interner.touch_ind(s) {
        Ok(TouchInd::New(k)) | Ok(TouchInd::Exists(k)) => k.id(),
        Err(e) => panic!("intern failed: {e}"),
    }
}

/// Build a record: `{ name: Str, age: Int, city: Str, score: F64 }`.
pub(in crate::query::read::tests) fn make_record(
    interner: &Interner,
    name: &str,
    age: i64,
    city: &str,
    score: f64,
) -> InnerValue {
    let mut map = new_map();
    map.insert(
        InternerKey::new(intern(interner, "name")),
        InnerValue::Str(name.into()),
    );
    map.insert(
        InternerKey::new(intern(interner, "age")),
        InnerValue::Int(age),
    );
    map.insert(
        InternerKey::new(intern(interner, "city")),
        InnerValue::Str(city.into()),
    );
    map.insert(
        InternerKey::new(intern(interner, "score")),
        InnerValue::F64(score),
    );
    InnerValue::Map(map)
}

pub(in crate::query::read::tests) fn make_test_records(
    interner: &Interner,
) -> Vec<(RecordId, InnerValue)> {
    vec![
        (
            RecordId::new(),
            make_record(interner, "Alice", 30, "NYC", 1.5),
        ),
        (RecordId::new(), make_record(interner, "Bob", 25, "LA", 2.5)),
        (
            RecordId::new(),
            make_record(interner, "Carol", 35, "NYC", 3.5),
        ),
        (
            RecordId::new(),
            make_record(interner, "Dave", 25, "LA", 0.5),
        ),
    ]
}

#[test]
fn select_value_specific_fields() {
    let interner = Interner::default();
    let records = make_test_records(&interner);
    let select = Select::fields(["name", "age", "city"]);

    let result = apply_select_value(
        &records,
        &select,
        &interner,
        ScalarResolver::builtins_only(),
    )
    .unwrap();
    assert_eq!(result.len(), 4);

    assert_eq!(result[0]["name"], QueryValue::Str("Alice".into()));
    assert_eq!(result[0]["age"], QueryValue::Int(30));
    assert_eq!(result[0]["city"], QueryValue::Str("NYC".into()));
    assert_eq!(result[1]["name"], QueryValue::Str("Bob".into()));
    assert_eq!(result[1]["age"], QueryValue::Int(25));
    assert_eq!(result[2]["name"], QueryValue::Str("Carol".into()));
    assert_eq!(result[2]["age"], QueryValue::Int(35));
}

#[test]
fn select_value_all_returns_all_fields() {
    let interner = Interner::default();
    let records = make_test_records(&interner);
    let select = Select::all();

    let result = apply_select_value(
        &records,
        &select,
        &interner,
        ScalarResolver::builtins_only(),
    )
    .unwrap();
    assert_eq!(result.len(), 4);

    // All four fields present
    assert_eq!(result[0]["name"], QueryValue::Str("Alice".into()));
    assert_eq!(result[0]["age"], QueryValue::Int(30));
    assert_eq!(result[0]["city"], QueryValue::Str("NYC".into()));
    assert_eq!(result[0]["score"], QueryValue::F64(1.5));
}

#[test]
fn path_b_distinct_order_qv() {
    // Path B: apply_select_value -> distinct_qv -> order_by_qv
    let interner = Interner::default();
    let records = make_test_records(&interner);
    let select = Select::fields(["city", "age"]);

    let qv_result = apply_select_value(
        &records,
        &select,
        &interner,
        ScalarResolver::builtins_only(),
    )
    .unwrap();
    // 4 rows: {city:NYC,age:30},{city:LA,age:25},{city:NYC,age:35},{city:LA,age:25}
    // After distinct: {NYC,30},{LA,25},{NYC,35} → 3 distinct rows
    let q_distinct = apply_distinct_qv(qv_result);
    assert_eq!(q_distinct.len(), 3);

    let mut q_sorted = q_distinct;
    apply_order_by_qv(&mut q_sorted, &OrderBy::asc("age"));

    // sorted by age: 25, 30, 35
    assert_eq!(q_sorted[0]["age"], QueryValue::Int(25));
    assert_eq!(q_sorted[1]["age"], QueryValue::Int(30));
    assert_eq!(q_sorted[2]["age"], QueryValue::Int(35));
}
