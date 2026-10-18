//! Bookmark v3 (S5c-X PR6b-1/PR6b-3): fence/bookmark/clean-horizon J plus an
//! OPTIONAL outstanding receipt reference. The receipt protocol is the ONLY
//! follower apply protocol: the record at the bookmark key is either a valid
//! v3 envelope, absent, or REFUSED with a typed
//! [`DbError::FollowerStateRefused`] — there is no legacy-read fallback and
//! no migration anywhere.

use crate::meta::repl::fence_record::FenceRecord;
use crate::meta::{MetaEnvelope, MetaError, MetaKey, ENVELOPE_MAGIC, ENVELOPE_VERSION};
use bytes::Bytes;
use serde::{Deserialize, Serialize};
use shamir_storage::error::{DbError, DbResult, FollowerRefusalReason};
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

/// Durable follower-role classification of the bookmark record at open /
/// first follower admission.
#[derive(Debug)]
pub enum FollowerBookmarkState {
    /// A valid v3 record exists: the repo is a durably identified follower.
    Native(BookmarkV3),
    /// No bookmark record and no durable allocation floor: eligible for
    /// explicit fresh follower initialization (adoption writes v3 first).
    Fresh,
    /// No bookmark record with a durable `LastCommittedVersion` floor > 0:
    /// unowned history. A LEADER with this state must still open; an attempt
    /// to adopt/apply as a follower is refused (reseed required).
    FloorWithoutBookmark { floor: u64 },
}

fn key() -> RecordKey {
    RecordKey::from_slice(MetaKey::ReplicationBookmark.as_record_id().as_bytes())
}

fn codec_error(e: crate::meta::MetaError) -> DbError {
    DbError::Storage(format!("replication bookmark codec: {e}"))
}

/// Sniff the tuple version from the encoded envelope, if the header is
/// present. Bytes shorter than the header are unreadable (`None`).
fn sniff_version(bytes: &[u8]) -> Option<u16> {
    if bytes.len() >= 16 {
        Some(u16::from_le_bytes([bytes[14], bytes[15]]))
    } else {
        None
    }
}

/// Why these non-v3 bytes are refused. Legacy encodings (fixed-width u64
/// envelope of length 22, tuple versions 1 and 2) are a legacy-format
/// refusal; everything else is malformed/unknown.
fn refusal_reason(bytes: &[u8]) -> FollowerRefusalReason {
    let version = sniff_version(bytes);
    if bytes.len() == 22 || version == Some(1) || version == Some(2) {
        FollowerRefusalReason::LegacyBookmarkFormat
    } else {
        FollowerRefusalReason::MalformedBookmark
    }
}

/// Encode a bookmark v3 record under tuple version 3.
pub fn encode_bookmark_v3(record: &BookmarkV3) -> DbResult<Vec<u8>> {
    if record
        .fence
        .as_ref()
        .is_some_and(|fence| fence.bookmark != record.bookmark)
    {
        return Err(DbError::Storage(
            "bookmark v3: fence.bookmark disagrees with the flat bookmark".into(),
        ));
    }
    MetaEnvelope::new((3u16, record.clone()))
        .encode()
        .map_err(codec_error)
}

/// Decode a bookmark v3 record. Records whose sniffed tuple version is not
/// 3 fail closed; so do records whose fence bookmark disagrees with the
/// flat bookmark (mirroring the v2 consistency check). Trailing bytes
/// after the envelope are rejected (strict length check).
pub fn decode_bookmark_v3(bytes: &[u8]) -> DbResult<BookmarkV3> {
    if sniff_version(bytes) != Some(3) {
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

/// Strict v3-only read of the bookmark key: `None` when absent; the decoded
/// record when v3; a typed [`DbError::FollowerStateRefused`] for ANY other
/// bytes (legacy formats and malformed/unknown records alike — a malformed
/// record is never treated as absent). The single strict-read helper for the
/// crate: the projection loaders in `crate::meta::repl_bookmark` and the
/// writer below share this reason logic.
pub(crate) async fn read_strict_record(info: &Arc<dyn Store>) -> DbResult<Option<BookmarkV3>> {
    let bytes = match info.get(key()).await {
        Ok(bytes) => bytes,
        Err(DbError::NotFound(_)) => return Ok(None),
        Err(error) => return Err(error),
    };
    if sniff_version(&bytes) == Some(3) {
        // Bytes that SNIFF as v3 but fail the strict decode are truncated,
        // trailed, or internally inconsistent — a typed format/corruption
        // refusal (state-table row "malformed, truncated or unknown-version"),
        // never a generic internal error. `decode_bookmark_v3`'s own Storage
        // errors remain internal invariants.
        return decode_bookmark_v3(&bytes)
            .map(Some)
            .map_err(|_| DbError::FollowerStateRefused {
                reason: FollowerRefusalReason::MalformedBookmark,
            });
    }
    Err(DbError::FollowerStateRefused {
        reason: refusal_reason(&bytes),
    })
}

/// Strict v3-only loader. `Ok(None)` on an absent key; anything that is not
/// a valid v3 record is refused (see `read_strict_record`).
pub async fn load_bookmark_v3(info: &Arc<dyn Store>) -> DbResult<Option<BookmarkV3>> {
    read_strict_record(info).await
}

/// Classify the durable follower-role state (see
/// [`FollowerBookmarkState`]). Absent bookmark + a durable
/// `LastCommittedVersion` floor is `FloorWithoutBookmark`; absent bookmark
/// and no floor is `Fresh`. Malformed/legacy records fail closed.
pub async fn classify_follower_bookmark(info: &Arc<dyn Store>) -> DbResult<FollowerBookmarkState> {
    match read_strict_record(info).await? {
        Some(record) => Ok(FollowerBookmarkState::Native(record)),
        None => {
            let floor = crate::meta::recovery_marker::load_last_committed(info)
                .await?
                .unwrap_or(0);
            if floor > 0 {
                Ok(FollowerBookmarkState::FloorWithoutBookmark { floor })
            } else {
                Ok(FollowerBookmarkState::Fresh)
            }
        }
    }
}

/// True when `v3`'s outstanding-receipt pointer (if any) resolves to a stored
/// receipt that is not `Checkpointed`. A dangling or retired pointer is a
/// corrupt durable state: admission-time resume must refuse it.
pub async fn receipt_pointer_resolves(info: &Arc<dyn Store>, v3: &BookmarkV3) -> DbResult<bool> {
    let Some(id) = v3.outstanding_receipt else {
        return Ok(true);
    };
    match crate::meta::repl::receipt::load_outstanding_receipt(info, &id).await? {
        None => Ok(false),
        Some(receipt) => Ok(receipt.phase != crate::meta::repl::ReceiptPhase::Checkpointed),
    }
}

/// Persist a bookmark v3 record (set + flush). The flush makes the bookmark
/// advance itself a durability boundary; the receipt pipeline's atomic batches
/// stage the encoded record directly and flush separately.
pub async fn save_bookmark_v3(info: &Arc<dyn Store>, record: &BookmarkV3) -> DbResult<()> {
    let bytes = encode_bookmark_v3(record)?;
    info.set(key(), Bytes::from(bytes)).await?;
    info.flush().await
}
