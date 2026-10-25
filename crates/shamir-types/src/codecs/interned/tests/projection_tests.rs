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

// ---------------------------------------------------------------------------
// RV-TYPES-93D11F4F: projection via the offset index (behavioural parity)
// ---------------------------------------------------------------------------

/// Behavioural parity lock for the indexed projection (perf card — the
/// O(fields × selected) → O(fields + selected) property itself is structural).
#[test]
fn rv_types_93d11f4f_projection_indexed_parity() {
    let interner = Interner::new();
    let ka = ik(&interner, "alpha");
    let kb = ik(&interner, "bravo");
    let kc = ik(&interner, "charlie");
    let kd = ik(&interner, "delta");
    let ke = ik(&interner, "echo");
    let kf = ik(&interner, "foxtrot");
    let kg = ik(&interner, "golf");
    let kh = ik(&interner, "hotel");

    let mut m = new_map_wc(8);
    m.insert(ka.clone(), InnerValue::Int(1));
    m.insert(kb.clone(), InnerValue::Str("vb".to_owned()));
    m.insert(kc.clone(), InnerValue::Bool(true));
    m.insert(kd.clone(), InnerValue::Str("vd".to_owned()));
    m.insert(ke.clone(), InnerValue::Int(5));
    m.insert(kf.clone(), InnerValue::Bool(false));
    m.insert(kg.clone(), InnerValue::Str("vg".to_owned()));
    m.insert(kh.clone(), InnerValue::Int(8));

    let record_bytes = InnerValue::Map(m).to_bytes().expect("to_bytes");
    let view = RecordView::new(&record_bytes).expect("RecordView::new");

    // Project a subset in non-record order, including one absent id.
    let absent = ik(&interner, "india");
    let selected = vec![kc.clone(), kh.clone(), ka.clone(), absent, ke.clone()];
    let projected = record_view_to_id_msgpack(&view, &selected).expect("record_view_to_id_msgpack");

    let proj_view = RecordView::new(&projected).expect("RecordView::new projected");
    let decoded = record_view_to_query_value(&proj_view, &interner).expect("decode projected");

    let mut expected = new_map_wc(4);
    expected.insert("charlie".to_owned(), Value::Bool(true));
    expected.insert("hotel".to_owned(), Value::Int(8));
    expected.insert("alpha".to_owned(), Value::Int(1));
    expected.insert("echo".to_owned(), Value::Int(5));
    assert_eq!(decoded, QueryValue::Map(expected));

    // Output order follows selected_ids order (absent id omitted).
    let order: Vec<String> = proj_view
        .fields()
        .map(|(k, _)| interner.get_str(&k).expect("interned").to_string())
        .collect();
    assert_eq!(order, vec!["charlie", "hotel", "alpha", "echo"]);

    // Parity between the new O(1) probe and the scanning field_value_bytes.
    let index = view.index();
    for (k, v) in [
        (&ka, InnerValue::Int(1)),
        (&kb, InnerValue::Str("vb".to_owned())),
        (&kc, InnerValue::Bool(true)),
        (&kd, InnerValue::Str("vd".to_owned())),
    ] {
        let bytes = index.value_bytes(k.clone()).expect("indexed probe");
        assert_eq!(
            InnerValue::from_bytes(bytes).unwrap(),
            v,
            "value_bytes parity mismatch for {k:?}"
        );
        assert_eq!(
            view.field_value_bytes(k.clone()),
            Some(bytes),
            "probe vs scan mismatch for {k:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// RV-TYPES-93D11F4F: deterministic single-top-level-pass bound
// ---------------------------------------------------------------------------

/// The indexed projection performs ONE top-level pass to build the index
/// (2 scan steps/field: read_bin_len + skip_value) then O(1) probes per
/// selected id. Reverting to per-id `field_value_bytes` rescans ~half the
/// record per lookup (~n·k steps) and blows the bound.
#[test]
fn rv_types_93d11f4f_projection_single_top_level_pass() {
    let interner = Interner::new();
    let n = 60usize;
    let mut m = new_map_wc(n);
    let mut keys = Vec::with_capacity(n);
    for i in 0..n {
        let k = ik(&interner, &format!("field_{i}"));
        m.insert(k.clone(), InnerValue::Int(i as i64));
        keys.push(k);
    }
    let record_bytes = InnerValue::Map(m).to_bytes().expect("to_bytes");
    let view = RecordView::new(&record_bytes).expect("RecordView::new");

    // Scrambled present ids + 2 absent ids.
    let absent1 = ik(&interner, "absent_one");
    let absent2 = ik(&interner, "absent_two");
    let selected: Vec<InternerKey> = vec![
        keys[41].clone(),
        keys[7].clone(),
        keys[23].clone(),
        absent1,
        keys[2].clone(),
        keys[59].clone(),
        absent2,
        keys[15].clone(),
    ];
    let k = selected.len();

    crate::record_view::reset_scan_steps();
    let projected = record_view_to_id_msgpack(&view, &selected).expect("record_view_to_id_msgpack");
    let steps = crate::record_view::scan_steps();

    // Output: the right subset, in selected order (absent omitted).
    let proj_view = RecordView::new(&projected).expect("RecordView::new projected");
    let order: Vec<u64> = proj_view.fields().map(|(key, _)| key.id()).collect();
    let expected_order: Vec<u64> = selected
        .iter()
        .filter(|s| keys.iter().any(|pk| pk == *s))
        .map(|s| s.id())
        .collect();
    assert_eq!(order, expected_order, "projected subset/order mismatch");
    assert_eq!(proj_view.len(), 6, "6 present ids expected");

    // Linear bound: one index pass (2 steps/field) + ≤ 1 skip per probe.
    assert!(
        steps <= 2 * n as u64 + k as u64 + 10,
        "scan work {steps} exceeds bound {} — projection reverted to per-id rescan?",
        2 * n as u64 + k as u64 + 10
    );
}

// ---------------------------------------------------------------------------
// RV-TYPES-93D11F4F: header-declared count must not reach a huge allocation
// ---------------------------------------------------------------------------

#[test]
fn rv_types_93d11f4f_projection_header_prealloc_bounded() {
    // map32 header declaring 0xFFFFFFFF entries with an EMPTY body — five
    // bytes. RecordView::new accepts it (header-only validation); the index
    // must cap its preallocation by the bytes actually present instead of
    // allocating from the declared count.
    let buf = [0xdf_u8, 0xff, 0xff, 0xff, 0xff];
    let view = RecordView::new(&buf).expect("header-only record constructs");
    // Empty selection: must return an empty id-keyed map (0x80), not abort.
    let empty = record_view_to_id_msgpack(&view, &[]).expect("empty projection");
    assert_eq!(empty.as_ref(), &[0x80]);
    // Non-empty selection over the empty body: nothing present, still empty map.
    let selected = vec![InternerKey::new(1)];
    let projected = record_view_to_id_msgpack(&view, &selected).expect("selection projection");
    assert_eq!(projected.as_ref(), &[0x80]);
}
