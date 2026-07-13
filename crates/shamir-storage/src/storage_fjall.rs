use super::types::{KvOp, RecordKey, Repo, Store};
use crate::error::{DbError, DbResult};
use async_stream::stream;
use async_trait::async_trait;
use bytes::Bytes;
use fjall::{Database, Keyspace, KeyspaceCreateOptions, PersistMode};
use futures::stream::Stream;
use shamir_types::types::record_id::RecordId;
use std::path::Path;
use std::pin::Pin;
use std::sync::mpsc::{Receiver, SyncSender};
use std::sync::Arc;
use std::thread::JoinHandle;
use tokio::sync::oneshot;
use tokio::task;

// ============================================================================
// Write worker (task #536) — single dedicated OS thread per FjallStore that
// serializes point-writes with NO embedded read (insert / transact) submitted
// against this store.
//
// Why writes but NOT reads: fjall's `Keyspace::insert`/`remove` each acquire
// the per-Database journal-writer `Mutex` (`journal.get_writer()`) for the
// full duration of the op (verified against fjall 3.1.6
// `src/keyspace/mod.rs`), so every write in a Database ALREADY serializes on
// one mutex regardless of the calling thread — routing them onto one worker
// loses NO parallelism. Reads, by contrast, clone an immutable snapshot under
// a brief shared lock and then proceed lock-free, so many threads read a hot
// keyspace truly in parallel; funnelling reads through one worker would cap a
// hot table's read throughput at a single thread's serial rate. Reads
// therefore stay on the existing per-op `spawn_blocking` path, untouched.
// See `docs/design/fjall-worker-loop-524-findings.md` for the full analysis.
//
// Why `set`/`remove` are NOT routed here (measured, not assumed): both embed
// a `contains_key` READ (to derive the `Store` trait's `bool` "existed"
// return) before their write. Benched under 8×-cache-size cold data + 32-way
// fan-out (`benches/storage_fjall_write_worker.rs`): routing `set` through
// this worker regressed it ~1.45× (1,124,693 -> 1,630,982 ns/op) — the
// embedded cold `contains_key` reads, previously overlapped across the
// `spawn_blocking` pool, got serialized onto one thread, exactly the
// read-parallelism hazard `docs/design/fjall-worker-loop-524-findings.md`
// warned about, just resurfacing through the read embedded inside a write op
// instead of a bare read op. `insert` has no such embedded read (the id is
// freshly generated, no pre-check) and benched ~1.7× faster
// (1,113,187 -> 663,637 ns/op); `transact` shares that no-embedded-read shape.
// So `set`/`remove` stay on `spawn_blocking`, unchanged from before this task.
// ============================================================================

/// A unit of write work submitted to the [`WriteWorker`]. Each variant
/// carries the op parameters plus a oneshot sender the worker uses to return
/// the result to the awaiting async caller.
///
/// Deliberately does NOT include `set`/`remove` — see the module comment
/// above for the measured reason they stay on `spawn_blocking` instead.
enum WriteJob {
    /// `insert(value)` — generates a fresh key on the worker, returns it.
    Insert {
        value: Bytes,
        reply: oneshot::Sender<DbResult<RecordKey>>,
    },
    /// `transact(ops)` — atomic mixed-op batch.
    Transact {
        ops: Vec<KvOp>,
        reply: oneshot::Sender<DbResult<()>>,
    },
}

/// Owns the dedicated write thread and the channel into it. One per
/// [`FjallStore`]. On drop the sender is closed and the thread is joined so
/// the worker is reaped deterministically (important because the test suite
/// and DDL churn create/drop many stores).
struct WriteWorker {
    /// `Option` so `Drop` can close the channel (take → drop) BEFORE joining
    /// the thread; joining while the sender is still open would deadlock, as
    /// the worker's `recv()` never returns.
    tx: Option<SyncSender<WriteJob>>,
    handle: Option<JoinHandle<()>>,
}

