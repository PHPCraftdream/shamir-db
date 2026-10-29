//! `CachedStore` — a full in-memory mirror of any `Store`.
//!
//! ROUND 4 design (RV-STORAGE-E70A0DDC / -897437C3): a single long-lived
//! FIFO owner task owns EVERY cache/backing mutation in BOTH write modes,
//! draining one command at a time — no per-key locks, no admission
//! counters. Reads are lock-free (epoch + mirror probe + inner fallback +
//! epoch-checked fill). Any backing mutation failure is sticky-terminal:
//! every queued and subsequent command, and every trusted read, fails
//! until the store is rebuilt.

use crate::error::{DbError, DbResult};
use crate::types::{RecordKey, RecordStream, Store};
mod admission;
#[cfg(test)]
pub(crate) use admission::COMMAND_CHANNEL_CAPACITY;
#[cfg(test)]
pub(crate) use admission::QUEUED_PAYLOAD_BUDGET_BYTES;
#[cfg(test)]
pub(crate) use admission::{
    budget_waits_for_tests, reset_retry_count_for_tests, retry_count_for_tests,
};
mod owner_health;
use async_stream::stream;
use async_trait::async_trait;
use bytes::Bytes;
use futures::{stream::Stream, StreamExt};
use owner_health::{await_reply, publish_failure, storage_err, HealthWatch, OwnerGuard};
use scc::TreeIndex;
use std::pin::Pin;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::Arc;
#[cfg(test)]
use tokio::sync::Notify;
use tokio::sync::{mpsc, oneshot, watch};

// ============================================================================
// WriteMode - write strategy for CachedStore
// ============================================================================

/// Write strategy for cache operations.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum WriteMode {
    /// Write-through: wait for disk write before returning (safer, slower;
    /// critical data like indexes).
    Sync,

    /// Write-behind: cache immediately, disk write in background (faster;
    /// data may be lost on crash — non-critical data).
    Async,
}

// ============================================================================
// Commands to the FIFO owner task
// ============================================================================

/// One queued unit of work for the owner; `reply` is completed exactly once
/// (a dropped caller means the reply send fails, which the owner ignores).
enum Command {
    Insert {
        value: Bytes,
        reply: oneshot::Sender<DbResult<RecordKey>>,
    },
    Set {
        key: RecordKey,
        value: Bytes,
        reply: oneshot::Sender<DbResult<bool>>,
    },
    Remove {
        key: RecordKey,
        reply: oneshot::Sender<DbResult<bool>>,
    },
    Transact {
        ops: Vec<crate::types::KvOp>,
        reply: oneshot::Sender<DbResult<()>>,
    },
    Reload {
        reply: oneshot::Sender<DbResult<()>>,
    },
    /// Reader-initiated miss fill. Applied only on an exact EVEN capture;
    /// a fill itself never bumps the epoch.
    Fill {
        epoch: u64,
        key: RecordKey,
        value: Option<Bytes>,
        reply: oneshot::Sender<DbResult<()>>,
    },
    Flush {
        reply: oneshot::Sender<DbResult<()>>,
    },
    FlushDrain {
        reply: oneshot::Sender<DbResult<()>>,
    },
}

fn bump_epoch(epoch: &AtomicU64) -> Result<u64, String> {
    let mut cur = epoch.load(Ordering::Acquire);
    loop {
        let next = cur
            .checked_add(1)
            .ok_or_else(|| "cache epoch counter exhausted".to_string())?;
        match epoch.compare_exchange_weak(cur, next, Ordering::AcqRel, Ordering::Acquire) {
            Ok(_) => return Ok(next),
            Err(actual) => cur = actual,
        }
    }
}

/// Completes a mutation: bumps the epoch back to EVEN before the ack /
/// next dequeue, so a Fill that captured the odd in-flight epoch is
/// rejected at completion too. Checked-exhaustion is sticky-terminal.
fn finish_mutation(epoch: &AtomicU64, health: &HealthWatch) -> Option<String> {
    match bump_epoch(epoch) {
        Ok(_) => None,
        Err(msg) => {
            publish_failure(health, msg.clone());
            Some(msg)
        }
    }
}

