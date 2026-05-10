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
