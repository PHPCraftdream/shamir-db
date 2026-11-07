//! FG-6: an `Eq` filter built via `lit_u64(large_value)` now MATCHES a stored
//! field whose value is a raw `u64 > i64::MAX` (promoted to `Big`/decimal
//! `Str`), on both the lens (hot) and tree (cold/MVCC-Owned) read paths.
//!
//! ## Background (FG-1 finding, now fixed by FG-6)
//!
//! After FG-1, a `u64 > i64::MAX` decodes losslessly to `Value::Big`/
//! `QueryValue::Big` instead of wrapping/clamping. `FilterNode::Compare`
//! extracts the record field via `RecordRef::scalar_at`, which returns `None`
//! for a promoted `Big` value on BOTH read paths:
//! - **Lens / hot path** (`RecordView`): `uint_to_record_value` maps the raw
//!   bytes to `RecordValue::Big(u64)` without allocation; the variant is
//!   non-comparable and materializes to `InnerValue::Big`.
//! - **Tree / cold (MVCC-Owned) path** (`InnerValue`): the field decodes to
//!   `InnerValue::Big(BigInt)`, and `ScalarRef` has no `Big` variant by
//!   design (see `scalar_ref.rs`).
//!
//! `FilterNode::Compare` (`filter_node.rs`) now distinguishes "field absent"
//! from "field present but `scalar_at`-non-comparable": when `scalar_at`
//! returns `None` AND `present_kind_at` reports the field IS present, it
//! falls back to `record.materialize_at(path)` (one owned `InnerValue` leaf)
//! plus `inner_value_to_query_value` (a single, cheap, interner-free
//! conversion for scalar/Dec/Big leaves) and compares via `compare_values`
//! — the SAME helper `Filter::ValueCompare` and the `Min`/`Max` aggregates
//! already use, which already has correct `Int`↔`Big` cross-type arms (f64
//! fallback). This mirrors the identical `FieldRef` resolution boundary
//! already used by `resolve_filter_query`, and the Dec/Big fallback already
//! used by `AggAccum`'s Sum/Avg/Min/Max in `aggregate.rs` — not a new
//! pattern.
//!
//! Every ordinary Bool/Int/F64/Str/Bin field still resolves via the
//! zero-copy `scalar_at` fast path above, unchanged; the `materialize_at`
//! fallback only triggers for the rare Dec/Big/promoted-u64 leaf.

use crate::query::filter::eval::{compile_filter, resolve_filter_query};
use crate::query::filter::eval_context::FilterContext;
use crate::query::filter::{Filter, FilterValue};
use shamir_index::base_index::index_info_item::IndexInfoItem;
use shamir_index::base_index::index_keys::build_index_key_from_record;
use shamir_query_builder::val::lit_u64;
use shamir_query_builder::{filter, val};
use shamir_types::core::interner::Interner;
use shamir_types::record_view::{RecordRef, RecordView};
use shamir_types::types::common::new_map;
use shamir_types::types::value::InnerValue;

use crate::query::filter::tests::eval_tests::shared::helpers::empty_refs;
use num_bigint::BigInt;
/// Build a raw msgpack map `{ <bin key for `field`>: <uint64 value> }` — the
/// exact storage shape a non-Rust encoder emits on the wire. Bypasses the
/// Rust builder's `i64`-typed surface so a genuine `uint64 > i64::MAX`
/// reaches the decoder.
fn raw_uint64_record(interner: &Interner, field: &str, value: u64) -> Vec<u8> {
    let key = interner.touch_ind(field).unwrap().into_key();
    let (key_buf, key_len) = key.as_bytes_buf();
    let key_bytes = &key_buf[..key_len];

    let mut blob = Vec::new();
    blob.push(0x81); // fixmap, 1 entry
    blob.push(0xc4); // bin8
    blob.push(key_len as u8);
    blob.extend_from_slice(key_bytes);
    blob.push(0xcf); // uint64
    blob.extend_from_slice(&value.to_be_bytes());
    blob
}

