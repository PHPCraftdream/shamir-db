//! Replication bookmark metadata (S5c-X PR6b-3, strict v3-only contract).
//!
//! The receipt protocol is the ONLY follower apply protocol. The public
//! loaders here are strict projections of a valid v3 record: an absent key
//! projects to defaults (`0` / `None` / `None`), and ANY other stored bytes
//! (legacy fixed-width u64, v1, v2, or malformed records) are refused with a
//! typed [`DbError::FollowerStateRefused`]. There is no legacy-read fallback,
//! no legacy writer, and no migration anywhere.
//!
//! The fence write [`save_fence_record_v3_aware`] is v3-only identity
//! adoption: it updates a v3 record in place (preserving bookmark, clean
//! horizon and outstanding-receipt pointer), refuses over a durable floor
//! without a v3 record, and writes a fresh v3 baseline only on a truly fresh
//! store.

use crate::meta::recovery_marker::load_last_committed;
use crate::meta::repl::bookmark_v3::{read_strict_record, save_bookmark_v3, BookmarkV3};
use crate::meta::repl::fence_record::FenceRecord;
use shamir_storage::error::{DbError, DbResult, FollowerRefusalReason};
use shamir_storage::types::Store;
use std::sync::Arc;

pub use crate::meta::repl::fence_record::FenceRecord;

/// Upstream bookmark projected from the strict v3 record; `0` when absent.
pub async fn load_replication_bookmark(info: &Arc<dyn Store>) -> DbResult<u64> {
    Ok(read_strict_record(info)
        .await?
        .map_or(0, |record| record.bookmark))
}

/// Upstream fence projected from the strict v3 record; `None` when absent.
pub async fn load_fence_record(info: &Arc<dyn Store>) -> DbResult<Option<FenceRecord>> {
    Ok(read_strict_record(info)
        .await?
        .and_then(|record| record.fence))
}

/// Follower-local clean horizon projected from the strict v3 record; `None`
/// when absent.
pub async fn load_clean_horizon(info: &Arc<dyn Store>) -> DbResult<Option<u64>> {
    Ok(read_strict_record(info)
        .await?
        .and_then(|record| record.clean_through))
}

/// v3-only identity adoption: persist the fence through the v3 record.
///
/// - A present v3 record is updated IN PLACE: its bookmark, clean horizon
///   and outstanding-receipt pointer are preserved, and the v3 decode
///   invariant `fence.bookmark == bookmark` is kept by syncing the fence's
///   own bookmark to the record's bookmark when they disagree.
/// - An absent record with a durable `LastCommittedVersion` floor > 0 is
///   pre-receipt follower state: refused (`LegacyFollowerState`); adoption
///   must not manufacture a v3 record over it.
/// - An absent record with no floor writes the fresh v3 baseline
///   (bookmark 0, no clean horizon, no receipt; the fence's cursor reflects
///   that authoritative starting point).
/// - A present legacy/malformed record is refused (never rewritten).
pub async fn save_fence_record_v3_aware(
    info: &Arc<dyn Store>,
    record: FenceRecord,
) -> DbResult<()> {
    match read_strict_record(info).await? {
        Some(mut v3) => {
            let fence = if record.bookmark == v3.bookmark {
                record
            } else {
                FenceRecord {
                    bookmark: v3.bookmark,
                    ..record
                }
            };
            v3.fence = Some(fence);
            save_bookmark_v3(info, &v3).await
        }
        None => {
            if load_last_committed(info).await?.unwrap_or(0) > 0 {
                return Err(DbError::FollowerStateRefused {
                    reason: FollowerRefusalReason::LegacyFollowerState,
                });
            }
            let v3 = BookmarkV3 {
                fence: Some(FenceRecord {
                    bookmark: 0,
                    ..record
                }),
                bookmark: 0,
                clean_through: None,
                outstanding_receipt: None,
            };
            save_bookmark_v3(info, &v3).await
        }
    }
}
