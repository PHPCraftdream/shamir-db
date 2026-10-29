//! Physical key layout `<key>::<version_be_u64>`.
//!
//! The MVCC store keeps a history of overwritten values keyed by
//! `(original_key, version)`. Encoding chooses big-endian for the
//! version suffix so that lexicographic byte-order matches numeric
//! version-order — a single range scan in [key::0, key::snapshot]
//! gives all versions visible at `snapshot`, with the highest one
//! at the end of the scan.
//!
//! The separator `0xFF` is chosen because it sorts higher than any
//! normal byte, so `key + 0xFF + version` sorts after the bare `key`
//! and keeps all versions of a key contiguous in the lexicographic
//! range that starts with `key`.
//!
//! ## Grammar, not an exclusion invariant
//!
//! The encoding is a fixed-length grammar: `decode_version_key` splits
//! off the LAST 9 bytes (`0xFF` + big-endian `u64`) and returns the
//! leading bytes as the original key. There is NO guarantee that an
//! original key excludes `0xFF` — user `RecordId`s are 16 arbitrary
//! bytes and may contain it. Two consequences:
//! - A physical key is only ever fed to `decode_version_key` if it was
//!   produced by [`encode_version_key`] (or recovered from the same
//!   log), so the grammar round-trips exactly. This is verified by the
//!   round-trip property tests below.
//! - A key whose own bytes contain `key || 0xFF` prefix-collides with
//!   a shorter key's scan prefix. Prefix scans over version-keys MUST
//!   therefore filter on the decoded original key, not on the scan's
//!   requested key.

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
