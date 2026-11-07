//! Byte-identity parity battery: proves that for every value shape the storage
//! encoder can produce, the lens path equals the tree path exactly:
//!
//!   record_view_to_query_value(&RecordView::new(&bytes)?, interner)
//!     == inner_value_to_query_value(&InnerValue::from_bytes(&bytes)?, interner)
//!
//! Both sides consume the SAME bytes, each through its own decoder. If any
//! shape diverges, the test fails with a diagnostic — divergences must be
//! reported, not papered over.

use crate::codecs::interned::{
    inner_value_to_query_value, record_view_deintern_with, record_view_to_query_value,
};
use crate::core::interner::{Interner, InternerKey};
use crate::record_view::{Kind, RecordRef, RecordView};
use crate::types::common::new_map_wc;
use crate::types::value::InnerValue;

/// Intern a string key, returning the `InternerKey` the tree map uses.
fn ik(interner: &Interner, s: &str) -> InternerKey {
    interner.touch_ind(s).unwrap().into_key()
}

/// Assert both de-intern paths (QueryValue) agree for the given storage bytes.
fn assert_query_value_parity(bytes: &[u8], interner: &Interner, label: &str) {
    let tree_iv = InnerValue::from_bytes(bytes).unwrap_or_else(|e| {
        panic!("from_bytes failed for '{label}': {e}");
    });
    let lens_view = RecordView::new(bytes).unwrap_or_else(|e| {
        panic!("RecordView::new failed for '{label}': {e}");
    });

    let tree_qv = inner_value_to_query_value(&tree_iv, interner)
        .unwrap_or_else(|e| panic!("inner_value_to_query_value failed for '{label}': {e}"));
    let lens_qv = record_view_to_query_value(&lens_view, interner)
        .unwrap_or_else(|e| panic!("record_view_to_query_value failed for '{label}': {e}"));

    assert_eq!(
        tree_qv, lens_qv,
        "QueryValue parity FAIL for '{label}':\n  tree: {tree_qv:?}\n  lens: {lens_qv:?}"
    );
}

/// Helper: build an `InnerValue::Map` record, serialise to storage bytes,
/// then assert QueryValue parity.
fn assert_parity_for_record(inner: InnerValue, interner: &Interner, label: &str) {
    let bytes = inner.to_bytes().unwrap_or_else(|e| {
        panic!("to_bytes failed for '{label}': {e}");
    });
    assert_query_value_parity(&bytes, interner, label);
}

// ─── flat scalar shapes ──────────────────────────────────────────────────────

#[test]
fn deintern_parity_flat_int() {
    let interner = Interner::new();
    let mut m = new_map_wc(2);
    m.insert(ik(&interner, "pos"), InnerValue::Int(42));
    m.insert(ik(&interner, "neg"), InnerValue::Int(-7));
    assert_parity_for_record(InnerValue::Map(m), &interner, "flat_int");
}

#[test]
fn deintern_parity_flat_int_neg() {
    let interner = Interner::new();
    let mut m = new_map_wc(3);
    m.insert(ik(&interner, "i64min"), InnerValue::Int(i64::MIN));
    m.insert(ik(&interner, "i64max"), InnerValue::Int(i64::MAX));
    m.insert(ik(&interner, "zero"), InnerValue::Int(0));
    assert_parity_for_record(InnerValue::Map(m), &interner, "flat_int_neg");
}

#[test]
fn deintern_parity_flat_f64() {
    let interner = Interner::new();
    let mut m = new_map_wc(2);
    m.insert(ik(&interner, "pi"), InnerValue::F64(std::f64::consts::PI));
    m.insert(ik(&interner, "neg"), InnerValue::F64(-0.5));
    assert_parity_for_record(InnerValue::Map(m), &interner, "flat_f64");
}

