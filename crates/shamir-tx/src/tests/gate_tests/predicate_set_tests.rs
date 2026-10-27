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

// RV-TX-52FB938F: tx-side sort_codec ordering and interval membership oracle.
// Cross-crate layout contract lives in the manager-backed engine oracle
// `rv_predicate_range_52fb938f_manager_posting_matches_shared_layout_and_tx_consumer`.
// This test does not invoke the index producer.

#[test]
fn rv_predicate_set_52fb938f_posting_layout_round_trips_through_sort_codec() {
    // (1) sort_codec's encodings are order-preserving under plain lexicographic compare.
    let ints = [-9223372036854775808i64, -1, 0, 1, 42, 9223372036854775807];
    let encoded: Vec<Vec<u8>> = ints.iter().map(|v| enc_i(*v)).collect();
    for w in encoded.windows(2) {
        assert!(
            w[0] < w[1],
            "sort_codec i64 encodings must be lexicographically ascending"
        );
    }
    let strs = ["bo", "bob", "boby"];
    let s_encoded: Vec<Vec<u8>> = strs.iter().map(|s| enc_s(s)).collect();
    for w in s_encoded.windows(2) {
        assert!(w[0] < w[1], "sort_codec str encodings must ascend");
    }

    // (2) Membership agrees with value-space comparison for both key kinds.
    for (i, &v) in ints.iter().enumerate() {
        for lo in ints {
            for hi in ints {
                if lo > hi {
                    continue;
                }
                let want = v >= lo && v <= hi;
                let got = key_in_interval(
                    &posting(7, &encoded[i], 0x42),
                    7,
                    &Included(Bytes::from(bound(7, &enc_i(lo)))),
                    &Included(Bytes::from(bound_hi_inclusive(7, &enc_i(hi)))),
                );
                assert_eq!(got, want, "i64 {v} in [{lo},{hi}]");
            }
        }
    }
    for (i, s) in strs.iter().enumerate() {
        for lo in strs {
            for hi in strs {
                if lo > hi {
                    continue;
                }
                let want = *s >= lo && *s <= hi;
                let got = key_in_interval(
                    &posting(7, &s_encoded[i], 0x42),
                    7,
                    &Included(Bytes::from(bound(7, &enc_s(lo)))),
                    &Included(Bytes::from(bound_hi_inclusive(7, &enc_s(hi)))),
                );
                assert_eq!(got, want, "str {s} in [{lo},{hi}]");
            }
        }
    }

    // (3) The full posting layout (tag || index_id BE8 || value || rid 16)
    // sorts identically to (value, rid) — the scan order range pulls rely on.
    let mut keys: Vec<(i64, u8, Vec<u8>)> = [(5i64, 2u8), (5, 1), (4, 9), (6, 0)]
        .into_iter()
        .map(|(v, rid)| (v, rid, posting(7, &enc_i(v), rid)))
        .collect();
    keys.sort_by(|a, b| a.2.cmp(&b.2));
    let mut expected: Vec<(i64, u8)> = keys.iter().map(|(v, r, _)| (*v, *r)).collect();
    expected.sort_unstable();
    let actual: Vec<(i64, u8)> = keys.iter().map(|(v, r, _)| (*v, *r)).collect();
    assert_eq!(actual, expected, "byte order must equal (value, rid) order");
}

fn bound(index_id: u64, tail: &[u8]) -> Vec<u8> {
    let mut b = Vec::with_capacity(SORTED_PREFIX_LEN + tail.len());
    b.push(SORTED_TAG);
    b.extend_from_slice(&index_id.to_be_bytes());
    b.extend_from_slice(tail);
    b
}

/// Inclusive upper bound in the producer's convention: `prefix || value ||
/// 0xFF × 16` — mirrors `SortedIndexManager::range_bounds` (shamir-index
/// scan.rs), which appends the 16-byte 0xFF rid suffix so that a posting
/// whose value equals the hi value still compares <= the bound.
fn bound_hi_inclusive(index_id: u64, tail: &[u8]) -> Vec<u8> {
    let mut b = bound(index_id, tail);
    b.extend_from_slice(&shamir_types::core::sorted_index_layout::inclusive_hi_suffix());
    b
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
