//! Interner manager for lazy loading and persistence

use async_trait::async_trait;

use crate::meta::MetaKey;
use crate::table::persistable::Persistable;
use bytes::Bytes;
use futures::StreamExt;
use shamir_storage::error::DbResult;
use shamir_storage::types::Store;
use shamir_tunables::store_defaults::MAINT_SCAN_BATCH;
use shamir_types::codecs::basic::bincode;
use shamir_types::core::interner::{Interner, InternerKey, UserKey};
use shamir_types::types::record_id::RecordId;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use tokio::sync::{Mutex, OnceCell};

/// Manages interned keys with lazy loading and persistence.
///
/// The interner is loaded lazily on first access. Persistence is
/// **incremental and append-only**: each `persist()` call writes only
/// the entries added since the previous call, as a fresh chunk record
/// in the info store. The old "rewrite the whole dictionary" path is
/// gone — on cold-write / schema-growth workloads it was O(N²) bytes
/// serialised and written across N first-touches; the chunked layout
/// makes the total O(N).
///
/// **On-disk layout (chunked):**
/// * Chunk N at `RecordId::system("i.dNNNNNNNNN")` (9-digit zero-padded
///   decimal index — fits in the 12-byte system-name budget). Each
///   chunk is `bincode::Vec<(InternerKey, UserKey)>` containing only
///   the entries added between persist call N-1 and N.
/// * Legacy single-blob format at `MetaKey::Internals` is still read on
///   boot (backward compatibility for repos written by older code).
///   New code does NOT write to `MetaKey::Internals`; the legacy blob
///   acts as the "chunk -1" seed.
///
/// **Boot reconstruction:**
/// 1. Read legacy `MetaKey::Internals` blob — apply if present.
/// 2. `scan_prefix_stream("\\0\\0\\0\\0i.d", ...)` — apply chunks in
///    lexicographic key order (which equals chunk-index order thanks
///    to zero-padded decimal).
/// 3. The forward + reverse mappings are reconstructed by
///    `Interner::with_state`, identical to the pre-chunking format.
///
/// **Concurrency:**
/// * `get()` is `Arc<OnceCell>` — at most one loader runs.
/// * `persist()` serialises writes through a `tokio::sync::Mutex` so
///   two concurrent persists don't race on the chunk index. The mutex
///   is only contended on the rare new-key path; reads never touch it.
/// * The forward (`UserKey → id`) and reverse (`id → UserKey`) maps
///   inside `Interner` keep their existing lock-free read semantics
///   (DashMap + ArcSwap); this manager only coordinates persistence.
///
/// Uses `Arc<OnceCell>` so all clones share the same interner.
pub struct InternerManager {
    info_store: Arc<dyn Store>,
    interner: Arc<OnceCell<Interner>>,
    /// Number of entries in the interner the last time `persist`
    /// actually wrote a chunk. Compared against `interner.len()` on
    /// each `persist()` call to skip no-op writes. Also identifies the
    /// "lower bound" of the delta to write on the next persist —
    /// because ids are dense 1..len, the entries with id in
    /// `(last_persisted_len, cur_len]` are exactly the new ones.
    last_persisted_len: Arc<AtomicUsize>,
    /// Index of the next chunk record to write. Initialised from
    /// `scan_prefix_stream` at boot (= number of existing chunks). Only
    /// touched under `persist_lock` so the increment-and-write pair is
    /// atomic relative to other persists.
    next_chunk_idx: Arc<AtomicUsize>,
    /// Serialises concurrent `persist()` calls so they don't race on
    /// `next_chunk_idx` / `last_persisted_len`. The mutex is only ever
    /// acquired when there's a real delta to write; read-only callers
    /// never touch it.
    persist_lock: Arc<Mutex<()>>,
}

/// System-record-name prefix for an incremental delta chunk. Combined
/// with a 9-digit zero-padded decimal index this fits in the 12-byte
/// `RecordId::system` name budget (`"i.d" + 9 = 12`).
const CHUNK_TAG_PREFIX: &str = "i.d";

