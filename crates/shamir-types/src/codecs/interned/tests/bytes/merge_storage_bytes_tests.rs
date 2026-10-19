//! Parity battery for [`merge_storage_bytes`] (W3 byte-level storage-map merge).
//!
//! Contract under test (MUST hold byte-for-byte, or UPDATE is corrupt):
//!
//! ```text
//! merge_storage_bytes(&old.to_bytes()?, &set_map)?
//!   == merge_inner_maps_ref(&old, &set_map).to_bytes()?
//! ```
//!
//! for a broad battery of (old_record, set_map) pairs covering every behaviour
//! path of the merge algorithm.

use crate::codecs::interned::merge_storage_bytes;
use crate::core::interner::InternerKey;
use crate::types::common::TMap;
use crate::types::value::InnerValue;
use bytes::Bytes;
use shamir_collections::{new_map, new_map_wc};

// ============================================================================
// Reference implementation — mirrors write_exec.rs:1022 verbatim.
//
// `merge_inner_maps` in write_exec.rs:
//   match original {
//       InnerValue::Map(orig_map) => {
//           let mut merged = orig_map.clone();
//           for (key, value) in set_map {
//               merged.insert(key.clone(), value.clone());
//           }
//           InnerValue::Map(merged)
//       }
//       _ => original.clone(),
//   }
//
// IndexMap::insert on an existing key UPDATES THE VALUE IN PLACE (keeps
// position); on a new key it APPENDS. This is the exact order contract we
// must match.
// ============================================================================

/// Reference merge — mirrors write_exec.rs:1022 exactly.
/// Clone the old map and apply each set_map entry; new keys are appended.
fn merge_inner_maps_ref(orig: &InnerValue, set_map: &TMap<InternerKey, InnerValue>) -> InnerValue {
    match orig {
        InnerValue::Map(orig_map) => {
            let mut merged = orig_map.clone();
            for (key, value) in set_map {
                merged.insert(key.clone(), value.clone());
            }
            InnerValue::Map(merged)
        }
        _ => orig.clone(),
    }
}

/// Helper: assert byte-for-byte equality between the two paths and return
/// the bytes for optional further inspection.
fn assert_byte_identical(
    label: &str,
    old: &InnerValue,
    set_map: &TMap<InternerKey, InnerValue>,
) -> Bytes {
    let old_bytes = old
        .to_bytes()
        .unwrap_or_else(|e| panic!("[{label}] old.to_bytes() failed: {e}"));

    let got = merge_storage_bytes(&old_bytes, set_map)
        .unwrap_or_else(|e| panic!("[{label}] merge_storage_bytes failed: {e:?}"));

    let want = merge_inner_maps_ref(old, set_map)
        .to_bytes()
        .unwrap_or_else(|e| panic!("[{label}] reference .to_bytes() failed: {e}"));

    assert_eq!(
        got.as_ref(),
        want.as_ref(),
        "[{label}] BYTE MISMATCH\n  merge_storage_bytes = {}\n  reference            = {}",
        hex_dump(&got),
        hex_dump(&want),
    );

    got
}

fn hex_dump(b: &Bytes) -> String {
    b.iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<Vec<_>>()
        .join(" ")
}

/// Build a single-key InnerValue::Map with the given InternerKey id and value.
fn make_map(entries: &[(u64, InnerValue)]) -> InnerValue {
    let mut m: TMap<InternerKey, InnerValue> = new_map_wc(entries.len());
    for (id, val) in entries {
        m.insert(InternerKey::new(*id), val.clone());
    }
    InnerValue::Map(m)
}

/// Build a set_map from (id, InnerValue) pairs.
fn make_set_map(entries: &[(u64, InnerValue)]) -> TMap<InternerKey, InnerValue> {
    let mut m: TMap<InternerKey, InnerValue> = new_map_wc(entries.len());
    for (id, val) in entries {
        m.insert(InternerKey::new(*id), val.clone());
    }
    m
}

// ============================================================================
// Empty set — must produce old bytes exactly.
// ============================================================================

#[test]
fn merge_empty_set_is_noop() {
    let old = make_map(&[
        (0, InnerValue::Int(1)),
        (1, InnerValue::Str("hello".into())),
    ]);
    let set_map = make_set_map(&[]);
    // When set is empty the result must be byte-for-byte identical to old.
    let got = assert_byte_identical("empty_set_is_noop", &old, &set_map);
    let old_bytes = old.to_bytes().unwrap();
    assert_eq!(
        got.as_ref(),
        old_bytes.as_ref(),
        "empty set_map must produce identical bytes to old"
    );
}