/// Build a materialised `InnerValue` tree record `{ field: Big(value) }`
/// — the in-memory shape fix site 1 produces for a raw `uint64 > i64::MAX`.
fn tree_big_record(interner: &Interner, field: &str, value: u64) -> InnerValue {
    let key = interner.touch_ind(field).unwrap().into_key();
    let mut map = new_map();
    map.insert(key, InnerValue::Big(BigInt::from(value)));
    InnerValue::Map(map)
}

const LOW: u64 = 9_999_999_999_999_999_999;
const HIGH: u64 = 10_000_000_000_000_000_000;

fn string_record(interner: &Interner, value: u64) -> InnerValue {
    let key = interner.touch_ind("n").unwrap().into_key();
    let mut map = new_map();
    map.insert(key, InnerValue::Str(value.to_string()));
    InnerValue::Map(map)
}

fn assert_contract(record: &impl RecordRef, interner: &Interner, value: u64, string: bool) {
    let refs = empty_refs();
    let ctx = FilterContext::new(interner, &refs);
    let matches = |filter: Filter| compile_filter(&filter, interner).matches(record, &ctx);
    let other = if value == LOW { HIGH } else { LOW };
    assert!(matches(filter::eq("n", lit_u64(value))));
    assert!(!matches(filter::eq("n", lit_u64(other))));
    // String/String is lexical; Big/String is numeric.
    assert_eq!(
        matches(filter::lt("n", lit_u64(other))),
        if string { value == HIGH } else { value == LOW }
    );
    assert_eq!(
        matches(filter::gt("n", lit_u64(other))),
        if string { value == LOW } else { value == HIGH }
    );
    assert!(matches(filter::gte("n", lit_u64(value))));
    assert!(matches(filter::lte("n", lit_u64(value))));
    // Between only extracts ScalarRef, so Big/raw-u64 are unsupported.
    assert_eq!(
        matches(filter::between("n", lit_u64(value), lit_u64(value))),
        string
    );
    // Literal InSet and dynamic In share the same scalar extraction limit.
    assert_eq!(matches(filter::in_("n", [lit_u64(value)])), string);
    assert!(!matches(filter::in_("n", [lit_u64(other)])));
    assert_eq!(matches(filter::in_("n", [val::col("n")])), string);
}

#[test]
fn lit_u64_order_range_and_membership_storage_contract() {
    let interner = Interner::new();
    for value in [LOW, HIGH] {
        let tree = tree_big_record(&interner, "n", value);
        let text = string_record(&interner, value);
        let bytes = raw_uint64_record(&interner, "n", value);
        let lens = RecordView::new(&bytes).unwrap();
        assert_contract(&tree, &interner, value, false);
        assert_contract(&text, &interner, value, true);
        assert_contract(&lens, &interner, value, false);
    }
}

#[test]
fn rv_types_18ac0cfc_index_key_raw_u64_parity() {
    let interner = Interner::new();
    let field = interner.touch_ind("n").unwrap().into_key();
    let name_interned = interner.touch_ind("n_index").unwrap().into_key().id();
    let paths = [IndexInfoItem::new(vec![field.id()])];

    for value in [
        i64::MAX as u64,
        i64::MAX as u64 + 1,
        i64::MAX as u64 + 3,
        u64::MAX,
    ] {
        let blob = raw_uint64_record(&interner, "n", value);
        let view = RecordView::new(&blob).expect("RecordView decodes uint64 record");
        let tree = InnerValue::from_bytes(&blob).expect("tree decodes uint64 record");
        let (view_key, view_canonical) =
            build_index_key_from_record(false, name_interned, &view, &paths)
                .expect("present raw field has an index key");
        let (tree_key, tree_canonical) =
            build_index_key_from_record(false, name_interned, &tree, &paths)
                .expect("present tree field has an index key");
        // Identity v2: the canonical identity replaces the two hash words, and
        // the key bytes carry the namespace (regular/unique) and the index name.
        assert_eq!(
            view_canonical, tree_canonical,
            "canonical identity for {value}"
        );
        assert_eq!(
            view_key.to_bytes(),
            tree_key.to_bytes(),
            "key bytes for {value}"
        );
        assert_eq!(view_key.name_interned(), tree_key.name_interned());
    }
}

