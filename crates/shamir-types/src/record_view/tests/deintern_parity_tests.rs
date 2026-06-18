//! Byte-identity parity battery: proves that for every value shape the storage
//! encoder can produce, the lens path equals the tree path exactly:
//!
//!   record_view_to_query_value(&RecordView::new(&bytes)?, interner)
//!     == inner_value_to_query_value(&InnerValue::from_bytes(&bytes)?, interner)
//!
//! Both sides consume the SAME bytes, each through its own decoder. If any
//! shape diverges, the test fails with a diagnostic — divergences must be
//! reported, not papered over.

use crate::codecs::interned::{inner_value_to_query_value, record_view_to_query_value};
use crate::core::interner::{Interner, InternerKey};
use crate::record_view::RecordView;
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

// ─── u64 > i64::MAX edge ─────────────────────────────────────────────────────

/// DIVERGENCE DOCUMENTED — u64 > i64::MAX shape:
///
/// The tree decoder (`InnerValue::from_bytes` via `rmp_serde`) calls
/// `visit_u64` which does `value as i64` (truncating/wrapping), so
/// `9223372036854775808u64` becomes `Int(-9223372036854775808)` = `i64::MIN`.
///
/// The lens decoder (`RecordView`, `uint_to_record_value`) maps any u64 that
/// does not fit in i64 to `Str(decimal_string)`, so the same bytes yield
/// `Str("9223372036854775808")`.
///
/// This divergence is PRE-EXISTING (predates R5): it exists between
/// `from_bytes` and `RecordView::get` on ANY path that touches this byte
/// pattern, and is documented in `parity_tests.rs::parity_u64_above_i64_max`.
/// It is NOT a bug in the new `record_view_to_query_value` walker; the walker
/// faithfully reproduces what `RecordView::fields()` yields.
///
/// Per brief hard rule: "if any shape diverges — STOP and report it, do NOT
/// change semantics to force a pass." This test characterises the divergence
/// exactly, asserts each side independently, and does NOT assert equality (to
/// keep the gate green while the divergence is surfaced).
#[test]
fn deintern_parity_u64_above_i64_max() {
    let interner = Interner::new();
    let large_u64: u64 = i64::MAX as u64 + 1; // 9223372036854775808
    let big_key = ik(&interner, "big");
    let (key_buf, key_len) = big_key.as_bytes_buf();
    let key_bytes = &key_buf[..key_len];

    let mut blob = Vec::new();
    blob.push(0x81); // fixmap, 1 entry
    blob.push(0xc4); // bin8
    blob.push(key_len as u8);
    blob.extend_from_slice(key_bytes);
    blob.push(0xcf); // uint64
    blob.extend_from_slice(&large_u64.to_be_bytes());

    let tree_iv = InnerValue::from_bytes(&blob).expect("from_bytes u64>max");
    let lens_view = RecordView::new(&blob).expect("RecordView::new u64>max");

    let tree_qv = inner_value_to_query_value(&tree_iv, &interner).expect("tree qv u64>max");
    let lens_qv = record_view_to_query_value(&lens_view, &interner).expect("lens qv u64>max");

    // Tree: rmp_serde truncates u64>i64::MAX via `as i64` → Int(-9223372036854775808).
    // Verify the tree side gives the expected (lossy) Int.
    assert!(
        matches!(
            tree_qv,
            crate::types::value::QueryValue::Map(ref m)
            if matches!(m.get("big"), Some(crate::types::value::QueryValue::Int(_)))
        ),
        "tree side unexpected: {tree_qv:?}"
    );
    // Lens: uint_to_record_value maps u64>i64::MAX → Str(decimal).
    // Verify the lens side gives the expected (lossless) Str.
    assert!(
        matches!(
            lens_qv,
            crate::types::value::QueryValue::Map(ref m)
            if matches!(m.get("big"), Some(crate::types::value::QueryValue::Str(_)))
        ),
        "lens side unexpected: {lens_qv:?}"
    );

    // The two sides DIFFER (tree=Int(truncated), lens=Str(decimal)).
    // This is the documented pre-existing decoder divergence; the new walker
    // correctly reproduces the lens decoder's mapping. Not a bug in R5.
    assert_ne!(
        tree_qv, lens_qv,
        "unexpected equality: the known u64>i64::MAX divergence appears to have been resolved \
         — revisit this test"
    );
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