/// Completion bump (back to even) + ack; on checked-exhaustion the store
/// goes sticky-terminal and the ack carries the failure instead of `ok`.
fn finish_and_ack<T>(
    epoch: &AtomicU64,
    health: &HealthWatch,
    failed: &mut Option<String>,
    reply: oneshot::Sender<DbResult<T>>,
    ok: DbResult<T>,
) {
    match finish_mutation(epoch, health) {
        Some(msg) => {
            *failed = Some(msg.clone());
            let _ = reply.send(Err(storage_err(msg)));
        }
        None => {
            let _ = reply.send(ok);
        }
    }
}

/// Cache upsert on a bare `TreeIndex` + size counter; returns `true` if
/// the key was newly created.
fn cache_upsert_into(
    cache: &TreeIndex<RecordKey, Bytes>,
    size: &AtomicUsize,
    key: RecordKey,
    value: Bytes,
) -> bool {
    let existed = cache.remove_sync(&key);
    if !existed {
        size.fetch_add(1, Ordering::Relaxed);
    }
    // insert always succeeds after remove (scc only rejects duplicates).
    let _ = cache.insert_sync(key, value);
    !existed
}

fn cache_evict_into(
    cache: &TreeIndex<RecordKey, Bytes>,
    size: &AtomicUsize,
    key: &RecordKey,
) -> bool {
    if cache.remove_sync(key) {
        size.fetch_sub(1, Ordering::Relaxed);
        true
    } else {
        false
    }
}

/// Post-commit cache action for one `transact` op, built before `ops` is
/// moved into `inner.transact`.
enum CacheAction {
    /// `KvOp::Set` — populate the cache with the committed value.
    Populate(RecordKey, Bytes),
    /// `KvOp::Remove` — evict the entry.
    Invalidate(RecordKey),
}

impl CacheAction {
    fn from_op(op: &crate::types::KvOp) -> Self {
        match op {
            crate::types::KvOp::Set(k, v) => Self::Populate(k.clone(), v.clone()),
            crate::types::KvOp::Remove(k) => Self::Invalidate(k.clone()),
        }
    }

    fn apply(self, cache: &TreeIndex<RecordKey, Bytes>, size: &AtomicUsize) {
        match self {
            Self::Populate(key, value) => {
                cache_upsert_into(cache, size, key, value);
            }
            Self::Invalidate(key) => {
                cache_evict_into(cache, size, &key);
            }
        }
    }
}

// ============================================================================
// FIFO owner task
// ============================================================================

/// Shared owner state, passed to the owner task as one handle.
struct OwnerState {
    inner: Arc<dyn Store>,
    cache: Arc<TreeIndex<RecordKey, Bytes>>,
    size: Arc<AtomicUsize>,
    epoch: Arc<AtomicU64>,
    pending: Arc<AtomicUsize>,
    health: HealthWatch,
    mode: WriteMode,
}