// ── Positive: representation is lossless on both paths ───────────────────────

/// The lens decodes a raw `uint64` field as its exact nonallocating Big value.
#[test]
fn fg1_lens_decodes_uint64_losslessly() {
    let interner = Interner::new();
    let blob = raw_uint64_record(&interner, "n", u64::MAX);
    let view = RecordView::new(&blob).expect("RecordView decodes uint64 record");

    let key = interner.touch_ind("n").unwrap().into_key();
    let lens_val = view.get(key);
    match lens_val {
        Some(shamir_types::record_view::RecordValue::Big(value)) => {
            assert_eq!(value, u64::MAX);
        }
        other => panic!("lens must map uint64>max to Big, got {other:?}"),
    }
}

/// The tree decoder (fix site 1) decodes a raw `uint64` field to a lossless
/// `Big(BigInt)`.
#[test]
fn fg1_tree_decodes_uint64_losslessly() {
    let interner = Interner::new();
    let blob = raw_uint64_record(&interner, "n", u64::MAX);
    let tree = InnerValue::from_bytes(&blob).expect("tree decodes uint64 record");

    let key = interner.touch_ind("n").unwrap().into_key();
    match &tree {
        InnerValue::Map(m) => match m.get(&key) {
            Some(InnerValue::Big(b)) => {
                assert_eq!(b, &BigInt::from(u64::MAX));
                assert_eq!(b.to_string(), "18446744073709551615");
            }
            other => panic!("tree must map uint64>max to Big, got {other:?}"),
        },
        other => panic!("expected Map, got {other:?}"),
    }
}

/// `lit_u64(u64::MAX)` produces the correct lossless `FilterValue::String`.
#[test]
fn fg1_lit_u64_produces_lossless_decimal_string() {
    match lit_u64(u64::MAX) {
        FilterValue::String(s) => assert_eq!(s, "18446744073709551615"),
        other => panic!("expected FilterValue::String, got {other:?}"),
    }
}

// ── FG-6: filter-MATCH now works on both read paths ──────────────────────────

/// **FG-6 fix (lens path).** An `Eq` filter via `lit_u64(u64::MAX)` now
/// MATCHES a stored `uint64` record through the real filter-eval path, even
/// though `RecordView::scalar_at` still returns `None` for the
/// `Str(Cow::Owned(_))` field — `FilterNode::Compare` detects the field IS
/// present (`present_kind_at`) and falls back to `materialize_at`.
#[test]
fn fg6_lit_u64_eq_lens_path_matches_via_materialize_fallback() {
    let interner = Interner::new();
    let refs = empty_refs();
    let ctx = FilterContext::new(&interner, &refs);

    let large: u64 = u64::MAX;
    let blob = raw_uint64_record(&interner, "n", large);
    let view = RecordView::new(&blob).expect("RecordView decodes uint64 record");

    // The field IS present and lossless in the lens — but scalar_at cannot
    // surface it (Cow::Owned decimal → None); this is the structural gap
    // that `FilterNode::Compare`'s materialize_at fallback now covers.
    let key = interner.touch_ind("n").unwrap().into_key();
    assert!(
        view.get(key.clone()).is_some(),
        "field must be present in the lens"
    );
    assert_eq!(
        view.scalar_at(&[key]),
        None,
        "scalar_at still returns None for Cow::Owned Str (the u64>max edge) \
         — the fallback in FilterNode::Compare handles this, not scalar_at itself"
    );

    // The filter operand resolves correctly to a QueryValue::Str.
    let operand = lit_u64(large);
    let resolved = resolve_filter_query(&operand, &view, &ctx);
    assert!(
        matches!(
            resolved,
            Some(shamir_types::types::value::QueryValue::Str(_))
        ),
        "filter operand resolves to Str: {resolved:?}"
    );

    // The Eq filter now DOES match — FilterNode::Compare falls back to
    // materialize_at + compare_values when scalar_at is None but the field
    // is present.
    let filter = Filter::Eq {
        field: vec!["n".to_string()],
        value: lit_u64(large),
    };
    let node = compile_filter(&filter, &interner);
    assert!(
        node.matches(&view, &ctx),
        "Eq DOES match: FilterNode::Compare falls back to materialize_at \
         for the Cow::Owned Str (promoted u64>max) edge"
    );
}

