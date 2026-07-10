use super::types::{RecordKey, Repo, Store};
use crate::error::{DbError, DbResult};
use async_stream::stream;
use async_trait::async_trait;
use bytes::Bytes;
use fjall::{Database, Keyspace, KeyspaceCreateOptions, PersistMode};
use futures::stream::Stream;
use shamir_types::types::record_id::RecordId;
use std::path::Path;
use std::pin::Pin;
use std::sync::Arc;
use tokio::task;

// ============================================================================
// FjallRepo - manages database connection
// ============================================================================

pub struct FjallRepo {
    db: Arc<Database>,
}

impl FjallRepo {
    pub fn new(path: impl AsRef<Path>) -> DbResult<Self> {
        let db = Database::builder(path.as_ref())
            .open()
            .map_err(|e| DbError::Storage(e.to_string()))?;
        Ok(Self { db: Arc::new(db) })
    }
}

#[async_trait]
impl Repo for FjallRepo {
    async fn store_get<S: AsRef<str> + Send>(&self, name: S) -> DbResult<Arc<dyn Store>> {
        let db = self.db.clone();
        let table_name = name.as_ref().to_string();

        let keyspace = task::spawn_blocking(move || -> DbResult<Keyspace> {
            db.keyspace(&table_name, KeyspaceCreateOptions::default)
                .map_err(|e| DbError::Storage(e.to_string()))
        })
        .await
        .map_err(|e| DbError::Internal(e.to_string()))??;

        Ok(Arc::new(FjallStore {
            keyspace,
            db: self.db.clone(),
        }))
    }

    async fn store_delete<S: AsRef<str> + Send>(&self, name: S) -> DbResult<bool> {
        let db = self.db.clone();
        let table_name = name.as_ref().to_string();

        task::spawn_blocking(move || -> DbResult<bool> {
            let keyspace = db
                .keyspace(&table_name, KeyspaceCreateOptions::default)
                .map_err(|e| DbError::Storage(e.to_string()))?;

            db.delete_keyspace(keyspace)
                .map_err(|e| DbError::Storage(e.to_string()))?;
            Ok(true)
        })
        .await
        .map_err(|e| DbError::Internal(e.to_string()))?
    }

    async fn stores_list(&self) -> DbResult<Vec<String>> {
        let db = self.db.clone();
        task::spawn_blocking(move || -> DbResult<Vec<String>> {
            let names: Vec<String> = db
                .list_keyspace_names()
                .into_iter()
                .map(|s| s.to_string())
                .collect();
            Ok(names)
        })
        .await
        .map_err(|e| DbError::Internal(e.to_string()))?
    }
}

// ============================================================================
// FjallStore - individual store (keyspace)
// ============================================================================

pub struct FjallStore {
    keyspace: Keyspace,
    /// Kept alongside the keyspace so `Store::flush()` can call
    /// `Database::persist(PersistMode::SyncAll)` — fjall journals
    /// to a per-database WAL, not per-keyspace, so durability is
    /// the database's concern.
    db: Arc<Database>,
}

#[async_trait]
impl Store for FjallStore {
    async fn insert(&self, value: Bytes) -> DbResult<RecordKey> {
        let keyspace = self.keyspace.clone();

        task::spawn_blocking(move || -> DbResult<RecordKey> {
            // §1.2 (audit 2026-07-06-perf-radical-o-notation): the previous
            // `contains_key` check before `insert` was a full LSM point-lookup
            // (memtable → bloom → on-disk levels) that doubled the cost of
            // every point-write — and it is provably pointless here because
            // `RecordId::new()` returns a fresh random 128-bit id per call;
            // the collision probability across concurrent inserts is
            // negligible (~2⁻¹²⁸). No caller relies on `insert` erroring on
            // an already-existing key (the key is generated INSIDE this
            // method and never seen by the caller before), so the check is
            // pure overhead. fjall 3.x `Keyspace::insert` returns
            // `Result<(), Error>` with no prior-value, so there is no
            // `compare_and_swap` at this layer either — atomic check-and-insert
            // would require a transaction (extra round-trip per write,
            // regressing hot-path throughput). The check is removed entirely.
            let id = RecordId::new();
            let key = RecordKey::from_slice(id.as_bytes());

            keyspace
                .insert(&key[..], &*value)
                .map_err(|e| DbError::Storage(e.to_string()))?;

            Ok(key)
        })
        .await
        .map_err(|e| DbError::Internal(e.to_string()))?
    }

