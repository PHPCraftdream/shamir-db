use super::types::{RecordKey, Repo, Store};
use crate::error::{DbError, DbResult};
use async_stream::stream;
use async_trait::async_trait;
use bytes::Bytes;
use futures::stream::Stream;
use nebari::{
    io::fs::StdFile,
    tree::{Operation, Root, ScanEvaluation, Unversioned, UnversionedTreeRoot},
    ArcBytes, Config, Roots, Tree,
};
use shamir_types::types::record_id::RecordId;
use std::path::Path;
use std::pin::Pin;
use std::sync::Arc;
use tokio::task::spawn_blocking;

const META_TREE_NAME: &str = "__SHAMIR_META_STORES__";

// ============================================================================
// NebariRepo - manages multiple stores (trees)
// ============================================================================

#[derive(Clone)]
pub struct NebariRepo {
    roots: Arc<Roots<StdFile>>,
}

impl NebariRepo {
    pub fn new(path: impl AsRef<Path>) -> DbResult<Self> {
        let roots = Config::default_for(path.as_ref())
            .open()
            .map_err(|e| DbError::Storage(format!("NebariDB open: {}", e)))?;
        Ok(Self {
            roots: Arc::new(roots),
        })
    }
}

#[async_trait]
impl Repo for NebariRepo {
    async fn store_get<S: AsRef<str> + Send>(&self, name: S) -> DbResult<Arc<dyn Store>> {
        let roots = self.roots.clone();
        let table_name = name.as_ref().to_string();

        // Регистрируем в мета-дереве
        let roots_clone = roots.clone();
        let table_name_clone = table_name.clone();
        spawn_blocking(move || -> DbResult<()> {
            let meta_tree = roots_clone
                .tree(Unversioned::tree(META_TREE_NAME))
                .map_err(|e| DbError::Storage(format!("NebariDB meta_tree: {}", e)))?;
            meta_tree
                .set(table_name_clone, [])
                .map_err(|e| DbError::Storage(format!("NebariDB meta_set: {}", e)))?;
            Ok(())
        })
        .await
        .map_err(|e| DbError::Storage(format!("Tokio join error: {}", e)))??;

        // Открываем дерево пользователя
        let roots_for_store = roots.clone();
        let name_for_store = name.as_ref().to_string();
        let tree = spawn_blocking(
            move || -> DbResult<Tree<UnversionedTreeRoot<()>, StdFile>> {
                roots
                    .tree(Unversioned::tree(table_name))
                    .map_err(|e| DbError::Storage(format!("NebariDB open_tree: {}", e)))
            },
        )
        .await
        .map_err(|e| DbError::Storage(format!("Tokio join error: {}", e)))??;

        let store = NebariStore {
            tree: Arc::new(tree),
            roots: roots_for_store,
            name: name_for_store,
        };
        Ok(Arc::new(store))
    }

    async fn store_delete<S: AsRef<str> + Send>(&self, name: S) -> DbResult<bool> {
        let roots = self.roots.clone();
        let table_name = name.as_ref().to_string();

        spawn_blocking(move || -> DbResult<bool> {
            // Удаляем все записи из пользовательского дерева
            let user_tree = roots
                .tree(Unversioned::tree(table_name.clone()))
                .map_err(|e| DbError::Storage(e.to_string()))?;

            // Собираем все ключи
            let mut keys: Vec<Vec<u8>> = Vec::new();
            user_tree
                .scan::<nebari::Error, _, _, _, _>(
                    &(..),
                    true, // forward
                    |_, _, _| ScanEvaluation::ReadData,
                    |_, _| ScanEvaluation::ReadData,
                    |key, _, _| {
                        keys.push(key.to_vec());
                        Ok(())
                    },
                )
                .map_err(|e| DbError::Storage(format!("NebariDB scan: {}", e)))?;

            // Удаляем каждый ключ
            for key in keys {
                user_tree
                    .remove(&key)
                    .map_err(|e| DbError::Storage(e.to_string()))?;
            }

            // Удаляем запись из мета-дерева
            let meta_tree = roots
                .tree(Unversioned::tree(META_TREE_NAME))
                .map_err(|e| DbError::Storage(e.to_string()))?;

            let existed = meta_tree
                .remove(table_name.as_bytes())
                .map_err(|e| DbError::Storage(e.to_string()))?
                .is_some();

            Ok(existed)
        })
        .await
        .map_err(|e| DbError::Storage(format!("Tokio join error: {}", e)))?
    }