#[test]
fn deintern_parity_flat_f64_non_finite() {
    // F64 non-finite (inf / -inf) — the encoder stores them as-is in msgpack;
    // the decoder restores them. Both the tree path and the lens path must
    // produce the same QueryValue. F64 PartialEq is well-defined for inf/-inf.
    let interner = Interner::new();
    let mut m = new_map_wc(2);
    m.insert(ik(&interner, "inf"), InnerValue::F64(f64::INFINITY));
    m.insert(ik(&interner, "neginf"), InnerValue::F64(f64::NEG_INFINITY));
    let bytes = InnerValue::Map(m).to_bytes().unwrap();

    let tree_iv = InnerValue::from_bytes(&bytes).unwrap();
    let lens_view = RecordView::new(&bytes).unwrap();

    let tree_qv = inner_value_to_query_value(&tree_iv, &interner).unwrap();
    let lens_qv = record_view_to_query_value(&lens_view, &interner).unwrap();
    assert_eq!(
        tree_qv, lens_qv,
        "QueryValue parity FAIL for non-finite f64"
    );
}

#[test]
fn deintern_parity_flat_str() {
    let interner = Interner::new();
    let mut m = new_map_wc(2);
    m.insert(ik(&interner, "hello"), InnerValue::Str("world".to_owned()));
    m.insert(ik(&interner, "empty"), InnerValue::Str(String::new()));
    assert_parity_for_record(InnerValue::Map(m), &interner, "flat_str");
}

#[test]
fn deintern_parity_unicode_keys_and_values() {
    let interner = Interner::new();
    let mut m = new_map_wc(2);
    // Unicode field names (interned) + unicode string values.
    m.insert(
        ik(&interner, "кириллица"),
        InnerValue::Str("значение".to_owned()),
    );
    m.insert(
        ik(&interner, "日本語"),
        InnerValue::Str("テスト".to_owned()),
    );
    assert_parity_for_record(InnerValue::Map(m), &interner, "unicode_keys_values");
}

#[test]
fn deintern_parity_flat_bool() {
    let interner = Interner::new();
    let mut m = new_map_wc(2);
    m.insert(ik(&interner, "t"), InnerValue::Bool(true));
    m.insert(ik(&interner, "f"), InnerValue::Bool(false));
    assert_parity_for_record(InnerValue::Map(m), &interner, "flat_bool");
}

#[test]
fn deintern_parity_flat_null() {
    let interner = Interner::new();
    let mut m = new_map_wc(1);
    m.insert(ik(&interner, "n"), InnerValue::Null);
    assert_parity_for_record(InnerValue::Map(m), &interner, "flat_null");
}

#[test]
fn deintern_parity_flat_bin() {
    let interner = Interner::new();
    let mut m = new_map_wc(2);
    m.insert(
        ik(&interner, "data"),
        InnerValue::Bin(vec![0xDE, 0xAD, 0xBE, 0xEF]),
    );
    m.insert(ik(&interner, "empty_bin"), InnerValue::Bin(Vec::new()));
    assert_parity_for_record(InnerValue::Map(m), &interner, "flat_bin");
}

// ─── nested map shapes ───────────────────────────────────────────────────────

#[test]
fn deintern_parity_nested_map_two_levels() {
    let interner = Interner::new();
    let mut inner = new_map_wc(2);
    inner.insert(
        ik(&interner, "city"),
        InnerValue::Str("Jerusalem".to_owned()),
    );
    inner.insert(ik(&interner, "zip"), InnerValue::Int(9_100_000));
    let mut m = new_map_wc(2);
    m.insert(ik(&interner, "name"), InnerValue::Str("user-1".to_owned()));
    m.insert(ik(&interner, "address"), InnerValue::Map(inner));
    assert_parity_for_record(InnerValue::Map(m), &interner, "nested_map_two_levels");
}

#[test]
fn deintern_parity_nested_map_three_levels() {
    let interner = Interner::new();
    let mut leaf = new_map_wc(1);
    leaf.insert(ik(&interner, "lat"), InnerValue::Int(100));
    let mut mid = new_map_wc(1);
    mid.insert(ik(&interner, "loc"), InnerValue::Map(leaf));
    let mut m = new_map_wc(1);
    m.insert(ik(&interner, "meta"), InnerValue::Map(mid));
    assert_parity_for_record(InnerValue::Map(m), &interner, "nested_map_three_levels");
}

// ─── list shapes ─────────────────────────────────────────────────────────────

