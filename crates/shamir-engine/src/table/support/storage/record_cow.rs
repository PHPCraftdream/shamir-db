//! Batch-item carrier for the cutover read path.
//!
//! `RecordCow` replaces the old `InnerValue`-only batch item with a
//! `Borrowed(Bytes)` | `Owned(InnerValue)` enum. The hot scan path
//! yields `Borrowed` (the raw storage bytes — no tree decode); the
//! `needs_raw` aggregate/GROUP BY arm yields `Owned` (full tree).

use bytes::Bytes;
use shamir_types::types::value::InnerValue;

/// A batch-item payload that is either borrowed storage bytes (the hot
/// path — no decode) or an owned `InnerValue` tree (the aggregate /
/// GROUP BY path that needs the tree for sort/group).
pub enum RecordCow {
    /// Raw storage msgpack bytes (refcounted). Build a `RecordView`
    /// over `&b[..]` for zero-copy field access.
    Borrowed(Bytes),
    /// Fully decoded `InnerValue` tree — only used by the `needs_raw`
    /// (aggregate / GROUP BY) arm and by `collect_all_current_records`.
    Owned(InnerValue),
}

impl RecordCow {
    /// Decode to an owned `InnerValue`. For `Owned`, returns the tree
    /// directly; for `Borrowed`, decodes the msgpack bytes.
    ///
    /// Used by out-of-scope callers (doctor, write_exec, read_temporal,
    /// replication, sorted_index backfill, tests) that need the full
    /// tree and have not been migrated to the lens yet.
    pub fn into_inner(self) -> Result<InnerValue, shamir_storage::error::DbError> {
        match self {
            RecordCow::Owned(v) => Ok(v),
            RecordCow::Borrowed(b) => InnerValue::from_bytes(b).map_err(|e| {
                shamir_storage::error::DbError::Codec(format!(
                    "Failed to deserialize record: {}",
                    e
                ))
            }),
        }
    }
}
