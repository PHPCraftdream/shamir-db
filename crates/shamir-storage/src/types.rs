use crate::error::{DbError, DbResult};
use crate::key_bytes::KeyBytes;
use async_trait::async_trait;
use bytes::Bytes;
use futures::stream::Stream;
use std::pin::Pin;
use std::sync::Arc;

pub type RecordKey = KeyBytes;

pub(crate) type RecordStream =
    Pin<Box<dyn Stream<Item = Result<Vec<(RecordKey, Bytes)>, DbError>> + Send>>;

/// A single mixed-op for `Store::transact`. Represents either a
/// `set` or a `remove` against a `RecordKey`.
///
/// Used by the transactional engine layer to bundle heterogeneous
/// writes into one atomic batch that backends with native write
/// transactions can commit as a single unit.
#[derive(Debug, Clone)]
pub enum KvOp {
    Set(RecordKey, Bytes),
    Remove(RecordKey),
}

/// An asynchronous, key-value store trait that operates on raw bytes.
///
/// This trait provides a low-level storage abstraction. It is the responsibility
/// of the caller to handle serialization and deserialization of the values.
/// The key is fixed to `RecordKey`.
#[async_trait]
pub trait Store: Send + Sync {
    /// Inserts a new record with a generated `RecordKey`.
    async fn insert(&self, value: Bytes) -> DbResult<RecordKey>;

    /// Creates or updates a record for a given `RecordKey`.
    /// Returns `true` if the record was created, `false` if it was updated.
    async fn set(&self, key: RecordKey, value: Bytes) -> DbResult<bool>;

    /// Retrieves a record's raw bytes by its `RecordKey`.
    async fn get(&self, key: RecordKey) -> DbResult<Bytes>;

    /// Vectored read — fetch many records in one logical call.
    ///
    /// Returns a vector parallel to `keys`: `Some(bytes)` for hits,
    /// `None` for keys that are not present (a missing key is NOT
    /// a `DbError::NotFound` — the caller decides per-key). Other
    /// storage-level errors fail the whole batch.
    ///
    /// Default impl loops over `self.get`, mapping NotFound to None.
    /// Disk backends override with a single transactional read to
    /// collapse N×`spawn_blocking` + N transaction setups into one.
    async fn get_many(&self, keys: Vec<RecordKey>) -> DbResult<Vec<Option<Bytes>>> {
        let mut out = Vec::with_capacity(keys.len());
        for k in keys {
            match self.get(k).await {
                Ok(b) => out.push(Some(b)),
                Err(DbError::NotFound(_)) => out.push(None),
                Err(e) => return Err(e),
            }
        }
        Ok(out)
    }

    /// Removes a record by its `RecordKey`.
    async fn remove(&self, key: RecordKey) -> DbResult<bool>;

    /// Force the backend to make all writes-so-far durable.
    ///
    /// **Durability contract.** The basic `insert` / `set` / `remove`
    /// operations are *eventually* durable — backends are free to
    /// buffer writes in a WAL / page cache / background flusher and
    /// only fsync periodically. Callers that need a strict
    /// commit-boundary (an RPC reply, a transaction end, an explicit
    /// `FLUSH` request from a user) call `flush()` and await it.
    ///
    /// Default impl is a no-op for backends that have no buffered
    /// state (in-memory) or that already fsync per write at a lower
    /// layer.
    ///
    /// Backends that buffer (sled, fjall, cached) MUST override.
    async fn flush(&self) -> DbResult<()> {
        Ok(())
    }

    /// Apply a hot-reload of MemBuffer config. Default no-op
    /// (raw backends don't have a buffer layer). `MemBufferStore`
    /// overrides this to update its atomic config fields; wrappers
    /// like `CachedStore` should override to propagate to inner.
    ///
    /// Lives on the `Store` trait so the engine can call it on
    /// `Arc<dyn Store>` without downcasting — most backends ignore
    /// the call, the MemBuffer in the stack picks it up.
    async fn apply_buffer_config(
        &self,
        _config: &crate::storage_membuffer::MemBufferConfig,
    ) -> DbResult<()> {
        Ok(())
    }

