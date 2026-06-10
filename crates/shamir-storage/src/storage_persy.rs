use super::types::{RecordKey, Repo, Store};
use crate::error::{DbError, DbResult};
use async_stream::stream;
use async_trait::async_trait;
use bytes::Bytes;
use futures::stream::Stream;
use persy::{ByteVec, Config, Persy, PersyId};
use shamir_types::types::record_id::RecordId;
use std::path::Path;
use std::pin::Pin;
use std::sync::Arc;
use tokio::task::spawn_blocking;

// ============================================================================
// PersyRepo - manages multiple stores (segments)
// ============================================================================

#[derive(Clone)]
pub struct PersyRepo {
    db: Arc<Persy>,
}

impl PersyRepo {
    pub fn new(path: impl AsRef<Path>) -> DbResult<Self> {
        Persy::create(path.as_ref()).map_err(|e| DbError::Storage(e.to_string()))?;
        let db = Persy::open(path.as_ref(), Config::default())
            .map_err(|e| DbError::Storage(e.to_string()))?;
        Ok(Self { db: Arc::new(db) })
    }
}

#[async_trait]
impl Repo for PersyRepo {
    async fn store_get<S: AsRef<str> + Send>(&self, name: S) -> DbResult<Arc<dyn Store>> {
        let db = self.db.clone();
        let table_name = name.as_ref().to_string();
        let index_name = format!("{}_idx", table_name);

        spawn_blocking(move || -> DbResult<()> {
            let mut tx = db.begin().map_err(|e| DbError::Storage(e.to_string()))?;

            // Create segment if it doesn't exist
            if !tx
                .exists_segment(&table_name)
                .map_err(|e| DbError::Storage(e.to_string()))?
            {
                tx.create_segment(&table_name)
                    .map_err(|e| DbError::Storage(e.to_string()))?;
            }

            // Create index for RecordKey -> PersyId mapping
            if !tx
                .exists_index(&index_name)
                .map_err(|e| DbError::Storage(e.to_string()))?
            {
                tx.create_index::<ByteVec, ByteVec>(&index_name, persy::ValueMode::Replace)
                    .map_err(|e| DbError::Storage(e.to_string()))?;
            }

            tx.prepare()
                .map_err(|e| DbError::Storage(e.to_string()))?
                .commit()
                .map_err(|e| DbError::Storage(e.to_string()))?;
            Ok(())
        })
        .await
        .map_err(|e| DbError::Storage(format!("Tokio join error: {}", e)))??;

        let store = PersyStore {
            db: self.db.clone(),
            table_name: name.as_ref().to_string(),
            index_name: format!("{}_idx", name.as_ref()),
        };
        Ok(Arc::new(store))
    }

    async fn store_delete<S: AsRef<str> + Send>(&self, name: S) -> DbResult<bool> {
        let db = self.db.clone();
        let table_name = name.as_ref().to_string();
        let index_name = format!("{}_idx", table_name);

        spawn_blocking(move || -> DbResult<bool> {
            let mut tx = db.begin().map_err(|e| DbError::Storage(e.to_string()))?;

            if tx
                .exists_index(&index_name)
                .map_err(|e| DbError::Storage(e.to_string()))?
            {
                tx.drop_index(&index_name)
                    .map_err(|e| DbError::Storage(e.to_string()))?;
            }

            if tx
                .exists_segment(&table_name)
                .map_err(|e| DbError::Storage(e.to_string()))?
            {
                tx.drop_segment(&table_name)
                    .map_err(|e| DbError::Storage(e.to_string()))?;
            }

            tx.prepare()
                .map_err(|e| DbError::Storage(e.to_string()))?
                .commit()
                .map_err(|e| DbError::Storage(e.to_string()))?;
            Ok(true)
        })
        .await
        .map_err(|e| DbError::Storage(format!("Tokio join error: {}", e)))?
    }

    async fn stores_list(&self) -> DbResult<Vec<String>> {
        let db = self.db.clone();
        spawn_blocking(move || -> DbResult<Vec<String>> {
            let segments = db
                .list_segments()
                .map_err(|e| DbError::Storage(e.to_string()))?;
            let names: Vec<String> = segments
                .into_iter()
                .map(|(name, _id)| name)
                .filter(|name| !name.ends_with("_idx"))
                .collect();
            Ok(names)
        })
        .await
        .map_err(|e| DbError::Storage(format!("Tokio join error: {}", e)))?
    }
}