// ============================================================================
// Overwrite an existing key — same value (no-op bytes).
// ============================================================================

#[test]
fn merge_overwrite_existing_key_same_value() {
    let old = make_map(&[(0, InnerValue::Int(42)), (1, InnerValue::Bool(true))]);
    // Overwriting key 0 with the SAME value must still produce identical bytes.
    let set_map = make_set_map(&[(0, InnerValue::Int(42))]);
    assert_byte_identical("overwrite_same_value", &old, &set_map);
}

// ============================================================================
// Overwrite an existing key — different value.
// ============================================================================

#[test]
fn merge_overwrite_existing_key_different_value() {
    let old = make_map(&[(0, InnerValue::Int(1)), (1, InnerValue::Str("old".into()))]);
    let set_map = make_set_map(&[(1, InnerValue::Str("new".into()))]);
    assert_byte_identical("overwrite_diff_value", &old, &set_map);
}

// ============================================================================
// Type change on an existing key (Int → Str).
// ============================================================================

#[test]
fn merge_type_change_int_to_str() {
    let old = make_map(&[(0, InnerValue::Int(999)), (1, InnerValue::Bool(false))]);
    // Change key 0 from Int to Str.
    let set_map = make_set_map(&[(0, InnerValue::Str("changed".into()))]);
    assert_byte_identical("type_change_int_to_str", &old, &set_map);
}

// ============================================================================
// Add new keys (not present in old).
// ============================================================================

#[test]
fn merge_add_new_keys() {
    let old = make_map(&[(0, InnerValue::Int(1))]);
    let set_map = make_set_map(&[
        (1, InnerValue::Str("new_a".into())),
        (2, InnerValue::Bool(true)),
    ]);
    assert_byte_identical("add_new_keys", &old, &set_map);
}

// ============================================================================
// Mixed: overwrite some, add some.
// ============================================================================

#[test]
fn merge_overwrite_and_add() {
    let old = make_map(&[
        (0, InnerValue::Int(10)),
        (1, InnerValue::Str("stay".into())),
        (2, InnerValue::Bool(false)),
    ]);
    // Overwrite 0 and 2, add 3.
    let set_map = make_set_map(&[
        (0, InnerValue::Int(99)),
        (2, InnerValue::Bool(true)),
        (3, InnerValue::Str("brand_new".into())),
    ]);
    assert_byte_identical("overwrite_and_add", &old, &set_map);
}

// ============================================================================
// Scalar / Str / Bin / List values in set_map.
// ============================================================================

#[test]
fn merge_scalar_values() {
    let old = make_map(&[
        (0, InnerValue::Int(0)),
        (1, InnerValue::Null),
        (2, InnerValue::Bool(false)),
        (3, InnerValue::F64(1.0)),
        (4, InnerValue::Str("x".into())),
        (5, InnerValue::Bin(vec![0xde, 0xad])),
    ]);
    // Overwrite every field with a different type.
    let set_map = make_set_map(&[
        (0, InnerValue::Null),
        (1, InnerValue::Bool(true)),
        (2, InnerValue::Int(-1)),
        (3, InnerValue::Str("float_was_here".into())),
        (4, InnerValue::Bin(vec![0xff])),
        (5, InnerValue::F64(1.2345_6789)),
    ]);
    assert_byte_identical("scalar_values", &old, &set_map);
}

#[test]
fn merge_list_value_in_set() {
    let old = make_map(&[(0, InnerValue::Int(1)), (1, InnerValue::Null)]);
    let set_map = make_set_map(&[(
        1,
        InnerValue::List(vec![
            InnerValue::Int(10),
            InnerValue::Str("a".into()),
            InnerValue::Bool(true),
        ]),
    )]);
    assert_byte_identical("list_value_in_set", &old, &set_map);
}

// ============================================================================
// Nested-map value in set_map.
// ============================================================================

#[test]
fn merge_nested_map_value_in_set() {
    let old = make_map(&[(0, InnerValue::Int(1)), (1, InnerValue::Null)]);

    let mut inner_map: TMap<InternerKey, InnerValue> = new_map();
    inner_map.insert(InternerKey::new(10), InnerValue::Str("deep".into()));
    inner_map.insert(InternerKey::new(11), InnerValue::Int(777));
    let nested = InnerValue::Map(inner_map);

    let set_map = make_set_map(&[(1, nested)]);
    assert_byte_identical("nested_map_value_in_set", &old, &set_map);
}

// ============================================================================
// Old record with nested maps — unchanged subtrees copied verbatim.
// ============================================================================

