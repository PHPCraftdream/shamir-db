use super::types::{RecordKey, Repo, Store};
use crate::error::{DbError, DbResult};
use async_stream::stream;
use async_trait::async_trait;
use bytes::Bytes;
use canopydb::Database;
use futures::stream::Stream;
use shamir_types::types::record_id::RecordId;
use std::path::Path;
use std::pin::Pin;
use std::sync::Arc;
use tokio::task::spawn_blocking;

const META_TREE_NAME: &[u8] = b"__SHAMIR_META_STORES__";

// ============================================================================
// CanopyRepo - manages multiple stores (trees)
// ============================================================================

#[derive(Clone)]
pub struct CanopyRepo {
    db: Arc<Database>,
}

impl CanopyRepo {
    pub fn new(path: impl AsRef<Path>) -> DbResult<Self> {
        let db =
            Database::new(path).map_err(|e| DbError::Storage(format!("CanopyDB new: {}", e)))?;
        Ok(Self { db: Arc::new(db) })
    }
}

#[async_trait]
impl Repo for CanopyRepo {
    async fn store_get<S: AsRef<str> + Send>(&self, name: S) -> DbResult<Arc<dyn Store>> {
        let db = self.db.clone();
        let table_name = name.as_ref().to_string();
        let table_name_bytes = table_name.as_bytes().to_vec();

        spawn_blocking(move || -> DbResult<()> {
            let tx = db
                .begin_write()
                .map_err(|e| DbError::Storage(format!("CanopyDB begin_write: {}", e)))?;
            {
                let mut meta_tree = tx
                    .get_or_create_tree(META_TREE_NAME)
                    .map_err(|e| DbError::Storage(format!("CanopyDB get_meta_tree: {}", e)))?;
                meta_tree
                    .insert(&table_name_bytes, &[])
                    .map_err(|e| DbError::Storage(format!("CanopyDB meta_insert: {}", e)))?;
            }
            tx.commit()
                .map_err(|e| DbError::Storage(format!("CanopyDB commit: {}", e)))?;
            Ok(())
        })
        .await
        .map_err(|e| DbError::Storage(format!("Tokio join error: {}", e)))??;

        let store = CanopyStore {
            db: self.db.clone(),
            table_name,
        };
        Ok(Arc::new(store))
    }

    async fn store_delete<S: AsRef<str> + Send>(&self, name: S) -> DbResult<bool> {
        let db = self.db.clone();
        let table_name = name.as_ref().to_string();

        spawn_blocking(move || -> DbResult<bool> {
            let tx = db
                .begin_write()
                .map_err(|e| DbError::Storage(format!("CanopyDB begin_write: {}", e)))?;
            let table_name_bytes = table_name.as_bytes();
            let existed;

            {
                // Clear the user tree
                if let Some(mut user_tree) = tx
                    .get_tree(table_name_bytes)
                    .map_err(|e| DbError::Storage(format!("CanopyDB get_tree: {}", e)))?
                {
                    let keys: Vec<Vec<u8>> = user_tree
                        .iter()
                        .map_err(|e| DbError::Storage(format!("CanopyDB iter: {}", e)))?
                        .map(|res| res.map(|(k, _v)| k.to_vec()))
                        .collect::<Result<_, _>>()
                        .map_err(|e| DbError::Storage(format!("CanopyDB iter collect: {}", e)))?;

                    for key in keys {
                        user_tree
                            .delete(&key)
                            .map_err(|e| DbError::Storage(format!("CanopyDB delete: {}", e)))?;
                    }
                }

                // Remove from meta tree
                let mut meta_tree = tx
                    .get_or_create_tree(META_TREE_NAME)
                    .map_err(|e| DbError::Storage(format!("CanopyDB get_meta_tree: {}", e)))?;

                existed = meta_tree
                    .delete(table_name_bytes)
                    .map_err(|e| DbError::Storage(format!("CanopyDB meta_delete: {}", e)))?;
            }

            tx.commit()
                .map_err(|e| DbError::Storage(format!("CanopyDB commit: {}", e)))?;

            Ok(existed)
        })
        .await
        .map_err(|e| DbError::Storage(format!("Tokio join error: {}", e)))?
    }

