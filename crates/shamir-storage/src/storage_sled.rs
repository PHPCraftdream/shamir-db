use super::types::{RecordKey, Repo, Store};
use crate::error::{DbError, DbResult};
use async_stream::stream;
use async_trait::async_trait;
use bytes::Bytes;
use futures::stream::Stream;
use shamir_types::types::record_id::RecordId;
use sled::{Db, Tree};
use std::path::Path;
use std::pin::Pin;
use std::sync::Arc;
use tokio::task::spawn_blocking;

// ============================================================================
// SledRepo - manages multiple stores (trees)
// ============================================================================

#[derive(Clone)]
pub struct SledRepo {
    db: Arc<Db>,
}

impl SledRepo {
    pub fn new(path: impl AsRef<Path>) -> DbResult<Self> {
        let db = sled::open(path).map_err(|e| DbError::Storage(format!("SledDB open: {}", e)))?;
        Ok(Self { db: Arc::new(db) })
    }
}

#[async_trait]
impl Repo for SledRepo {
    async fn store_get<S: AsRef<str> + Send>(&self, name: S) -> DbResult<Arc<dyn Store>> {
        let db = self.db.clone();
        let table_name = name.as_ref().to_string();

        let tree = spawn_blocking(move || -> DbResult<Tree> {
            db.open_tree(table_name.as_bytes())
                .map_err(|e| DbError::Storage(format!("SledDB open_tree: {}", e)))
        })
        .await
        .map_err(|e| DbError::Storage(format!("Tokio join error: {}", e)))??;

        let store = SledStore {
            tree: Arc::new(tree),
        };
        Ok(Arc::new(store))
    }

    async fn store_delete<S: AsRef<str> + Send>(&self, name: S) -> DbResult<bool> {
        let db = self.db.clone();
        let table_name = name.as_ref().to_string();

        spawn_blocking(move || -> DbResult<bool> {
            db.drop_tree(table_name.as_bytes())
                .map_err(|e| DbError::Storage(format!("SledDB drop_tree: {}", e)))
        })
        .await
        .map_err(|e| DbError::Storage(format!("Tokio join error: {}", e)))?
    }

    async fn stores_list(&self) -> DbResult<Vec<String>> {
        let db = self.db.clone();
        spawn_blocking(move || -> DbResult<Vec<String>> {
            let names = db
                .tree_names()
                .into_iter()
                .filter(|name| *name != b"__sled__default")
                .map(|bytes| String::from_utf8(bytes.to_vec()))
                .collect::<Result<Vec<String>, _>>()
                .map_err(|e| DbError::Codec(format!("UTF-8 decode error: {}", e)))?;
            Ok(names)
        })
        .await
        .map_err(|e| DbError::Storage(format!("Tokio join error: {}", e)))?
    }
}

// ============================================================================
// SledStore - individual store (tree)
// ============================================================================

pub struct SledStore {
    tree: Arc<Tree>,
}

// SAFETY: `sled::Tree` is documented Send+Sync (sled exposes it for
// concurrent multi-thread access). `Arc<Tree>` preserves both. These
// impls are technically redundant given auto-impl on `Arc<T: Send+Sync>`
// — kept explicit to make the contract visible per §B5.
unsafe impl Send for SledStore {}
unsafe impl Sync for SledStore {}

#[async_trait]
impl Store for SledStore {
    async fn insert(&self, value: Bytes) -> DbResult<RecordKey> {
        let tree = self.tree.clone();

        spawn_blocking(move || -> DbResult<RecordKey> {
            let id = RecordId::new();
            let key = RecordKey::copy_from_slice(id.as_bytes());

            tree.insert(&key[..], &*value)
                .map_err(|e| DbError::Storage(format!("SledDB insert: {}", e)))?;

            Ok(key)
        })
        .await
        .map_err(|e| DbError::Storage(format!("Tokio join error: {}", e)))?
    }

