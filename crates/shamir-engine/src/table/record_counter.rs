//! Record counter for tracking number of records in a table.
//!
//! **Performance (Opt #3):** the counter lives as an in-memory
//! `AtomicU64`; increments are a single `fetch_add`. The on-disk copy
//! is rewritten lazily — only by `persist()` and only when the cache
//! has actually changed since the previous write. Previously every
//! `increment(1)` called the store twice (`get` + `set`) inside a
//! mutex, costing ~2 µs each; in a bulk insert of N records that was
//! 2N redundant store ops. After this change increments are
//! free-modulo-an-atomic; the durable bump rides whatever periodic
//! persist call the engine already makes (and is itself a no-op when
//! nothing changed).

use crate::meta::MetaKey;
use shamir_storage::error::{DbError, DbResult};
use shamir_storage::types::Store;
use shamir_types::codecs::basic::bincode;
use shamir_types::types::record_id::RecordId;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use tokio::sync::Mutex;
use tokio::sync::OnceCell;

/// Get the system record key for storing record count
fn count_key() -> RecordId {
    MetaKey::Count.as_record_id()
}

/// Manages record count in a table.
///
/// Increments/decrements run lock-free against an in-memory
/// `AtomicU64`. The persistent copy in the info_store is only
/// rewritten when `persist()` is called AND the in-memory value
/// differs from the last persisted snapshot.
pub struct RecordCounter {
    info_store: Arc<dyn Store>,
    /// Lazily-initialised on first `get()` — reads the persisted
    /// count from `info_store` into memory exactly once.
    cache: Arc<OnceCell<AtomicU64>>,
    /// Last value we actually wrote to the info_store. Compared
    /// against `cache.load()` in `persist()` to decide whether to
    /// re-serialise + write.
    last_persisted: Arc<AtomicU64>,
    /// `true` iff cache has been incremented since `last_persisted`
    /// — quick skip flag for `persist()`.
    dirty: Arc<AtomicBool>,
    /// Guards `persist()` so concurrent persists don't race on the
    /// `last_persisted` update.
    persist_lock: Arc<Mutex<()>>,
}

impl Clone for RecordCounter {
    fn clone(&self) -> Self {
        Self {
            info_store: Arc::clone(&self.info_store),
            cache: Arc::clone(&self.cache),
            last_persisted: Arc::clone(&self.last_persisted),
            dirty: Arc::clone(&self.dirty),
            persist_lock: Arc::clone(&self.persist_lock),
        }
    }
}

impl RecordCounter {
    /// Create a new record counter
    pub fn new(info_store: Arc<dyn Store>) -> Self {
        Self {
            info_store,
            cache: Arc::new(OnceCell::new()),
            last_persisted: Arc::new(AtomicU64::new(0)),
            dirty: Arc::new(AtomicBool::new(false)),
            persist_lock: Arc::new(Mutex::new(())),
        }
    }

    /// Get current record count — reads the in-memory cache, lazily
    /// hydrating from the info_store on first access.
    pub async fn get(&self) -> DbResult<u64> {
        let cache = self.ensure_cache().await?;
        Ok(cache.load(Ordering::Acquire))
    }

    /// Set record count (useful for initialization or manual correction).
    /// Writes through both cache and store synchronously.
    pub async fn set(&self, count: u64) -> DbResult<()> {
        let cache = self.ensure_cache().await?;
        cache.store(count, Ordering::Release);
        self.write_through(count).await?;
        self.dirty.store(false, Ordering::Release);
        Ok(())
    }

    /// Increment record count by delta. Lock-free against an atomic;
    /// the store is NOT touched here — call `persist()` later (or
    /// rely on the engine's existing post-write persist hook) to
    /// flush the new value.
    pub async fn increment(&self, delta: i64) -> DbResult<()> {
        let cache = self.ensure_cache().await?;
        if delta == 0 {
            return Ok(());
        }
        if delta > 0 {
            cache.fetch_add(delta as u64, Ordering::AcqRel);
        } else {
            // Saturate at 0 — counter must not go negative.
            let mag = (-delta) as u64;
            // CAS loop because fetch_sub would underflow.
            loop {
                let cur = cache.load(Ordering::Acquire);
                if cur < mag {
                    return Err(DbError::Internal(format!(
                        "Record count cannot go below zero: current={cur}, delta={delta}"
                    )));
                }
                let new = cur - mag;
                if cache
                    .compare_exchange_weak(cur, new, Ordering::AcqRel, Ordering::Acquire)
                    .is_ok()
                {
                    break;
                }
            }
        }
        self.dirty.store(true, Ordering::Release);
        Ok(())
    }

    /// Set the counter to an absolute value. Used by the doctor to
    /// reconcile the cached counter with a fresh count of records
    /// in the data store.
    pub async fn set_to(&self, n: u64) -> DbResult<()> {
        let cache = self.ensure_cache().await?;
        cache.store(n, Ordering::Release);
        self.dirty.store(true, Ordering::Release);
        self.persist().await
    }

    /// Flush the in-memory counter to the info_store if it differs
    /// from the last persisted value. No-op otherwise.
    pub async fn persist(&self) -> DbResult<()> {
        if !self.dirty.load(Ordering::Acquire) {
            return Ok(());
        }
        let _guard = self.persist_lock.lock().await;
        // Re-check inside the lock — another task may have flushed.
        if !self.dirty.load(Ordering::Acquire) {
            return Ok(());
        }
        let cache = self.ensure_cache().await?;
        let cur = cache.load(Ordering::Acquire);
        let last = self.last_persisted.load(Ordering::Acquire);
        if cur == last {
            self.dirty.store(false, Ordering::Release);
            return Ok(());
        }
        self.write_through(cur).await?;
        self.last_persisted.store(cur, Ordering::Release);
        self.dirty.store(false, Ordering::Release);
        Ok(())
    }

    async fn ensure_cache(&self) -> DbResult<&AtomicU64> {
        if let Some(c) = self.cache.get() {
            return Ok(c);
        }
        let info_store = Arc::clone(&self.info_store);
        let last_persisted = Arc::clone(&self.last_persisted);
        let cell = &self.cache;
        let _ = cell
            .get_or_init(|| async move {
                let key_bytes = count_key().to_bytes();
                let initial: u64 = match info_store.get(key_bytes).await {
                    Ok(bytes) => bincode::from_bytes(&bytes).unwrap_or(0),
                    Err(_) => 0,
                };
                last_persisted.store(initial, Ordering::Release);
                AtomicU64::new(initial)
            })
            .await;
        Ok(self.cache.get().unwrap())
    }

    async fn write_through(&self, count: u64) -> DbResult<()> {
        let key_bytes = count_key().to_bytes();
        let bytes = bincode::to_bytes(&count)
            .map_err(|e| DbError::Codec(format!("Failed to serialize count: {}", e)))?;
        self.info_store.set(key_bytes, bytes).await?;
        Ok(())
    }
}
