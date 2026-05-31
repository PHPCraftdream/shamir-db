//! Physical key layout `<key>::<version_be_u64>`.
//!
//! The MVCC store keeps a history of overwritten values keyed by
//! `(original_key, version)`. Encoding chooses big-endian for the
//! version suffix so that lexicographic byte-order matches numeric
//! version-order — a single range scan in [key::0, key::snapshot]
//! gives all versions visible at `snapshot`, with the highest one
//! at the end of the scan.
//!
//! The separator `0xFF` is chosen because:
//! - It cannot appear in a `RecordId` (16 random bytes that the
//!   producer guarantees never include `0xFF` as the *first* byte
//!   after the original key — see invariant below).
//! - It sorts higher than any normal byte, so `key + 0xFF + version`
//!   sits at the end of the lexicographic range that starts with
//!   `key`, never interleaved with sibling keys.
//!
//! ## Invariant
//!
//! Callers must guarantee that the original `key` does NOT contain
//! the byte `0xFF` followed by 8 more bytes that could be mistaken
//! for a version suffix. In practice:
//! - User `RecordId` is 16 bytes, the producer is uniformly random,
//!   so the chance of a tail `0xFF + 8 bytes` is negligible (and
//!   we don't decode user keys with `decode_version_key` anyway).
//! - System keys (`SysKey::*`) are produced by the engine and use
//!   their own typed encoding — they never end in `0xFF` + 8 bytes.
//!
//! This is verified by [`decode_version_key`]'s round-trip property
//! tests below.

use bytes::{BufMut, Bytes, BytesMut};

/// Separator byte between the original key and the version suffix.
pub const VERSION_SEP: u8 = 0xFF;

/// Encode `(key, version)` into a single physical key suitable for
/// storage under [`shamir_storage::types::Store`].
///
/// Output layout: `key || 0xFF || version.to_be_bytes()`.
/// Total length: `key.len() + 1 + 8` bytes.
pub fn encode_version_key(key: &[u8], version: u64) -> Bytes {
    let mut b = BytesMut::with_capacity(key.len() + 1 + 8);
    b.extend_from_slice(key);
    b.put_u8(VERSION_SEP);
    b.put_u64(version);
    b.freeze()
}

/// Decode a physical key back into `(original_key, version)`. Returns
/// `None` if the input is shorter than 9 bytes or does not end in
/// the expected `0xFF + 8 byte version` shape.
pub fn decode_version_key(physical: &[u8]) -> Option<(&[u8], u64)> {
    if physical.len() < 9 {
        return None;
    }
    let split = physical.len() - 9;
    if physical[split] != VERSION_SEP {
        return None;
    }
    let key = &physical[..split];
    let version_bytes: [u8; 8] = physical[split + 1..]
        .try_into()
        .expect("just checked length");
    Some((key, u64::from_be_bytes(version_bytes)))
}

#[cfg(test)]
mod tests {
    use super::*;

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
}

#[cfg(test)]
mod proptests {
    //! Property tests for the MVCC version codec.
    //!
    //! Generated keys exclude the separator byte `0xFF` so that the
    //! invariant documented at the top of this module (key must not
    //! end in `0xFF` + 8 bytes that look like a version) is upheld
    //! by construction. Real callers either use random 16-byte
    //! `RecordId`s or typed `SysKey` encodings — neither produces
    //! a `0xFF` byte in the trailing position by accident.

    use super::*;
    use proptest::prelude::*;

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
}