/// The single long-lived owner task. Drains `rx` strictly FIFO, one
/// command at a time; each command is fully awaited (backing AND cache)
/// before the next dequeue, so cache and backing mutate in submission
/// order. Exits when every `CachedStore` handle is dropped.
async fn run_owner(
    mut rx: mpsc::Receiver<admission::QueuedCommand>,
    state: OwnerState,
    guard: OwnerGuard,
    #[cfg(test)] start_gate: Option<Arc<Notify>>,
) {
    let OwnerState {
        inner,
        cache,
        size,
        epoch,
        pending,
        health,
        mode,
    } = state;
    let _guard = guard;
    // Test-only pause so an abort can land before owner work.
    #[cfg(test)]
    if let Some(gate) = start_gate {
        gate.notified().await;
    }

    // Sticky terminal failure: every remaining queued command fails with
    // this message; the loop then drains-and-fails forever.
    let mut failed: Option<String> = None;

    while let Some(queued) = rx.recv().await {
        // Drop the command's reservation AT DEQUEUE — its `Drop` refunds
        // the retained-payload charge (byte budget, RV-STORAGE-E02C8A80).
        // The same refund happens automatically if the owner aborts or
        // the receiver drops while commands are still queued.
        let (cmd, reservation) = queued.into_parts();
        drop(reservation);
        if let Some(msg) = failed.clone() {
            fail_command(cmd, &msg);
            continue;
        }
        // Odd/even protocol: bump ODD before a mutation, EVEN on
        // completion; `Fill` itself never bumps.
        let is_mutation = matches!(
            cmd,
            Command::Insert { .. }
                | Command::Set { .. }
                | Command::Remove { .. }
                | Command::Transact { .. }
                | Command::Reload { .. }
        );
        if is_mutation {
            if let Err(msg) = bump_epoch(&epoch) {
                publish_failure(&health, msg.clone());
                failed = Some(msg.clone());
                fail_command(cmd, &msg);
                continue;
            }
        }
        match cmd {
            Command::Insert { value, reply } => match inner.insert(value.clone()).await {
                Ok(key) => {
                    // Bump exact membership only when the insert succeeded.
                    if cache.insert_sync(key.clone(), value).is_ok() {
                        size.fetch_add(1, Ordering::Relaxed);
                    }
                    finish_and_ack(&epoch, &health, &mut failed, reply, Ok(key));
                }
                Err(e) => {
                    publish_failure(&health, e.to_string());
                    failed = Some(e.to_string());
                    let _ = reply.send(Err(e));
                }
            },
            Command::Set { key, value, reply } => match mode {
                WriteMode::Sync => match inner.set(key.clone(), value.clone()).await {
                    Ok(created) => {
                        cache_upsert_into(&cache, &size, key, value);
                        finish_and_ack(&epoch, &health, &mut failed, reply, Ok(created));
                    }
                    Err(e) => {
                        publish_failure(&health, e.to_string());
                        failed = Some(e.to_string());
                        let _ = reply.send(Err(e));
                    }
                },
                WriteMode::Async => {
                    // Tentative cache publication + ack FIRST, then the
                    // backing await (one command at a time keeps FIFO).
                    let created = cache_upsert_into(&cache, &size, key.clone(), value.clone());
                    pending.fetch_add(1, Ordering::Relaxed);
                    let _ = reply.send(Ok(created));
                    if let Err(e) = inner.set(key, value).await {
                        pending.fetch_sub(1, Ordering::Relaxed);
                        log::error!("storage_cached async set failed terminally: {}", e);
                        publish_failure(&health, e.to_string());
                        failed = Some(e.to_string());
                    } else {
                        pending.fetch_sub(1, Ordering::Relaxed);
                        // Completion bump (even) before the next dequeue.
                        if let Some(msg) = finish_mutation(&epoch, &health) {
                            failed = Some(msg);
                        }
                    }
                }
            },
            Command::Remove { key, reply } => match mode {
                WriteMode::Sync => match inner.remove(key.clone()).await {
                    Ok(removed) => {
                        cache_evict_into(&cache, &size, &key);
                        finish_and_ack(&epoch, &health, &mut failed, reply, Ok(removed));
                    }
                    Err(e) => {
                        publish_failure(&health, e.to_string());
                        failed = Some(e.to_string());
                        let _ = reply.send(Err(e));
                    }
                },
                WriteMode::Async => {
                    let existed = cache_evict_into(&cache, &size, &key);
                    pending.fetch_add(1, Ordering::Relaxed);
                    let _ = reply.send(Ok(existed));
                    if let Err(e) = inner.remove(key).await {
                        pending.fetch_sub(1, Ordering::Relaxed);
                        log::error!("storage_cached async remove failed terminally: {}", e);
                        publish_failure(&health, e.to_string());
                        failed = Some(e.to_string());
                    } else {
                        pending.fetch_sub(1, Ordering::Relaxed);
                        if let Some(msg) = finish_mutation(&epoch, &health) {
                            failed = Some(msg);
                        }
                    }
                }
            },
            Command::Transact { ops, reply } => {
                let actions: Vec<CacheAction> = ops.iter().map(CacheAction::from_op).collect();
                match inner.transact(ops).await {
                    Ok(()) => {
                        for action in actions {
                            action.apply(&cache, &size);
                        }
                        finish_and_ack(&epoch, &health, &mut failed, reply, Ok(()));
                    }
                    Err(e) => {
                        publish_failure(&health, e.to_string());
                        failed = Some(e.to_string());
                        let _ = reply.send(Err(e));
                    }
                }
            }
            Command::Reload { reply } => {
                // Scan into a PRIVATE tree first; on scan error the live
                // tree is left unchanged and the store stays healthy.
                let fresh = TreeIndex::new();
                let mut scan_err: Option<DbError> = None;
                {
                    let mut s = inner.iter_stream(shamir_tunables::store_defaults::FULL_SCAN_BATCH);
                    while let Some(batch_result) = s.next().await {
                        match batch_result {
                            Ok(batch) => {
                                for (key, value) in batch {
                                    let _ = fresh.insert_sync(key, value);
                                }
                            }
                            Err(e) => {
                                scan_err = Some(e);
                                break;
                            }
                        }
                    }
                }
                if let Some(e) = scan_err {
                    // Finish even; live mirror unchanged, store healthy
                    // (a scan failure is not a backing MUTATION failure).
                    match finish_mutation(&epoch, &health) {
                        Some(msg) => {
                            failed = Some(msg.clone());
                            let _ = reply.send(Err(storage_err(msg)));
                        }
                        None => {
                            let _ = reply.send(Err(e));
                        }
                    }
                    continue;
                }
                // Synchronous diff application; the live tree is
                // diff-applied, NEVER `clear()`ed.
                let mut vanished: Vec<RecordKey> = Vec::new();
                {
                    let g = scc::Guard::new();
                    for (k, _) in cache.iter(&g) {
                        if !fresh.contains(k) {
                            vanished.push(k.clone());
                        }
                    }
                }
                for k in vanished {
                    cache_evict_into(&cache, &size, &k);
                }
                {
                    let g = scc::Guard::new();
                    let updates: Vec<(RecordKey, Bytes)> = fresh
                        .iter(&g)
                        .map(|(k, v)| (k.clone(), v.clone()))
                        .collect();
                    for (k, v) in updates {
                        cache_upsert_into(&cache, &size, k, v);
                    }
                }
                let _ = reply.send(match finish_mutation(&epoch, &health) {
                    Some(msg) => {
                        failed = Some(msg.clone());
                        Err(storage_err(msg))
                    }
                    None => Ok(()),
                });
            }
            Command::Fill {
                epoch: captured,
                key,
                value,
                reply,
            } => {
                // Applies only on an exact EVEN capture.
                let cur = epoch.load(Ordering::Acquire);
                if cur != captured || cur % 2 == 1 {
                    let _ = reply.send(Ok(()));
                    continue;
                }
                if let Some(v) = value {
                    cache_upsert_into(&cache, &size, key, v);
                }
                let _ = reply.send(Ok(()));
            }
            Command::Flush { reply } => {
                // Barrier: every earlier mutation has reached `inner`.
                // A flush failure is NOT sticky-terminal.
                let _ = reply.send(inner.flush().await);
            }
            Command::FlushDrain { reply } => {
                let _ = reply.send(inner.flush_drain().await);
            }
        }
    }
}