#[test]
fn merge_old_record_with_nested_maps_unchanged() {
    // Build an old record where field 1 is itself a nested map.
    let mut inner: TMap<InternerKey, InnerValue> = new_map();
    inner.insert(InternerKey::new(20), InnerValue::Str("nested_val".into()));
    inner.insert(InternerKey::new(21), InnerValue::Int(42));
    let old = make_map(&[
        (0, InnerValue::Int(100)),
        (1, InnerValue::Map(inner)),
        (2, InnerValue::Bool(true)),
    ]);

    // Only overwrite field 0; the nested map at field 1 must be copied verbatim.
    let set_map = make_set_map(&[(0, InnerValue::Int(999))]);
    assert_byte_identical("nested_unchanged_subtrees", &old, &set_map);
}

// ============================================================================
// Key present in old at a non-last position being overwritten —
// proves position is PRESERVED (not moved to tail).
// ============================================================================

#[test]
fn merge_overwrite_middle_position_preserved() {
    // Three keys; overwrite the MIDDLE one. In the reference IndexMap::insert
    // updates in-place so position 1 stays at position 1. We must do the same.
    let old = make_map(&[
        (0, InnerValue::Str("first".into())),
        (1, InnerValue::Int(100)), // ← will be overwritten
        (2, InnerValue::Str("last".into())),
    ]);
    let set_map = make_set_map(&[(1, InnerValue::Int(999))]);
    let got = assert_byte_identical("middle_position_preserved", &old, &set_map);

    // Extra structural check: verify key order in the output.
    // Decode the merged bytes and confirm entry order: 0, 1, 2.
    use crate::record_view::RecordView;
    let view = RecordView::new(&got).expect("valid merged bytes");
    let ids: Vec<u64> = view.fields().map(|(k, _)| k.id()).collect();
    assert_eq!(ids, vec![0, 1, 2], "key order must be preserved");
}

// ============================================================================
// set_map key order vs old key order — new keys appended in set_map order.
// ============================================================================

#[test]
fn merge_new_keys_appended_in_set_order() {
    let old = make_map(&[(0, InnerValue::Int(1))]);
    // Three new keys inserted in a specific order — they must appear in the
    // merged output in that same order (after the old key).
    let set_map = make_set_map(&[
        (3, InnerValue::Str("c".into())),
        (1, InnerValue::Str("a".into())),
        (2, InnerValue::Str("b".into())),
    ]);
    let got = assert_byte_identical("new_keys_set_order", &old, &set_map);

    use crate::record_view::RecordView;
    let view = RecordView::new(&got).expect("valid merged bytes");
    let ids: Vec<u64> = view.fields().map(|(k, _)| k.id()).collect();
    // Old key first, then new keys in set_map iteration order (3, 1, 2).
    assert_eq!(
        ids,
        vec![0, 3, 1, 2],
        "new keys must be appended in set_map order"
    );
}

// ============================================================================
// FixMap → Map16 boundary crossing (total > 15 fields).
// ============================================================================

#[test]
fn merge_fixmap_to_map16_boundary() {
    // Build an old record with 14 fields (still FixMap range).
    let old_entries: Vec<(u64, InnerValue)> =
        (0..14u64).map(|i| (i, InnerValue::Int(i as i64))).collect();
    let old = make_map(&old_entries);

    // Add 3 new fields → total = 17, which forces Map16 header.
    let set_map = make_set_map(&[
        (14, InnerValue::Str("new_14".into())),
        (15, InnerValue::Str("new_15".into())),
        (16, InnerValue::Str("new_16".into())),
    ]);
    let got = assert_byte_identical("fixmap_to_map16_boundary", &old, &set_map);

    // Verify the output starts with Map16 header (0xde).
    assert_eq!(
        got[0], 0xde,
        "total=17 must emit Map16 header (0xde), got {:#04x}",
        got[0]
    );
    let encoded_len = u16::from_be_bytes([got[1], got[2]]) as usize;
    assert_eq!(encoded_len, 17, "Map16 length must be 17");
}

// ============================================================================
// Exactly 15 entries stays FixMap (boundary check from below).
// ============================================================================

#[test]
fn merge_exactly_15_entries_is_fixmap() {
    // 14 old + 1 new = 15 → must still be FixMap.
    let old_entries: Vec<(u64, InnerValue)> =
        (0..14u64).map(|i| (i, InnerValue::Int(i as i64))).collect();
    let old = make_map(&old_entries);
    let set_map = make_set_map(&[(14, InnerValue::Int(99))]);
    let got = assert_byte_identical("exactly_15_fixmap", &old, &set_map);

    // FixMap header: 0x80 | 15 = 0x8f
    assert_eq!(
        got[0], 0x8f,
        "total=15 must emit FixMap header (0x8f), got {:#04x}",
        got[0]
    );
}