    async fn stores_list(&self) -> DbResult<Vec<String>> {
        let roots = self.roots.clone();
        spawn_blocking(move || -> DbResult<Vec<String>> {
            let meta_tree = roots
                .tree(Unversioned::tree(META_TREE_NAME))
                .map_err(|e| DbError::Storage(format!("NebariDB meta_tree: {}", e)))?;

            let mut names = Vec::new();
            meta_tree
                .scan::<nebari::Error, _, _, _, _>(
                    &(..),
                    true, // forward
                    |_, _, _| ScanEvaluation::ReadData,
                    |_, _| ScanEvaluation::ReadData,
                    |key, _, _| {
                        let key_bytes = key.to_vec();
                        let name = String::from_utf8(key_bytes).map_err(|e| {
                            nebari::Error::from(std::io::Error::new(
                                std::io::ErrorKind::InvalidData,
                                e.to_string(),
                            ))
                        })?;
                        names.push(name);
                        Ok(())
                    },
                )
                .map_err(|e| DbError::Storage(format!("NebariDB scan: {}", e)))?;

            Ok(names)
        })
        .await
        .map_err(|e| DbError::Storage(format!("Tokio join error: {}", e)))?
    }
}

// ============================================================================
// NebariStore - individual store (tree)
// ============================================================================

pub struct NebariStore {
    tree: Arc<Tree<UnversionedTreeRoot<()>, StdFile>>,
    /// Kept alongside `tree` so batched-write paths can open a
    /// transactional handle (`roots.transaction(&[root])`) which
    /// commits all N writes with one fsync.
    roots: Arc<Roots<StdFile>>,
    /// The tree's name — needed to identify it when building the
    /// transactional handle for batched writes.
    name: String,
}

// SAFETY: nebari `Tree` and `Roots` are designed for concurrent access
// (the crate's `Tree::insert` / `Roots::transaction` take `&self`). Both
// are wrapped in `Arc` here so the handle is shared safely across the
// tokio worker pool; all writes go through `spawn_blocking`. `String`
// is inherently Send+Sync. Impls are explicit per §B5 — auto-impl would
// otherwise apply.
unsafe impl Send for NebariStore {}
unsafe impl Sync for NebariStore {}

#[async_trait]
impl Store for NebariStore {
    async fn insert(&self, value: Bytes) -> DbResult<RecordKey> {
        let tree = self.tree.clone();

        spawn_blocking(move || -> DbResult<RecordKey> {
            let id = RecordId::new();
            let key = RecordKey::copy_from_slice(id.as_bytes());

            tree.set(key.to_vec(), value.to_vec())
                .map_err(|e| DbError::Storage(format!("NebariDB set: {}", e)))?;

            Ok(key)
        })
        .await
        .map_err(|e| DbError::Storage(format!("Tokio join error: {}", e)))?
    }

    async fn set(&self, key: RecordKey, value: Bytes) -> DbResult<bool> {
        let tree = self.tree.clone();

        spawn_blocking(move || -> DbResult<bool> {
            let key_bytes = key.to_vec();

            let existed = tree
                .get(&key_bytes)
                .map_err(|e| DbError::Storage(format!("NebariDB get: {}", e)))?
                .is_some();

            tree.set(key_bytes, value.to_vec())
                .map_err(|e| DbError::Storage(format!("NebariDB set: {}", e)))?;

            Ok(!existed)
        })
        .await
        .map_err(|e| DbError::Storage(format!("Tokio join error: {}", e)))?
    }

