//! Durable follower bookmark and pinned upstream identity in ONE store value.
//! Legacy u64 bookmarks adopt an identity on the next successful contact.

use std::sync::Arc;

use bytes::Bytes;
use serde::{Deserialize, Serialize};
use shamir_storage::error::{DbError, DbResult};
use shamir_storage::types::{RecordKey, Store};

use crate::meta::recovery_marker::save_u64;
use crate::meta::repo_identity::RepoIdentity;
use crate::meta::{MetaEnvelope, MetaKey};

/// The advancing bookmark and its upstream incarnation are inseparable.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct FenceRecord {
    pub bookmark: u64,
    pub lineage_id: [u8; 16],
    pub history_epoch: u64,
    pub observed_identity: Option<RepoIdentity>,
}

fn key() -> RecordKey {
    RecordKey::from_slice(MetaKey::ReplicationBookmark.as_record_id().as_bytes())
}

async fn load(info: &Arc<dyn Store>) -> DbResult<(u64, Option<FenceRecord>)> {
    let bytes = match info.get(key()).await {
        Ok(bytes) => bytes,
        Err(DbError::NotFound(_)) => return Ok((0, None)),
        Err(error) => return Err(error),
    };
    // The legacy fixed-width envelope is 14 header bytes + one u64.
    if bytes.len() == 22 {
        let bookmark = MetaEnvelope::open(&bytes)
            .map_err(|e| DbError::Storage(format!("replication bookmark codec: {e}")))?;
        return Ok((bookmark, None));
    }
    let (version, record): (u16, FenceRecord) = MetaEnvelope::open(&bytes)
        .map_err(|e| DbError::Storage(format!("replication bookmark codec: {e}")))?;
    let expected_len = if record.observed_identity.is_some() {
        73
    } else {
        49
    };
    if version != 1 || bytes.len() != expected_len {
        return Err(DbError::Storage(
            "invalid replication bookmark record".into(),
        ));
    }
    Ok((record.bookmark, Some(record)))
}

/// Load the cursor, including legacy bookmarks; absent means zero.
pub async fn load_replication_bookmark(info: &Arc<dyn Store>) -> DbResult<u64> {
    Ok(load(info).await?.0)
}

/// Load the identity bound to the current bookmark, if adopted.
pub async fn load_fence_record(info: &Arc<dyn Store>) -> DbResult<Option<FenceRecord>> {
    Ok(load(info).await?.1)
}

/// One atomic set and flush contains BOTH identity and current cursor.
/// Callers serialize subscription ownership and flush covered history first.
pub async fn save_fence_record(info: &Arc<dyn Store>, record: FenceRecord) -> DbResult<()> {
    let bytes = MetaEnvelope::new((1u16, record))
        .encode()
        .map_err(|e| DbError::Storage(format!("replication bookmark codec: {e}")))?;
    info.set(key(), Bytes::from(bytes)).await?;
    info.flush().await
}

/// Persist and flush the advancing cursor, preserving its pinned identity.
/// Covered history must be flushed first; callers enforce monotonicity.
pub async fn save_replication_bookmark(info: &Arc<dyn Store>, version: u64) -> DbResult<()> {
    if let Some(mut record) = load_fence_record(info).await? {
        record.bookmark = version;
        save_fence_record(info, record).await
    } else {
        save_u64(info, MetaKey::ReplicationBookmark, version).await?;
        info.flush().await
    }
}
