//! Bookmark v3 (S5c-X PR6b-1): fence/bookmark/clean-horizon J plus an
//! OPTIONAL outstanding receipt reference. Decode-capable now; no
//! production writer — PR6b-2 activates writing.

use crate::meta::repl::bookmark::{
    load_clean_horizon, load_fence_record, load_replication_bookmark, FenceRecord,
};
use crate::meta::{MetaEnvelope, MetaError, MetaKey, ENVELOPE_MAGIC, ENVELOPE_VERSION};
use serde::{Deserialize, Serialize};
use shamir_storage::error::{DbError, DbResult};
use shamir_storage::types::{RecordKey, Store};
use std::sync::Arc;

/// Bookmark v3 record: everything v2 carried plus an optional reference to
/// the outstanding replication redo receipt (see
/// [`crate::meta::repl::receipt`]).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BookmarkV3 {
    pub fence: Option<FenceRecord>,
    pub bookmark: u64,
    pub clean_through: Option<u64>,
    pub outstanding_receipt: Option<[u8; 16]>,
}

fn key() -> RecordKey {
    RecordKey::from_slice(MetaKey::ReplicationBookmark.as_record_id().as_bytes())
}

fn codec_error(e: crate::meta::MetaError) -> DbError {
    DbError::Storage(format!("replication bookmark codec: {e}"))
}

/// Encode a bookmark v3 record under tuple version 3. No production path
/// calls this in PR6b-1 (tests only).
pub fn encode_bookmark_v3(record: &BookmarkV3) -> DbResult<Vec<u8>> {
    MetaEnvelope::new((3u16, record.clone()))
        .encode()
        .map_err(codec_error)
}

/// Decode a bookmark v3 record. Records whose sniffed tuple version is not
/// 3 fail closed; so do records whose fence bookmark disagrees with the
/// flat bookmark (mirroring the v2 consistency check). Trailing bytes
/// after the envelope are rejected (strict length check).
pub fn decode_bookmark_v3(bytes: &[u8]) -> DbResult<BookmarkV3> {
    if bytes.len() >= 16 && u16::from_le_bytes([bytes[14], bytes[15]]) != 3 {
        return Err(DbError::Storage("not a v3 bookmark record".into()));
    }
    let envelope: MetaEnvelope<(u16, BookmarkV3)> =
        bincode::deserialize(bytes).map_err(|e| codec_error(MetaError::Decode(e.to_string())))?;
    if envelope.magic != ENVELOPE_MAGIC {
        return Err(codec_error(MetaError::BadMagic {
            expected: ENVELOPE_MAGIC,
            got: envelope.magic,
        }));
    }
    if envelope.version != ENVELOPE_VERSION {
        return Err(codec_error(MetaError::UnsupportedVersion(envelope.version)));
    }
    // bincode::deserialize allows trailing bytes (bincode 1.3 default
    // options); re-encoding the decoded envelope is deterministic, so it
    // reproduces the input length IFF nothing trailed.
    if envelope.encode().map_err(codec_error)?.len() != bytes.len() {
        return Err(DbError::Storage(
            "replication bookmark codec: trailing bytes".into(),
        ));
    }
    let (version, record) = envelope.payload;
    if version != 3
        || record
            .fence
            .as_ref()
            .is_some_and(|f| f.bookmark != record.bookmark)
    {
        return Err(DbError::Storage("not a v3 bookmark record".into()));
    }
    Ok(record)
}

/// Read-capable loader. A v3 record at the bookmark key decodes directly;
/// anything else falls back to the existing v2/v1/u64 loaders and wraps the
/// result with `outstanding_receipt: None`. This costs two extra reads on
/// the legacy path (tests / single-reader use only; PR6b-2 replaces the
/// read path). Unknown versions on the fallback path already fail closed
/// inside the legacy loaders.
pub async fn load_bookmark_v3(info: &Arc<dyn Store>) -> DbResult<Option<BookmarkV3>> {
    let bytes = match info.get(key()).await {
        Ok(bytes) => bytes,
        Err(DbError::NotFound(_)) => return Ok(None),
        Err(error) => return Err(error),
    };
    if bytes.len() >= 16 && u16::from_le_bytes([bytes[14], bytes[15]]) == 3 {
        return decode_bookmark_v3(&bytes).map(Some);
    }
    let fence = load_fence_record(info).await?;
    let bookmark = load_replication_bookmark(info).await?;
    let clean_through = load_clean_horizon(info).await?;
    Ok(Some(BookmarkV3 {
        fence,
        bookmark,
        clean_through,
        outstanding_receipt: None,
    }))
}
