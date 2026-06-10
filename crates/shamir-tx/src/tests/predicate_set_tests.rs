use crate::predicate_set::{
    key_in_interval, PredicateDep, PredicateSet, SORTED_PREFIX_LEN, SORTED_TAG,
};
use bytes::Bytes;
use shamir_types::core::sort_codec;
use std::ops::Bound::{Excluded, Included, Unbounded};

// ---- helpers ----------------------------------------------------------

fn posting(index_id: u64, encoded_value: &[u8], rid_byte: u8) -> Vec<u8> {
    let mut k = Vec::with_capacity(SORTED_PREFIX_LEN + encoded_value.len() + 16);
    k.push(SORTED_TAG);
    k.extend_from_slice(&index_id.to_be_bytes());
    k.extend_from_slice(encoded_value);
    k.extend_from_slice(&[rid_byte; 16]);
    k
}

fn enc_i(v: i64) -> Vec<u8> {
    let mut b = Vec::new();
    sort_codec::encode_i64(&mut b, v);
    b
}
fn enc_s(s: &str) -> Vec<u8> {
    let mut b = Vec::new();
    sort_codec::encode_str(&mut b, s);
    b
}
fn bound_with_prefix(index_id: u64, tail: &[u8]) -> Bytes {
    let mut b = Vec::with_capacity(SORTED_PREFIX_LEN + tail.len());
    b.push(SORTED_TAG);
    b.extend_from_slice(&index_id.to_be_bytes());
    b.extend_from_slice(tail);
    Bytes::from(b)
}
fn full_max_upper(index_id: u64) -> Bytes {
    let mut b = Vec::with_capacity(SORTED_PREFIX_LEN + 64);
    b.push(SORTED_TAG);
    b.extend_from_slice(&index_id.to_be_bytes());
    b.extend_from_slice(&[0xFFu8; 64]);
    Bytes::from(b)
}

// ---- key_in_interval tests -------------------------------------------

#[test]
fn key_in_interval_unbounded_matches_within_index() {
    let k = posting(7, &enc_i(42), 0xAA);
    assert!(key_in_interval(&k, 7, &Unbounded, &Unbounded));
}

#[test]
fn key_in_interval_rejects_wrong_index_id() {
    let k = posting(7, &enc_i(42), 0xAA);
    assert!(!key_in_interval(&k, 8, &Unbounded, &Unbounded));
}

#[test]
fn key_in_interval_rejects_short_key() {
    let k = vec![SORTED_TAG, 0, 0, 0];
    assert!(!key_in_interval(&k, 0, &Unbounded, &Unbounded));
}

#[test]
fn key_in_interval_rejects_non_sorted_tag() {
    let mut k = posting(7, &enc_i(42), 0xAA);
    k[0] = 0x00;
    assert!(!key_in_interval(&k, 7, &Unbounded, &Unbounded));
}

#[test]
fn key_in_interval_gte_inclusive_lo() {
    let lo = bound_with_prefix(7, &enc_i(30));
    let hi = full_max_upper(7);
    assert!(key_in_interval(
        &posting(7, &enc_i(30), 0x00),
        7,
        &Included(lo.clone()),
        &Included(hi.clone())
    ));
    assert!(key_in_interval(
        &posting(7, &enc_i(31), 0x00),
        7,
        &Included(lo.clone()),
        &Included(hi.clone())
    ));
    assert!(!key_in_interval(
        &posting(7, &enc_i(29), 0xFF),
        7,
        &Included(lo),
        &Included(hi)
    ));
}

#[test]
fn key_in_interval_gt_excludes_boundary_value_across_all_rids() {
    let mut bound_tail = enc_i(30);
    bound_tail.extend_from_slice(&[0xFFu8; 16]);
    let lo = bound_with_prefix(7, &bound_tail);
    let hi = full_max_upper(7);
    for rid in [0x00u8, 0x7F, 0xFF] {
        assert!(
            !key_in_interval(
                &posting(7, &enc_i(30), rid),
                7,
                &Excluded(lo.clone()),
                &Included(hi.clone())
            ),
            "rid byte {rid:#x} at boundary should be excluded"
        );
    }
    assert!(key_in_interval(
        &posting(7, &enc_i(31), 0x00),
        7,
        &Excluded(lo),
        &Included(hi)
    ));
}

#[test]
fn key_in_interval_lt_excludes_boundary_value() {
    let hi = bound_with_prefix(7, &enc_i(20));
    let lo = bound_with_prefix(7, b"");
    assert!(!key_in_interval(
        &posting(7, &enc_i(20), 0x00),
        7,
        &Included(lo.clone()),
        &Excluded(hi.clone())
    ));
    assert!(key_in_interval(
        &posting(7, &enc_i(19), 0xFF),
        7,
        &Included(lo),
        &Excluded(hi)
    ));
}

#[test]
fn key_in_interval_between_inclusive() {
    let lo = bound_with_prefix(7, &enc_i(10));
    let mut hi_tail = enc_i(20);
    hi_tail.extend_from_slice(&[0xFFu8; 16]);
    let hi = bound_with_prefix(7, &hi_tail);
    for v in [10, 15, 20] {
        assert!(
            key_in_interval(
                &posting(7, &enc_i(v), 0x42),
                7,
                &Included(lo.clone()),
                &Included(hi.clone())
            ),
            "{v} should be in [10,20]"
        );
    }
    for v in [9, 21] {
        assert!(
            !key_in_interval(
                &posting(7, &enc_i(v), 0x42),
                7,
                &Included(lo.clone()),
                &Included(hi.clone())
            ),
            "{v} should be out of [10,20]"
        );
    }
}

#[test]
fn key_in_interval_string_eq_degenerate_range() {
    let lo = bound_with_prefix(7, &enc_s("bob"));
    let mut hi_tail = enc_s("bob");
    hi_tail.extend_from_slice(&[0xFFu8; 16]);
    let hi = bound_with_prefix(7, &hi_tail);
    assert!(key_in_interval(
        &posting(7, &enc_s("bob"), 0x00),
        7,
        &Included(lo.clone()),
        &Included(hi.clone())
    ));
    assert!(!key_in_interval(
        &posting(7, &enc_s("bo"), 0xFF),
        7,
        &Included(lo.clone()),
        &Included(hi.clone())
    ));
    assert!(!key_in_interval(
        &posting(7, &enc_s("boby"), 0x00),
        7,
        &Included(lo),
        &Included(hi)
    ));
}

#[test]
fn key_in_interval_prefix_tag_matches_engine_constant() {
    assert_eq!(SORTED_TAG, 0x80);
}

// ---- PredicateSet tests (pre-existing) --------------------------------

#[test]
fn new_predicate_set_is_empty() {
    let ps = PredicateSet::new();
    assert!(ps.is_empty());
    assert_eq!(ps.len(), 0);
}

#[test]
fn push_and_len() {
    let ps = PredicateSet::new();
    ps.push(PredicateDep::TableScan { table_token: 7 });
    ps.push(PredicateDep::IndexRange {
        table_token: 7,
        index_id: 42,
        lo: Included(Bytes::from_static(b"\x00")),
        hi: Unbounded,
    });
    assert_eq!(ps.len(), 2);
    let mut seen = 0usize;
    ps.with_iter(|_| seen += 1);
    assert_eq!(seen, 2);
}

#[test]
fn push_through_shared_ref() {
    let ps = PredicateSet::new();
    let r: &PredicateSet = &ps;
    r.push(PredicateDep::TableScan { table_token: 1 });
    assert_eq!(ps.len(), 1);
}
