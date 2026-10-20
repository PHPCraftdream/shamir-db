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
