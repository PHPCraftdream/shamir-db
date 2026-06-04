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
            .set(key, value)
            .await
            .map(|_created| ())
            .map_err(|e| format!("changelog store set: {e}"))
    }

    async fn range_from(&self, from_key: Bytes, limit: usize) -> Result<Vec<Bytes>, String> {
        // Read all keys >= from_key ascending. Disk B-tree backends seek;
        // the in-memory backend filters a full scan. We sort defensively so
        // the order is numeric regardless of the backend's iteration order,
        // then truncate to `limit`.
        let batch = limit.clamp(1, 1024);
        let mut stream = self.store.iter_range_stream(Some(from_key), None, batch);

        let mut pairs: Vec<(Bytes, Bytes)> = Vec::new();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|e| format!("changelog store range: {e}"))?;
            pairs.extend(chunk);
        }
        pairs.sort_by(|a, b| a.0.as_ref().cmp(b.0.as_ref()));
        pairs.truncate(limit);
        Ok(pairs.into_iter().map(|(_, v)| v).collect())
    }
}