// ============================================================================
// PersyStore - individual store (segment)
// ============================================================================

pub struct PersyStore {
    db: Arc<Persy>,
    table_name: String,
    index_name: String,
}

// SAFETY: `persy::Persy` exposes transactional APIs that take `&self`
// and the crate documents thread-safety via internal locking around its
// page cache + WAL. `Arc<Persy>` keeps that thread-safe handle alive;
// `String` fields (`table_name`, `index_name`) are inherently Send+Sync.
// Impls are explicit to make the trust on persy's internal sync visible
// per §B5 — auto-impl would otherwise apply.
unsafe impl Send for PersyStore {}
unsafe impl Sync for PersyStore {}

#[async_trait]
impl Store for PersyStore {
    async fn insert(&self, value: Bytes) -> DbResult<RecordKey> {
        let db = self.db.clone();
        let table_name = self.table_name.clone();
        let index_name = self.index_name.clone();

        spawn_blocking(move || -> DbResult<RecordKey> {
            let mut tx = db.begin().map_err(|e| DbError::Storage(e.to_string()))?;

            let id = RecordId::new();
            let key = RecordKey::copy_from_slice(id.as_bytes());

            let persy_id = tx
                .insert(&table_name, &value)
                .map_err(|e| DbError::Storage(e.to_string()))?;

            // Store RecordKey -> PersyId mapping in index
            let key_bytes_index = ByteVec::new(key.to_vec());
            let val = ByteVec::new(persy_id.to_string().into_bytes());
            tx.put(&index_name, key_bytes_index, val)
                .map_err(|e| DbError::Storage(e.to_string()))?;

            tx.prepare()
                .map_err(|e| DbError::Storage(e.to_string()))?
                .commit()
                .map_err(|e| DbError::Storage(e.to_string()))?;
            Ok(key)
        })
        .await
        .map_err(|e| DbError::Storage(format!("Tokio join error: {}", e)))?
    }

    async fn set(&self, key: RecordKey, value: Bytes) -> DbResult<bool> {
        let db = self.db.clone();
        let table_name = self.table_name.clone();
        let index_name = self.index_name.clone();

        spawn_blocking(move || -> DbResult<bool> {
            let mut tx = db.begin().map_err(|e| DbError::Storage(e.to_string()))?;
            let key_bytes = ByteVec::new(key.to_vec());

            // Find PersyId from index
            let mut iter = tx
                .get::<ByteVec, ByteVec>(&index_name, &key_bytes)
                .map_err(|e| DbError::Storage(e.to_string()))?;

            let created = if let Some(persy_id_str_bytes) = iter.next() {
                // Existing record - update
                let persy_id_str = String::from_utf8(persy_id_str_bytes.to_vec())
                    .map_err(|e| DbError::Codec(e.to_string()))?;
                let persy_id: PersyId = persy_id_str
                    .parse()
                    .map_err(|e| DbError::Codec(format!("Invalid PersyId: {}", e)))?;

                tx.update(&table_name, &persy_id, &value)
                    .map_err(|e| DbError::Storage(e.to_string()))?;
                false
            } else {
                // New record - insert and update index
                let persy_id = tx
                    .insert(&table_name, &value)
                    .map_err(|e| DbError::Storage(e.to_string()))?;

                let val = ByteVec::new(persy_id.to_string().into_bytes());
                tx.put(&index_name, key_bytes, val)
                    .map_err(|e| DbError::Storage(e.to_string()))?;
                true
            };

            tx.prepare()
                .map_err(|e| DbError::Storage(e.to_string()))?
                .commit()
                .map_err(|e| DbError::Storage(e.to_string()))?;
            Ok(created)
        })
        .await
        .map_err(|e| DbError::Storage(format!("Tokio join error: {}", e)))?
    }

