//! Category 5 — Boundary / edge cases.
//!
//! Directly asserts the internal representation choice (`Inline` vs
//! `Heap`) at the boundary lengths via the test-only `is_inline()`
//! accessor: empty (Inline), exactly `INLINE_CAP` (Inline), and
//! `INLINE_CAP + 1` (Heap).

use super::super::{KeyBytes, INLINE_CAP};

#[test]
fn empty_key_is_inline_with_zero_length_slice() {
    let k = KeyBytes::from_slice(&[]);
    assert!(k.is_inline(), "empty key must be inline");
    assert_eq!(k.as_slice().len(), 0);
    assert!(k.as_slice().is_empty());
}

#[test]
fn exactly_inline_cap_bytes_is_inline() {
    let bytes = [0xCDu8; INLINE_CAP];
    let k = KeyBytes::from_slice(&bytes);
    assert!(
        k.is_inline(),
        "key of exactly INLINE_CAP bytes must be inline"
    );
    assert_eq!(k.as_slice(), &bytes[..]);
    assert_eq!(k.len(), INLINE_CAP);
}

#[test]
fn inline_cap_plus_one_bytes_is_heap() {
    let bytes = vec![0xABu8; INLINE_CAP + 1];
    let k = KeyBytes::from_slice(&bytes);
    assert!(!k.is_inline(), "key of INLINE_CAP+1 bytes must be heap");
    assert_eq!(k.as_slice(), &bytes[..]);
}

#[test]
fn single_byte_key_is_inline() {
    let k = KeyBytes::from_slice(&[42]);
    assert!(k.is_inline());
    assert_eq!(k.as_slice(), &[42]);
}

#[test]
fn long_arbitrary_key_is_heap_and_preserves_bytes() {
    let bytes: Vec<u8> = (0..200).map(|i| (i & 0xFF) as u8).collect();
    let k = KeyBytes::from_slice(&bytes);
    assert!(!k.is_inline());
    assert_eq!(k.as_slice(), bytes.as_slice());
}