#[test]
fn deintern_parity_list_of_scalars() {
    let interner = Interner::new();
    let mut m = new_map_wc(1);
    m.insert(
        ik(&interner, "tags"),
        InnerValue::List(vec![
            InnerValue::Str("alpha".to_owned()),
            InnerValue::Int(42),
            InnerValue::Bool(true),
            InnerValue::Null,
        ]),
    );
    assert_parity_for_record(InnerValue::Map(m), &interner, "list_of_scalars");
}

#[test]
fn deintern_parity_list_of_maps() {
    let interner = Interner::new();
    let mut row1 = new_map_wc(2);
    row1.insert(ik(&interner, "id"), InnerValue::Int(1));
    row1.insert(ik(&interner, "name"), InnerValue::Str("Alice".to_owned()));
    let mut row2 = new_map_wc(2);
    row2.insert(ik(&interner, "id"), InnerValue::Int(2));
    row2.insert(ik(&interner, "name"), InnerValue::Str("Bob".to_owned()));
    let mut m = new_map_wc(1);
    m.insert(
        ik(&interner, "rows"),
        InnerValue::List(vec![InnerValue::Map(row1), InnerValue::Map(row2)]),
    );
    assert_parity_for_record(InnerValue::Map(m), &interner, "list_of_maps");
}

// ─── empty containers ────────────────────────────────────────────────────────

#[test]
fn deintern_parity_empty_map() {
    let interner = Interner::new();
    let m = new_map_wc(0);
    assert_parity_for_record(InnerValue::Map(m), &interner, "empty_map");
}

#[test]
fn deintern_parity_empty_list() {
    let interner = Interner::new();
    let mut m = new_map_wc(1);
    m.insert(ik(&interner, "items"), InnerValue::List(Vec::new()));
    assert_parity_for_record(InnerValue::Map(m), &interner, "empty_list");
}

