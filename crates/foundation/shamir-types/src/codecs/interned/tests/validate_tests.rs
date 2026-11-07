//! Tests for [`validate_keys_resolve`] and [`validate_keys_resolve_interner`].
//!
//! Build interner + records the same way `record_view/tests/deintern_parity_tests.rs`
//! does: `interner.touch_ind(name).unwrap().into_key()` returns the `InternerKey`
//! used as a map key, then `InnerValue::Map(m).to_bytes()` yields the storage bytes,
//! and `RecordView::new(&bytes)` gives the lens.
//!
//! Tests are WRITTEN but NOT RUN (orchestrator runs `./scripts/test.sh`).

use crate::codecs::interned::{validate_keys_resolve, validate_keys_resolve_interner};
use crate::core::interner::{Interner, InternerKey};
use crate::record_view::RecordView;
use crate::types::common::new_map_wc;
use crate::types::value::InnerValue;

/// Intern a field name and return its `InternerKey`.
fn ik(interner: &Interner, s: &str) -> InternerKey {
    interner.touch_ind(s).unwrap().into_key()
}

// ---------------------------------------------------------------------------
// P1-A: record whose keys ALL resolve → Ok
// ---------------------------------------------------------------------------

/// All top-level keys interned → `validate_keys_resolve` returns `Ok`.
#[test]
fn validate_all_keys_resolve_ok() {
    let interner = Interner::new();
    let mut m = new_map_wc(3);
    m.insert(ik(&interner, "name"), InnerValue::Str("Alice".to_owned()));
    m.insert(ik(&interner, "age"), InnerValue::Int(30));
    m.insert(ik(&interner, "active"), InnerValue::Bool(true));

    let bytes = InnerValue::Map(m).to_bytes().expect("to_bytes");
    let view = RecordView::new(&bytes).expect("RecordView::new");

    let result = validate_keys_resolve_interner(&view, &interner);
    assert!(
        result.is_ok(),
        "expected Ok for fully-interned record, got: {result:?}"
    );
}

// ---------------------------------------------------------------------------
// P1-B: key id BEYOND interner's range → Err
// ---------------------------------------------------------------------------

/// Hand-build a storage record with a key id that is beyond the interner's
/// reverse vec (e.g., id = 999 when the interner has only ids 1 and 2).
/// `validate_keys_resolve` must return `Err` naming the unresolved id.
#[test]
fn validate_forged_key_id_out_of_range_err() {
    let interner = Interner::new();
    // Intern two keys so the interner is non-empty (ids 1, 2).
    let _ = ik(&interner, "known_a");
    let _ = ik(&interner, "known_b");

    // Forge a record with key id = 999 (no such id exists in the interner).
    let forged_id: u64 = 999;
    let forged_key = InternerKey::new(forged_id);

    let mut m = new_map_wc(1);
    m.insert(forged_key, InnerValue::Int(42));

    let bytes = InnerValue::Map(m).to_bytes().expect("to_bytes");
    let view = RecordView::new(&bytes).expect("RecordView::new");

    let rev = interner.reverse_snapshot();
    let result = validate_keys_resolve(&view, rev.as_slice());

    assert!(
        result.is_err(),
        "expected Err for out-of-range key id, got Ok"
    );
    let msg = format!("{:?}", result.unwrap_err());
    assert!(
        msg.contains("999"),
        "error message should mention the bad id 999, got: {msg}"
    );
}

// ---------------------------------------------------------------------------
// P1-C: nested map with one unresolved nested key → Err
// ---------------------------------------------------------------------------

/// Top-level key resolves; the nested map contains a key id that does NOT
/// resolve. The walk must reach into the nested map and return `Err`.
#[test]
fn validate_nested_map_unresolved_key_err() {
    let interner = Interner::new();
    let outer_key = ik(&interner, "meta");

    // Build a nested map with a forged key id.
    let forged_nested_id: u64 = 888;
    let forged_nested_key = InternerKey::new(forged_nested_id);
    let mut nested = new_map_wc(1);
    nested.insert(forged_nested_key, InnerValue::Str("value".to_owned()));

    let mut m = new_map_wc(1);
    m.insert(outer_key, InnerValue::Map(nested));

    let bytes = InnerValue::Map(m).to_bytes().expect("to_bytes");
    let view = RecordView::new(&bytes).expect("RecordView::new");

    let rev = interner.reverse_snapshot();
    let result = validate_keys_resolve(&view, rev.as_slice());

    assert!(
        result.is_err(),
        "expected Err for unresolved nested map key, got Ok"
    );
    let msg = format!("{:?}", result.unwrap_err());
    assert!(
        msg.contains("888"),
        "error message should mention the bad nested id 888, got: {msg}"
    );
}