/// 9-digit zero-padded decimal index → 10^9 chunks max. At one chunk
/// per persist on the schema-growth path that's 1B persists per repo,
/// way past any realistic working set.
fn chunk_record_id(idx: usize) -> RecordId {
    // `format!` is fine here — chunks are written at most once per
    // genuinely-new-key persist, and the bincode + storage write that
    // follows dwarfs an 11-byte format call.
    let tag = format!("{}{:09}", CHUNK_TAG_PREFIX, idx);
    RecordId::system(&tag)
}

/// Bytes prefix for `scan_prefix_stream` — 4 system zero-bytes +
/// the chunk-tag prefix. Matches every chunk record id without
/// matching the legacy `internals` blob (different second byte:
/// `'.'` vs `'n'`).
fn chunk_scan_prefix() -> Bytes {
    let mut prefix = Vec::with_capacity(4 + CHUNK_TAG_PREFIX.len());
    prefix.extend_from_slice(&[0u8, 0, 0, 0]);
    prefix.extend_from_slice(CHUNK_TAG_PREFIX.as_bytes());
    Bytes::from(prefix)
}

impl Clone for InternerManager {
    fn clone(&self) -> Self {
        Self {
            info_store: Arc::clone(&self.info_store),
            interner: Arc::clone(&self.interner),
            last_persisted_len: Arc::clone(&self.last_persisted_len),
            next_chunk_idx: Arc::clone(&self.next_chunk_idx),
            persist_lock: Arc::clone(&self.persist_lock),
        }
    }
}

impl InternerManager {
    /// Create a new interner manager
    pub fn new(info_store: Arc<dyn Store>) -> Self {
        Self {
            info_store,
            interner: Arc::new(OnceCell::new()),
            last_persisted_len: Arc::new(AtomicUsize::new(0)),
            next_chunk_idx: Arc::new(AtomicUsize::new(0)),
            persist_lock: Arc::new(Mutex::new(())),
        }
    }

    /// Get interner, loading it lazily on first access
    pub async fn get(&self) -> DbResult<&Interner> {
        if self.interner.get().is_some() {
            return Ok(self.interner.get().unwrap());
        }

        let info_store = Arc::clone(&self.info_store);
        let interner_cell = &self.interner;
        let last_persisted_len = Arc::clone(&self.last_persisted_len);
        let next_chunk_idx = Arc::clone(&self.next_chunk_idx);

        interner_cell
            .get_or_init(|| async move {
                // 1) Legacy single-blob seed — repos written by older
                //    code stored the whole dictionary under
                //    `MetaKey::Internals`. New code never writes here,
                //    but must still read it to upgrade old data in
                //    place. If absent (or unreadable / corrupt), we
                //    fall through to the chunk scan.
                let internals_id = MetaKey::Internals.as_record_id().to_bytes();
                let mut entries: Vec<(InternerKey, UserKey)> =
                    match info_store.get(internals_id).await {
                        Ok(bytes) => bincode::from_bytes(&bytes).unwrap_or_else(|e| {
                            log::error!("Failed to deserialize legacy interner blob: {}", e);
                            Vec::new()
                        }),
                        Err(_) => Vec::new(),
                    };

                // 2) Apply append-only delta chunks in order. The
                //    `scan_prefix_stream` API yields records in
                //    lexicographic key order; the 9-digit
                //    zero-padded decimal index ensures
                //    lexicographic order == numeric order.
                let prefix = chunk_scan_prefix();
                let mut stream = info_store.scan_prefix_stream(prefix, MAINT_SCAN_BATCH);
                let mut chunk_count: usize = 0;
                while let Some(batch_res) = stream.next().await {
                    match batch_res {
                        Ok(batch) => {
                            for (_key, val) in batch {
                                match bincode::from_bytes::<Vec<(InternerKey, UserKey)>>(&val) {
                                    Ok(chunk) => entries.extend(chunk),
                                    Err(e) => {
                                        log::error!(
                                            "Failed to deserialize interner delta chunk: {}",
                                            e
                                        );
                                    }
                                }
                                chunk_count += 1;
                            }
                        }
                        Err(e) => {
                            log::error!("Error scanning interner delta chunks: {}", e);
                            break;
                        }
                    }
                }

                // Seed both atomics so the first `persist()` after boot
                // writes the right chunk index and only emits the true
                // delta (= zero, since on-disk == in-memory at this
                // point).
                let total = entries.len();
                last_persisted_len.store(total, Ordering::Release);
                next_chunk_idx.store(chunk_count, Ordering::Release);

                Interner::with_state(entries)
            })
            .await;

        Ok(self.interner.get().unwrap())
    }