/// Fails a queued command with the sticky terminal message (owner has
/// already failed; every queued job is rejected in order).
fn fail_reply<T>(reply: oneshot::Sender<DbResult<T>>, msg: &str) {
    let _ = reply.send(Err(storage_err(msg)));
}

fn fail_command(cmd: Command, msg: &str) {
    match cmd {
        Command::Insert { reply, .. } => fail_reply(reply, msg),
        Command::Set { reply, .. } => fail_reply(reply, msg),
        Command::Remove { reply, .. } => fail_reply(reply, msg),
        Command::Transact { reply, .. } => fail_reply(reply, msg),
        Command::Reload { reply, .. } => fail_reply(reply, msg),
        Command::Fill { reply, .. } => fail_reply(reply, msg),
        Command::Flush { reply, .. } => fail_reply(reply, msg),
        Command::FlushDrain { reply, .. } => fail_reply(reply, msg),
    }
}

/// Supervisor: awaits the owner's `JoinHandle`; publishes a failure if the owner
/// finished without a more specific one (alongside [`OwnerGuard`]).
async fn supervise_owner(handle: tokio::task::JoinHandle<()>, health: HealthWatch) {
    let _ = handle.await;
    publish_failure(&health, "cache owner task finished unexpectedly".into());
}

// ============================================================================
// CachedStore
// ============================================================================

