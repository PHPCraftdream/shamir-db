//! `ChangelogStore` adapter over a `shamir_storage::Store` (Phase 3b).
//!
//! Bridges the storage-agnostic [`shamir_tx::ChangelogStore`] trait the
//! changefeed writer/reader speak to a concrete per-repo durable `Store`
//! (the `"__changelog__"` namespace). Keys are big-endian `commit_version`
//! bytes so the store's natural key order is numeric order; values are the
//! msgpack-serialized `ChangelogEvent`.

use std::sync::Arc;

use bytes::Bytes;
use futures::StreamExt;
use shamir_storage::error::{DbError, DbResult};
use shamir_storage::types::Store;
use shamir_tx::ChangelogError;

#[cfg(test)]
mod tests;
pub mod write_domain;

/// Per-repo durable changelog journal backed by a `Store`.
pub struct StoreChangelog {
    store: Arc<dyn Store>,
}

impl StoreChangelog {
    pub fn new(store: Arc<dyn Store>) -> Self {
        Self { store }
    }

    pub(crate) async fn load_clean_horizon(&self) -> DbResult<u64> {
        match self
            .store
            .get(shamir_tx::changefeed::clean_horizon_key().into())
            .await
        {
            Ok(bytes) => {
                shamir_tx::changefeed::decode_clean_horizon(&bytes).map_err(DbError::Storage)
            }
            Err(DbError::NotFound(_)) => Ok(0),
            Err(e) => Err(e),
        }
    }

    /// Load extended invalidation metadata, accepting legacy single-u64 points.
    /// Corruption is invalidation of an unknown horizon, never a small point.
    /// The bool asks init to flush the conservative replacement before seeding.
    pub(crate) async fn load_gap_marker(&self) -> DbResult<(Option<(u64, u64)>, bool)> {
        match self
            .store
            .get(shamir_tx::changefeed::gap_marker_key().into())
            .await
        {
            Ok(bytes) => match shamir_tx::changefeed::decode_gap_marker(&bytes) {
                Ok(interval) => Ok((Some(interval), false)),
                Err(e) => {
                    log::warn!("invalid changefeed gap marker: {e}; invalidating all versions");
                    Ok((Some((1, u64::MAX)), true))
                }
            },
            Err(DbError::NotFound(_)) => Ok((None, false)),
            Err(e) => Err(e),
        }
    }
}

#[async_trait::async_trait]
impl shamir_tx::ChangelogStore for StoreChangelog {
    async fn put(&self, key: Bytes, value: Bytes) -> Result<(), ChangelogError> {
        let is_clean = key == shamir_tx::changefeed::clean_horizon_key();
        let is_marker = key == shamir_tx::changefeed::gap_marker_key() || is_clean;
        if is_clean {
            // Establish event/gap durability before staging any certificate.
            self.store
                .flush()
                .await
                .map_err(|e| ChangelogError::Journal(format!("changelog checkpoint flush: {e}")))?;
        }
        self.store
            .set(key.into(), value)
            .await
            .map_err(|e| ChangelogError::Journal(format!("changelog store set: {e}")))?;
        if is_marker {
            // Store::set is only eventually durable. Marker success means
            // the backend's explicit durability boundary has completed.
            self.store
                .flush()
                .await
                .map_err(|e| ChangelogError::Journal(format!("changelog marker flush: {e}")))?;
        }
        Ok(())
    }

    async fn put_many(&self, items: Vec<(Bytes, Bytes)>) -> Vec<Result<(), ChangelogError>> {
        // Metadata must retain put's flush boundaries even for invalid callers.
        if items.iter().any(|(key, _)| {
            *key == shamir_tx::changefeed::gap_marker_key()
                || *key == shamir_tx::changefeed::clean_horizon_key()
        }) {
            let mut results = Vec::with_capacity(items.len());
            for (key, value) in items {
                results.push(self.put(key, value).await);
            }
            return results;
        }
        let count = items.len();
        let items = items
            .into_iter()
            .map(|(key, value)| (key.into(), value))
            .collect();
        match self.store.set_many(items).await {
            Ok(flags) if flags.len() == count => (0..count).map(|_| Ok(())).collect(),
            Ok(_) => (0..count)
                .map(|_| {
                    Err(ChangelogError::Journal(
                        "changelog batch returned wrong result count".into(),
                    ))
                })
                .collect(),
            // set_many may have applied an unknown prefix: conservatively gap all.
            Err(error) => (0..count)
                .map(|_| {
                    Err(ChangelogError::Journal(format!(
                        "changelog batch set: {error}"
                    )))
                })
                .collect(),
        }
    }

    async fn range_from(
        &self,
        from_key: Bytes,
        limit: usize,
    ) -> Result<Vec<Bytes>, ChangelogError> {
        // `Store::iter_stream` is a workspace-wide MUST-ascending contract
        // (`shamir-storage/src/types.rs` doc on `iter_stream`), and every
        // `iter_range_stream` this crate can be handed upholds it too: the
        // TreeIndex-backed seek in storage_in_memory.rs / storage_cached.rs,
        // fjall's native B-tree range in storage_fjall.rs, and the sorted
        // overlay merge in storage_membuffer.rs. `ChangelogStore::range_from`
        // itself documents "ascending" as part of its contract
        // (`shamir-tx/src/changefeed.rs`). So the first `limit` values off
        // the stream already ARE the answer — stop as soon as we have them
        // instead of draining and sorting the whole journal tail.
        if limit == 0 {
            return Ok(Vec::new());
        }
        let batch = limit.clamp(1, 1024);
        let mut stream = self.store.iter_range_stream(Some(from_key), None, batch);

        let mut values: Vec<Bytes> = Vec::with_capacity(limit.min(1024));
        let max_bytes = 16 * 1024 * 1024;
        let mut remaining_bytes = max_bytes;
        while values.len() < limit {
            let Some(chunk) = stream.next().await else {
                break;
            };
            let chunk = chunk
                .map_err(|e| ChangelogError::Journal(format!("changelog store range: {e}")))?;
            for (key, v) in chunk {
                if key.as_ref() == shamir_tx::changefeed::gap_marker_key().as_ref()
                    || key.as_ref() == shamir_tx::changefeed::clean_horizon_key().as_ref()
                {
                    continue;
                }
                if v.len() > remaining_bytes {
                    if values.is_empty() {
                        return Err(ChangelogError::Oversized { budget: max_bytes });
                    }
                    return Ok(values);
                }
                remaining_bytes -= v.len();
                values.push(v);
                if values.len() >= limit {
                    break;
                }
            }
        }
        Ok(values)
    }
}
