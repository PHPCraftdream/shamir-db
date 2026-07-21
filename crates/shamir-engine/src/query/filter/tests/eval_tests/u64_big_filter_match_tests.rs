//! FG-1 mandatory empirical verification: does an `Eq` filter built via
//! `lit_u64(large_value)` actually MATCH a stored field whose value is a
//! raw `u64 > i64::MAX`?
//!
//! ## Finding (PRECISE — per brief's "STOP and report" rule)
//!
//! **No — the filter does NOT match on either read path.** The brief
//! hypothesised that the existing `Big`↔`Str` cross-type EQUALITY bridge in
//! `hashable_query_value.rs` (`canonical_eq`) would feed
//! `FilterValue::String` (from `lit_u64`) into a comparison against a stored
//! `Big`/`Str` value. That hypothesis is structurally wrong:
//!
//! `FilterNode::Compare` (the `Eq`/`Gt`/... arm) extracts the record field via
//! `RecordRef::scalar_at` and compares it against the resolved filter operand
//! via `scalar_ref_cmp_qv`. **Neither path can surface a `u64 > i64::MAX`
//! field as a `ScalarRef`:**
//!
//! * **Lens / hot read path** (`RecordView`): `uint_to_record_value` maps the
//!   raw bytes to `RecordValue::Str(Cow::Owned(decimal_string))` (the decimal
//!   text is allocated because it isn't in the buffer). `scalar_at` then
//!   returns `None` — `ScalarRef::Str(&'a str)` requires a borrow tied to the
//!   `RecordView`'s lifetime, but an owned `String` can't be borrowed that way
//!   (see `record_ref.rs` lines 267-278, the explicit `Cow::Owned(_) => None`
//!   arm).
//! * **Tree / cold (MVCC-Owned) path** (`InnerValue`): fix site 1 now decodes
//!   the raw `uint64` losslessly to `InnerValue::Big(BigInt)`. But
//!   `inner_to_scalar` maps `Big` to `None` (`ScalarRef` has no `Big` variant
//!   by design — see `scalar_ref.rs` lines 28-30).
//!
//! In both cases `scalar_at` returns `None`, so `FilterNode::Compare` falls
//! to `(None, _) | (_, None) => matches!(op, CompareOp::Ne)` → `Eq` is
//! `false`. The `canonical_eq` bridge is only consulted by the
//! DEDUP/group-by/distinct layer (`HashableQueryValue`), NOT by the
//! filter-eval comparison.
//!
//! ## What IS correct (this task's fixes)
//!
//! The fix sites 1-6 are about **lossless REPRESENTATION** (no more silent
//! wrapping/clamping). They are correct: the stored value is now the exact
//! `Big`/`Str(decimal)`, not `-1` or `i64::MAX`. The `lit_u64` →
//! `FilterValue::String` representation is the correct lossless choice.
//!
//! The filter-MATCH gap is a **pre-existing structural limitation** of the
//! `scalar_at` extraction layer: before this task the value was SILENTLY
//! CORRUPTED (wrapped to -1 / clamped to i64::MAX), so a filter "matched"
//! against the corrupted value. Now the value is CORRECT in storage, but
//! `scalar_at` cannot surface it for comparison. This is a strictly better
//! state (correct value + a known, documented limitation), but the gap must
//! NOT be papered over — it is reported here precisely so it can be
//! re-scoped as its own follow-up (e.g. a `ScalarRef::Big` variant, or a
//! `FilterNode::Compare` fallback to `materialize_at` for non-scalar fields).

use crate::query::filter::eval::{compile_filter, resolve_filter_query};
use crate::query::filter::eval_context::FilterContext;
use crate::query::filter::{Filter, FilterValue};
use shamir_query_builder::val::lit_u64;
use shamir_types::core::interner::Interner;
use shamir_types::record_view::{RecordRef, RecordView};
use shamir_types::types::common::new_map;
use shamir_types::types::value::InnerValue;