/// **FG-6 fix (tree path).** An `Eq` filter via `lit_u64(u64::MAX)` now
/// MATCHES a stored `Big` field through the real filter-eval path, even
/// though `InnerValue::scalar_at` still returns `None` for a `Big` leaf
/// (`ScalarRef` has no `Big` variant) — `FilterNode::Compare` falls back to
/// `materialize_at` + `compare_values`.
#[test]
fn fg6_lit_u64_eq_tree_path_matches_via_materialize_fallback() {
    let interner = Interner::new();
    let refs = empty_refs();
    let ctx = FilterContext::new(&interner, &refs);

    let large: u64 = u64::MAX;
    let record = tree_big_record(&interner, "n", large);

    let key = interner.touch_ind("n").unwrap().into_key();
    assert_eq!(
        record.scalar_at(&[key]),
        None,
        "scalar_at still returns None for InnerValue::Big (no Big variant in \
         ScalarRef) — the fallback in FilterNode::Compare handles this"
    );

    let filter = Filter::Eq {
        field: vec!["n".to_string()],
        value: lit_u64(large),
    };
    let node = compile_filter(&filter, &interner);
    assert!(
        node.matches(&record, &ctx),
        "Eq DOES match: FilterNode::Compare falls back to materialize_at \
         for InnerValue::Big"
    );
}

/// FG-6 regression: a non-matching `Big` value must still correctly NOT
/// match (proves the fallback compares VALUES, not just presence).
#[test]
fn fg6_lit_u64_eq_tree_path_big_mismatch_does_not_match() {
    let interner = Interner::new();
    let refs = empty_refs();
    let ctx = FilterContext::new(&interner, &refs);

    // Stored value is u64::MAX, filter looks for a different large value.
    let record = tree_big_record(&interner, "n", u64::MAX);
    let other_large: u64 = i64::MAX as u64 + 1; // also promotes to Big, but != u64::MAX

    let filter = Filter::Eq {
        field: vec!["n".to_string()],
        value: lit_u64(other_large),
    };
    let node = compile_filter(&filter, &interner);
    assert!(
        !node.matches(&record, &ctx),
        "Eq must NOT match a different Big value"
    );
}

// ── Regression: lit_u64 on an ordinary Int field still matches ──────────────

#[test]
fn fg1_lit_u64_eq_matches_normal_int_field() {
    let interner = Interner::new();
    let refs = empty_refs();
    let ctx = FilterContext::new(&interner, &refs);

    let mut map = new_map();
    let k = interner.touch_ind("n").unwrap().into_key();
    map.insert(k, InnerValue::Int(9_000_000_000)); // fits in i64
    let record = InnerValue::Map(map);

    let filter = Filter::Eq {
        field: vec!["n".to_string()],
        value: lit_u64(9_000_000_000),
    };
    let node = compile_filter(&filter, &interner);
    assert!(
        node.matches(&record, &ctx),
        "lit_u64(small) must still match a normal Int field"
    );
}