    async fn get(&self, key: RecordKey) -> DbResult<Bytes> {
        let db = self.db.clone();
        let table_name = self.table_name.clone();
        let index_name = self.index_name.clone();

        spawn_blocking(move || -> DbResult<Bytes> {
            let mut tx = db.begin().map_err(|e| DbError::Storage(e.to_string()))?;
            let key_bytes = ByteVec::new(key.to_vec());

            let mut iter = tx
                .get::<ByteVec, ByteVec>(&index_name, &key_bytes)
                .map_err(|e| DbError::Storage(e.to_string()))?;

            let persy_id_str_bytes = iter
                .next()
                .ok_or_else(|| DbError::NotFound(format!("{:?}", key)))?;
            let persy_id_str = String::from_utf8(persy_id_str_bytes.to_vec())
                .map_err(|e| DbError::Codec(e.to_string()))?;
            let persy_id: PersyId = persy_id_str
                .parse()
                .map_err(|e| DbError::Codec(format!("Invalid PersyId: {}", e)))?;

            let val = tx
                .read(&table_name, &persy_id)
                .map_err(|e| DbError::Storage(e.to_string()))?
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
        let index_name = self.index_name.clone();
        spawn_blocking(move || -> DbResult<Vec<Option<Bytes>>> {
            let mut tx = db.begin().map_err(|e| DbError::Storage(e.to_string()))?;
            let mut out = Vec::with_capacity(keys.len());
            for k in keys {
                let key_bytes = ByteVec::new(k.to_vec());
                let mut iter = tx
                    .get::<ByteVec, ByteVec>(&index_name, &key_bytes)
                    .map_err(|e| DbError::Storage(e.to_string()))?;
                let val_opt = match iter.next() {
                    Some(persy_id_str_bytes) => {
                        let persy_id_str = String::from_utf8(persy_id_str_bytes.to_vec())
                            .map_err(|e| DbError::Codec(e.to_string()))?;
                        let persy_id: PersyId = persy_id_str
                            .parse()
                            .map_err(|e| DbError::Codec(format!("Invalid PersyId: {}", e)))?;
                        tx.read(&table_name, &persy_id)
                            .map_err(|e| DbError::Storage(e.to_string()))?
                            .map(|v| Bytes::copy_from_slice(&v))
                    }
                    None => None,
                };
                out.push(val_opt);
            }
            Ok(out)
        })
        .await
        .map_err(|e| DbError::Storage(format!("Tokio join error: {}", e)))?
    }

    async fn remove(&self, key: RecordKey) -> DbResult<bool> {
        let db = self.db.clone();
        let table_name = self.table_name.clone();
        let index_name = self.index_name.clone();

        spawn_blocking(move || -> DbResult<bool> {
            let mut tx = db.begin().map_err(|e| DbError::Storage(e.to_string()))?;
            let key_bytes = ByteVec::new(key.to_vec());

            let mut iter = tx
                .get::<ByteVec, ByteVec>(&index_name, &key_bytes)
                .map_err(|e| DbError::Storage(e.to_string()))?;

            if let Some(persy_id_str_bytes) = iter.next() {
                let persy_id_str = String::from_utf8(persy_id_str_bytes.to_vec())
                    .map_err(|e| DbError::Codec(e.to_string()))?;
                let persy_id: PersyId = persy_id_str
                    .parse()
                    .map_err(|e| DbError::Codec(format!("Invalid PersyId: {}", e)))?;

                tx.delete(&table_name, &persy_id)
                    .map_err(|e| DbError::Storage(e.to_string()))?;
                tx.remove(&index_name, key_bytes, None::<ByteVec>)
                    .map_err(|e| DbError::Storage(e.to_string()))?;

                tx.prepare()
                    .map_err(|e| DbError::Storage(e.to_string()))?
                    .commit()
                    .map_err(|e| DbError::Storage(e.to_string()))?;
                Ok(true)
            } else {
                Ok(false)
            }
        })
        .await
        .map_err(|e| DbError::Storage(format!("Tokio join error: {}", e)))?
    }

    /// Native atomic `transact` via a single Persy `Tx`.
    ///
    /// Mixed set + delete ops within one transaction, committed once.
    /// If any op fails, the transaction is implicitly dropped (not
    /// committed) — no partial state is observable.
    async fn transact(&self, ops: Vec<super::types::KvOp>) -> DbResult<()> {
        if ops.is_empty() {
            return Ok(());
        }
        let db = self.db.clone();
        let table_name = self.table_name.clone();
        let index_name = self.index_name.clone();
        spawn_blocking(move || -> DbResult<()> {
            let mut tx = db.begin().map_err(|e| DbError::Storage(e.to_string()))?;
            for op in ops {
                match op {
                    super::types::KvOp::Set(key, value) => {
                        let key_bytes = ByteVec::new(key.to_vec());
                        let mut iter = tx
                            .get::<ByteVec, ByteVec>(&index_name, &key_bytes)
                            .map_err(|e| DbError::Storage(e.to_string()))?;
                        if let Some(persy_id_str_bytes) = iter.next() {
                            // Existing record — update
                            let persy_id_str = String::from_utf8(persy_id_str_bytes.to_vec())
                                .map_err(|e| DbError::Codec(e.to_string()))?;
                            let persy_id: PersyId = persy_id_str
                                .parse()
                                .map_err(|e| DbError::Codec(format!("Invalid PersyId: {}", e)))?;
                            tx.update(&table_name, &persy_id, &value)
                                .map_err(|e| DbError::Storage(e.to_string()))?;
                        } else {
                            // New record — insert + index
                            let persy_id = tx
                                .insert(&table_name, &value)
                                .map_err(|e| DbError::Storage(e.to_string()))?;
                            let val = ByteVec::new(persy_id.to_string().into_bytes());
                            tx.put(&index_name, key_bytes, val)
                                .map_err(|e| DbError::Storage(e.to_string()))?;
                        }
                    }
                    super::types::KvOp::Remove(key) => {
                        let key_bytes = ByteVec::new(key.to_vec());
                        let mut iter = tx
                            .get::<ByteVec, ByteVec>(&index_name, &key_bytes)
                            .map_err(|e| DbError::Storage(e.to_string()))?;
                        if let Some(persy_id_str_bytes) = iter.next() {
                            let persy_id_str = String::from_utf8(persy_id_str_bytes.to_vec())
                                .map_err(|e| DbError::Codec(e.to_string()))?;
                            let persy_id: PersyId = persy_id_str
                                .parse()
                                .map_err(|e| DbError::Codec(format!("Invalid PersyId: {}", e)))?;
                            tx.delete(&table_name, &persy_id)
                                .map_err(|e| DbError::Storage(e.to_string()))?;
                            tx.remove(&index_name, key_bytes, None::<ByteVec>)
                                .map_err(|e| DbError::Storage(e.to_string()))?;
                        }
                    }
                }
            }
            tx.prepare()
                .map_err(|e| DbError::Storage(e.to_string()))?
                .commit()
                .map_err(|e| DbError::Storage(e.to_string()))?;
            Ok(())
        })
        .await
        .map_err(|e| DbError::Storage(format!("Tokio join error: {}", e)))?
    }

    /// Batched insert via ONE transaction containing N inserts +
    /// index puts, committed once. One fsync per batch instead of
    /// one per record — the entire reason persy's per-write cost
    /// (~2.95 ms/record on NTFS) was bottlenecked.
    async fn insert_many(&self, values: Vec<Bytes>) -> DbResult<Vec<RecordKey>> {
        if values.is_empty() {
            return Ok(Vec::new());
        }
        let db = self.db.clone();
        let table_name = self.table_name.clone();
        let index_name = self.index_name.clone();
        spawn_blocking(move || -> DbResult<Vec<RecordKey>> {
            let mut tx = db.begin().map_err(|e| DbError::Storage(e.to_string()))?;
            let mut ids = Vec::with_capacity(values.len());
            for value in values {
                let id = RecordId::new();
                let key = RecordKey::copy_from_slice(id.as_bytes());

                let persy_id = tx
                    .insert(&table_name, &value)
                    .map_err(|e| DbError::Storage(e.to_string()))?;

                let key_bytes_index = ByteVec::new(key.to_vec());
                let val = ByteVec::new(persy_id.to_string().into_bytes());
                tx.put(&index_name, key_bytes_index, val)
                    .map_err(|e| DbError::Storage(e.to_string()))?;
                ids.push(key);
            }
            tx.prepare()
                .map_err(|e| DbError::Storage(e.to_string()))?
                .commit()
                .map_err(|e| DbError::Storage(e.to_string()))?;
            Ok(ids)
        })
        .await
        .map_err(|e| DbError::Storage(format!("Tokio join error: {}", e)))?
    }

    /// Batched upsert. `bool` per item = created. Same one-fsync-
    /// per-batch story.
    async fn set_many(&self, items: Vec<(RecordKey, Bytes)>) -> DbResult<Vec<bool>> {
        if items.is_empty() {
            return Ok(Vec::new());
        }
        let db = self.db.clone();
        let table_name = self.table_name.clone();
        let index_name = self.index_name.clone();
        spawn_blocking(move || -> DbResult<Vec<bool>> {
            let mut tx = db.begin().map_err(|e| DbError::Storage(e.to_string()))?;
            let mut flags = Vec::with_capacity(items.len());
            for (key, value) in items {
                let key_bytes = ByteVec::new(key.to_vec());
                let mut iter = tx
                    .get::<ByteVec, ByteVec>(&index_name, &key_bytes)
                    .map_err(|e| DbError::Storage(e.to_string()))?;
                let created = if let Some(persy_id_str_bytes) = iter.next() {
                    let persy_id_str = String::from_utf8(persy_id_str_bytes.to_vec())
                        .map_err(|e| DbError::Codec(e.to_string()))?;
                    let persy_id: PersyId = persy_id_str
                        .parse()
                        .map_err(|e| DbError::Codec(format!("Invalid PersyId: {}", e)))?;
                    tx.update(&table_name, &persy_id, &value)
                        .map_err(|e| DbError::Storage(e.to_string()))?;
                    false
                } else {
                    let persy_id = tx
                        .insert(&table_name, &value)
                        .map_err(|e| DbError::Storage(e.to_string()))?;
                    let val = ByteVec::new(persy_id.to_string().into_bytes());
                    tx.put(&index_name, key_bytes, val)
                        .map_err(|e| DbError::Storage(e.to_string()))?;
                    true
                };
                flags.push(created);
            }
            tx.prepare()
                .map_err(|e| DbError::Storage(e.to_string()))?
                .commit()
                .map_err(|e| DbError::Storage(e.to_string()))?;
            Ok(flags)
        })
        .await
        .map_err(|e| DbError::Storage(format!("Tokio join error: {}", e)))?
    }

    /// Batched remove. `bool` per key = existed.
    async fn remove_many(&self, keys: Vec<RecordKey>) -> DbResult<Vec<bool>> {
        if keys.is_empty() {
            return Ok(Vec::new());
        }
        let db = self.db.clone();
        let table_name = self.table_name.clone();
        let index_name = self.index_name.clone();
        spawn_blocking(move || -> DbResult<Vec<bool>> {
            let mut tx = db.begin().map_err(|e| DbError::Storage(e.to_string()))?;
            let mut flags = Vec::with_capacity(keys.len());
            for key in keys {
                let key_bytes = ByteVec::new(key.to_vec());
                let mut iter = tx
                    .get::<ByteVec, ByteVec>(&index_name, &key_bytes)
                    .map_err(|e| DbError::Storage(e.to_string()))?;
                if let Some(persy_id_str_bytes) = iter.next() {
                    let persy_id_str = String::from_utf8(persy_id_str_bytes.to_vec())
                        .map_err(|e| DbError::Codec(e.to_string()))?;
                    let persy_id: PersyId = persy_id_str
                        .parse()
                        .map_err(|e| DbError::Codec(format!("Invalid PersyId: {}", e)))?;
                    tx.delete(&table_name, &persy_id)
                        .map_err(|e| DbError::Storage(e.to_string()))?;
                    tx.remove(&index_name, key_bytes, None::<ByteVec>)
                        .map_err(|e| DbError::Storage(e.to_string()))?;
                    flags.push(true);
                } else {
                    flags.push(false);
                }
            }
            tx.prepare()
                .map_err(|e| DbError::Storage(e.to_string()))?
                .commit()
                .map_err(|e| DbError::Storage(e.to_string()))?;
            Ok(flags)
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
        let index_name = self.index_name.clone();

        Box::pin(stream! {
            let mut mappings: Vec<(RecordKey, PersyId)> = Vec::new();
            let mut collected = false;

            loop {
                // First iteration: collect all mappings
                if !collected {
                    let db_clone = db.clone();
                    let index_name_clone = index_name.clone();

                    let collect_result: DbResult<Vec<_>> = spawn_blocking(move || {
                        let mut tx = db_clone.begin().map_err(|e| DbError::Storage(e.to_string()))?;
                        let mut result = Vec::new();

                        let index_iter = tx.range::<ByteVec, ByteVec, _>(&index_name_clone, ..)
                            .map_err(|e| DbError::Storage(e.to_string()))?;

                        for (key_bytes, mut val_iter) in index_iter {
                            let key = RecordKey::copy_from_slice(key_bytes.as_ref());

                            if let Some(val_bytes) = val_iter.next() {
                                let persy_id_str = String::from_utf8(val_bytes.to_vec())
                                    .map_err(|e| DbError::Codec(e.to_string()))?;
                                let persy_id: PersyId = persy_id_str.parse()
                                    .map_err(|e| DbError::Codec(format!("Invalid PersyId: {}", e)))?;
                                result.push((key, persy_id));
                            }
                        }

                        Ok(result)
                    })
                    .await
                    .map_err(|e| DbError::Storage(format!("Tokio join error: {}", e)))?;

                    mappings = collect_result?;
                    collected = true;
                }

                // Yield next batch
                if mappings.is_empty() {
                    break;
                }

                let end_idx = batch_size.min(mappings.len());
                let batch_mappings = mappings.drain(..end_idx).collect::<Vec<_>>();

                let db_clone = db.clone();
                let table_name_clone = table_name.clone();

                let batch_result: DbResult<Vec<_>> = spawn_blocking(move || {
                    let mut tx = db_clone.begin().map_err(|e| DbError::Storage(e.to_string()))?;
                    let mut out = Vec::new();

                    for (key, persy_id) in batch_mappings {
                        let content = tx.read(&table_name_clone, &persy_id)
                            .map_err(|e| DbError::Storage(e.to_string()))?
                            .ok_or_else(|| DbError::NotFound("PersyId not found".to_string()))?;
                        out.push((key, Bytes::copy_from_slice(&content)));
                    }

                    Ok(out)
                })
                .await
                .map_err(|e| DbError::Storage(format!("Tokio join error: {}", e)))?;

                let batch = batch_result?;

                if batch.is_empty() {
                    break;
                }

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
        let index_name = self.index_name.clone();

        Box::pin(stream! {
            let mut mappings: Vec<(RecordKey, PersyId)> = Vec::new();
            let mut collected = false;
            let mut mapping_offset = 0;

            // Calculate upper bound for prefix
            let mut prefix_end = prefix.to_vec();
            if let Some(last_byte) = prefix_end.last_mut() {
                *last_byte = last_byte.wrapping_add(1);
            }

            loop {
                // First iteration: collect all mappings
                if !collected {
                    let db_clone = db.clone();
                    let index_name_clone = index_name.clone();
                    let prefix_clone = prefix.clone();
                    let prefix_end_clone = prefix_end.clone();

                    let collect_result: DbResult<Vec<_>> = spawn_blocking(move || {
                        let mut tx = db_clone.begin().map_err(|e| DbError::Storage(e.to_string()))?;
                        let mut result = Vec::new();

                        let prefix_bv = ByteVec::new(prefix_clone.to_vec());
                        let prefix_end_bv = ByteVec::new(prefix_end_clone);

                        let index_iter = tx
                            .range::<ByteVec, ByteVec, _>(&index_name_clone, prefix_bv..prefix_end_bv)
                            .map_err(|e| DbError::Storage(e.to_string()))?;

                        for (key_bytes, mut val_iter) in index_iter {
                            let key = RecordKey::copy_from_slice(key_bytes.as_ref());

                            if let Some(val_bytes) = val_iter.next() {
                                let persy_id_str = String::from_utf8(val_bytes.to_vec())
                                    .map_err(|e| DbError::Codec(e.to_string()))?;
                                let persy_id: PersyId = persy_id_str.parse()
                                    .map_err(|e| DbError::Codec(format!("Invalid PersyId: {}", e)))?;
                                result.push((key, persy_id));
                            }
                        }

                        Ok(result)
                    })
                    .await
                    .map_err(|e| DbError::Storage(format!("Tokio join error: {}", e)))?;

                    mappings = collect_result?;
                    collected = true;
                }

                // Yield next batch
                if mapping_offset >= mappings.len() {
                    break;
                }

                let end_idx = (mapping_offset + batch_size).min(mappings.len());
                let batch_mappings = mappings[mapping_offset..end_idx].to_vec();
                mapping_offset = end_idx;

                let db_clone = db.clone();
                let table_name_clone = table_name.clone();

                let batch_result: DbResult<Vec<_>> = spawn_blocking(move || {
                    let mut tx = db_clone.begin().map_err(|e| DbError::Storage(e.to_string()))?;
                    let mut out = Vec::new();

                    for (key, persy_id) in batch_mappings {
                        let content = tx.read(&table_name_clone, &persy_id)
                            .map_err(|e| DbError::Storage(e.to_string()))?
                            .ok_or_else(|| DbError::NotFound("PersyId not found".to_string()))?;
                        out.push((key, Bytes::copy_from_slice(&content)));
                    }

                    Ok(out)
                })
                .await
                .map_err(|e| DbError::Storage(format!("Tokio join error: {}", e)))?;

                let batch = batch_result?;

                if batch.is_empty() {
                    break;
                }

                yield Ok(batch);
            }
        })
    }
}

// ============================================================================
// Tests
// ============================================================================