    async fn get(&self, key: RecordKey) -> DbResult<Bytes> {
        let tree = self.tree.clone();

        spawn_blocking(move || -> DbResult<Bytes> {
            let key_bytes = key.to_vec();
            let val = tree
                .get(&key_bytes)
                .map_err(|e| DbError::Storage(format!("NebariDB get: {}", e)))?
                .ok_or_else(|| DbError::NotFound(format!("{:?}", key)))?;

            Ok(Bytes::copy_from_slice(val.as_ref()))
        })
        .await
        .map_err(|e| DbError::Storage(format!("Tokio join error: {}", e)))?
    }

    async fn remove(&self, key: RecordKey) -> DbResult<bool> {
        let tree = self.tree.clone();

        spawn_blocking(move || -> DbResult<bool> {
            let key_bytes = key.to_vec();
            let existed = tree
                .remove(&key_bytes)
                .map_err(|e| DbError::Storage(format!("NebariDB remove: {}", e)))?
                .is_some();

            Ok(existed)
        })
        .await
        .map_err(|e| DbError::Storage(format!("Tokio join error: {}", e)))?
    }

    async fn get_many(&self, keys: Vec<RecordKey>) -> DbResult<Vec<Option<Bytes>>> {
        if keys.is_empty() {
            return Ok(Vec::new());
        }
        let tree = self.tree.clone();
        spawn_blocking(move || -> DbResult<Vec<Option<Bytes>>> {
            let mut out = Vec::with_capacity(keys.len());
            for k in keys {
                let key_bytes = k.to_vec();
                match tree
                    .get(&key_bytes)
                    .map_err(|e| DbError::Storage(format!("NebariDB get: {}", e)))?
                {
                    Some(val) => out.push(Some(Bytes::copy_from_slice(val.as_ref()))),
                    None => out.push(None),
                }
            }
            Ok(out)
        })
        .await
        .map_err(|e| DbError::Storage(format!("Tokio join error: {}", e)))?
    }

    /// Reverse range scan using nebari's `tree.scan(.., forwards: false, ..)`
    /// — the B-tree is walked backwards from the upper bound.
    /// Compared to the default impl (full forward scan + collect +
    /// in-memory reverse), this gives O(log N + K) for a top-K read.
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
            // Cursor moves downward; the upper bound shrinks each batch.
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

                    let mut out: Vec<(Bytes, Bytes)> = Vec::new();
                    let mut count = 0;
                    tree_clone
                        .scan::<nebari::Error, _, _, _, _>(
                            &(lower, upper),
                            false, // backwards
                            |_, _, _| ScanEvaluation::ReadData,
                            |_, _| ScanEvaluation::ReadData,
                            |key, _, value| {
                                if count >= batch_size {
                                    return Ok(());
                                }
                                out.push((
                                    Bytes::copy_from_slice(key.as_ref()),
                                    Bytes::copy_from_slice(value.as_ref()),
                                ));
                                count += 1;
                                Ok(())
                            },
                        )
                        .map_err(|e| DbError::Storage(format!("NebariDB rev-scan: {}", e)))?;
                    Ok(out)
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