// ============================================================================
// Bin values in set_map.
// ============================================================================

#[test]
fn merge_bin_value_in_set() {
    let old = make_map(&[(0, InnerValue::Null)]);
    let set_map = make_set_map(&[(0, InnerValue::Bin(vec![0x01, 0x02, 0x03, 0xff]))]);
    assert_byte_identical("bin_value_in_set", &old, &set_map);
}

// ============================================================================
// Large integer values (boundary widths in the value).
// ============================================================================

#[test]
fn merge_large_int_values() {
    let old = make_map(&[(0, InnerValue::Int(0))]);
    for &v in &[i64::MAX, i64::MIN, i32::MAX as i64, -1i64, 128i64, 65536i64] {
        let set_map = make_set_map(&[(0, InnerValue::Int(v))]);
        assert_byte_identical(&format!("large_int_{v}"), &old, &set_map);
    }
}

// ============================================================================
// F64 value in set_map.
// ============================================================================

#[test]
fn merge_f64_value_in_set() {
    let old = make_map(&[(0, InnerValue::Null)]);
    for &f in &[0.0f64, -1.5, f64::INFINITY, f64::NEG_INFINITY] {
        let set_map = make_set_map(&[(0, InnerValue::F64(f))]);
        assert_byte_identical(&format!("f64_{f:?}"), &old, &set_map);
    }
}

// ============================================================================
// Key id width boundaries for old record keys (1/2/4/8-byte ids).
// ============================================================================

#[test]
fn merge_old_key_id_width_boundaries() {
    // Use ids that span each LE-width bucket.
    let old = make_map(&[
        (0u64, InnerValue::Int(1)),       // 1-byte id (fits in u8)
        (256u64, InnerValue::Int(2)),     // 2-byte id (fits in u16)
        (65536u64, InnerValue::Int(3)),   // 4-byte id (fits in u32)
        (1u64 << 33, InnerValue::Int(4)), // 8-byte id
    ]);
    // Overwrite all of them.
    let set_map = make_set_map(&[
        (0u64, InnerValue::Str("a".into())),
        (256u64, InnerValue::Str("b".into())),
        (65536u64, InnerValue::Str("c".into())),
        (1u64 << 33, InnerValue::Str("d".into())),
    ]);
    assert_byte_identical("old_key_id_width_boundaries", &old, &set_map);
}

// ============================================================================
// set_map key id width boundaries for NEW keys.
// ============================================================================

#[test]
fn merge_new_key_id_width_boundaries() {
    let old = make_map(&[(0, InnerValue::Int(0))]);
    let set_map = make_set_map(&[
        (255u64, InnerValue::Int(1)),     // 1-byte new key
        (256u64, InnerValue::Int(2)),     // 2-byte new key
        (65536u64, InnerValue::Int(3)),   // 4-byte new key
        (1u64 << 33, InnerValue::Int(4)), // 8-byte new key
    ]);
    assert_byte_identical("new_key_id_width_boundaries", &old, &set_map);
}

// ============================================================================
// Empty old record + all new keys.
// ============================================================================

#[test]
fn merge_empty_old_all_new() {
    let old = make_map(&[]);
    let set_map = make_set_map(&[(0, InnerValue::Int(1)), (1, InnerValue::Str("a".into()))]);
    assert_byte_identical("empty_old_all_new", &old, &set_map);
}

// ============================================================================
// Empty old record + empty set_map → empty map bytes.
// ============================================================================

#[test]
fn merge_both_empty() {
    let old = make_map(&[]);
    let set_map = make_set_map(&[]);
    let got = assert_byte_identical("both_empty", &old, &set_map);
    // FixMap with 0 entries = 0x80
    assert_eq!(got.as_ref(), &[0x80u8], "empty map must be 0x80");
}

// ============================================================================
// String length boundaries in new values (fixstr / str8 / str16).
// ============================================================================

#[test]
fn merge_str_length_boundaries_in_set() {
    let old = make_map(&[
        (0, InnerValue::Null),
        (1, InnerValue::Null),
        (2, InnerValue::Null),
    ]);
    let set_map = make_set_map(&[
        (0, InnerValue::Str("a".repeat(31))),  // fixstr boundary (max)
        (1, InnerValue::Str("b".repeat(32))),  // str8
        (2, InnerValue::Str("c".repeat(256))), // str16
    ]);
    assert_byte_identical("str_length_boundaries", &old, &set_map);
}