    async fn set(&self, key: RecordKey, value: Bytes) -> DbResult<bool> {
        let tree = self.tree.clone();

        spawn_blocking(move || -> DbResult<bool> {
            let existed = tree
                .get(&key[..])
                .map_err(|e| DbError::Storage(format!("SledDB get: {}", e)))?
                .is_some();

            tree.insert(&key[..], &*value)
                .map_err(|e| DbError::Storage(format!("SledDB insert: {}", e)))?;

            Ok(!existed)
        })
        .await
        .map_err(|e| DbError::Storage(format!("Tokio join error: {}", e)))?
    }

    async fn get(&self, key: RecordKey) -> DbResult<Bytes> {
        let tree = self.tree.clone();

        spawn_blocking(move || -> DbResult<Bytes> {
            let val = tree
                .get(&key[..])
                .map_err(|e| DbError::Storage(format!("SledDB get: {}", e)))?
                .ok_or_else(|| DbError::NotFound(format!("{:?}", key)))?;

            Ok(Bytes::copy_from_slice(&val))
        })
        .await
        .map_err(|e| DbError::Storage(format!("Tokio join error: {}", e)))?
    }

    async fn remove(&self, key: RecordKey) -> DbResult<bool> {
        let tree = self.tree.clone();

        spawn_blocking(move || -> DbResult<bool> {
            let existed = tree
                .remove(&key[..])
                .map_err(|e| DbError::Storage(format!("SledDB remove: {}", e)))?
                .is_some();

            Ok(existed)
        })
        .await
        .map_err(|e| DbError::Storage(format!("Tokio join error: {}", e)))?
    }

    fn iter_stream(
        &self,
        batch_size: usize,
    ) -> Pin<Box<dyn Stream<Item = Result<Vec<(RecordKey, Bytes)>, DbError>> + Send>> {
        let tree = self.tree.clone();

        Box::pin(stream! {
            let mut last_key: Option<Vec<u8>> = None;

            loop {
                // Fetch next batch in spawn_blocking
                let tree_clone = tree.clone();
                let start_key = last_key.clone();

                let batch: DbResult<Vec<(Bytes, Bytes)>> = spawn_blocking(move || {
                    use std::ops::Bound;
                    let iter = match start_key {
                        Some(ref start) => tree_clone.range::<&[u8], _>((
                            Bound::Excluded(start.as_slice()),
                            Bound::Unbounded,
                        )),
                        None => tree_clone.iter(),
                    };

                    let mut items = Vec::new();

                    for item in iter {
                        if items.len() >= batch_size {
                            break;
                        }

                        let (key, val) = item.map_err(|e| DbError::Storage(format!("SledDB iter error: {}", e)))?;
                        items.push((Bytes::copy_from_slice(&key), Bytes::copy_from_slice(&val)));
                    }
                    Ok(items)
                })
                .await
                .map_err(|e| DbError::Storage(format!("Tokio join error: {}", e)))?;

                let batch = batch?;

                if batch.is_empty() {
                    break; // No more records
                }

                // Remember last key for next iteration
                last_key = batch.last().map(|(k, _)| k.to_vec());

                yield Ok(batch);
            }
        })
    }

    fn scan_prefix_stream(
        &self,
        prefix: Bytes,
        batch_size: usize,
    ) -> Pin<Box<dyn Stream<Item = Result<Vec<(RecordKey, Bytes)>, DbError>> + Send>> {
        let tree = self.tree.clone();

        Box::pin(stream! {
            let mut last_key: Option<Vec<u8>> = None;
            let prefix_slice = prefix.to_vec();

            loop {
                let tree_clone = tree.clone();
                let start_key = last_key.clone();
                let prefix_clone = prefix_slice.clone();

                let batch: DbResult<Vec<(Bytes, Bytes)>> = spawn_blocking(move || {
                    let prefix_ref = &prefix_clone;

                    // Sled's scan_prefix doesn't support cursor, so we need to skip
                    let mut items = Vec::new();
                    let mut skip_until = start_key;

                    for item in tree_clone.scan_prefix(prefix_ref) {
                        let (key, val) = item.map_err(|e| DbError::Storage(format!("SledDB scan_prefix item: {}", e)))?;

                        // Skip until we pass the cursor
                        if let Some(ref start) = skip_until {
                            if key.as_ref() <= start.as_slice() {
                                continue;
                            }
                            skip_until = None; // Done skipping
                        }

                        if items.len() >= batch_size {
                            break;
                        }

                        items.push((Bytes::copy_from_slice(&key), Bytes::copy_from_slice(&val)));
                    }

                    Ok(items)
                })
                .await
                .map_err(|e| DbError::Storage(format!("Tokio join error: {}", e)))?;

                let batch = batch?;

                if batch.is_empty() {
                    break;
                }

                last_key = batch.last().map(|(k, _)| k.to_vec());

                yield Ok(batch);
            }
        })
    }