    /// Native atomic `transact` via nebari `Roots::transaction`.
    ///
    /// Collects sets and removes separately (nebari requires
    /// sorted keys per `modify` call), executes both within the
    /// same transaction, then commits atomically. If any operation
    /// fails, the transaction is dropped — no partial state.
    async fn transact(&self, ops: Vec<super::types::KvOp>) -> DbResult<()> {
        if ops.is_empty() {
            return Ok(());
        }
        let roots = self.roots.clone();
        let name = self.name.clone();
        spawn_blocking(move || -> DbResult<()> {
            // Partition ops into sets and removes, sorting each by key
            // (nebari requires ascending key order for modify).
            let mut sets: Vec<(Vec<u8>, Vec<u8>)> = Vec::new();
            let mut removes: Vec<Vec<u8>> = Vec::new();
            for op in ops {
                match op {
                    super::types::KvOp::Set(k, v) => {
                        sets.push((k.to_vec(), v.to_vec()));
                    }
                    super::types::KvOp::Remove(k) => {
                        removes.push(k.to_vec());
                    }
                }
            }
            sets.sort_by(|a, b| a.0.cmp(&b.0));
            removes.sort();
            removes.dedup();

            let transaction = roots
                .transaction::<_, _>(&[Unversioned::tree(name.clone())])
                .map_err(|e| DbError::Storage(format!("NebariDB transaction: {}", e)))?;
            {
                let mut tree_handle = transaction
                    .tree::<UnversionedTreeRoot<()>>(0)
                    .ok_or_else(|| DbError::Storage("NebariDB tree handle".to_string()))?;

                if !sets.is_empty() {
                    let keys: Vec<ArcBytes<'static>> = sets
                        .iter()
                        .map(|(k, _)| ArcBytes::from(k.clone()))
                        .collect();
                    let vals: Vec<ArcBytes<'static>> =
                        sets.into_iter().map(|(_, v)| ArcBytes::from(v)).collect();
                    tree_handle
                        .modify(keys, Operation::SetEach(vals))
                        .map_err(|e| DbError::Storage(format!("NebariDB modify set: {}", e)))?;
                }
                if !removes.is_empty() {
                    let keys: Vec<ArcBytes<'static>> =
                        removes.into_iter().map(ArcBytes::from).collect();
                    tree_handle
                        .modify(keys, Operation::Remove)
                        .map_err(|e| DbError::Storage(format!("NebariDB modify remove: {}", e)))?;
                }
            }
            transaction
                .commit()
                .map_err(|e| DbError::Storage(format!("NebariDB commit: {}", e)))?;
            Ok(())
        })
        .await
        .map_err(|e| DbError::Storage(format!("Tokio join error: {}", e)))?
    }

    /// Batched insert via `Roots::transaction(&[root]) → modify →
    /// commit` — one fsync for the whole batch instead of one per
    /// record. nebari's `Modification::SetEach` requires keys in
    /// strictly-ascending order, so we sort internally and return
    /// record_ids in the original input order.
    async fn insert_many(&self, values: Vec<Bytes>) -> DbResult<Vec<RecordKey>> {
        if values.is_empty() {
            return Ok(Vec::new());
        }
        let roots = self.roots.clone();
        let name = self.name.clone();
        spawn_blocking(move || -> DbResult<Vec<RecordKey>> {
            // Generate ids in input order; the caller sees this order.
            let ids: Vec<RecordKey> = (0..values.len())
                .map(|_| RecordKey::copy_from_slice(RecordId::new().as_bytes()))
                .collect();
            // Sort (key, value) pairs by key for nebari.
            let mut pairs: Vec<(RecordKey, Bytes)> = ids.iter().cloned().zip(values).collect();
            pairs.sort_by(|a, b| a.0.as_ref().cmp(b.0.as_ref()));

            let keys: Vec<ArcBytes<'static>> = pairs
                .iter()
                .map(|(k, _)| ArcBytes::from(k.to_vec()))
                .collect();
            let vals: Vec<ArcBytes<'static>> = pairs
                .into_iter()
                .map(|(_, v)| ArcBytes::from(v.to_vec()))
                .collect();

            let transaction = roots
                .transaction::<_, _>(&[Unversioned::tree(name.clone())])
                .map_err(|e| DbError::Storage(format!("NebariDB transaction: {}", e)))?;
            transaction
                .tree::<UnversionedTreeRoot<()>>(0)
                .ok_or_else(|| DbError::Storage("NebariDB tree handle".to_string()))?
                .modify(keys, Operation::SetEach(vals))
                .map_err(|e| DbError::Storage(format!("NebariDB modify: {}", e)))?;
            transaction
                .commit()
                .map_err(|e| DbError::Storage(format!("NebariDB commit: {}", e)))?;
            Ok(ids)
        })
        .await
        .map_err(|e| DbError::Storage(format!("Tokio join error: {}", e)))?
    }