    /// Save new interned keys to storage as a single delta chunk.
    ///
    /// This is the explicit form callers can use when they have the
    /// new entries in hand. `persist()` is the more common path —
    /// it computes the delta from the interner's current length.
    pub async fn save_new_keys(&self, new_keys: &[(InternerKey, UserKey)]) -> DbResult<()> {
        if new_keys.is_empty() {
            return Ok(());
        }

        // Ensure the interner is loaded so `next_chunk_idx` /
        // `last_persisted_len` reflect any on-disk state. Without this,
        // a fresh `InternerManager` would write chunk 0 on top of an
        // existing chunk 0 from a prior process.
        let _ = self.get().await?;

        let _guard = self.persist_lock.lock().await;
        let idx = self.next_chunk_idx.load(Ordering::Acquire);
        let bytes = bincode::to_bytes(&new_keys.to_vec()).map_err(|e| {
            shamir_storage::error::DbError::Codec(format!(
                "Failed to serialize interner chunk: {}",
                e
            ))
        })?;
        self.info_store
            .set(chunk_record_id(idx).to_bytes(), bytes)
            .await?;
        self.next_chunk_idx.store(idx + 1, Ordering::Release);
        // `last_persisted_len` is advanced by `persist()` callers based
        // on `interner.len()`. For `save_new_keys`, advance by the
        // number of entries written — but only if it doesn't regress.
        let cur = self.last_persisted_len.load(Ordering::Acquire);
        let advanced = cur + new_keys.len();
        self.last_persisted_len.store(advanced, Ordering::Release);
        Ok(())
    }

    /// Persist the delta accumulated since the previous `persist()` as
    /// a fresh append-only chunk. No-op when nothing has been added.
    ///
    /// Callers (insert/update/set on the write path) invoke this after
    /// every op; thanks to the monotonic + length-tracking trick, only
    /// the ops that actually intern a new key pay the I/O cost — and
    /// that cost is O(delta), not O(total dictionary size).
    pub async fn persist(&self) -> DbResult<()> {
        let interner = self.get().await?;
        let cur_len = interner.len();
        let last = self.last_persisted_len.load(Ordering::Acquire);
        if cur_len == last {
            // Interner hasn't grown — disk copy is already current.
            return Ok(());
        }

        // Serialise concurrent persists so two writers can't pick the
        // same chunk index. The lock is only acquired on the rare
        // genuinely-new-key path (the fast no-op return above handles
        // every other call).
        let guard = self.persist_lock.lock().await;
        // Re-check under the lock — another persist may have raced us
        // and already written the same delta.
        let last = self.last_persisted_len.load(Ordering::Acquire);
        if interner.len() == last {
            drop(guard);
            return Ok(());
        }

        // Capture only the new entries (id > last). Uses the
        // reverse-vec atomic snapshot to avoid clone-the-world; under
        // concurrent `touch_ind` the returned `new_high` may be less
        // than `interner.len()` (the forward map can advance ahead of
        // the reverse vec by a window). We advance `last_persisted_len`
        // to exactly `new_high` so the very next persist picks up
        // anything that was mid-insert here.
        let (delta, new_high) = interner.entries_after(last);
        if delta.is_empty() {
            drop(guard);
            return Ok(());
        }

        let idx = self.next_chunk_idx.load(Ordering::Acquire);
        let bytes = bincode::to_bytes(&delta).map_err(|e| {
            shamir_storage::error::DbError::Codec(format!(
                "Failed to serialize interner chunk: {}",
                e
            ))
        })?;
        self.info_store
            .set(chunk_record_id(idx).to_bytes(), bytes)
            .await?;
        self.next_chunk_idx.store(idx + 1, Ordering::Release);
        self.last_persisted_len.store(new_high, Ordering::Release);
        drop(guard);
        Ok(())
    }
}

#[async_trait]
impl Persistable for InternerManager {
    async fn persist(&self) -> DbResult<()> {
        InternerManager::persist(self).await
    }
}
