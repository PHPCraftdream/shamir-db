//! Category 1 — Property test: `Eq`/`Ord` agreement between `KeyBytes`
//! and `bytes::Bytes` over the byte slice, across both inline and heap
//! representations and the cross-length comparison boundary.
//!
//! Plan doc §5.1/#489-class guard: this is the red-first test that
//! catches a `#[derive]`-on-enum leak — if any of these fail, the
//! representation is escaping through `Eq` or `Ord`.

use super::super::{KeyBytes, INLINE_CAP};
use bytes::Bytes;
use std::cmp::Ordering;

/// Deterministic LCG so the test is reproducible without pulling a
/// `rand` dev-dependency into `shamir-storage` for this one test.
fn lcg(seed: &mut u64) -> u8 {
    *seed = seed
        .wrapping_mul(6364136223846793005)
        .wrapping_add(1442695040888963407);
    (*seed >> 33) as u8
}

fn rand_bytes(seed: &mut u64, len: usize) -> Vec<u8> {
    (0..len).map(|_| lcg(seed)).collect()
}

/// Boundary lengths to probe specifically: 0, 1, the inline-cap
/// boundary on both sides, and a comfortably-heap length.
const BOUNDARY_LENS: [usize; 6] = [0, 1, INLINE_CAP - 1, INLINE_CAP, INLINE_CAP + 1, 64];

#[test]
fn keybytes_as_ref_equals_bytes_as_ref_for_same_input() {
    let mut seed = 0xA5A5_A5A5_A5A5_A5A5u64;
    // Many random inputs at every boundary length.
    for &len in &BOUNDARY_LENS {
        for _ in 0..32 {
            let bytes = rand_bytes(&mut seed, len);
            let kb = KeyBytes::from_slice(&bytes);
            let bb = Bytes::copy_from_slice(&bytes);
            assert_eq!(
                kb.as_ref(),
                bb.as_ref(),
                "as_ref mismatch at len={len} bytes={bytes:?}"
            );
            // Direct slice equality too.
            assert_eq!(&kb[..], bb.as_ref());
        }
    }
}

#[test]
fn keybytes_ord_matches_slice_ord_for_pairs_including_cross_length() {
    let mut seed = 0x1234_5678_9ABC_DEF0u64;

    // Probe many random same-ish-length pairs.
    for &len_a in &BOUNDARY_LENS {
        for &len_b in &BOUNDARY_LENS {
            for _ in 0..8 {
                let a = rand_bytes(&mut seed, len_a);
                let b = rand_bytes(&mut seed, len_b);
                let ka = KeyBytes::from_slice(&a);
                let kb = KeyBytes::from_slice(&b);
                assert_eq!(ka.cmp(&kb), a.cmp(&b), "Ord mismatch: a={a:?} b={b:?}");
                assert_eq!(
                    ka.partial_cmp(&kb),
                    Some(a.cmp(&b)),
                    "PartialOrd mismatch: a={a:?} b={b:?}"
                );
            }
        }
    }
}

#[test]
fn prefix_key_compares_less_than_longer_key_matching_slice_semantics() {
    // The specific cross-length case the plan doc calls out: a 5-byte
    // key that is a strict prefix of a 6-byte key must compare `Less`,
    // exactly like `[u8]`'s `Ord` — NOT numeric comparison of any kind.
    let short = KeyBytes::from_slice(&[1, 2, 3, 4, 5]);
    let long = KeyBytes::from_slice(&[1, 2, 3, 4, 5, 6]);
    assert_eq!(short.cmp(&long), Ordering::Less, "prefix must be Less");
    assert_eq!(long.cmp(&short), Ordering::Greater);

    // And the mirror via raw slices to pin the source of truth: arrays
    // of differing lengths must be compared as slices ([u8]::cmp), not
    // via the same-N-only inherent array Ord.
    assert_eq!(
        [1u8, 2, 3, 4, 5][..].cmp(&[1u8, 2, 3, 4, 5, 6][..]),
        Ordering::Less
    );
}

#[test]
fn eq_is_byte_identity_not_representation() {
    // Same bytes via inline constructor vs via forced-heap constructor
    // must be equal — this is the inline-vs-heap unobservability check
    // for `Eq` specifically (the Hash half lives in hash_consistency_tests).
    let bytes = [7u8; INLINE_CAP]; // exactly at the inline boundary
    let inline = KeyBytes::from_slice(&bytes);
    let heap = KeyBytes::force_heap_for_test(&bytes);
    assert!(
        inline.is_inline(),
        "sanity: inline variant should be inline"
    );
    assert!(
        !heap.is_inline(),
        "sanity: forced-heap variant should be heap"
    );
    assert_eq!(inline, heap, "inline vs heap of same bytes must be Eq");
    assert_eq!(inline.as_slice(), heap.as_slice());
}

#[test]
fn different_bytes_are_unequal_across_repr_boundary() {
    // A 16-byte inline key must differ from any other 16-byte inline key,
    // and from a >INLINE_CAP heap key, purely on byte content.
    let a = KeyBytes::from_slice(&[0u8; 16]);
    let b = KeyBytes::from_slice(&[1u8; 16]);
    assert_ne!(a, b);
    assert_ne!(
        KeyBytes::from_slice(&[0u8; 16]),
        KeyBytes::from_slice(&[0u8; 17])
    );
}

#[test]
fn partial_eq_bytes_and_slice_conveniences() {
    let kb = KeyBytes::from_slice(&[1, 2, 3]);
    // PartialEq<[u8]> — compare against a fixed-size array, which coerces
    // to [u8] via the generic slice comparison. Test the [u8] path
    // directly through a slice reference deref.
    let arr: [u8; 3] = [1, 2, 3];
    assert!(kb == arr[..], "PartialEq<[u8]> should hold for equal bytes");
    assert_eq!(kb, Bytes::from(vec![1u8, 2, 3])); // PartialEq<Bytes>
                                                  // Negative:
    let other: [u8; 3] = [9, 9, 9];
    assert!(kb != other[..]);
    assert_ne!(kb, Bytes::from(vec![9u8, 9, 9]));
}
