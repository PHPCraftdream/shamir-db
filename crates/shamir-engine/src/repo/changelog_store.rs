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
use shamir_storage::types::Store;

/// Per-repo durable changelog journal backed by a `Store`.
pub struct StoreChangelog {
    store: Arc<dyn Store>,
}

impl StoreChangelog {
    pub fn new(store: Arc<dyn Store>) -> Self {
        Self { store }
    }
}

#[async_trait::async_trait]
impl shamir_tx::ChangelogStore for StoreChangelog {
    async fn put(&self, key: Bytes, value: Bytes) -> Result<(), String> {
        self.store
            .set(key.into(), value)
            .await
            .map(|_created| ())
            .map_err(|e| format!("changelog store set: {e}"))
    }

    async fn range_from(&self, from_key: Bytes, limit: usize) -> Result<Vec<Bytes>, String> {
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

        let mut values: Vec<Bytes> = Vec::with_capacity(limit);
        while values.len() < limit {
            let Some(chunk) = stream.next().await else {
                break;
            };
            let chunk = chunk.map_err(|e| format!("changelog store range: {e}"))?;
            for (_, v) in chunk {
                values.push(v);
                if values.len() >= limit {
                    break;
                }
            }
        }
        Ok(values)
    }
}