impl WriteWorker {
    /// Spawn the worker thread. It owns one clone of the keyspace + database
    /// handle and drains jobs strictly one-at-a-time, so submitted writes
    /// execute in submission order.
    ///
    /// The channel is bounded (`sync_channel`) so a burst of submitters
    /// applies natural backpressure instead of letting an unbounded queue
    /// grow without limit — the depth is generous enough that steady-state
    /// submitters never block, but a pathological fan-out can't OOM the queue.
    fn spawn(keyspace: Keyspace, db: Arc<Database>) -> Self {
        // Bound chosen well above realistic in-flight write fan-out; a full
        // queue simply parks the submitting task until the worker drains one.
        let (tx, rx): (SyncSender<WriteJob>, Receiver<WriteJob>) =
            std::sync::mpsc::sync_channel(1024);

        let handle = std::thread::Builder::new()
            .name("fjall-write-worker".to_string())
            .spawn(move || Self::run(rx, keyspace, db))
            .expect("spawn fjall write worker thread");

        Self {
            tx: Some(tx),
            handle: Some(handle),
        }
    }

    /// The live sender. `tx` is only ever `None` transiently inside `Drop`,
    /// which cannot overlap a method call on the owning `FjallStore` (drop
    /// takes `&mut self` by value), so this never panics in practice.
    fn sender(&self) -> &SyncSender<WriteJob> {
        self.tx
            .as_ref()
            .expect("fjall write worker sender used after drop")
    }

    /// The drain loop. Blocks on `recv()`; exits when every `tx` is dropped
    /// (the owning `FjallStore` was dropped). Each job is executed
    /// synchronously and its result sent back on the job's oneshot. A dropped
    /// receiver (caller went away before the reply) is ignored.
    fn run(rx: Receiver<WriteJob>, keyspace: Keyspace, db: Arc<Database>) {
        while let Ok(job) = rx.recv() {
            match job {
                WriteJob::Insert { value, reply } => {
                    let _ = reply.send(exec_insert(&keyspace, value));
                }
                WriteJob::Transact { ops, reply } => {
                    let _ = reply.send(exec_transact(&db, &keyspace, ops));
                }
            }
        }
    }
}

impl Drop for WriteWorker {
    fn drop(&mut self) {
        // Close the channel FIRST (take + drop the sender) so the worker's
        // `recv()` returns `Err` and the drain loop exits, THEN join. Any jobs
        // still queued are drained before the loop sees the closed channel, so
        // in-flight writes complete and their oneshots fire — no lost replies.
        drop(self.tx.take());
        if let Some(handle) = self.handle.take() {
            // Best-effort join: reaps the thread promptly. A panic inside the
            // worker (shouldn't happen — every exec fn returns Result) would
            // surface here as an `Err`; we ignore it rather than double-panic
            // during unwind.
            let _ = handle.join();
        }
    }
}

/// Execute `insert` synchronously on the worker thread. Mirrors the exact
/// logic the old per-op `spawn_blocking` closure ran.
fn exec_insert(keyspace: &Keyspace, value: Bytes) -> DbResult<RecordKey> {
    // §1.2 (audit 2026-07-06-perf-radical-o-notation): no pre-insert
    // `contains_key` — `RecordId::new()` is a fresh random 128-bit id, so a
    // collision probe is provably pointless (~2⁻¹²⁸). See the trait doc.
    let id = RecordId::new();
    let key = RecordKey::from_slice(id.as_bytes());
    keyspace
        .insert(&key[..], &*value)
        .map_err(|e| DbError::Storage(e.to_string()))?;
    Ok(key)
}

/// Execute `transact` synchronously on the worker thread — native atomic
/// `OwnedWriteBatch` commit.
fn exec_transact(db: &Arc<Database>, keyspace: &Keyspace, ops: Vec<KvOp>) -> DbResult<()> {
    let mut batch = db.batch();
    for op in ops {
        match op {
            KvOp::Set(k, v) => batch.insert(keyspace, k.as_ref(), v.as_ref()),
            KvOp::Remove(k) => batch.remove(keyspace, k.as_ref()),
        }
    }
    batch
        .commit()
        .map_err(|e| DbError::Storage(format!("Fjall batch commit: {}", e)))?;
    Ok(())
}

