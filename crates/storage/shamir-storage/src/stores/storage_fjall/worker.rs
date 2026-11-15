use crate::error::{DbError, DbResult};
use crate::types::{KvOp, RecordKey};
use bytes::Bytes;
use fjall::{Database, Keyspace};
use shamir_types::types::record_id::RecordId;
use std::io;
use std::sync::mpsc::{Receiver, SyncSender};
use std::sync::Arc;
use std::thread::JoinHandle;
use tokio::sync::oneshot;
use tokio::task;

#[cfg(test)]
use super::write_worker_test_seam;

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
// See `docs/dev-artifacts/design/fjall-worker-loop-524-findings.md` for the full analysis.
//
// Why `set`/`remove` are NOT routed here (measured, not assumed): both embed
// a `contains_key` READ (to derive the `Store` trait's `bool` "existed"
// return) before their write. Benched under 8×-cache-size cold data + 32-way
// fan-out (`benches/storage_fjall_write_worker.rs`): routing `set` through
// this worker regressed it ~1.45× (1,124,693 -> 1,630,982 ns/op) — the
// embedded cold `contains_key` reads, previously overlapped across the
// `spawn_blocking` pool, got serialized onto one thread, exactly the
// read-parallelism hazard `docs/dev-artifacts/design/fjall-worker-loop-524-findings.md`
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
pub(super) enum WriteJob {
    /// `insert(value)` — generates a fresh key on the worker, returns it.
    Insert {
        value: Bytes,
        reply: oneshot::Sender<DbResult<RecordKey>>,
    },
    /// `transact(ops)` — mixed-op batch.
    Transact {
        ops: Vec<KvOp>,
        reply: oneshot::Sender<DbResult<()>>,
    },
}

/// Owns the dedicated write thread and the channel into it. One per
/// [`FjallStore`]. On drop the sender is closed and the thread is joined so
/// the worker is reaped deterministically (important because the test suite
/// and DDL churn create/drop many stores).
pub(super) struct WriteWorker {
    /// `Option` so `Drop` can close the channel (take → drop) BEFORE joining
    /// the thread; joining while the sender is still open would deadlock, as
    /// the worker's `recv()` never returns.
    tx: Option<SyncSender<WriteJob>>,
    handle: Option<JoinHandle<()>>,
}

impl WriteWorker {
    /// Spawn the worker thread. It owns one clone of the keyspace + database
    /// handle and drains jobs one-at-a-time in channel admission order.
    /// Concurrent invocation order is not guaranteed; await preceding writes
    /// when ordering matters. Other handles, write routes, and repository
    /// batches are not sequenced by this worker.
    ///
    /// The channel is bounded (`sync_channel`) so a burst of submitters
    /// applies natural backpressure instead of letting an unbounded queue
    /// grow without limit — the depth is generous enough that steady-state
    /// submitters never block, but a pathological fan-out can't OOM the queue.
    pub(super) fn spawn(keyspace: Keyspace, db: Arc<Database>) -> io::Result<Self> {
        // Bound chosen well above realistic in-flight write fan-out; a full
        // queue simply parks the submitting task until the worker drains one.
        let (tx, rx): (SyncSender<WriteJob>, Receiver<WriteJob>) =
            std::sync::mpsc::sync_channel(1024);

        let handle = std::thread::Builder::new()
            .name("fjall-write-worker".to_string())
            .spawn(move || Self::run(rx, keyspace, db))?;

        Ok(Self {
            tx: Some(tx),
            handle: Some(handle),
        })
    }

    /// The live sender. `tx` is only ever `None` transiently inside `Drop`,
    /// which cannot overlap a method call on the owning `FjallStore` (drop
    /// takes `&mut self` by value), so this never panics in practice.
    pub(super) fn sender(&self) -> &SyncSender<WriteJob> {
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

/// Submit a job to the worker and await its oneshot reply.
///
/// Worker-gone (`try_send` `Disconnected`) and dropped-reply failures map
/// onto `DbError::Internal`, matching the old `spawn_blocking` path's
/// `JoinError → DbError::Internal` mapping so error shapes are unchanged.
///
/// RV-STORAGE-9E9A0A71: admission never blocks the executing runtime
/// thread — `try_send` first; only when the bounded queue is full do we
/// cooperatively `yield_now` and retry, so saturation parks this TASK,
/// never the executor thread.
pub(super) async fn submit<T>(
    tx: &SyncSender<WriteJob>,
    build: impl FnOnce(oneshot::Sender<DbResult<T>>) -> WriteJob,
) -> DbResult<T> {
    let (reply_tx, reply_rx) = oneshot::channel();
    let mut job = build(reply_tx);
    loop {
        match tx.try_send(job) {
            Ok(()) => break,
            Err(std::sync::mpsc::TrySendError::Full(returned)) => {
                job = returned;
                #[cfg(test)]
                write_worker_test_seam::notify_submit_full_for_tests();
                task::yield_now().await;
            }
            Err(std::sync::mpsc::TrySendError::Disconnected(_)) => {
                return Err(DbError::Internal(
                    "fjall write worker channel closed (worker thread gone)".to_string(),
                ));
            }
        }
    }
    match reply_rx.await {
        Ok(result) => result,
        Err(_) => Err(DbError::Internal(
            "fjall write worker dropped reply (worker thread gone)".to_string(),
        )),
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
    // RecordId::new uses a timestamp and 64-bit Xoshiro random tail; see
    // the canonical RecordId type. No collision probe is made: uniqueness is
    // probabilistic and a collision overwrites the existing value. Under an
    // equal-timestamp, independent uniform-tail model, a pair collides with
    // probability 2^-64; this is not a uniform random 128-bit identifier.
    let id = RecordId::new();
    let key = RecordKey::from_slice(id.as_bytes());
    keyspace
        .insert(&key[..], &*value)
        .map_err(|e| DbError::Storage(e.to_string()))?;
    Ok(key)
}

/// Execute `transact` synchronously on the worker thread — native atomic
/// `OwnedWriteBatch` commit.
///
/// RV-STORAGE-EC3A171B: fjall < 3.1.7 discarded `journal_writer.write_batch`'s
/// Result inside `WriteBatch::commit`, so a batch could be acknowledged with
/// an incomplete journal suffix. fjall >= 3.1.7 propagates the error and
/// poisons the database; the locked version is pinned by
/// `journal_error_lock_tests` so a lockfile downgrade fails CI.
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
