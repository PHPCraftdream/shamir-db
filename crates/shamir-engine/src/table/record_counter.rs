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

use async_trait::async_trait;

use crate::meta::MetaKey;
use crate::table::persistable::Persistable;
use shamir_storage::error::{DbError, DbResult};
use shamir_storage::types::{RecordKey, Store};
use shamir_types::codecs::basic::bincode;
use shamir_types::types::record_id::RecordId;
use std::sync::atomic::{AtomicU64, Ordering};
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
    /// Lazily-initialised on first access — reads the persisted count
    /// from `info_store` into memory exactly once. Populated via
    /// `get_or_try_init` (task-group-14 / #1's fix): a genuine
    /// info_store error OR a corrupt persisted blob leaves the cell
    /// UNINITIALISED and propagates the error, instead of silently
    /// seeding the cache (and `last_persisted`) at `0`. Defaulting to
    /// `0` on error used to be indistinguishable from a genuinely
    /// fresh table and would let the very next `persist()` overwrite a
    /// durable count with a bogus near-zero value — permanent data
    /// loss invisible until a doctor `repair()` happened to run. Only
    /// `DbError::NotFound` (key genuinely absent — fresh table) seeds
    /// `0`; every other error, and every decode failure, propagates so
    /// the caller sees the fault instead of the counter resetting.
    cache: Arc<OnceCell<AtomicU64>>,
    /// Value we last durably wrote to the info_store (or hydrated at
    /// init). `persist()`/`set()` derive "is there unpersisted work"
    /// directly by comparing `cache.load()` against this — there is no
    /// separate dirty flag to fall out of sync (task-group-14 / #2's
    /// fix): a boolean `dirty` flag needed a second atomic write
    /// (`dirty.store(false)`) after every durable write, and a
    /// concurrent `increment()` landing inside that write's `.await`
    /// window could have its `dirty.store(true)` mark silently
    /// overwritten back to `false`, losing visibility of its delta.
    /// Deriving dirtiness from `cache != last_persisted` removes the
    /// redundant state entirely, so there is nothing to fall out of
    /// sync with.
    last_persisted: Arc<AtomicU64>,
    /// Guards `persist()`/`set()` so concurrent flushers don't race on
    /// the `write_through` + `last_persisted` update pair.
    persist_lock: Arc<Mutex<()>>,
}

impl Clone for RecordCounter {
    fn clone(&self) -> Self {
        Self {
            info_store: Arc::clone(&self.info_store),
            cache: Arc::clone(&self.cache),
            last_persisted: Arc::clone(&self.last_persisted),
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
        // Serialize against `persist()` so the two never race on
        // `write_through` + `last_persisted` (see `last_persisted`'s
        // doc for the race this closes).
        let _guard = self.persist_lock.lock().await;
        self.write_through(count).await?;
        self.last_persisted.store(count, Ordering::Release);
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
        Ok(())
    }

    /// Set the counter to an absolute value. Used by the doctor to
    /// reconcile the cached counter with a fresh count of records
    /// in the data store.
    pub async fn set_to(&self, n: u64) -> DbResult<()> {
        let cache = self.ensure_cache().await?;
        cache.store(n, Ordering::Release);
        self.persist().await
    }

    /// Flush the in-memory counter to the info_store if it differs
    /// from the last persisted value. No-op otherwise.
    ///
    /// "Differs" is computed by directly comparing `cache` against
    /// `last_persisted` — both before AND after taking `persist_lock`
    /// — rather than consulting a separate dirty flag. This closes the
    /// task-group-14 / #2 race: a concurrent `increment()` landing
    /// while `write_through` is in-flight bumps `cache` with no flag
    /// to lose track of, so the post-write re-check below reliably
    /// notices it and leaves work for the next `persist()` call
    /// instead of silently dropping the delta.
    pub async fn persist(&self) -> DbResult<()> {
        let cache = self.ensure_cache().await?;
        if cache.load(Ordering::Acquire) == self.last_persisted.load(Ordering::Acquire) {
            return Ok(());
        }
        let _guard = self.persist_lock.lock().await;
        // Re-check inside the lock — another task may have flushed.
        let cur = cache.load(Ordering::Acquire);
        if cur == self.last_persisted.load(Ordering::Acquire) {
            return Ok(());
        }
        self.write_through(cur).await?;
        // `cur` is exactly what we just wrote — record it as
        // persisted regardless of what `cache` holds NOW (a
        // concurrent `increment()` may have moved it further while
        // `write_through` awaited). Nothing is lost: if `cache` has
        // since diverged from `cur`, the next `persist()` call will
        // see that divergence and flush the remainder.
        self.last_persisted.store(cur, Ordering::Release);
        Ok(())
    }

    async fn ensure_cache(&self) -> DbResult<&AtomicU64> {
        if let Some(c) = self.cache.get() {
            return Ok(c);
        }
        let info_store = Arc::clone(&self.info_store);
        let last_persisted = Arc::clone(&self.last_persisted);
        self.cache
            .get_or_try_init(|| async move {
                let key = RecordKey::from_slice(count_key().as_bytes());
                let initial: u64 = match info_store.get(key).await {
                    Ok(bytes) => bincode::from_bytes(&bytes).map_err(|e| {
                        DbError::Codec(format!(
                            "Record counter blob is corrupt — refusing to default to 0 \
                             (that would let the next persist() overwrite the durable \
                             count with a bogus near-zero value): {e}"
                        ))
                    })?,
                    Err(DbError::NotFound(_)) => 0,
                    Err(e) => return Err(e),
                };
                last_persisted.store(initial, Ordering::Release);
                Ok(AtomicU64::new(initial))
            })
            .await
    }
}

#[async_trait]
impl Persistable for RecordCounter {
    async fn persist(&self) -> DbResult<()> {
        // Delegates to the inherent method; the trait just provides a
        // uniform flushing surface for `PersistRegistry`.
        RecordCounter::persist(self).await
    }
}

impl RecordCounter {
    async fn write_through(&self, count: u64) -> DbResult<()> {
        let key = RecordKey::from_slice(count_key().as_bytes());
        let bytes = bincode::to_bytes(&count)
            .map_err(|e| DbError::Codec(format!("Failed to serialize count: {}", e)))?;
        self.info_store.set(key, bytes).await?;
        Ok(())
    }
}