    /// Batched upsert. `bool` per item = created (not present before).
    /// Same one-fsync-per-batch story as `insert_many`.
    async fn set_many(&self, items: Vec<(RecordKey, Bytes)>) -> DbResult<Vec<bool>> {
        if items.is_empty() {
            return Ok(Vec::new());
        }
        let tree = self.tree.clone();
        let roots = self.roots.clone();
        let name = self.name.clone();
        spawn_blocking(move || -> DbResult<Vec<bool>> {
            // Probe existence per key (read path is independent of
            // the batched commit transaction).
            let created_flags: Vec<bool> = items
                .iter()
                .map(|(k, _)| {
                    tree.get(k.as_ref())
                        .map(|opt| opt.is_none())
                        .unwrap_or(false)
                })
                .collect();

            let mut indexed: Vec<(usize, RecordKey, Bytes)> = items
                .into_iter()
                .enumerate()
                .map(|(i, (k, v))| (i, k, v))
                .collect();
            indexed.sort_by(|a, b| a.1.as_ref().cmp(b.1.as_ref()));

            let keys: Vec<ArcBytes<'static>> = indexed
                .iter()
                .map(|(_, k, _)| ArcBytes::from(k.to_vec()))
                .collect();
            let vals: Vec<ArcBytes<'static>> = indexed
                .into_iter()
                .map(|(_, _, v)| ArcBytes::from(v.to_vec()))
                .collect();

            let transaction = roots
                .transaction::<_, _>(&[Unversioned::tree(name.clone())])
                .map_err(|e| DbError::Storage(format!("NebariDB transaction: {}", e)))?;
            transaction
                .tree::<UnversionedTreeRoot<()>>(0)
                .ok_or_else(|| DbError::Storage("NebariDB tree handle".to_string()))?
                .modify(keys, Operation::SetEach(vals))
                .map_err(|e| DbError::Storage(format!("NebariDB modify: {}", e)))?;
            transaction
                .commit()
                .map_err(|e| DbError::Storage(format!("NebariDB commit: {}", e)))?;
            Ok(created_flags)
        })
        .await
        .map_err(|e| DbError::Storage(format!("Tokio join error: {}", e)))?
    }