/// Submit a job to the worker and await its oneshot reply.
///
/// `build` receives the reply-sender and returns the fully-constructed
/// [`WriteJob`]. Both failure modes — the bounded channel's `send` failing
/// (worker thread gone) and the oneshot being dropped without a reply — map
/// onto `DbError::Internal`, matching the old `spawn_blocking` path's
/// `JoinError → DbError::Internal` mapping so error shapes are unchanged.
///
/// `send` on a `SyncSender` blocks only when the bounded queue is full; the
/// worker drains it continuously, so under normal load this returns
/// immediately. Enqueuing is a cheap, non-blocking-in-practice synchronous op,
/// so it runs directly on the async caller — the whole point of the worker is
/// to REMOVE the per-op `spawn_blocking` hop, so we must not reintroduce one
/// just to enqueue.
async fn submit<T>(
    tx: &SyncSender<WriteJob>,
    build: impl FnOnce(oneshot::Sender<DbResult<T>>) -> WriteJob,
) -> DbResult<T> {
    let (reply_tx, reply_rx) = oneshot::channel();
    tx.send(build(reply_tx)).map_err(|_| {
        DbError::Internal("fjall write worker channel closed (worker thread gone)".to_string())
    })?;
    match reply_rx.await {
        Ok(result) => result,
        Err(_) => Err(DbError::Internal(
            "fjall write worker dropped reply (worker thread gone)".to_string(),
        )),
    }
}

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
            worker: std::sync::OnceLock::new(),
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
    /// Dedicated write thread (task #536), spawned LAZILY on first
    /// `insert`/`transact` submission — not eagerly in `store_get`. Point-
    /// writes with NO embedded read — `insert`, `transact` — are submitted
    /// here and executed one at a time in submission order. `set`/`remove`
    /// stay on `spawn_blocking` (their embedded `contains_key` read measured
    /// a net regression when routed through the worker); reads stay on
    /// `spawn_blocking` too. See the worker's module comment for the
    /// measured reasoning behind the split.
    ///
    /// Lazy spawn matters: `store_get` is called per-store, including for
    /// stores that never issue `insert`/`transact` (e.g. the engine's
    /// `__tx__` marker store, fetched fresh on every transaction commit via
    /// `RepoInstance::tx_info_store` — only `set`/`get`/`flush` traffic).
    /// Eagerly spawning a worker thread in `store_get` would spin up and
    /// then immediately tear down an OS thread on every commit for zero
    /// work — a real regression on the commit hot path an `@fl` review
    /// caught (the bench couldn't see it: it constructs each store once).
    /// `OnceLock` makes the spawn a one-time cost per store instance, paid
    /// only by callers that actually submit a job.
    worker: std::sync::OnceLock<WriteWorker>,
}

impl FjallStore {
    /// Lazily spawn (on first call) or return the already-spawned write
    /// worker for this store. See the `worker` field's doc comment for why
    /// this must be lazy rather than eager.
    fn worker(&self) -> &WriteWorker {
        self.worker
            .get_or_init(|| WriteWorker::spawn(self.keyspace.clone(), self.db.clone()))
    }
}

#[async_trait]
impl Store for FjallStore {
    async fn insert(&self, value: Bytes) -> DbResult<RecordKey> {
        // §1.2 (audit 2026-07-06-perf-radical-o-notation): no pre-insert
        // `contains_key` — `RecordId::new()` is a fresh random 128-bit id, so a
        // collision probe would be provably pointless (~2⁻¹²⁸). The actual
        // insert runs on the dedicated write worker (task #536), replacing the
        // former per-op `spawn_blocking` hop — see `exec_insert`.
        submit(self.worker().sender(), |reply| WriteJob::Insert {
            value,
            reply,
        })
        .await
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
            //
            // NOT routed through the write worker (task #536): benched under
            // 8×-cache-size cold data + 32-way fan-out
            // (`benches/storage_fjall_write_worker.rs`) at ~1.45× SLOWER
            // (1,124,693 -> 1,630,982 ns/op) — the embedded `contains_key`
            // read, previously overlapped across the `spawn_blocking` pool,
            // gets serialized onto one thread when routed through the
            // worker, reproducing the read-parallelism hazard
            // `docs/design/fjall-worker-loop-524-findings.md` warned about.
            // `insert`/`transact` have no such embedded read and DO route
            // through the worker (a measured ~1.7× win) — see the worker's
            // module comment.
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
    async fn transact(&self, ops: Vec<KvOp>) -> DbResult<()> {
        if ops.is_empty() {
            return Ok(());
        }
        // Routed through the write worker (task #536) so the atomic batch
        // commit is ordered against every other point-write on this store.
        submit(self.worker().sender(), |reply| WriteJob::Transact {
            ops,
            reply,
        })
        .await
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
            // variant is left as a follow-up, and why this is NOT routed
            // through the write worker (task #536) — the embedded
            // `contains_key` read measured a net regression there, same as
            // `set`.
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