    fn iter_range_stream(
        &self,
        start_inclusive: Option<Bytes>,
        end_inclusive: Option<Bytes>,
        batch_size: usize,
    ) -> Pin<Box<dyn Stream<Item = Result<Vec<(RecordKey, Bytes)>, DbError>> + Send>> {
        let tree = self.tree.clone();
        let start_bytes = start_inclusive.map(|b| b.to_vec());
        let end_bytes = end_inclusive.map(|b| b.to_vec());

        Box::pin(stream! {
            // After the first batch we advance past the last yielded key.
            let mut cursor: Option<Vec<u8>> = None;

            loop {
                let tree_clone = tree.clone();
                let cur = cursor.clone();
                let initial_start = start_bytes.clone();
                let upper = end_bytes.clone();

                let batch: DbResult<Vec<(Bytes, Bytes)>> = spawn_blocking(move || {
                    // sled's `range` takes any RangeBounds<IVec>;
                    // we build it from Vec<u8>. Use `(Bound, Bound)`
                    // for explicit inclusive/exclusive control.
                    use std::ops::Bound;
                    let lower: Bound<&[u8]> = match (&cur, &initial_start) {
                        (Some(c), _) => Bound::Excluded(c.as_slice()),
                        (None, Some(s)) => Bound::Included(s.as_slice()),
                        (None, None) => Bound::Unbounded,
                    };
                    let upper: Bound<&[u8]> = match &upper {
                        Some(e) => Bound::Included(e.as_slice()),
                        None => Bound::Unbounded,
                    };

                    let mut items = Vec::new();
                    for kv in tree_clone.range::<&[u8], _>((lower, upper)).take(batch_size) {
                        let (key, val) = kv.map_err(|e| {
                            DbError::Storage(format!("SledDB range item: {}", e))
                        })?;
                        items.push((
                            Bytes::copy_from_slice(&key),
                            Bytes::copy_from_slice(&val),
                        ));
                    }
                    Ok(items)
                })
                .await
                .map_err(|e| DbError::Storage(format!("Tokio join error: {}", e)))?;

                let batch = batch?;
                if batch.is_empty() {
                    break;
                }
                cursor = batch.last().map(|(k, _)| k.to_vec());
                yield Ok(batch);
            }
        })
    }

    /// Vectored read: ONE `spawn_blocking`, N `tree.get` calls.
    /// Saves N-1 tokio task hops vs the default loop on bulk index
    /// lookups.
    async fn get_many(&self, keys: Vec<RecordKey>) -> DbResult<Vec<Option<Bytes>>> {
        if keys.is_empty() {
            return Ok(Vec::new());
        }
        let tree = self.tree.clone();
        spawn_blocking(move || -> DbResult<Vec<Option<Bytes>>> {
            let mut out = Vec::with_capacity(keys.len());
            for k in keys {
                match tree
                    .get(&k[..])
                    .map_err(|e| DbError::Storage(format!("SledDB get: {}", e)))?
                {
                    Some(val) => out.push(Some(Bytes::copy_from_slice(&val))),
                    None => out.push(None),
                }
            }
            Ok(out)
        })
        .await
        .map_err(|e| DbError::Storage(format!("Tokio join error: {}", e)))?
    }

