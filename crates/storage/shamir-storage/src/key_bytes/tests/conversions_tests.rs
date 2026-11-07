//! Category 7 — `From<Bytes>`, `From<Vec<u8>>`, `From<&'static [u8]>`,
//! and `From<KeyBytes> for Bytes` round-trips.

use super::super::{KeyBytes, INLINE_CAP};
use bytes::Bytes;

#[test]
fn from_bytes_roundtrips_inline_and_heap() {
    // Short input: From<Bytes> takes the inline-copy path; the round
    // trip back to Bytes must still preserve the bytes.
    let short = Bytes::from_static(&[1, 2, 3, 4, 5]);
    let kb: KeyBytes = short.clone().into();
    assert!(kb.is_inline());
    let back: Bytes = kb.into();
    assert_eq!(back, short);

    // Long input: From<Bytes> takes the heap variant verbatim (refcount
    // bump, zero-copy), and From<KeyBytes> for Bytes moves it back out
    // without copy.
    let long = Bytes::from(vec![0x5Au8; INLINE_CAP * 4]);
    let kb: KeyBytes = long.clone().into();
    assert!(!kb.is_inline());
    let back: Bytes = kb.into();
    assert_eq!(back, long);
}

#[test]
fn from_vec_roundtrips_inline_and_heap() {
    let short = vec![9u8; INLINE_CAP];
    let kb: KeyBytes = short.clone().into();
    assert!(kb.is_inline());
    assert_eq!(kb.as_slice(), &short[..]);

    let long = vec![0x11u8; INLINE_CAP + 10];
    let kb: KeyBytes = long.clone().into();
    assert!(!kb.is_inline());
    assert_eq!(kb.as_slice(), &long[..]);
}

#[test]
fn from_static_slice_roundtrips() {
    // Explicitly type the source as `&'static [u8]` so the conversion
    // resolves to `From<&'static [u8]>`.
    let lit: &'static [u8] = b"hello world, this is a key";
    let kb: KeyBytes = lit.into();
    let kb2: KeyBytes = KeyBytes::from_slice(lit);
    assert_eq!(kb.as_slice(), lit);
    assert_eq!(kb, kb2);
}

#[test]
fn from_static_byte_literal_is_inline_when_short() {
    let lit: &'static [u8] = b"abc";
    let kb: KeyBytes = lit.into();
    assert!(kb.is_inline());
    assert_eq!(kb.as_slice(), lit);
}

#[test]
fn from_keybytes_to_bytes_preserves_length_and_content() {
    // Two-way conversion parity for both representations.
    let inline_src = KeyBytes::from_slice(&[0u8; 16]);
    let bytes: Bytes = inline_src.clone().into();
    assert_eq!(bytes.as_ref(), inline_src.as_slice());
    let back: KeyBytes = bytes.into();
    assert_eq!(back, inline_src);

    let heap_src = KeyBytes::from_slice(&[0x77u8; 100]);
    let bytes: Bytes = heap_src.clone().into();
    assert_eq!(bytes.as_ref(), heap_src.as_slice());
    let back: KeyBytes = bytes.into();
    assert_eq!(back, heap_src);
}
