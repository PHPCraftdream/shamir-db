//! Tests for [`record_view_to_id_msgpack`] — the S-read projection primitive.
//!
//! Strategy: build a wide record with a real `Interner`, project a subset of
//! fields, then assert the result by:
//! 1. Decoding the projected bytes back through `RecordView::new` +
//!    `record_view_to_query_value` to get a name-keyed `QueryValue::Map`.
//! 2. Comparing it to the expected name-keyed map for the projected fields only.
//!
//! This also exercises the field ORDER guarantee (output order = `selected_ids`
//! order) and the absent-field omission guarantee.
//!
//! Tests are WRITTEN but NOT RUN (orchestrator runs `./scripts/test.sh`).

use std::f64::consts::PI;

use crate::codecs::interned::{record_view_to_id_msgpack, record_view_to_query_value};
use crate::core::interner::{Interner, InternerKey};
use crate::record_view::RecordView;
use crate::types::common::new_map_wc;
use crate::types::value::{InnerValue, QueryValue, Value};

/// Intern a field name and return its `InternerKey`.
fn ik(interner: &Interner, s: &str) -> InternerKey {
    interner.touch_ind(s).unwrap().into_key()
}

// ---------------------------------------------------------------------------
// P2-A: project a subset of fields; de-intern and compare to expected map
// ---------------------------------------------------------------------------

/// Build a 4-field record `{a, b, c, d}`. Project `[a, c]`. Verify the decoded
/// result equals `{a: va, c: vc}` — the correct subset, names resolved.
#[test]
fn projection_subset_two_fields() {
    let interner = Interner::new();
    let ka = ik(&interner, "a");
    let kb = ik(&interner, "b");
    let kc = ik(&interner, "c");
    let kd = ik(&interner, "d");

    let mut m = new_map_wc(4);
    m.insert(ka.clone(), InnerValue::Int(10));
    m.insert(kb.clone(), InnerValue::Int(20));
    m.insert(kc.clone(), InnerValue::Str("hello".to_owned()));
    m.insert(kd.clone(), InnerValue::Bool(true));

    let record_bytes = InnerValue::Map(m).to_bytes().expect("to_bytes");
    let view = RecordView::new(&record_bytes).expect("RecordView::new");

    // Project only {a, c}.
    let selected = vec![ka, kc];
    let projected = record_view_to_id_msgpack(&view, &selected).expect("record_view_to_id_msgpack");

    // Decode the projected bytes back to a name-keyed QueryValue.
    let proj_view = RecordView::new(&projected).expect("RecordView::new projected");
    let decoded = record_view_to_query_value(&proj_view, &interner)
        .expect("record_view_to_query_value projected");

    // Build the expected name-keyed map: {a: 10, c: "hello"}.
    let mut expected = new_map_wc(2);
    expected.insert("a".to_owned(), Value::Int(10));
    expected.insert("c".to_owned(), Value::Str("hello".to_owned()));
    let expected_qv = QueryValue::Map(expected);

    assert_eq!(
        decoded, expected_qv,
        "projected subset mismatch:\n  decoded:   {decoded:?}\n  expected:  {expected_qv:?}"
    );
}

// ---------------------------------------------------------------------------
// P2-B: absent selected id is silently omitted
// ---------------------------------------------------------------------------

/// If `selected_ids` contains an id that is not present in the record,
/// it is skipped and the output map contains only the present ids.
#[test]
fn projection_absent_id_omitted() {
    let interner = Interner::new();
    let ka = ik(&interner, "x");
    let kz = ik(&interner, "z"); // will NOT be inserted in the record

    let mut m = new_map_wc(1);
    m.insert(ka.clone(), InnerValue::Int(99));

    let record_bytes = InnerValue::Map(m).to_bytes().expect("to_bytes");
    let view = RecordView::new(&record_bytes).expect("RecordView::new");

    // Select both `x` (present) and `z` (absent).
    let selected = vec![ka, kz];
    let projected = record_view_to_id_msgpack(&view, &selected).expect("record_view_to_id_msgpack");

    let proj_view = RecordView::new(&projected).expect("RecordView::new projected");
    // Only `x` should appear.
    assert_eq!(
        proj_view.len(),
        1,
        "expected 1 entry in projection (absent id omitted), got {}",
        proj_view.len()
    );

    let decoded = record_view_to_query_value(&proj_view, &interner).expect("decode projected");
    let mut expected = new_map_wc(1);
    expected.insert("x".to_owned(), Value::Int(99));
    assert_eq!(decoded, QueryValue::Map(expected));
}

// ---------------------------------------------------------------------------
// P2-C: key order follows selected_ids order
// ---------------------------------------------------------------------------