    async fn set(&self, key: RecordKey, value: Bytes) -> DbResult<bool> {
        let keyspace = self.keyspace.clone();
        task::spawn_blocking(move || -> DbResult<bool> {
            // §1.2 (audit 2026-07-06-perf-radical-o-notation): the
            // `contains_key` here doubles the LSM point-lookup cost of every
            // `set`. The flag is needed by the `Store` trait contract
            // (`set` returns `bool` = "was created") and several callers
            // consume it (engine's `delete_returning_version`, CachedStore's
            // size tracking, the storage fjall tests). fjall 3.x
            // `Keyspace::insert` returns `Result<(), Error>` — no prior-value
            // — so `existed` CANNOT be derived from the write op itself; the
            // separate lookup is the only way to honor the contract here.
            //
            // The engine layer already does its own existence check via
            // `self.get(id).await.ok()` before calling through (see
            // `table_manager_crud.rs::delete_returning_version`), so the
            // storage-side flag is technically redundant for the engine — but
            // removing the trait's `bool` return would be a cross-workspace
            // API change, out of scope for this surgical perf fix. A flag-free
            // fast-path variant on `Store` is the proper follow-up.
            //
            // §B13 (acknowledged TOCTOU): the engine never issues two
            // concurrent `set` calls for the same `RecordKey` on a single
            // table (writes are serialised through `TableManager` dispatch),
            // so the `existed` flag stays consistent with the actual write
            // under normal use. Concurrent calls from outside the engine
            // (e.g. tooling) would race; documented here so the contract is
            // explicit.
            let existed = keyspace
                .contains_key(&key[..])
                .map_err(|e| DbError::Storage(e.to_string()))?;

            keyspace
                .insert(&key[..], &*value)
                .map_err(|e| DbError::Storage(e.to_string()))?;

            Ok(!existed)
        })
        .await
        .map_err(|e| DbError::Internal(e.to_string()))?
    }

    async fn get(&self, key: RecordKey) -> DbResult<Bytes> {
        let keyspace = self.keyspace.clone();
        task::spawn_blocking(move || -> DbResult<Bytes> {
            match keyspace
                .get(&key[..])
                .map_err(|e| DbError::Storage(e.to_string()))?
            {
                // §1.1 (audit 2026-07-06-perf-radical-o-notation): with the
                // fjall `bytes_1` feature on, `Slice` IS a `bytes::Bytes`
                // under the hood and `From<Slice> for Bytes` is a true
                // zero-copy move (just unwraps the inner `Bytes` — both are
                // refcounted byte buffers). The previous `copy_from_slice`
                // did a full memcpy + alloc per point-read.
                Some(slice) => Ok(Bytes::from(slice)),
                None => Err(DbError::NotFound(format!("record not found: {:?}", key))),
            }
        })
        .await
        .map_err(|e| DbError::Internal(e.to_string()))?
    }

    /// Reverse range scan using fjall's `keyspace.range(...)` which
    /// implements `DoubleEndedIterator` — so `.rev()` walks the
    /// LSM tree backwards natively, no in-memory collect.
    /// Replaces the default `collect-forward + reverse` impl.
    fn iter_range_stream_reverse(
        &self,
        start_inclusive: Option<Bytes>,
        end_inclusive: Option<Bytes>,
        batch_size: usize,
    ) -> Pin<Box<dyn Stream<Item = Result<Vec<(RecordKey, Bytes)>, DbError>> + Send>> {
        let keyspace = self.keyspace.clone();
        let start_bytes = start_inclusive.map(|b| b.to_vec());
        let end_bytes = end_inclusive.map(|b| b.to_vec());

        Box::pin(stream! {
            // Cursor walks downward; upper bound shrinks each batch.
            let mut cursor: Option<Vec<u8>> = None;

            loop {
                let keyspace_clone = keyspace.clone();
                let cur = cursor.clone();
                let lower_init = start_bytes.clone();
                let upper_init = end_bytes.clone();

                let batch: DbResult<Vec<(RecordKey, Bytes)>> = task::spawn_blocking(move || {
                    use std::ops::Bound;
                    let lower: Bound<Vec<u8>> = match &lower_init {
                        Some(s) => Bound::Included(s.clone()),
                        None => Bound::Unbounded,
                    };
                    let upper: Bound<Vec<u8>> = match (&cur, &upper_init) {
                        (Some(c), _) => Bound::Excluded(c.clone()),
                        (None, Some(e)) => Bound::Included(e.clone()),
                        (None, None) => Bound::Unbounded,
                    };

                    let mut items = Vec::new();
                    for guard in keyspace_clone.range((lower, upper)).rev().take(batch_size) {
                        let (key, val) = guard
                            .into_inner()
                            .map_err(|e| DbError::Storage(e.to_string()))?;
                        // §1.1: zero-copy conversion (see `get`).
                        items.push((RecordKey::from(Bytes::from(key)), Bytes::from(val)));
                    }
                    Ok(items)
                })
                .await
                .map_err(|e| DbError::Internal(e.to_string()))?;

                let batch = batch?;
                if batch.is_empty() {
                    break;
                }
                cursor = batch.last().map(|(k, _)| k.to_vec());
                yield Ok(batch);
            }
        })
    }