/// Full mirror cache that loads ALL data from inner store on creation.
/// `Sync` = write-through (safer, indexes); `Async` = write-behind (the
/// owner publishes the tentative cache entry and acks, then awaits the
/// backing write before dequeuing the next command).
///
/// ## Concurrency (ROUND 4, RV-STORAGE-E70A0DDC):
/// One FIFO owner task owns every mutation in both modes; reads are
/// lock-free (epoch + mirror probe + inner fallback + epoch-checked fill).
/// A backing mutation failure is sticky-terminal for the whole store.
pub struct CachedStore {
    inner: Arc<dyn Store>,
    cache: Arc<TreeIndex<RecordKey, Bytes>>,
    mode: WriteMode,
    /// Telemetry: async set/remove jobs not yet completed in the backing.
    pending_writes: Arc<AtomicUsize>,
    /// Cache entry count (`TreeIndex` has no O(1) `len()`).
    size: Arc<AtomicUsize>,
    /// Odd/even: ODD while a mutation/reload is in flight, EVEN when
    /// quiescent; completion bumps invalidate captures taken mid-flight.
    epoch: Arc<AtomicU64>,
    /// Bounded command channel to the FIFO owner (RV-STORAGE-E02C8A80):
    /// value-bearing `Async` commands are admitted via
    /// [`admission::admit`]'s `try_send` + `yield_now` retry so a stalled
    /// owner cannot grow the queue without bound. Each queued item
    /// carries its own byte-budget reservation guard.
    tx: mpsc::Sender<admission::QueuedCommand>,
    /// Shared retained-payload byte counter backing the admission budget
    /// (RV-STORAGE-E02C8A80 round 2); charges are released by the
    /// reservation guards traveling inside the queued commands.
    queued_bytes: Arc<AtomicUsize>,
    health: watch::Receiver<Option<String>>,
    /// Health sender clone: lets a dropped-reply diagnostic be published
    /// sticky even if the `OwnerGuard` publication races.
    health_tx: HealthWatch,
    #[cfg(test)]
    abort_handle: tokio::task::AbortHandle,
}

impl CachedStore {
    async fn new_with_mode(inner: Arc<dyn Store>, mode: WriteMode) -> DbResult<Self> {
        Self::build(inner, mode, None).await
    }

    async fn build(
        inner: Arc<dyn Store>,
        mode: WriteMode,
        #[cfg(test)] start_gate: Option<Arc<Notify>>,
        #[cfg(not(test))] _start_gate: Option<Arc<tokio::sync::Notify>>,
    ) -> DbResult<Self> {
        let cache = Arc::new(TreeIndex::new());
        let size = Arc::new(AtomicUsize::new(0));

        // Load ALL data from inner store into cache (streaming).
        let mut stream = inner.iter_stream(shamir_tunables::store_defaults::FULL_SCAN_BATCH);
        while let Some(batch_result) = stream.next().await {
            let batch = batch_result?;
            for (key, value) in batch {
                if cache.insert_sync(key, value).is_ok() {
                    size.fetch_add(1, Ordering::Relaxed);
                }
            }
        }

        let pending_writes = Arc::new(AtomicUsize::new(0));
        let epoch = Arc::new(AtomicU64::new(0));
        let queued_bytes = Arc::new(AtomicUsize::new(0));
        let (tx, rx) =
            mpsc::channel::<admission::QueuedCommand>(admission::COMMAND_CHANNEL_CAPACITY);
        let (health_tx, health_rx) = watch::channel(None);
        // Built BEFORE spawn: even an abort before the first poll marks
        // the store failed on drop.
        let guard = OwnerGuard {
            health: health_tx.clone(),
        };
        let state = OwnerState {
            inner: inner.clone(),
            cache: cache.clone(),
            size: size.clone(),
            epoch: epoch.clone(),
            pending: pending_writes.clone(),
            health: health_tx.clone(),
            mode,
        };
        let health_tx_store = health_tx.clone();
        let handle = tokio::spawn(run_owner(
            rx,
            state,
            guard,
            #[cfg(test)]
            start_gate,
        ));
        #[cfg(test)]
        let abort_handle = handle.abort_handle();
        tokio::spawn(supervise_owner(handle, health_tx));

        Ok(Self {
            inner,
            cache,
            mode,
            pending_writes,
            size,
            epoch,
            tx,
            queued_bytes,
            health: health_rx,
            health_tx: health_tx_store,
            #[cfg(test)]
            abort_handle,
        })
    }

    /// New cached store, Sync write mode (safer, for indexes).
    pub async fn new_sync(inner: Arc<dyn Store>) -> DbResult<Self> {
        Self::new_with_mode(inner, WriteMode::Sync).await
    }

    /// New cached store, Async write mode (faster, for data).
    pub async fn new_async(inner: Arc<dyn Store>) -> DbResult<Self> {
        Self::new_with_mode(inner, WriteMode::Async).await
    }

    /// Get reference to the inner store.
    pub fn inner(&self) -> &Arc<dyn Store> {
        &self.inner
    }

    /// Reference to the underlying `TreeIndex` cache (inspection/debugging).
    pub fn cache(&self) -> &Arc<TreeIndex<RecordKey, Bytes>> {
        &self.cache
    }

    /// Get write mode.
    pub fn mode(&self) -> WriteMode {
        self.mode
    }