    /// Return the unwrapped underlying backend, bypassing any wrapper
    /// layers (MemBuffer, Cached).
    ///
    /// Default: `None` — this Store is already raw. Wrappers override
    /// and return `Some(inner.clone())`.
    ///
    /// Used by the upcoming MvccStore at construction time to obtain
    /// a write path that bypasses write-back caches: tx commits go
    /// straight to the durable backend because durability IS the commit
    /// point.
    ///
    /// Note: returns `Option<Arc<dyn Store>>` rather than always-Some
    /// so non-wrapper backends can answer cheaply (a single None) — the
    /// caller iterates [`fully_unwrap_store`] only when it needs the
    /// chain-unwrapped raw store.
    async fn raw_backend(&self) -> Option<Arc<dyn Store>> {
        None
    }

    /// Insert many records in one logical batch — for backends that
    /// expose a transactional write API, this collapses N×fsync into
    /// one. For backends that already amortise durability per write
    /// (sled with `flush()`, redb with `Durability::None`), the
    /// win is smaller — and falling through to the default loop is
    /// fine.
    ///
    /// **Atomicity.** When a backend overrides this with a
    /// transactional impl, the batch is all-or-nothing. The default
    /// loop impl is NOT atomic — it inherits per-element semantics.
    /// Callers that need atomicity should not rely on the default.
    ///
    /// Returns one `RecordKey` per input value, in input order.
    async fn insert_many(&self, values: Vec<Bytes>) -> DbResult<Vec<RecordKey>> {
        let mut out = Vec::with_capacity(values.len());
        for v in values {
            out.push(self.insert(v).await?);
        }
        Ok(out)
    }

    /// Upsert many records in one logical batch. Same atomicity story
    /// as `insert_many`. Returns one `bool` per input pair, in input
    /// order — `true` if the record was created, `false` if it was
    /// updated.
    async fn set_many(&self, items: Vec<(RecordKey, Bytes)>) -> DbResult<Vec<bool>> {
        let mut out = Vec::with_capacity(items.len());
        for (k, v) in items {
            out.push(self.set(k, v).await?);
        }
        Ok(out)
    }

    /// Remove many records in one logical batch. Same atomicity story
    /// as `insert_many`. Returns one `bool` per input key, in input
    /// order — `true` if the record existed and was removed.
    async fn remove_many(&self, keys: Vec<RecordKey>) -> DbResult<Vec<bool>> {
        let mut out = Vec::with_capacity(keys.len());
        for k in keys {
            out.push(self.remove(k).await?);
        }
        Ok(out)
    }

    /// Atomic mixed-op batch — either ALL ops succeed and are visible
    /// to subsequent reads, or NONE are. The default impl applies ops
    /// sequentially and is **NOT atomic** — disk backends with a
    /// native write-transaction API (redb, sled, fjall, persy, nebari,
    /// canopy) override this to give the real atomicity guarantee.
    ///
    /// Used by the transactional engine layer to bundle a tx commit
    /// into one apply (data + index postings + counter updates). Empty
    /// `ops` is a no-op.
    ///
    /// **Atomicity contract.** When a backend overrides this with a
    /// transactional impl, partial state is never observable. The
    /// default loop impl below is per-op atomic only — callers that
    /// need true cross-op atomicity must verify their backend overrides
    /// `transact`.
    async fn transact(&self, ops: Vec<KvOp>) -> DbResult<()> {
        for op in ops {
            match op {
                KvOp::Set(k, v) => {
                    let _ = self.set(k, v).await?;
                }
                KvOp::Remove(k) => {
                    let _ = self.remove(k).await?;
                }
            }
        }
        Ok(())
    }