    /// Native atomic `transact` via fjall `OwnedWriteBatch`.
    ///
    /// `Database::batch()` returns an `OwnedWriteBatch` that collects
    /// insert/remove ops across keyspaces. `commit()` applies them
    /// atomically — all succeed or none are visible.
    async fn transact(&self, ops: Vec<super::types::KvOp>) -> DbResult<()> {
        if ops.is_empty() {
            return Ok(());
        }
        let db = self.db.clone();
        let keyspace = self.keyspace.clone();
        task::spawn_blocking(move || -> DbResult<()> {
            let mut batch = db.batch();
            for op in ops {
                match op {
                    super::types::KvOp::Set(k, v) => {
                        batch.insert(&keyspace, k.as_ref(), v.as_ref());
                    }
                    super::types::KvOp::Remove(k) => {
                        batch.remove(&keyspace, k.as_ref());
                    }
                }
            }
            batch
                .commit()
                .map_err(|e| DbError::Storage(format!("Fjall batch commit: {}", e)))?;
            Ok(())
        })
        .await
        .map_err(|e| DbError::Internal(e.to_string()))?
    }

    /// Force the WAL to fsync-on-disk. fjall buffers individual
    /// writes in the journal; `persist(SyncAll)` fsyncs the journal
    /// + writes any pending metadata. Reachable through
    /// `Arc<dyn Store>` — without this override callers hitting
    /// the default no-op would silently get "eventually durable"
    /// even after an explicit `flush()`.
    async fn flush(&self) -> DbResult<()> {
        let db = self.db.clone();
        task::spawn_blocking(move || -> DbResult<()> {
            db.persist(PersistMode::SyncAll)
                .map_err(|e| DbError::Storage(format!("Fjall persist: {}", e)))?;
            Ok(())
        })
        .await
        .map_err(|e| DbError::Internal(e.to_string()))?
    }

    async fn get_many(&self, keys: Vec<RecordKey>) -> DbResult<Vec<Option<Bytes>>> {
        if keys.is_empty() {
            return Ok(Vec::new());
        }
        let keyspace = self.keyspace.clone();
        task::spawn_blocking(move || -> DbResult<Vec<Option<Bytes>>> {
            let mut out = Vec::with_capacity(keys.len());
            for k in keys {
                match keyspace
                    .get(&k[..])
                    .map_err(|e| DbError::Storage(e.to_string()))?
                {
                    // §1.1: zero-copy conversion (see `get`).
                    Some(slice) => out.push(Some(Bytes::from(slice))),
                    None => out.push(None),
                }
            }
            Ok(out)
        })
        .await
        .map_err(|e| DbError::Internal(e.to_string()))?
    }

    async fn remove(&self, key: RecordKey) -> DbResult<bool> {
        let keyspace = self.keyspace.clone();
        task::spawn_blocking(move || -> DbResult<bool> {
            // §1.2 (audit 2026-07-06-perf-radical-o-notation): same shape as
            // `set` — the `contains_key` doubles the LSM point-lookup cost.
            // The flag is required by the `Store` trait contract (`remove`
            // returns `bool` = "existed and was removed") and consumed by
            // callers (engine's `delete_returning_version`, the storage
            // fjall tests). fjall 3.x `Keyspace::remove` returns
            // `Result<(), Error>` — no prior-value — so `existed` cannot be
            // derived from the tombstone write itself. See `set`'s comment
            // for the full rationale on why the trait-surface fast-path
            // variant is left as a follow-up.
            let existed = keyspace
                .contains_key(&key[..])
                .map_err(|e| DbError::Storage(e.to_string()))?;

            if existed {
                keyspace
                    .remove(&key[..])
                    .map_err(|e| DbError::Storage(e.to_string()))?;
            }

            Ok(existed)
        })
        .await
        .map_err(|e| DbError::Internal(e.to_string()))?
    }