    async fn stores_list(&self) -> DbResult<Vec<String>> {
        let db = self.db.clone();
        spawn_blocking(move || -> DbResult<Vec<String>> {
            let tx = db
                .begin_read()
                .map_err(|e| DbError::Storage(format!("CanopyDB begin_read: {}", e)))?;

            let mut names = Vec::new();
            if let Some(meta_tree) = tx
                .get_tree(META_TREE_NAME)
                .map_err(|e| DbError::Storage(format!("CanopyDB get_meta_tree: {}", e)))?
            {
                for item in meta_tree
                    .iter()
                    .map_err(|e| DbError::Storage(format!("CanopyDB meta_iter: {}", e)))?
                {
                    let (key, _) =
                        item.map_err(|e| DbError::Storage(format!("CanopyDB iter item: {}", e)))?;
                    let name = String::from_utf8(key.to_vec())
                        .map_err(|e| DbError::Codec(format!("UTF-8 decode error: {}", e)))?;
                    names.push(name);
                }
            }
            Ok(names)
        })
        .await
        .map_err(|e| DbError::Storage(format!("Tokio join error: {}", e)))?
    }
}

// ============================================================================
// CanopyStore - individual store (tree)
// ============================================================================

pub struct CanopyStore {
    db: Arc<Database>,
    table_name: String,
}

// SAFETY: `canopydb::Database` is internally synchronised (its read/write
// transactions take `&self` and the crate documents concurrent access).
// `Arc<Database>` keeps the handle alive across threads. All mutation
// of `CanopyStore` happens through the transactional API which serialises
// writers internally. These impls cover the case where canopydb's
// auto-impl would not fire (e.g. PhantomData or future internal fields).
unsafe impl Send for CanopyStore {}
unsafe impl Sync for CanopyStore {}

#[async_trait]
impl Store for CanopyStore {
    async fn insert(&self, value: Bytes) -> DbResult<RecordKey> {
        let db = self.db.clone();
        let table_name = self.table_name.clone();

        spawn_blocking(move || -> DbResult<RecordKey> {
            let tx = db
                .begin_write()
                .map_err(|e| DbError::Storage(format!("CanopyDB begin_write: {}", e)))?;

            let id = RecordId::new();
            let key = RecordKey::copy_from_slice(id.as_bytes());

            {
                let mut tree = tx
                    .get_or_create_tree(table_name.as_bytes())
                    .map_err(|e| DbError::Storage(format!("CanopyDB get_or_create_tree: {}", e)))?;

                tree.insert(&key[..], &value)
                    .map_err(|e| DbError::Storage(format!("CanopyDB insert: {}", e)))?;
            }

            tx.commit()
                .map_err(|e| DbError::Storage(format!("CanopyDB commit: {}", e)))?;

            Ok(key)
        })
        .await
        .map_err(|e| DbError::Storage(format!("Tokio join error: {}", e)))?
    }

    async fn set(&self, key: RecordKey, value: Bytes) -> DbResult<bool> {
        let db = self.db.clone();
        let table_name = self.table_name.clone();

        spawn_blocking(move || -> DbResult<bool> {
            let tx = db
                .begin_write()
                .map_err(|e| DbError::Storage(format!("CanopyDB begin_write: {}", e)))?;

            let created;
            {
                let mut tree = tx
                    .get_or_create_tree(table_name.as_bytes())
                    .map_err(|e| DbError::Storage(format!("CanopyDB get_or_create_tree: {}", e)))?;

                let existed = tree
                    .get(&key[..])
                    .map_err(|e| DbError::Storage(format!("CanopyDB get: {}", e)))?
                    .is_some();

                tree.insert(&key[..], &value)
                    .map_err(|e| DbError::Storage(format!("CanopyDB insert: {}", e)))?;

                created = !existed;
            }

            tx.commit()
                .map_err(|e| DbError::Storage(format!("CanopyDB commit: {}", e)))?;

            Ok(created)
        })
        .await
        .map_err(|e| DbError::Storage(format!("Tokio join error: {}", e)))?
    }

    async fn get(&self, key: RecordKey) -> DbResult<Bytes> {
        let db = self.db.clone();
        let table_name = self.table_name.clone();

        spawn_blocking(move || -> DbResult<Bytes> {
            let tx = db
                .begin_read()
                .map_err(|e| DbError::Storage(format!("CanopyDB begin_read: {}", e)))?;

            let tree = tx
                .get_tree(table_name.as_bytes())
                .map_err(|e| DbError::Storage(format!("CanopyDB get_tree: {}", e)))?
                .ok_or_else(|| DbError::NotFound(table_name.clone()))?;

            let val = tree
                .get(&key[..])
                .map_err(|e| DbError::Storage(format!("CanopyDB get: {}", e)))?
                .ok_or_else(|| DbError::NotFound(format!("{:?}", key)))?;

            Ok(Bytes::copy_from_slice(&val))
        })
        .await
        .map_err(|e| DbError::Storage(format!("Tokio join error: {}", e)))?
    }