    /// Returns an async stream that yields batches of records.
    /// Like PHP generators but with batching - yields Vec of size batch_size.
    /// Uses concurrent prefetching: while yielding current batch, fetches next batch in background.
    ///
    /// # Ordering guarantee
    ///
    /// Keys within and across batches are yielded in ascending
    /// lexicographic byte order — the SAME guarantee documented on
    /// [`Store::scan_prefix_stream`], every implementor MUST uphold it.
    /// Callers rely on this for correctness, not just performance —
    /// `storage_membuffer.rs`'s merge-overlay scans (task #530) do a
    /// linear 2-way sorted merge of the dirty overlay against this
    /// stream and would silently resurrect a tombstoned or stale key if
    /// an implementor ever yielded out of order.
    ///
    /// # Arguments
    /// * `batch_size` - Number of records per batch
    ///
    /// # Returns
    /// Stream that yields DbResult<Vec<(RecordKey, Bytes)>> - each batch has exactly batch_size items
    /// (except possibly the last batch which may be smaller)
    fn iter_stream(&self, batch_size: usize) -> RecordStream;

    /// Returns an async stream that yields batches of records with keys starting with the given prefix.
    ///
    /// Like `iter_stream()` but filtered by prefix. More efficient than loading all results into memory.
    ///
    /// # Ordering guarantee
    ///
    /// Keys within and across batches are yielded in ascending
    /// lexicographic byte order, and each batch resumes strictly past
    /// the previous batch's last key (`Bound::Excluded`) — every
    /// implementor MUST uphold this (see `storage_in_memory.rs`,
    /// `storage_fjall.rs`, `storage_cached.rs`, `storage_membuffer.rs`
    /// for the reference pattern). Callers rely on this for
    /// correctness, not just performance — e.g.
    /// `IndexManager::lookup_by_index`'s posting-list cache
    /// (`Arc<[RecordId]>`, audit 3.2 / task #499) depends on the scan
    /// already being sorted and duplicate-free instead of re-sorting
    /// via an intermediate `BTreeSet`.
    ///
    /// # Arguments
    /// * `prefix` - The prefix to search for
    /// * `batch_size` - Number of records per batch
    ///
    /// # Returns
    /// Stream that yields batches of matching records
    fn scan_prefix_stream(&self, prefix: Bytes, batch_size: usize) -> RecordStream;

    /// Returns an async stream of records whose keys fall in
    /// `[start_inclusive ..= end_inclusive]` (lexicographic byte
    /// order). `None` on either side = unbounded.
    ///
    /// Used by sorted-index range / order / min queries. Disk-backed
    /// backends (redb, sled, fjall, persy, nebari, canopy) override
    /// this with their native `range()` API → genuine
    /// `O(log N + K)`. The default impl below leans on
    /// `iter_stream` + filter — correct everywhere but
    /// `O(N)` (used as fallback for `in_memory` / `cached` whose
    /// hash-keyed storage can't seek).
    ///
    /// # Arguments
    /// * `start_inclusive` — lower bound. `None` = scan from the start.
    /// * `end_inclusive` — upper bound. `None` = scan to the end.
    /// * `batch_size` — number of records per yielded batch.
    fn iter_range_stream(
        &self,
        start_inclusive: Option<Bytes>,
        end_inclusive: Option<Bytes>,
        batch_size: usize,
    ) -> RecordStream {
        // Default impl: full scan + in-stream filter. Correct for
        // any backend that has `iter_stream`. Native impls below
        // make it O(log N + K) by seeking into the B-tree.
        let inner = self.iter_stream(batch_size);
        Box::pin(default_range_filter(inner, start_inclusive, end_inclusive))
    }

    /// Same as `iter_range_stream`, but walks the range in
    /// REVERSE byte order — high → low. Used by reverse-iter
    /// sorted-index reads: `lookup_last_k`, `lookup_max`, and
    /// `ORDER BY field DESC LIMIT K`.
    ///
    /// Default impl: collect the forward-range stream and reverse
    /// it in memory. Correct for any backend, but O(N) memory.
    /// Disk B-tree backends (sled, redb, …) override with a true
    /// native reverse cursor.
    fn iter_range_stream_reverse(
        &self,
        start_inclusive: Option<Bytes>,
        end_inclusive: Option<Bytes>,
        batch_size: usize,
    ) -> RecordStream {
        let inner = self.iter_range_stream(start_inclusive, end_inclusive, batch_size);
        Box::pin(default_reverse(inner, batch_size))
    }
}