use super::helpers::empty_refs;
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

// ── Positive: representation is lossless on both paths ───────────────────────

/// The lens decodes a raw `uint64` field to a lossless decimal `Str`
/// (`Cow::Owned` — the value is NOT corrupted to `-1` or `i64::MAX`).
#[test]
fn fg1_lens_decodes_uint64_losslessly() {
    let interner = Interner::new();
    let blob = raw_uint64_record(&interner, "n", u64::MAX);
    let view = RecordView::new(&blob).expect("RecordView decodes uint64 record");

    let key = interner.touch_ind("n").unwrap().into_key();
    let lens_val = view.get(key);
    match lens_val {
        Some(shamir_types::record_view::RecordValue::Str(s)) => {
            assert_eq!(s.as_ref(), "18446744073709551615");
        }
        other => panic!("lens must map uint64>max to Str(decimal), got {other:?}"),
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

// ── Empirical verification: filter-MATCH outcome (the gap) ──────────────────

/// **MANDATORY empirical finding (lens path).** An `Eq` filter via
/// `lit_u64(u64::MAX)` does NOT match a stored `uint64` record through the
/// real filter-eval path. The reason: `RecordView::scalar_at` returns `None`
/// for the `Str(Cow::Owned(_))` field (the decimal text can't be borrowed
/// into `ScalarRef::Str(&'a str)`). So `FilterNode::Compare` sees an absent
/// field → `Eq` is `false`.
#[test]
fn fg1_lit_u64_eq_lens_path_scalar_at_is_none() {
    let interner = Interner::new();
    let refs = empty_refs();
    let ctx = FilterContext::new(&interner, &refs);

    let large: u64 = u64::MAX;
    let blob = raw_uint64_record(&interner, "n", large);
    let view = RecordView::new(&blob).expect("RecordView decodes uint64 record");

    // The field IS present and lossless in the lens — but scalar_at cannot
    // surface it (Cow::Owned decimal → None).
    let key = interner.touch_ind("n").unwrap().into_key();
    assert!(
        view.get(key.clone()).is_some(),
        "field must be present in the lens"
    );
    assert_eq!(
        view.scalar_at(&[key]),
        None,
        "scalar_at returns None for Cow::Owned Str (the u64>max edge)"
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

    // But the Eq filter does NOT match (scalar_at is None on the record side).
    let filter = Filter::Eq {
        field: vec!["n".to_string()],
        value: lit_u64(large),
    };
    let node = compile_filter(&filter, &interner);
    assert!(
        !node.matches(&view, &ctx),
        "Eq does NOT match: scalar_at is None for Cow::Owned Str \
         (documented gap — see module docs)"
    );
}

/// **MANDATORY empirical finding (tree path).** An `Eq` filter via
/// `lit_u64(u64::MAX)` does NOT match a stored `Big` field through the
/// real filter-eval path. The reason: `InnerValue::scalar_at` returns `None`
/// for a `Big` leaf (`ScalarRef` has no `Big` variant by design).
#[test]
fn fg1_lit_u64_eq_tree_path_scalar_at_is_none() {
    let interner = Interner::new();
    let refs = empty_refs();
    let ctx = FilterContext::new(&interner, &refs);

    let large: u64 = u64::MAX;
    let record = tree_big_record(&interner, "n", large);

    let key = interner.touch_ind("n").unwrap().into_key();
    assert_eq!(
        record.scalar_at(&[key]),
        None,
        "scalar_at returns None for InnerValue::Big (no Big variant in ScalarRef)"
    );

    let filter = Filter::Eq {
        field: vec!["n".to_string()],
        value: lit_u64(large),
    };
    let node = compile_filter(&filter, &interner);
    assert!(
        !node.matches(&record, &ctx),
        "Eq does NOT match: scalar_at is None for Big \
         (documented gap — see module docs)"
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
