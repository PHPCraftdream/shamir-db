use crate::error::{DbError, DbResult};
use async_trait::async_trait;
use bytes::Bytes;
use futures::stream::Stream;
#[cfg(test)]
use futures::stream::StreamExt;
use std::pin::Pin;
use std::sync::Arc;

pub type RecordKey = Bytes;

type RecordStream = Pin<Box<dyn Stream<Item = Result<Vec<(RecordKey, Bytes)>, DbError>> + Send>>;

/// Collect all records from a stream into a single vector.
/// Used to convert iter_stream() results to a flat Vec.
///
/// **DEPRECATED & FOR TESTS ONLY**
///
/// WARNING: Only use in tests! Can consume all memory on large datasets.
#[cfg(test)]
#[deprecated(since = "0.1.0", note = "FOR TESTS ONLY.")]
pub async fn collect_stream(stream: RecordStream) -> DbResult<Vec<(RecordKey, Bytes)>> {
    let mut all_records = Vec::new();
    let mut stream = stream;
    while let Some(batch_result) = stream.next().await {
        let batch = batch_result?;
        all_records.extend(batch);
    }
    Ok(all_records)
}

/// Backend-agnostic test coverage for the `insert_many`, `set_many`,
/// `remove_many`, and `flush` trait methods. Each backend test
/// invokes this helper to verify both the default-loop impl and any
/// native overrides behave identically.
///
/// WARNING: tests only.
#[cfg(test)]
pub async fn run_batch_store_tests(store: Arc<dyn Store>) {
    // ---- insert_many --------------------------------------------------
    let values: Vec<Bytes> = (0..5u8)
        .map(|i| Bytes::copy_from_slice(&[i, i + 1, i + 2]))
        .collect();
    let keys = store
        .insert_many(values.clone())
        .await
        .expect("insert_many");
    assert_eq!(keys.len(), 5);

    // Every returned key is readable and round-trips to its value.
    for (k, v) in keys.iter().zip(values.iter()) {
        let got = store.get(k.clone()).await.expect("get after insert_many");
        assert_eq!(got.as_ref(), v.as_ref());
    }
    // Keys are unique.
    let mut sorted = keys.clone();
    sorted.sort();
    sorted.dedup();
    assert_eq!(sorted.len(), 5, "insert_many returned duplicate keys");

    // Empty input must return empty output (no transaction, no fsync).
    let empty = store
        .insert_many(Vec::new())
        .await
        .expect("insert_many empty");
    assert!(empty.is_empty());

    // ---- set_many -----------------------------------------------------
    // Mix: update existing 2, create new 2.
    let new_id1 = shamir_types::types::record_id::RecordId::new();
    let new_id2 = shamir_types::types::record_id::RecordId::new();
    let new_k1 = Bytes::copy_from_slice(new_id1.as_bytes());
    let new_k2 = Bytes::copy_from_slice(new_id2.as_bytes());

    let items = vec![
        (keys[0].clone(), Bytes::from_static(b"updated-0")),
        (keys[1].clone(), Bytes::from_static(b"updated-1")),
        (new_k1.clone(), Bytes::from_static(b"fresh-1")),
        (new_k2.clone(), Bytes::from_static(b"fresh-2")),
    ];
    let flags = store.set_many(items).await.expect("set_many");
    assert_eq!(flags, vec![false, false, true, true]);

    // Values landed.
    assert_eq!(
        store.get(keys[0].clone()).await.unwrap().as_ref(),
        b"updated-0"
    );
    assert_eq!(
        store.get(new_k2.clone()).await.unwrap().as_ref(),
        b"fresh-2"
    );

    // Empty input.
    let empty_set = store.set_many(Vec::new()).await.expect("set_many empty");
    assert!(empty_set.is_empty());

    // ---- remove_many --------------------------------------------------
    let missing_id = shamir_types::types::record_id::RecordId::new();
    let missing_k = Bytes::copy_from_slice(missing_id.as_bytes());
    let to_remove = vec![keys[2].clone(), keys[3].clone(), missing_k];
    let remove_flags = store.remove_many(to_remove).await.expect("remove_many");
    assert_eq!(remove_flags, vec![true, true, false]);
    assert!(store.get(keys[2].clone()).await.is_err());
    assert!(store.get(keys[3].clone()).await.is_err());

    // Empty input.
    let empty_rm = store
        .remove_many(Vec::new())
        .await
        .expect("remove_many empty");
    assert!(empty_rm.is_empty());

    // ---- flush --------------------------------------------------------
    // Must succeed on any backend. After flush, prior writes remain
    // visible (consistency, not just durability).
    store.flush().await.expect("flush");
    assert_eq!(
        store.get(keys[0].clone()).await.unwrap().as_ref(),
        b"updated-0",
        "data lost across flush"
    );

    // ---- get_many -----------------------------------------------------
    // Mix of hits (the just-set keys) and a missing key. Result must
    // preserve input order: Some(bytes) per hit, None per miss.
    let missing_id = shamir_types::types::record_id::RecordId::new();
    let missing_k = Bytes::copy_from_slice(missing_id.as_bytes());
    let probe_keys = vec![
        keys[0].clone(),   // hit — was set to "updated-0"
        missing_k.clone(), // miss
        new_k1.clone(),    // hit — set to "fresh-1"
        new_k2.clone(),    // hit — set to "fresh-2"
    ];
    let got = store.get_many(probe_keys.clone()).await.expect("get_many");
    assert_eq!(got.len(), 4, "get_many length mismatch");
    assert_eq!(got[0].as_deref(), Some(&b"updated-0"[..]));
    assert_eq!(got[1], None, "missing key must be None");
    assert_eq!(got[2].as_deref(), Some(&b"fresh-1"[..]));
    assert_eq!(got[3].as_deref(), Some(&b"fresh-2"[..]));

    // Empty input → empty output, no I/O.
    let empty = store.get_many(Vec::new()).await.expect("get_many empty");
    assert!(empty.is_empty());

    // ---- iter_range_stream_reverse ------------------------------------
    // Insert a small set of records with predictable keys, then walk
    // them via the reverse stream and assert order is high → low.
    // Must work both for the default impl (in-memory / cached /
    // fjall / persy / canopy / nebari) and the native overrides on
    // sled / redb.
    let mut rev_keys: Vec<RecordKey> = (0u8..8)
        .map(|i| RecordKey::copy_from_slice(&[0xCC, i]))
        .collect();
    for (i, k) in rev_keys.iter().enumerate() {
        store
            .set(k.clone(), Bytes::copy_from_slice(&[i as u8]))
            .await
            .expect("seed reverse");
    }
    // Build range bounds covering exactly the seeded prefix.
    let lower = Bytes::copy_from_slice(&[0xCC, 0x00]);
    let upper = Bytes::copy_from_slice(&[0xCC, 0xFF]);
    let stream = store.iter_range_stream_reverse(Some(lower), Some(upper), 3);
    futures::pin_mut!(stream);
    let mut collected: Vec<RecordKey> = Vec::new();
    while let Some(batch) = stream.next().await {
        for (k, _) in batch.expect("reverse batch") {
            collected.push(k);
        }
    }
    assert_eq!(
        collected.len(),
        8,
        "iter_range_stream_reverse returned {} entries, expected 8",
        collected.len()
    );
    rev_keys.sort();
    rev_keys.reverse();
    assert_eq!(
        collected, rev_keys,
        "iter_range_stream_reverse did not yield keys in high→low order"
    );
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

    /// Returns an async stream that yields batches of records.
    /// Like PHP generators but with batching - yields Vec of size batch_size.
    /// Uses concurrent prefetching: while yielding current batch, fetches next batch in background.
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
}