/// The output map's iteration order matches `selected_ids` order,
/// even when the record stores fields in a different insertion order.
///
/// We verify by decoding the projected bytes via `RecordView::fields()` and
/// collecting the field names in iteration order — they must match
/// `selected_ids` order.
#[test]
fn projection_key_order_follows_selected_ids() {
    let interner = Interner::new();
    // Insert in order a, b, c, d in the source record.
    let ka = ik(&interner, "field_a");
    let kb = ik(&interner, "field_b");
    let kc = ik(&interner, "field_c");
    let kd = ik(&interner, "field_d");

    let mut m = new_map_wc(4);
    m.insert(ka.clone(), InnerValue::Int(1));
    m.insert(kb.clone(), InnerValue::Int(2));
    m.insert(kc.clone(), InnerValue::Int(3));
    m.insert(kd.clone(), InnerValue::Int(4));

    let record_bytes = InnerValue::Map(m).to_bytes().expect("to_bytes");
    let view = RecordView::new(&record_bytes).expect("RecordView::new");

    // Project in REVERSE order: d, b.
    let selected = vec![kd.clone(), kb.clone()];
    let projected = record_view_to_id_msgpack(&view, &selected).expect("record_view_to_id_msgpack");

    let proj_view = RecordView::new(&projected).expect("RecordView::new projected");

    // Collect the key ids from the projected bytes in iteration order.
    let field_ids: Vec<u64> = proj_view.fields().map(|(k, _v)| k.id()).collect();

    assert_eq!(
        field_ids,
        vec![kd.id(), kb.id()],
        "projected key order does not match selected_ids order: {field_ids:?}"
    );
}

// ---------------------------------------------------------------------------
// P2-D: wide record, project one field; check byte-level correctness
// ---------------------------------------------------------------------------

/// Build a 20-field record. Project a single field. Verify the projected map
/// decodes correctly and the single present field has the right value.
#[test]
fn projection_single_field_wide_record() {
    let interner = Interner::new();
    let n = 20usize;
    let mut keys: Vec<InternerKey> = Vec::with_capacity(n);
    let mut m = new_map_wc(n);

    for i in 0..n {
        let k = ik(&interner, &format!("field_{i}"));
        m.insert(k.clone(), InnerValue::Int(i as i64 * 10));
        keys.push(k);
    }

    let record_bytes = InnerValue::Map(m).to_bytes().expect("to_bytes");
    let view = RecordView::new(&record_bytes).expect("RecordView::new");

    // Project field_7 only.
    let selected = vec![keys[7].clone()];
    let projected = record_view_to_id_msgpack(&view, &selected).expect("record_view_to_id_msgpack");

    let proj_view = RecordView::new(&projected).expect("RecordView::new projected");
    assert_eq!(proj_view.len(), 1, "expected 1 field in projection");

    let decoded = record_view_to_query_value(&proj_view, &interner).expect("decode projected");

    let mut expected = new_map_wc(1);
    expected.insert("field_7".to_owned(), Value::Int(70));
    assert_eq!(decoded, QueryValue::Map(expected));
}

// ---------------------------------------------------------------------------
// P2-E: empty selected_ids → empty map output
// ---------------------------------------------------------------------------

#[test]
fn projection_empty_selected_ids_empty_output() {
    let interner = Interner::new();
    let ka = ik(&interner, "p");
    let mut m = new_map_wc(1);
    m.insert(ka, InnerValue::Int(1));

    let record_bytes = InnerValue::Map(m).to_bytes().expect("to_bytes");
    let view = RecordView::new(&record_bytes).expect("RecordView::new");

    let projected = record_view_to_id_msgpack(&view, &[]).expect("record_view_to_id_msgpack empty");

    let proj_view = RecordView::new(&projected).expect("RecordView::new projected");
    assert_eq!(
        proj_view.len(),
        0,
        "empty selected_ids should produce a 0-entry map"
    );
}

// ---------------------------------------------------------------------------
// P2-F: projected bytes re-decoded through original interner match name subset
// ---------------------------------------------------------------------------

/// Comprehensive parity: build a 5-field record, project 3, verify that
/// de-interning the projection gives exactly those 3 fields with the correct
/// values, matching what you would get by building the subset map directly.
#[test]
fn projection_parity_with_direct_build() {
    let interner = Interner::new();
    let ka = ik(&interner, "alpha");
    let kb = ik(&interner, "beta");
    let kc = ik(&interner, "gamma");
    let kd = ik(&interner, "delta");
    let ke = ik(&interner, "epsilon");

    let mut m = new_map_wc(5);
    m.insert(ka.clone(), InnerValue::Str("va".to_owned()));
    m.insert(kb.clone(), InnerValue::Int(42));
    m.insert(kc.clone(), InnerValue::Bool(false));
    m.insert(kd.clone(), InnerValue::Null);
    m.insert(ke.clone(), InnerValue::F64(PI));

    let record_bytes = InnerValue::Map(m).to_bytes().expect("to_bytes");
    let view = RecordView::new(&record_bytes).expect("RecordView::new");

    // Project alpha, gamma, epsilon (in that order).
    let selected = vec![ka, kc, ke];
    let projected = record_view_to_id_msgpack(&view, &selected).expect("record_view_to_id_msgpack");

    let proj_view = RecordView::new(&projected).expect("RecordView::new projected");
    let decoded = record_view_to_query_value(&proj_view, &interner).expect("decode projected");

    // Build the expected map directly.
    let mut expected = new_map_wc(3);
    expected.insert("alpha".to_owned(), Value::Str("va".to_owned()));
    expected.insert("gamma".to_owned(), Value::Bool(false));
    expected.insert("epsilon".to_owned(), Value::F64(PI));
    let expected_qv = QueryValue::Map(expected);

    assert_eq!(
        decoded, expected_qv,
        "projection parity failed:\n  decoded:  {decoded:?}\n  expected: {expected_qv:?}"
    );
}