    /// Get number of entries currently in cache.
    pub fn cache_size(&self) -> usize {
        self.size.load(Ordering::Relaxed)
    }

    /// Number of async set/remove jobs not yet completed (0 for Sync).
    pub fn pending_writes(&self) -> usize {
        self.pending_writes.load(Ordering::Relaxed)
    }

    /// Returns the sticky terminal failure message, if any.
    fn sticky_failure(&self) -> Option<String> {
        self.health.borrow().clone()
    }

    /// Submit a command to the FIFO owner and await its reply, racing the
    /// persistent terminal-failure watch.
    async fn command<T>(
        &self,
        make: impl FnOnce(oneshot::Sender<DbResult<T>>) -> Command,
    ) -> DbResult<T> {
        if let Some(msg) = self.sticky_failure() {
            return Err(storage_err(msg));
        }
        let (reply_tx, reply_rx) = oneshot::channel();
        admission::admit(&self.tx, &self.queued_bytes, &self.health, make(reply_tx)).await?;
        await_reply(reply_rx, self.health.clone(), self.health_tx.clone()).await
    }

    /// Reload all data from inner store (re-sync the mirror). On scan
    /// error the live mirror is left unchanged and the store stays
    /// healthy; on success the diff is applied to the SAME live tree
    /// synchronously (never `cache.clear()`ed).
    pub async fn reload(&self) -> DbResult<()> {
        self.command(|reply| Command::Reload { reply }).await
    }

    /// Flush every queued mutation down to `inner` (FIFO barrier) and
    /// propagate to `inner`'s own buffered state. If the owner is
    /// terminally failed, the inner durability op is still attempted directly.
    pub async fn flush(&self) -> DbResult<()> {
        match self.command(|reply| Command::Flush { reply }).await {
            Ok(()) => Ok(()),
            Err(e) => {
                if self.sticky_failure().is_some() {
                    self.flush_owner_down(e.to_string(), false).await
                } else {
                    // Owner healthy; the reply itself is inner.flush's error.
                    Err(e)
                }
            }
        }
    }

    /// Drain queued mutations into the inner backend WITHOUT the inner
    /// `flush()` (the shutdown pass persists the backend once). Same
    /// sticky-failure handling as `flush`.
    pub async fn flush_drain(&self) -> DbResult<()> {
        match self.command(|reply| Command::FlushDrain { reply }).await {
            Ok(()) => Ok(()),
            Err(e) => {
                if self.sticky_failure().is_some() {
                    self.flush_owner_down(e.to_string(), true).await
                } else {
                    Err(e)
                }
            }
        }
    }

    /// Terminal-failure flush path: the inner durability op is attempted
    /// DIRECTLY; the error merges the sticky failure and the inner result.
    async fn flush_owner_down(&self, sticky: String, drain: bool) -> DbResult<()> {
        let inner_result = if drain {
            self.inner.flush_drain().await
        } else {
            self.inner.flush().await
        };
        match inner_result {
            Ok(()) => Err(storage_err(format!(
                "cache owner failed terminally ({sticky}); inner {} succeeded but the \
                 cache mirror is unusable",
                if drain { "flush_drain" } else { "flush" }
            ))),
            Err(e) => {
                log::error!(
                    "storage_cached flush_down: inner {} failed AND the cache owner is \
                     terminally failed ({sticky}); returning inner's error, sticky \
                     failure logged here",
                    if drain { "flush_drain" } else { "flush" }
                );
                Err(e)
            }
        }
    }

    #[cfg(test)]
    pub(crate) fn abort_owner_for_tests(&self) {
        self.abort_handle.abort();
    }

    /// Current retained-payload byte charge (RV-STORAGE-E02C8A80 oracle).
    #[cfg(test)]
    pub(crate) fn queued_bytes_for_tests(&self) -> usize {
        self.queued_bytes.load(Ordering::Relaxed)
    }

    /// Resolves once the health watch holds a terminal value.
    #[cfg(test)]
    pub(crate) async fn await_health_terminal_for_tests(&self) {
        let mut rx = self.health.clone();
        while rx.borrow_and_update().is_none() && rx.changed().await.is_ok() {}
    }

    #[cfg(test)]
    pub(crate) async fn new_sync_paused_owner_for_tests(
        inner: Arc<dyn Store>,
    ) -> DbResult<(Self, Arc<Notify>)> {
        let gate = Arc::new(Notify::new());
        Ok((
            Self::build(inner, WriteMode::Sync, Some(gate.clone())).await?,
            gate,
        ))
    }
}