    fn iter_stream(
        &self,
        batch_size: usize,
    ) -> Pin<Box<dyn Stream<Item = Result<Vec<(RecordKey, Bytes)>, DbError>> + Send>> {
        let keyspace = self.keyspace.clone();

        Box::pin(stream! {
            let mut last_key: Option<Vec<u8>> = None;

            loop {
                let keyspace_clone = keyspace.clone();
                let start_key = last_key;

                let batch: DbResult<(Vec<_>, Option<Vec<u8>>)> = task::spawn_blocking(move || {
                    use std::ops::Bound;
                    let lower: Bound<Vec<u8>> = match &start_key {
                        Some(c) => Bound::Excluded(c.clone()),
                        None => Bound::Unbounded,
                    };
                    let upper: Bound<Vec<u8>> = Bound::Unbounded;

                    let mut items = Vec::with_capacity(256);
                    let mut last_batch_key: Option<Vec<u8>> = None;

                    for guard in keyspace_clone.range((lower, upper)).take(batch_size) {
                        let (key, value_slice) = guard
                            .into_inner()
                            .map_err(|e| DbError::Storage(e.to_string()))?;

                        last_batch_key = Some(key.to_vec());
                        // §1.1: zero-copy conversion for both key and value
                        // (see `get`).
                        items.push((RecordKey::from(Bytes::from(key)), Bytes::from(value_slice)));
                    }

                    Ok((items, last_batch_key))
                })
                .await
                .map_err(|e| DbError::Internal(e.to_string()))?;

                let (batch, next_key) = batch?;

                if batch.is_empty() {
                    break;
                }

                last_key = next_key;
                yield Ok(batch);
            }
        })
    }

    /// Prefix scan via `keyspace.range` — O(log N + M) per batch.
    ///
    /// Replaces the old O(N) full-iter + linear cursor re-seek that scanned
    /// every key on every batch call. Fjall's `range` seeks directly to the
    /// first matching key using the LSM-tree index, then `take_while` stops
    /// at the first key that no longer starts with the prefix. Subsequent
    /// batches use `Bound::Excluded(last_key)` to resume at exactly the right
    /// position — same pattern as `iter_stream` above (lines ~323).
    fn scan_prefix_stream(
        &self,
        prefix: Bytes,
        batch_size: usize,
    ) -> Pin<Box<dyn Stream<Item = Result<Vec<(RecordKey, Bytes)>, DbError>> + Send>> {
        let keyspace = self.keyspace.clone();

        Box::pin(stream! {
            let mut last_key: Option<Vec<u8>> = None;
            let prefix_vec = prefix.to_vec();

            loop {
                let keyspace_clone = keyspace.clone();
                let cur_last = last_key;
                let pfx = prefix_vec.clone();

                let batch: DbResult<(Vec<_>, Option<Vec<u8>>)> = task::spawn_blocking(move || {
                    use std::ops::Bound;

                    // Seek directly to the prefix boundary (or just past the cursor).
                    let lower: Bound<Vec<u8>> = match cur_last {
                        Some(ref c) => Bound::Excluded(c.clone()),
                        None => Bound::Included(pfx.clone()),
                    };
                    let upper: Bound<Vec<u8>> = Bound::Unbounded;

                    let mut items = Vec::with_capacity(256);
                    let mut last_batch_key: Option<Vec<u8>> = None;

                    // Range-seek + prefix boundary: take up to batch_size entries
                    // that still start with `pfx`. The `take(batch_size)` bounds
                    // the per-batch cost; the explicit prefix check terminates the
                    // scan once we've passed the prefix range (fjall yields lex order).
                    'batch: for guard in keyspace_clone.range((lower, upper)).take(batch_size) {
                        let (key, value_slice) = guard
                            .into_inner()
                            .map_err(|e| DbError::Storage(e.to_string()))?;

                        if !key.starts_with(&pfx) {
                            // We've exited the prefix range — stop this batch.
                            // The outer loop will also break because next batch
                            // would start at last_batch_key (already past prefix).
                            // We use a sentinel: push nothing, end loop.
                            break 'batch;
                        }

                        last_batch_key = Some(key.to_vec());
                        // §1.1: zero-copy conversion for both key and value
                        // (see `get`).
                        items.push((RecordKey::from(Bytes::from(key)), Bytes::from(value_slice)));
                    }

                    Ok((items, last_batch_key))
                })
                .await
                .map_err(|e| DbError::Internal(e.to_string()))?;

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
