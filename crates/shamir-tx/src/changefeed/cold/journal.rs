use bytes::Bytes;
use futures::StreamExt;
use shamir_storage::error::{DbError, DbResult};
use shamir_storage::key_bytes::KeyBytes;
use shamir_storage::types::Store;

use crate::changefeed::{clean_horizon_key, gap_marker_key, version_key, ChangelogEvent};

/// Read exactly the event at `version`; missing records return `Ok(None)`.
pub async fn read_event(store: &dyn Store, version: u64) -> DbResult<Option<ChangelogEvent>> {
    match store.get(KeyBytes::from(version_key(version))).await {
        Ok(bytes) => decode_event(bytes).and_then(|event| {
            if event.commit_version == version {
                Ok(Some(event))
            } else {
                Err(DbError::Storage(format!(
                    "journal key/version mismatch at {version}"
                )))
            }
        }),
        Err(DbError::NotFound(_)) => Ok(None),
        Err(error) => Err(error),
    }
}

/// Encode exactly as the background journal writer does (`rmp_serde::to_vec`).
pub fn encode_event(event: &ChangelogEvent) -> DbResult<Bytes> {
    rmp_serde::to_vec(event)
        .map(Bytes::from)
        .map_err(|error| DbError::Storage(format!("encode changelog event: {error}")))
}

fn decode_event(bytes: Bytes) -> DbResult<ChangelogEvent> {
    rmp_serde::from_slice(&bytes)
        .map_err(|error| DbError::Storage(format!("decode changelog event: {error}")))
}

fn key_version(key: &[u8]) -> DbResult<Option<u64>> {
    if key == gap_marker_key().as_ref() || key == clean_horizon_key().as_ref() {
        return Ok(None);
    }
    if key.len() != 8 {
        return Err(DbError::Storage(format!(
            "malformed journal key ({} bytes)",
            key.len()
        )));
    }
    let bytes: [u8; 8] = key
        .try_into()
        .map_err(|_| DbError::Storage("malformed journal key".into()))?;
    Ok(Some(u64::from_be_bytes(bytes)))
}

/// Insert when absent; a present record must decode and match canonical bytes exactly.
pub async fn put_event_if_missing(store: &dyn Store, event: &ChangelogEvent) -> DbResult<()> {
    let key = KeyBytes::from(version_key(event.commit_version));
    let encoded = encode_event(event)?;
    match store.get(key.clone()).await {
        Ok(existing) => {
            let decoded = decode_event(existing.clone())?;
            if decoded.commit_version != event.commit_version || existing != encoded {
                return Err(DbError::Storage(format!(
                    "journal event mismatch at version {}",
                    event.commit_version
                )));
            }
            Ok(())
        }
        Err(DbError::NotFound(_)) => store.set_no_flag(key, encoded).await,
        Err(error) => Err(error),
    }
}

/// Delete one event after validating its bytes and key/version, without flushing.
pub async fn delete_event(store: &dyn Store, version: u64) -> DbResult<()> {
    if read_event(store, version).await?.is_some() {
        store
            .remove_no_flag(KeyBytes::from(version_key(version)))
            .await?;
    }
    Ok(())
}

/// Enumerate valid events in `(after, through]` using a bounded storage range.
pub async fn events_in_range(
    store: &dyn Store,
    after: u64,
    through: u64,
    batch_size: usize,
) -> DbResult<Vec<ChangelogEvent>> {
    if through <= after {
        return Ok(Vec::new());
    }
    let mut stream = store.iter_range_stream(
        Some(version_key(after.saturating_add(1))),
        Some(version_key(through)),
        batch_size.max(1),
    );
    let mut events = Vec::new();
    while let Some(batch) = stream.next().await {
        for (key, bytes) in batch? {
            let Some(version) = key_version(key.as_slice())? else {
                continue;
            };
            let event = decode_event(bytes)?;
            if event.commit_version != version {
                return Err(DbError::Storage(format!(
                    "journal key/version mismatch at {version}"
                )));
            }
            events.push(event);
        }
    }
    Ok(events)
}

/// Collect keys in `(after, through]`, drop the stream, then remove them.
/// Flush remains a separate recovery phase boundary.
pub async fn delete_events_in_range(
    store: &dyn Store,
    after: u64,
    through: u64,
    batch_size: usize,
) -> DbResult<()> {
    if through <= after {
        return Ok(());
    }
    let mut stream = store.iter_range_stream(
        Some(version_key(after.saturating_add(1))),
        Some(version_key(through)),
        batch_size.max(1),
    );
    let mut keys = Vec::new();
    while let Some(batch) = stream.next().await {
        for (key, bytes) in batch? {
            let Some(version) = key_version(key.as_slice())? else {
                continue;
            };
            let event = decode_event(bytes)?;
            if event.commit_version != version {
                return Err(DbError::Storage(format!(
                    "journal key/version mismatch at {version}"
                )));
            }
            keys.push(key);
        }
    }
    drop(stream);
    for key in keys {
        store.remove_no_flag(key).await?;
    }
    Ok(())
}

/// Flush all preceding journal writes.
pub async fn flush_journal(store: &dyn Store) -> DbResult<()> {
    store.flush().await
}
