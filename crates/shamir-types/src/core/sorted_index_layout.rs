//! Shared physical layout contract for sorted-index posting keys.

/// Tag byte distinguishing sorted-index postings.
pub const SORTED_TAG: u8 = 0x80;

/// Byte length of the sorted index's fixed prefix (tag + big-endian index id).
pub const SORTED_PREFIX_LEN: usize = 1 + 8;

/// Build the fixed physical prefix `SORTED_TAG || index_id.to_be_bytes()`.
#[inline]
pub fn sorted_prefix(index_id: u64) -> [u8; SORTED_PREFIX_LEN] {
    let mut prefix = [0; SORTED_PREFIX_LEN];
    prefix[0] = SORTED_TAG;
    prefix[1..].copy_from_slice(&index_id.to_be_bytes());
    prefix
}

/// Upper-bound record-id bytes include every posting at an inclusive value.
#[inline]
pub fn inclusive_hi_suffix() -> [u8; 16] {
    [0xFF; 16]
}

/// Build a sorted-index posting key: shared prefix, encoded value, then rid.
#[inline]
pub fn sorted_posting_key(index_id: u64, encoded_value: &[u8], rid: &[u8; 16]) -> Vec<u8> {
    let prefix = sorted_prefix(index_id);
    let mut key = Vec::with_capacity(SORTED_PREFIX_LEN + encoded_value.len() + rid.len());
    key.extend_from_slice(&prefix);
    key.extend_from_slice(encoded_value);
    key.extend_from_slice(rid);
    key
}