/// Default reverse wrapper: drains the inner stream into memory,
/// reverses, and re-emits in `batch_size` chunks. Memory ≈ N items
/// — fine for in-memory backend (small datasets, no I/O latency
/// pressure); disk backends override with a native reverse cursor.
fn default_reverse(
    mut inner: RecordStream,
    batch_size: usize,
) -> impl Stream<Item = Result<Vec<(RecordKey, Bytes)>, DbError>> + Send {
    use futures::StreamExt;
    async_stream::stream! {
        let mut all: Vec<(RecordKey, Bytes)> = Vec::new();
        while let Some(batch) = inner.next().await {
            let batch = batch?;
            all.extend(batch);
        }
        all.reverse();
        while !all.is_empty() {
            let take = batch_size.min(all.len());
            // `drain(0..take)` after a reverse drains the rear of the
            // pre-reversed vec, but we want high→low order from the
            // reversed front. So drain prefix.
            let batch: Vec<_> = all.drain(..take).collect();
            yield Ok(batch);
        }
    }
}

/// Default range-filter wrapper around an existing iter stream.
///
/// Filters each yielded batch element-by-element against the
/// `[start..=end]` window and yields filtered batches as soon as we
/// have any (preserving the stream's batching semantics).
fn default_range_filter(
    mut inner: RecordStream,
    start: Option<Bytes>,
    end: Option<Bytes>,
) -> impl Stream<Item = Result<Vec<(RecordKey, Bytes)>, DbError>> + Send {
    use futures::StreamExt;
    async_stream::stream! {
        while let Some(batch) = inner.next().await {
            let batch = batch?;
            let filtered: Vec<(RecordKey, Bytes)> = batch
                .into_iter()
                .filter(|(k, _)| {
                    let after_start = match &start {
                        Some(s) => k.as_ref() >= s.as_ref(),
                        None => true,
                    };
                    let before_end = match &end {
                        Some(e) => k.as_ref() <= e.as_ref(),
                        None => true,
                    };
                    after_start && before_end
                })
                .collect();
            if !filtered.is_empty() {
                yield Ok(filtered);
            }
        }
    }
}

/// Walk the `raw_backend` chain until we hit a backend that isn't
/// a wrapper. Useful when an unwrapper (MvccStore, GcWorker) needs
/// a write path that no buffer / cache layer intercepts.
///
/// Stops at the first `None` — usually one or two hops.
pub async fn fully_unwrap_store(store: &Arc<dyn Store>) -> Arc<dyn Store> {
    let mut cur = Arc::clone(store);
    while let Some(inner) = cur.raw_backend().await {
        cur = inner;
    }
    cur
}

/// A trait for a repository that can manage multiple `Store` instances.
#[async_trait]
pub trait Repo: Send + Sync {
    /// Retrieves a store by name. Creates it if it doesn't exist.
    async fn store_get<S>(&self, name: S) -> DbResult<Arc<dyn Store>>
    where
        S: AsRef<str> + Send;

    /// Deletes an entire store by name.
    async fn store_delete<S: AsRef<str> + Send>(&self, name: S) -> DbResult<bool>;

    /// Lists all stores in the repository.
    async fn stores_list(&self) -> DbResult<Vec<String>>;

    /// Copy every key/value from store `from` into store `to`.
    ///
    /// Used by `RENAME TABLE`: a table's physical data lives in three
    /// name-keyed stores (`__data__<t>`, `__info__<t>`, `__history__<t>`)
    /// and the storage layer has no native store-rename primitive, so
    /// rename is implemented as copy-then-orphan (the old store is left
    /// in place — same disposition as `DROP TABLE`, which intentionally
    /// orphans `__data__` since the catalogue is the source of truth).
    ///
    /// Default impl streams the source in batches of 256 and calls
    /// `set_many` on the destination. Backends with a native copy/rename
    /// primitive may override.
    async fn copy_store(&self, from: &str, to: &str) -> DbResult<()> {
        use futures::StreamExt;

        const BATCH: usize = 256;
        let src = self.store_get(from).await?;
        let dst = self.store_get(to).await?;
        let mut stream = src.iter_stream(BATCH);
        while let Some(batch) = stream.next().await {
            let batch = batch?;
            if batch.is_empty() {
                continue;
            }
            dst.set_many(batch).await?;
        }
        Ok(())
    }
}