#[async_trait]
impl Store for CachedStore {
    async fn insert(&self, value: Bytes) -> DbResult<RecordKey> {
        self.command(|reply| Command::Insert { value, reply }).await
    }

    async fn set(&self, key: RecordKey, value: Bytes) -> DbResult<bool> {
        self.command(|reply| Command::Set { key, value, reply })
            .await
    }

    async fn get(&self, key: RecordKey) -> DbResult<Bytes> {
        // Trusted read: refuse terminally-failed stores outright.
        if let Some(msg) = self.sticky_failure() {
            return Err(storage_err(msg));
        }
        // Capture the epoch BEFORE the mirror probe and the inner read.
        let captured = self.epoch.load(Ordering::Acquire);
        if let Some(v) = self.cache.peek_with(&key, |_, v| v.clone()) {
            // Health re-check after the (lock-free) probe.
            if let Some(msg) = self.sticky_failure() {
                return Err(storage_err(msg));
            }
            return Ok(v);
        }
        let value = match self.inner.get(key.clone()).await {
            Ok(v) => Some(v),
            Err(DbError::NotFound(_)) => None,
            Err(e) => return Err(e),
        };
        self.command(|reply| Command::Fill {
            epoch: captured,
            key: key.clone(),
            value: value.clone(),
            reply,
        })
        .await?;
        value.ok_or_else(|| DbError::missing_key(&key))
    }

    async fn remove(&self, key: RecordKey) -> DbResult<bool> {
        self.command(|reply| Command::Remove { key, reply }).await
    }

    fn iter_stream(
        &self,
        batch_size: usize,
    ) -> Pin<Box<dyn Stream<Item = Result<Vec<(RecordKey, Bytes)>, DbError>> + Send>> {
        // Incremental/cursor stream over the mirror: one bounded `range`
        // query per batch, each under its own short-lived `scc::ebr::Guard`,
        // re-seeking past the last yielded key. O(batch_size) per batch;
        // TreeIndex ascending key order.
        let cache = self.cache.clone();
        let health = self.health.clone();

        Box::pin(stream! {
            if batch_size == 0 { return; }
            let mut last_key: Option<RecordKey> = None;

            loop {
                // Trusted read: fail closed when terminally failed (one
                // diagnostic batch, then stop). Clone out of the borrow
                // guard so the future stays Send across yields.
                let terminal = health.borrow().clone();
                if let Some(msg) = terminal {
                    yield Err(storage_err(msg));
                    return;
                }
                let cur_last = last_key.clone();
                let batch: Vec<(RecordKey, Bytes)> = {
                    let g = scc::Guard::new();
                    // Tuple `(Bound, Bound)` form so scc infers
                    // `Q = RecordKey` (which is `Comparable<RecordKey>`).
                    let iter = match &cur_last {
                        Some(c) => cache.range((std::ops::Bound::Excluded(c.clone()), std::ops::Bound::Unbounded), &g),
                        None => cache.range((std::ops::Bound::Unbounded, std::ops::Bound::Unbounded), &g),
                    };
                    let mut items = Vec::with_capacity(batch_size.min(256));
                    let mut batch_last: Option<RecordKey> = None;
                    for (k, v) in iter.take(batch_size) {
                        batch_last = Some(k.clone());
                        items.push((k.clone(), v.clone()));
                    }
                    // Full batch advances the cursor; a short batch ends
                    // the stream.
                    if items.len() == batch_size {
                        last_key = batch_last;
                    } else {
                        last_key = None;
                    }
                    items
                };

                if batch.is_empty() {
                    break;
                }
                yield Ok(batch);

                if last_key.is_none() {
                    break;
                }
            }
        })
    }