    /// Batched remove. `bool` per key = existed-before-remove.
    async fn remove_many(&self, keys: Vec<RecordKey>) -> DbResult<Vec<bool>> {
        if keys.is_empty() {
            return Ok(Vec::new());
        }
        let tree = self.tree.clone();
        let roots = self.roots.clone();
        let name = self.name.clone();
        spawn_blocking(move || -> DbResult<Vec<bool>> {
            // Existed-flags in input order (probe + sort + bulk remove).
            let existed: Vec<bool> = keys
                .iter()
                .map(|k| tree.get(k.as_ref()).map(|o| o.is_some()).unwrap_or(false))
                .collect();
            let mut sorted: Vec<RecordKey> = keys.clone();
            sorted.sort_by(|a, b| a.as_ref().cmp(b.as_ref()));
            sorted.dedup();
            let arc_keys: Vec<ArcBytes<'static>> = sorted
                .into_iter()
                .map(|k| ArcBytes::from(k.to_vec()))
                .collect();
            let transaction = roots
                .transaction::<_, _>(&[Unversioned::tree(name.clone())])
                .map_err(|e| DbError::Storage(format!("NebariDB transaction: {}", e)))?;
            transaction
                .tree::<UnversionedTreeRoot<()>>(0)
                .ok_or_else(|| DbError::Storage("NebariDB tree handle".to_string()))?
                .modify(arc_keys, Operation::Remove)
                .map_err(|e| DbError::Storage(format!("NebariDB modify remove: {}", e)))?;
            transaction
                .commit()
                .map_err(|e| DbError::Storage(format!("NebariDB commit: {}", e)))?;
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
                let tree_clone = tree.clone();
                let start_key = last_key;

                let batch: DbResult<(Vec<_>, Option<Vec<u8>>)> = spawn_blocking(move || {
                    let mut out = Vec::new();
                    let mut next_cursor = None;
                    let mut count = 0;
                    let mut skipping = start_key.is_some();

                    tree_clone.scan::<nebari::Error, _, _, _, _>(
                        &(..),
                        true, // forward
                        |_, _, _| ScanEvaluation::ReadData,
                        |_, _| ScanEvaluation::ReadData,
                        |key, _, value| {
                            // Skip records until we pass start_key
                            if skipping {
                                if let Some(ref start) = start_key {
                                    if key.as_ref() <= start.as_slice() {
                                        return Ok(());
                                    }
                                }
                                skipping = false;
                                // key > cursor — fall through to include this record
                            }

                            if count < batch_size {
                                next_cursor = Some(key.to_vec());
                                out.push((Bytes::copy_from_slice(key.as_ref()), Bytes::copy_from_slice(value.as_ref())));
                                count += 1;
                            }
                            Ok(())
                        },
                    )
                        .map_err(|e| DbError::Storage(format!("NebariDB scan: {}", e)))?;

                    Ok((out, next_cursor))
                })
                .await
                .map_err(|e| DbError::Storage(format!("Tokio join error: {}", e)))?;

                let (batch, next_key) = batch?;

                if batch.is_empty() {
                    break;
                }

                last_key = next_key;
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

            // Calculate upper bound for prefix
            let mut prefix_end = prefix.to_vec();
            if let Some(last_byte) = prefix_end.last_mut() {
                *last_byte = last_byte.wrapping_add(1);
            }

            loop {
                let tree_clone = tree.clone();
                let start_key = last_key;
                let prefix_clone = prefix.clone();
                let prefix_end_clone = prefix_end.clone();

                let batch: DbResult<(Vec<_>, Option<Vec<u8>>)> = spawn_blocking(move || {
                    let mut out = Vec::new();
                    let mut next_cursor = None;
                    let mut count = 0;
                    let mut skipping = start_key.is_some();

                    let range = if let Some(ref start) = start_key {
                        &start[..]..&prefix_end_clone
                    } else {
                        &prefix_clone[..]..&prefix_end_clone
                    };

                    tree_clone.scan::<nebari::Error, _, _, _, _>(
                        &range,
                        true,
                        |_, _, _| ScanEvaluation::ReadData,
                        |_, _| ScanEvaluation::ReadData,
                        |key, _, value| {
                            // Skip records until we pass start_key
                            if skipping {
                                if let Some(ref start) = start_key {
                                    if key.as_ref() <= start.as_slice() {
                                        return Ok(());
                                    }
                                }
                                skipping = false;
                                // key > cursor — fall through to include this record
                            }

                            // Stop if we've collected enough
                            if count >= batch_size {
                                return Ok(());
                            }

                            next_cursor = Some(key.to_vec());
                            out.push((Bytes::copy_from_slice(key.as_ref()), Bytes::copy_from_slice(value.as_ref())));
                            count += 1;
                            Ok(())
                        },
                    )
                    .map_err(|e| DbError::Storage(format!("NebariDB scan: {}", e)))?;

                    Ok((out, next_cursor))
                })
                .await
                .map_err(|e| DbError::Storage(format!("Tokio join error: {}", e)))?;

                let (batch, next_key) = batch?;

                if batch.is_empty() {
                    break;
                }

                last_key = next_key;
                yield Ok(batch);
            }
        })
    }
}

// ============================================================================
// Tests
// ============================================================================
