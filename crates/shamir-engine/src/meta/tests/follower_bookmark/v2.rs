use crate::meta::{MetaEnvelope, MetaKey};
use bytes::Bytes;
use serde::{Deserialize, Serialize};
use shamir_storage::error::{DbError, DbResult};
use shamir_storage::types::{RecordKey, Store};
use std::sync::Arc;

#[path = "fence_record.rs"]
mod fence_record;
pub use fence_record::FenceRecord;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
struct Version2Record {
    fence: Option<FenceRecord>,
    bookmark: u64,
    clean_through: Option<u64>,
}

fn key() -> RecordKey {
    RecordKey::from_slice(MetaKey::ReplicationBookmark.as_record_id().as_bytes())
}

async fn load(info: &Arc<dyn Store>) -> DbResult<Version2Record> {
    let bytes = match info.get(key()).await {
        Ok(bytes) => bytes,
        Err(DbError::NotFound(_)) => {
            return Ok(Version2Record {
                fence: None,
                bookmark: 0,
                clean_through: None,
            })
        }
        Err(error) => return Err(error),
    };
    if bytes.len() == 22 {
        let bookmark: u64 = MetaEnvelope::open(&bytes)
            .map_err(|e| DbError::Storage(format!("replication bookmark codec: {e}")))?;
        return Ok(Version2Record {
            fence: None,
            bookmark,
            clean_through: None,
        });
    }
    if bytes.len() == 49 || bytes.len() == 73 {
        let (version, fence): (u16, FenceRecord) = MetaEnvelope::open(&bytes)
            .map_err(|e| DbError::Storage(format!("replication bookmark codec: {e}")))?;
        if version != 1
            || bytes.len()
                != if fence.observed_identity.is_some() {
                    73
                } else {
                    49
                }
        {
            return Err(DbError::Storage(
                "invalid replication bookmark record".into(),
            ));
        }
        return Ok(Version2Record {
            fence: Some(fence),
            bookmark: fence.bookmark,
            clean_through: None,
        });
    }
    let (version, record): (u16, Version2Record) = MetaEnvelope::open(&bytes)
        .map_err(|e| DbError::Storage(format!("replication bookmark codec: {e}")))?;
    if version != 2 {
        return Err(DbError::Storage(
            "invalid replication bookmark record".into(),
        ));
    }
    if record.fence.is_some_and(|f| f.bookmark != record.bookmark) {
        return Err(DbError::Storage(
            "invalid replication bookmark record".into(),
        ));
    }
    Ok(record)
}

pub async fn load_replication_bookmark(info: &Arc<dyn Store>) -> DbResult<u64> {
    Ok(load(info).await?.bookmark)
}
pub async fn load_fence_record(info: &Arc<dyn Store>) -> DbResult<Option<FenceRecord>> {
    Ok(load(info).await?.fence)
}
pub async fn load_clean_horizon(info: &Arc<dyn Store>) -> DbResult<Option<u64>> {
    Ok(load(info).await?.clean_through)
}

async fn save_record(info: &Arc<dyn Store>, record: Version2Record) -> DbResult<()> {
    let bytes = MetaEnvelope::new((2u16, record))
        .encode()
        .map_err(|e| DbError::Storage(format!("replication bookmark codec: {e}")))?;
    info.set(key(), Bytes::from(bytes)).await?;
    info.flush().await
}

pub async fn save_fence_record(info: &Arc<dyn Store>, record: FenceRecord) -> DbResult<()> {
    let old = load(info).await?;
    save_record(
        info,
        Version2Record {
            fence: Some(record),
            bookmark: record.bookmark,
            clean_through: old.clean_through,
        },
    )
    .await
}

pub async fn save_replication_bookmark(info: &Arc<dyn Store>, bookmark: u64) -> DbResult<()> {
    let mut record = load(info).await?;
    record.bookmark = bookmark;
    if let Some(mut fence) = record.fence {
        fence.bookmark = bookmark;
        record.fence = Some(fence);
    }
    save_record(info, record).await
}

pub async fn save_bookmark_and_clean_horizon(
    info: &Arc<dyn Store>,
    bookmark: u64,
    clean_horizon: u64,
) -> DbResult<()> {
    let mut record = load(info).await?;
    record.bookmark = bookmark;
    record.clean_through = Some(
        record
            .clean_through
            .map_or(clean_horizon, |old| old.max(clean_horizon)),
    );
    if let Some(mut fence) = record.fence {
        fence.bookmark = bookmark;
        record.fence = Some(fence);
    }
    save_record(info, record).await
}