// ---------------------------------------------------------------------------
// P1-D: list-of-maps with an unresolved key in the 2nd map element → Err
// ---------------------------------------------------------------------------

/// A top-level field contains a list of maps. The FIRST map's key resolves;
/// the SECOND map contains an unresolved key. The walk must iterate through
/// the list and catch the error in the second element.
#[test]
fn validate_list_of_maps_second_element_err() {
    let interner = Interner::new();
    let list_field = ik(&interner, "rows");
    let row_id_key = ik(&interner, "id"); // resolves

    // First map element: all keys resolve.
    let mut row1 = new_map_wc(1);
    row1.insert(row_id_key.clone(), InnerValue::Int(1));

    // Second map element: contains a forged key.
    let forged_row_id: u64 = 777;
    let forged_row_key = InternerKey::new(forged_row_id);
    let mut row2 = new_map_wc(1);
    row2.insert(forged_row_key, InnerValue::Int(2));

    let list = InnerValue::List(vec![InnerValue::Map(row1), InnerValue::Map(row2)]);
    let mut m = new_map_wc(1);
    m.insert(list_field, list);

    let bytes = InnerValue::Map(m).to_bytes().expect("to_bytes");
    let view = RecordView::new(&bytes).expect("RecordView::new");

    let rev = interner.reverse_snapshot();
    let result = validate_keys_resolve(&view, rev.as_slice());

    assert!(
        result.is_err(),
        "expected Err for unresolved key in 2nd list-of-maps element, got Ok"
    );
    let msg = format!("{:?}", result.unwrap_err());
    assert!(
        msg.contains("777"),
        "error message should mention the bad id 777 in 2nd map, got: {msg}"
    );
}

// ---------------------------------------------------------------------------
// P1-E: list-of-maps where ALL elements resolve → Ok
// ---------------------------------------------------------------------------

/// Positive counterpart: all keys in all list-of-map elements resolve.
#[test]
fn validate_list_of_maps_all_resolve_ok() {
    let interner = Interner::new();
    let list_field = ik(&interner, "items");
    let name_key = ik(&interner, "name");
    let val_key = ik(&interner, "val");

    let mut row1 = new_map_wc(2);
    row1.insert(name_key.clone(), InnerValue::Str("a".to_owned()));
    row1.insert(val_key.clone(), InnerValue::Int(1));

    let mut row2 = new_map_wc(2);
    row2.insert(name_key.clone(), InnerValue::Str("b".to_owned()));
    row2.insert(val_key.clone(), InnerValue::Int(2));

    let list = InnerValue::List(vec![InnerValue::Map(row1), InnerValue::Map(row2)]);
    let mut m = new_map_wc(1);
    m.insert(list_field, list);

    let bytes = InnerValue::Map(m).to_bytes().expect("to_bytes");
    let view = RecordView::new(&bytes).expect("RecordView::new");

    let result = validate_keys_resolve_interner(&view, &interner);
    assert!(
        result.is_ok(),
        "expected Ok for fully-interned list-of-maps, got: {result:?}"
    );
}

// ---------------------------------------------------------------------------
// P1-F: empty record → Ok (vacuously)
// ---------------------------------------------------------------------------

#[test]
fn validate_empty_record_ok() {
    let interner = Interner::new();
    let m = new_map_wc(0);
    let bytes = InnerValue::Map(m).to_bytes().expect("to_bytes");
    let view = RecordView::new(&bytes).expect("RecordView::new");

    let result = validate_keys_resolve_interner(&view, &interner);
    assert!(
        result.is_ok(),
        "expected Ok for empty record, got: {result:?}"
    );
}

// ---------------------------------------------------------------------------
// P1-G: key id = 0 (sentinel, always None in interner) → Err
// ---------------------------------------------------------------------------

/// Id 0 is the reserved sentinel — always `None` in the reverse vec.
/// A record with key id 0 must be rejected.
#[test]
fn validate_sentinel_id_zero_err() {
    let interner = Interner::new();
    let _ = ik(&interner, "anything"); // make reverse vec non-empty

    let sentinel_key = InternerKey::new(0);
    let mut m = new_map_wc(1);
    m.insert(sentinel_key, InnerValue::Int(0));

    let bytes = InnerValue::Map(m).to_bytes().expect("to_bytes");
    let view = RecordView::new(&bytes).expect("RecordView::new");

    let rev = interner.reverse_snapshot();
    let result = validate_keys_resolve(&view, rev.as_slice());

    assert!(result.is_err(), "expected Err for sentinel id 0, got Ok");
}
