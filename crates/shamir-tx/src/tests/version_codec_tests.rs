use crate::version_codec::{decode_version_key, encode_version_key};
use bytes::Bytes;
use proptest::prelude::*;

#[test]
fn round_trip() {
    for v in [0u64, 1, 42, u64::MAX / 2, u64::MAX] {
        let enc = encode_version_key(b"user:42", v);
        let (key, ver) = decode_version_key(&enc).expect("decode");
        assert_eq!(key, b"user:42");
        assert_eq!(ver, v);
    }
}

#[test]
fn empty_key_round_trip() {
    let enc = encode_version_key(b"", 7);
    let (key, ver) = decode_version_key(&enc).unwrap();
    assert_eq!(key, b"");
    assert_eq!(ver, 7);
}

#[test]
fn sort_order_matches_version() {
    // Same original key, increasing version → lexicographic order.
    let mut keys: Vec<Bytes> = (0..10).map(|v| encode_version_key(b"k", v)).collect();
    let original = keys.clone();
    keys.sort();
    assert_eq!(keys, original, "BE encoding must give natural sort order");
}

#[test]
fn different_keys_dont_interleave() {
    // Versions of `aaa` come before any version of `aab`.
    let a_high = encode_version_key(b"aaa", u64::MAX);
    let b_low = encode_version_key(b"aab", 0);
    assert!(a_high < b_low, "key prefix must dominate version suffix");
}

#[test]
fn short_input_decodes_to_none() {
    assert!(decode_version_key(b"").is_none());
    assert!(decode_version_key(&[0u8; 8]).is_none());
}

#[test]
fn missing_separator_decodes_to_none() {
    // 9+ bytes but no 0xFF at position len-9.
    let bad = vec![0x42u8; 16];
    assert!(decode_version_key(&bad).is_none());
}

// Property tests for the MVCC version codec.
//
// Generated keys exclude the separator byte `0xFF` so that the
// invariant documented at the top of this module (key must not
// end in `0xFF` + 8 bytes that look like a version) is upheld
// by construction. Real callers either use random 16-byte
// `RecordId`s or typed `SysKey` encodings — neither produces
// a `0xFF` byte in the trailing position by accident.

/// Arbitrary key that respects the codec invariant: no `0xFF`
/// anywhere. Length 0..=32 covers the empty-key edge case the
/// existing unit tests already pin.
fn key_strategy() -> impl Strategy<Value = Vec<u8>> {
    prop::collection::vec(0u8..=0xFEu8, 0..=32)
}

proptest! {
    /// `decode . encode == identity` for any (key, version) where
    /// `key` upholds the codec invariant.
    #[test]
    fn prop_round_trip(key in key_strategy(), version in any::<u64>()) {
        let enc = encode_version_key(&key, version);
        let (dec_key, dec_ver) = decode_version_key(&enc)
            .expect("encoded bytes must always decode");
        prop_assert_eq!(dec_key, key.as_slice());
        prop_assert_eq!(dec_ver, version);
        prop_assert_eq!(enc.len(), key.len() + 1 + 8);
    }

    /// Byte-lex order on the encoded form matches numeric order
    /// on `version`, for a fixed key. This is the load-bearing
    /// property for MVCC range-scan-by-version.
    #[test]
    fn prop_sort_order_matches_version(
        key in key_strategy(),
        v1 in any::<u64>(),
        v2 in any::<u64>(),
    ) {
        let e1 = encode_version_key(&key, v1);
        let e2 = encode_version_key(&key, v2);
        prop_assert_eq!(e1.as_ref().cmp(e2.as_ref()), v1.cmp(&v2));
    }

    /// Strict-monotonicity restatement: a higher version sorts
    /// strictly after a lower one (same key).
    #[test]
    fn prop_higher_version_sorts_after_lower(
        key in key_strategy(),
        v1 in any::<u64>(),
        v2 in any::<u64>(),
    ) {
        prop_assume!(v1 < v2);
        let e1 = encode_version_key(&key, v1);
        let e2 = encode_version_key(&key, v2);
        prop_assert!(e1 < e2);
    }

    /// Key prefix dominates the version suffix: any two distinct
    /// keys of the same length never interleave in the encoded
    /// space, regardless of version.  The same-length constraint
    /// reflects reality — MVCC keys are fixed-size `RecordId`s
    /// (16 bytes) or typed `SysKey` encodings.  Different-length
    /// keys can violate lex order when the `0xFF` separator in
    /// the shorter key sorts above the next byte of the longer
    /// key (e.g. `encode([], u64::MAX)` > `encode([0x00], 0)`).
    #[test]
    fn prop_key_prefix_dominates_version(
        len in 1usize..=32,
        k1_bytes in prop::collection::vec(0u8..=0xFEu8, 32),
        k2_bytes in prop::collection::vec(0u8..=0xFEu8, 32),
    ) {
        let k1 = &k1_bytes[..len];
        let k2 = &k2_bytes[..len];
        prop_assume!(k1 != k2);
        let (lo, hi) = if k1 < k2 { (k1, k2) } else { (k2, k1) };
        let e_lo_max = encode_version_key(lo, u64::MAX);
        let e_hi_min = encode_version_key(hi, 0);
        prop_assert!(e_lo_max < e_hi_min);
    }
}