    fn scan_prefix_stream(
        &self,
        prefix: Bytes,
        batch_size: usize,
    ) -> Pin<Box<dyn Stream<Item = Result<Vec<(RecordKey, Bytes)>, DbError>> + Send>> {
        // Same incremental/cursor pattern as `iter_stream`, bounded below
        // by the prefix.
        let cache = self.cache.clone();
        let health = self.health.clone();

        Box::pin(stream! {
            if batch_size == 0 { return; }
            let mut last_key: Option<RecordKey> = None;

            loop {
                // Trusted read: fail closed when terminally failed (one
                // diagnostic batch, then stop). Clone out of the borrow
                // guard so the future stays Send across yields.
                let terminal = health.borrow().clone();
                if let Some(msg) = terminal {
                    yield Err(storage_err(msg));
                    return;
                }
                let cur_last = last_key.clone();
                let pfx = prefix.clone();
                let batch: Vec<(RecordKey, Bytes)> = {
                    let g = scc::Guard::new();
                    let iter = match &cur_last {
                        Some(c) => cache.range((std::ops::Bound::Excluded(c.clone()), std::ops::Bound::Unbounded), &g),
                        None => cache.range((std::ops::Bound::Included(RecordKey::from(pfx.clone())), std::ops::Bound::Unbounded), &g),
                    };
                    let mut items = Vec::with_capacity(batch_size.min(256));
                    let mut batch_last: Option<RecordKey> = None;
                    let mut exited_prefix = false;
                    for (k, v) in iter.take(batch_size) {
                        if !k.starts_with(&pfx[..]) {
                            exited_prefix = true;
                            break;
                        }
                        batch_last = Some(k.clone());
                        items.push((k.clone(), v.clone()));
                    }
                    if exited_prefix || items.len() < batch_size {
                        last_key = None;
                    } else {
                        last_key = batch_last;
                    }
                    items
                };

                if batch.is_empty() {
                    break;
                }
                yield Ok(batch);
                if last_key.is_none() {
                    break;
                }
            }
        })
    }

    fn iter_range_stream_reverse(
        &self,
        start_inclusive: Option<Bytes>,
        end_inclusive: Option<Bytes>,
        batch_size: usize,
    ) -> RecordStream {
        let cache = self.cache.clone();
        let health = self.health.clone();
        Box::pin(stream! {
            if batch_size == 0 { return; }
            let mut cursor: Option<RecordKey> = None;
            loop {
                let terminal = health.borrow().clone();
                if let Some(msg) = terminal { yield Err(storage_err(msg)); return; }
                let current = cursor.clone();
                let batch = {
                    let guard = scc::Guard::new();
                    let lower = start_inclusive.clone().map(RecordKey::from).map(std::ops::Bound::Included).unwrap_or(std::ops::Bound::Unbounded);
                    let upper = current.clone().map(std::ops::Bound::Excluded).or_else(|| end_inclusive.clone().map(RecordKey::from).map(std::ops::Bound::Included)).unwrap_or(std::ops::Bound::Unbounded);
                    cache.range((lower, upper), &guard).rev().take(batch_size).map(|(k,v)| (k.clone(),v.clone())).collect::<Vec<_>>()
                };
                if batch.is_empty() { break; }
                cursor = batch.last().map(|(k,_)| k.clone());
                let done = batch.len() < batch_size;
                yield Ok(batch);
                if done { break; }
            }
        })
    }

    /// Delegate to inner store's `transact`, then populate (`Set`) /
    /// evict (`Remove`) the cache. Ack after backing AND cache.
    async fn transact(&self, ops: Vec<crate::types::KvOp>) -> DbResult<()> {
        self.command(|reply| Command::Transact { ops, reply }).await
    }

    /// RV-STORAGE-007DEF6B: the atomic-visibility capability is WITHDRAWN
    /// at the cache layer. A committed `transact` republishes its cache
    /// actions ONE KEY AT A TIME while cache-hit reads (`get`) trust an
    /// unfenced mirror probe (no epoch validation on hit), so a reader
    /// running during the publication window can observe new A alongside
    /// old B REGARDLESS of how atomic the inner backend is. The flag is
    /// therefore never re-derived from `inner` (F-77 honesty contract).
    fn supports_atomic_transact(&self) -> bool {
        false
    }

    /// Pass-through: the underlying store owns the buffer knobs.
    async fn apply_buffer_config(
        &self,
        config: &crate::storage_membuffer::MemBufferConfig,
    ) -> DbResult<()> {
        self.inner.apply_buffer_config(config).await
    }

    async fn raw_backend(&self) -> Option<Arc<dyn Store>> {
        Some(Arc::clone(&self.inner))
    }

    /// Delegates to the inherent [`CachedStore::flush`] (exactly one
    /// implementation; method resolution prefers the inherent one).
    async fn flush(&self) -> DbResult<()> {
        self.flush().await
    }

    /// Delegates to the inherent [`CachedStore::flush_drain`] (as above).
    async fn flush_drain(&self) -> DbResult<()> {
        self.flush_drain().await
    }
}