    async fn get_many(&self, keys: Vec<RecordKey>) -> DbResult<Vec<Option<Bytes>>> {
        if keys.is_empty() {
            return Ok(Vec::new());
        }
        let db = self.db.clone();
        let table_name = self.table_name.clone();
        spawn_blocking(move || -> DbResult<Vec<Option<Bytes>>> {
            let tx = db
                .begin_read()
                .map_err(|e| DbError::Storage(format!("CanopyDB begin_read: {}", e)))?;
            let tree = tx
                .get_tree(table_name.as_bytes())
                .map_err(|e| DbError::Storage(format!("CanopyDB get_tree: {}", e)))?
                .ok_or_else(|| DbError::NotFound(table_name.clone()))?;
            let mut out = Vec::with_capacity(keys.len());
            for k in keys {
                match tree
                    .get(&k[..])
                    .map_err(|e| DbError::Storage(format!("CanopyDB get: {}", e)))?
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

    async fn remove(&self, key: RecordKey) -> DbResult<bool> {
        let db = self.db.clone();
        let table_name = self.table_name.clone();

        spawn_blocking(move || -> DbResult<bool> {
            let tx = db
                .begin_write()
                .map_err(|e| DbError::Storage(format!("CanopyDB begin_write: {}", e)))?;

            let existed;
            {
                let mut tree = tx
                    .get_tree(table_name.as_bytes())
                    .map_err(|e| DbError::Storage(format!("CanopyDB get_tree: {}", e)))?
                    .ok_or_else(|| DbError::NotFound(table_name.clone()))?;

                existed = tree
                    .delete(&key[..])
                    .map_err(|e| DbError::Storage(format!("CanopyDB delete: {}", e)))?;
            }

            tx.commit()
                .map_err(|e| DbError::Storage(format!("CanopyDB commit: {}", e)))?;

            Ok(existed)
        })
        .await
        .map_err(|e| DbError::Storage(format!("Tokio join error: {}", e)))?
    }

    /// Native atomic `transact` via canopy's write transaction.
    ///
    /// Opens one `begin_write()`, applies all `KvOp`s (insert /
    /// delete) within the same tree handle, then commits. If any
    /// operation fails, the transaction is dropped (not committed)
    /// — no partial state is observable.
    async fn transact(&self, ops: Vec<super::types::KvOp>) -> DbResult<()> {
        if ops.is_empty() {
            return Ok(());
        }
        let db = self.db.clone();
        let table_name = self.table_name.clone();
        spawn_blocking(move || -> DbResult<()> {
            let tx = db
                .begin_write()
                .map_err(|e| DbError::Storage(format!("CanopyDB begin_write: {}", e)))?;
            {
                let mut tree = tx
                    .get_or_create_tree(table_name.as_bytes())
                    .map_err(|e| DbError::Storage(format!("CanopyDB get_or_create_tree: {}", e)))?;
                for op in ops {
                    match op {
                        super::types::KvOp::Set(k, v) => {
                            tree.insert(&k[..], &v[..])
                                .map_err(|e| DbError::Storage(format!("CanopyDB insert: {}", e)))?;
                        }
                        super::types::KvOp::Remove(k) => {
                            tree.delete(&k[..])
                                .map_err(|e| DbError::Storage(format!("CanopyDB delete: {}", e)))?;
                        }
                    }
                }
            }
            tx.commit()
                .map_err(|e| DbError::Storage(format!("CanopyDB commit: {}", e)))?;
            Ok(())
        })
        .await
        .map_err(|e| DbError::Storage(format!("Tokio join error: {}", e)))?
    }

    fn iter_stream(
        &self,
        batch_size: usize,
    ) -> Pin<Box<dyn Stream<Item = Result<Vec<(RecordKey, Bytes)>, DbError>> + Send>> {
        let db = self.db.clone();
        let table_name = self.table_name.clone();

        Box::pin(stream! {
            let mut last_key: Option<Vec<u8>> = None;

            loop {
                let db_clone = db.clone();
                let table_name_clone = table_name.clone();
                let start_key = last_key;

                let batch: DbResult<(Vec<_>, Option<Vec<u8>>)> = spawn_blocking(move || {
                    let tx = db_clone
                        .begin_read()
                        .map_err(|e| DbError::Storage(format!("CanopyDB begin_read: {}", e)))?;

                    let tree_res = tx
                        .get_tree(table_name_clone.as_bytes())
                        .map_err(|e| DbError::Storage(format!("CanopyDB get_tree: {}", e)))?;

                    if let Some(tree) = tree_res {
                        let mut out = Vec::new();
                        let mut next_cursor = None;

                        // Use range with exclusive start to avoid duplicates
                        use std::ops::Bound;

                        let excluded_key = start_key;
                        let range_bounds: (Bound<&[u8]>, Bound<&[u8]>) = if let Some(ref start) = excluded_key {
                            (Bound::Excluded(start.as_slice()), Bound::Unbounded)
                        } else {
                            (Bound::Unbounded, Bound::Unbounded)
                        };

                        let mut iter = tree
                            .range::<&[u8]>(range_bounds)
                            .map_err(|e| DbError::Storage(format!("CanopyDB iter: {}", e)))?;

                        // Collect up to batch_size items
                        for _ in 0..batch_size {
                            match iter.next() {
                                Some(Ok(item)) => {
                                    let (key, val) = item;
                                    next_cursor = Some(key.to_vec());
                                    out.push((Bytes::copy_from_slice(&key), Bytes::copy_from_slice(&val)));
                                }
                                Some(Err(e)) => {
                                    return Err(DbError::Storage(format!("CanopyDB iter item: {}", e)));
                                }
                                None => break, // No more items
                            }
                        }

                        Ok((out, next_cursor))
                    } else {
                        Ok((vec![], None))
                    }
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
        let db = self.db.clone();
        let table_name = self.table_name.clone();

        Box::pin(stream! {
            let mut last_key: Option<Vec<u8>> = None;

            // Calculate upper bound for prefix
            let mut prefix_end = prefix.to_vec();
            if let Some(last_byte) = prefix_end.last_mut() {
                *last_byte = last_byte.wrapping_add(1);
            }

            loop {
                let db_clone = db.clone();
                let table_name_clone = table_name.clone();
                let start_key = last_key;
                let prefix_clone = prefix.clone();
                let prefix_end_clone = prefix_end.clone();

                let batch: DbResult<(Vec<_>, Option<Vec<u8>>)> = spawn_blocking(move || {
                    let tx = db_clone
                        .begin_read()
                        .map_err(|e| DbError::Storage(format!("CanopyDB begin_read: {}", e)))?;

                    let tree_res = tx
                        .get_tree(table_name_clone.as_bytes())
                        .map_err(|e| DbError::Storage(format!("CanopyDB get_tree: {}", e)))?;

                    if let Some(tree) = tree_res {
                        let mut out = Vec::new();
                        let mut next_cursor = None;

                        // Use range with exclusive start to avoid duplicates
                        use std::ops::Bound;

                        let range_bounds: (Bound<&[u8]>, Bound<&[u8]>) = if let Some(start) = &start_key {
                            (Bound::Excluded(start.as_slice()), Bound::Excluded(prefix_end_clone.as_slice()))
                        } else {
                            (Bound::Included(prefix_clone.as_ref()), Bound::Excluded(prefix_end_clone.as_slice()))
                        };

                        let mut iter = tree
                            .range::<&[u8]>(range_bounds)
                            .map_err(|e| DbError::Storage(format!("CanopyDB iter: {}", e)))?;

                        // Collect up to batch_size items
                        for _ in 0..batch_size {
                            match iter.next() {
                                Some(Ok(item)) => {
                                    let (key, val) = item;
                                    // Stop if no longer starts with prefix
                                    if !key.starts_with(&prefix_clone) {
                                        break;
                                    }
                                    next_cursor = Some(key.to_vec());
                                    out.push((Bytes::copy_from_slice(&key), Bytes::copy_from_slice(&val)));
                                }
                                Some(Err(e)) => {
                                    return Err(DbError::Storage(format!("CanopyDB iter item: {}", e)));
                                }
                                None => break,
                            }
                        }

                        Ok((out, next_cursor))
                    } else {
                        Ok((vec![], None))
                    }
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