#[test]
fn rv_types_18ac0cfc_deintern_parity_u64_exact_variants() {
    let interner = Interner::new();
    let big_key = ik(&interner, "big");
    let nested_key = ik(&interner, "nested");
    let items_key = ik(&interner, "items");
    let values = [
        i64::MAX as u64,
        i64::MAX as u64 + 1,
        i64::MAX as u64 + 3,
        u64::MAX,
    ];

    for value in values {
        for shape in 0..3 {
            let expected = if value <= i64::MAX as u64 {
                InnerValue::Int(value as i64)
            } else {
                InnerValue::Big(num_bigint::BigInt::from(value))
            };
            let mut scalar = vec![0xcf];
            scalar.extend_from_slice(&value.to_be_bytes());
            let string_map = |name: &str, payload: &[u8]| {
                let mut bytes = vec![0x81, 0xa0 | name.len() as u8];
                bytes.extend_from_slice(name.as_bytes());
                bytes.extend_from_slice(payload);
                bytes
            };
            let interned_map = |key: &InternerKey, payload: &[u8]| {
                let (key_bytes, key_len) = key.as_bytes_buf();
                let mut bytes = vec![0x81, 0xc4, key_len as u8];
                bytes.extend_from_slice(&key_bytes[..key_len]);
                bytes.extend_from_slice(payload);
                bytes
            };

            let (string_bytes, interned_bytes, expected_tree, path): (
                Vec<u8>,
                Vec<u8>,
                InnerValue,
                Vec<InternerKey>,
            ) = match shape {
                0 => (
                    string_map("big", &scalar),
                    interned_map(&big_key, &scalar),
                    {
                        let mut map = new_map_wc(1);
                        map.insert(big_key.clone(), expected.clone());
                        InnerValue::Map(map)
                    },
                    vec![big_key.clone()],
                ),
                1 => {
                    let inner_string = string_map("big", &scalar);
                    let inner_interned = interned_map(&big_key, &scalar);
                    let outer_string = string_map("nested", &inner_string);
                    let outer_interned = interned_map(&nested_key, &inner_interned);
                    let mut inner = new_map_wc(1);
                    inner.insert(big_key.clone(), expected.clone());
                    let mut outer = new_map_wc(1);
                    outer.insert(nested_key.clone(), InnerValue::Map(inner));
                    (
                        outer_string,
                        outer_interned,
                        InnerValue::Map(outer),
                        vec![nested_key.clone(), big_key.clone()],
                    )
                }
                _ => {
                    let array = [&[0x91][..], &scalar].concat();
                    let mut list = new_map_wc(1);
                    list.insert(items_key.clone(), InnerValue::List(vec![expected.clone()]));
                    (
                        string_map("items", &array),
                        interned_map(&items_key, &array),
                        InnerValue::Map(list),
                        vec![items_key.clone()],
                    )
                }
            };

            let query = crate::types::value::QueryValue::from_bytes(&string_bytes).unwrap();
            let custom =
                crate::codecs::interned::msgpack_to_inner(&interner, &string_bytes).unwrap();
            let legacy = crate::codecs::interned::messagepack::msgpack_to_inner_legacy(
                &interner,
                &string_bytes,
            )
            .unwrap();
            let tree = InnerValue::from_bytes(&interned_bytes).unwrap();
            let view = RecordView::new(&interned_bytes).unwrap();
            assert_eq!(tree, expected_tree);
            assert_eq!(tree, custom);
            assert_eq!(tree, legacy);
            assert_eq!(query, inner_value_to_query_value(&tree, &interner).unwrap());
            assert_eq!(query, record_view_to_query_value(&view, &interner).unwrap());

            if shape < 2 {
                assert_eq!(tree.materialize_at(&path), Some(expected.clone()));
                assert_eq!(view.materialize_at(&path), Some(expected.clone()));
                let kind = if value <= i64::MAX as u64 {
                    Kind::Scalar
                } else {
                    Kind::NonComparable
                };
                assert_eq!(tree.present_kind_at(&path), Some(kind));
                assert_eq!(view.present_kind_at(&path), Some(kind));
                assert_eq!(tree.scalar_at(&path), view.scalar_at(&path));
                assert_eq!(view.scalar_at(&path).is_some(), value <= i64::MAX as u64);
            } else {
                let list_expected = InnerValue::List(vec![expected.clone()]);
                assert_eq!(tree.materialize_at(&path), Some(list_expected.clone()));
                assert_eq!(view.materialize_at(&path), Some(list_expected));
                assert_eq!(
                    tree.any_seq_elem(&path, &mut |_| true),
                    Some(value <= i64::MAX as u64)
                );
                assert_eq!(
                    view.any_seq_elem(&path, &mut |_| true),
                    Some(value <= i64::MAX as u64)
                );
            }
        }
    }
}

#[test]
fn rv_types_suffix98656aaa_map_allocation_bounded_by_body() {
    let interner = Interner::new();
    let bytes = [0xde, 0x03, 0xe8]; // map16 with 1000 entries and zero-byte body
    let view = RecordView::new(&bytes).unwrap();
    let value = record_view_to_query_value(&view, &interner).unwrap();
    match value {
        crate::types::value::Value::Map(map) => assert!(map.capacity() <= bytes.len() - 3),
        other => panic!("expected map, got {other:?}"),
    }
    let deinterned = record_view_deintern_with(&view, &|_| None).unwrap();
    match deinterned {
        crate::types::value::Value::Map(map) => assert_eq!(map.capacity(), 0),
        other => panic!("expected map, got {other:?}"),
    }
}

// ─── wide record ─────────────────────────────────────────────────────────────

#[test]
fn deintern_parity_wide_record() {
    // Many fields — exercises the O(N) lens path over a wide record.
    let interner = Interner::new();
    let n = 30usize;
    let mut m = new_map_wc(n);
    for i in 0..n {
        let key = format!("field_{i}");
        m.insert(
            ik(&interner, &key),
            match i % 5 {
                0 => InnerValue::Int(i as i64),
                1 => InnerValue::Str(format!("val_{i}")),
                2 => InnerValue::Bool(i % 2 == 0),
                3 => InnerValue::F64(i as f64 * 0.1),
                _ => InnerValue::Null,
            },
        );
    }
    assert_parity_for_record(InnerValue::Map(m), &interner, "wide_record_30_fields");
}
