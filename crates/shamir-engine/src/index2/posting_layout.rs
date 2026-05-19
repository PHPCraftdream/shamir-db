//! Zero-copy posting-key encoding / decoding.
//!
//! Layout: `[index_id: u32 LE][type_tag: u8][value_bytes...][record_id: 16 bytes]`
//!
//! `index_id` is short (u32) and assigned by `IndexRegistry`. `type_tag`
//! lives in the key (not just metadata) so storage-level scans are
//! type-safe — a Btree scan can't accidentally read FTS postings.

use shamir_types::types::record_id::RecordId;

pub mod type_tag {
    pub const BTREE: u8 = 0;
    pub const FTS: u8 = 1;
    pub const FUNCTIONAL: u8 = 2;
    pub const VECTOR: u8 = 3;
}

/// `index_id` (4) + `type_tag` (1) + `record_id` (16). The variable
/// part is `value_bytes`.
const FIXED_OVERHEAD: usize = 4 + 1 + 16;

/// Borrowed view of a posting key. Decodes from `&[u8]` without
/// allocating — the caller keeps ownership of the underlying buffer.
#[derive(Debug)]
pub struct PostingKeyRef<'a> {
    pub index_id: u32,
    pub type_tag: u8,
    pub value_bytes: &'a [u8],
    pub record_id: &'a [u8; 16],
}

impl<'a> PostingKeyRef<'a> {
    pub fn decode(bytes: &'a [u8]) -> Option<Self> {
        if bytes.len() < FIXED_OVERHEAD {
            return None;
        }
        let index_id = u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]);
        let type_tag = bytes[4];
        let value_len = bytes.len() - FIXED_OVERHEAD;
        let value_bytes = &bytes[5..5 + value_len];
        let rid_slice = &bytes[5 + value_len..];
        // Safe: rid_slice.len() == 16 by arithmetic above.
        let record_id: &[u8; 16] = rid_slice.try_into().ok()?;
        Some(Self {
            index_id,
            type_tag,
            value_bytes,
            record_id,
        })
    }

    pub fn record_id_owned(&self) -> RecordId {
        RecordId(*self.record_id)
    }
}

/// Build a posting key with a single capacity-correct allocation.
pub fn build_posting_key(
    index_id: u32,
    type_tag: u8,
    value_bytes: &[u8],
    rid: &RecordId,
) -> Vec<u8> {
    let mut out = Vec::with_capacity(FIXED_OVERHEAD + value_bytes.len());
    out.extend_from_slice(&index_id.to_le_bytes());
    out.push(type_tag);
    out.extend_from_slice(value_bytes);
    out.extend_from_slice(rid.as_bytes());
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rid_with(n: u8) -> RecordId {
        let mut a = [0u8; 16];
        a[15] = n;
        RecordId(a)
    }

    #[test]
    fn round_trip_btree() {
        let val = [0xAAu8; 16];
        let rid = rid_with(7);
        let bytes = build_posting_key(123, type_tag::BTREE, &val, &rid);
        let r = PostingKeyRef::decode(&bytes).unwrap();
        assert_eq!(r.index_id, 123);
        assert_eq!(r.type_tag, type_tag::BTREE);
        assert_eq!(r.value_bytes, &val);
        assert_eq!(r.record_id, rid.as_bytes());
    }

    #[test]
    fn round_trip_empty_value() {
        // FTS minimal posting: token-hash-only (8 bytes) is the
        // smallest realistic case, but the layout must also work
        // with empty value_bytes (e.g., Vector backends).
        let rid = rid_with(1);
        let bytes = build_posting_key(0, type_tag::VECTOR, &[], &rid);
        let r = PostingKeyRef::decode(&bytes).unwrap();
        assert_eq!(r.index_id, 0);
        assert_eq!(r.type_tag, type_tag::VECTOR);
        assert!(r.value_bytes.is_empty());
        assert_eq!(r.record_id, rid.as_bytes());
    }

    #[test]
    fn round_trip_fts_token() {
        // FTS: 8-byte token hash.
        let token: [u8; 8] = 0x1234_5678_9abc_def0u64.to_le_bytes();
        let rid = rid_with(42);
        let bytes = build_posting_key(0xCAFE, type_tag::FTS, &token, &rid);
        let r = PostingKeyRef::decode(&bytes).unwrap();
        assert_eq!(r.index_id, 0xCAFE);
        assert_eq!(r.type_tag, type_tag::FTS);
        assert_eq!(r.value_bytes, &token);
        assert_eq!(r.record_id, rid.as_bytes());
    }

    #[test]
    fn decode_rejects_truncated() {
        let too_short = [0u8; FIXED_OVERHEAD - 1];
        assert!(PostingKeyRef::decode(&too_short).is_none());
    }

    #[test]
    fn record_id_owned_is_copy_of_borrowed() {
        let rid = rid_with(99);
        let bytes = build_posting_key(1, type_tag::BTREE, &[0; 16], &rid);
        let r = PostingKeyRef::decode(&bytes).unwrap();
        assert_eq!(r.record_id_owned(), rid);
    }
}