    /// Reverse range scan via sled's `Tree::range(...).rev()`.
    /// Cursor advances past the last yielded key on the upper side
    /// (`Bound::Excluded(cursor)` becomes the new upper).
    fn iter_range_stream_reverse(
        &self,
        start_inclusive: Option<Bytes>,
        end_inclusive: Option<Bytes>,
        batch_size: usize,
    ) -> Pin<Box<dyn Stream<Item = Result<Vec<(RecordKey, Bytes)>, DbError>> + Send>> {
        let tree = self.tree.clone();
        let start_bytes = start_inclusive.map(|b| b.to_vec());
        let end_bytes = end_inclusive.map(|b| b.to_vec());

        Box::pin(stream! {
            // Cursor walks downward: the upper bound shrinks each batch.
            let mut cursor: Option<Vec<u8>> = None;

            loop {
                let tree_clone = tree.clone();
                let cur = cursor.clone();
                let lower_init = start_bytes.clone();
                let upper_init = end_bytes.clone();

                let batch: DbResult<Vec<(Bytes, Bytes)>> = spawn_blocking(move || {
                    use std::ops::Bound;
                    let lower: Bound<&[u8]> = match &lower_init {
                        Some(s) => Bound::Included(s.as_slice()),
                        None => Bound::Unbounded,
                    };
                    let upper: Bound<&[u8]> = match (&cur, &upper_init) {
                        (Some(c), _) => Bound::Excluded(c.as_slice()),
                        (None, Some(e)) => Bound::Included(e.as_slice()),
                        (None, None) => Bound::Unbounded,
                    };

                    let mut items = Vec::new();
                    for kv in tree_clone
                        .range::<&[u8], _>((lower, upper))
                        .rev()
                        .take(batch_size)
                    {
                        let (key, val) = kv.map_err(|e| {
                            DbError::Storage(format!("SledDB rev-range item: {}", e))
                        })?;
                        items.push((
                            Bytes::copy_from_slice(&key),
                            Bytes::copy_from_slice(&val),
                        ));
                    }
                    Ok(items)
                })
                .await
                .map_err(|e| DbError::Storage(format!("Tokio join error: {}", e)))?;

                let batch = batch?;
                if batch.is_empty() {
                    break;
                }
                cursor = batch.last().map(|(k, _)| k.to_vec());
                yield Ok(batch);
            }
        })
    }

    /// Native atomic `transact` via `sled::Batch`.
    ///
    /// Builds a `sled::Batch` containing all `KvOp`s, then applies it
    /// atomically via `tree.apply_batch`. Either the entire batch is
    /// visible or none of it.
    async fn transact(&self, ops: Vec<super::types::KvOp>) -> DbResult<()> {
        if ops.is_empty() {
            return Ok(());
        }
        let tree = self.tree.clone();
        spawn_blocking(move || -> DbResult<()> {
            let mut batch = sled::Batch::default();
            for op in ops {
                match op {
                    super::types::KvOp::Set(k, v) => batch.insert(k.as_ref(), v.as_ref()),
                    super::types::KvOp::Remove(k) => batch.remove(k.as_ref()),
                }
            }
            tree.apply_batch(batch)
                .map_err(|e| DbError::Storage(format!("SledDB apply_batch: {}", e)))
        })
        .await
        .map_err(|e| DbError::Storage(format!("Tokio join error: {}", e)))?
    }

    /// Explicit fsync. Individual writes are buffered and made durable
    /// by sled's background flusher (default: every 500 ms). Call this
    /// when an external durability boundary is needed (end of batch
    /// request, explicit user FLUSH, graceful shutdown).
    async fn flush(&self) -> DbResult<()> {
        let tree = self.tree.clone();
        spawn_blocking(move || -> DbResult<()> {
            tree.flush()
                .map_err(|e| DbError::Storage(format!("SledDB flush: {}", e)))?;
            Ok(())
        })
        .await
        .map_err(|e| DbError::Storage(format!("Tokio join error: {}", e)))?
    }
}

// ============================================================================
// Tests
// ============================================================================
