use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use shamir_collections::{TFxSet, THasher};
use tokio::sync::OnceCell;

/// One name↔id mirror for a single `(db, repo)` interner.
///
/// `epoch` is the contiguous completeness watermark; sparse touch responses
/// do not advance it. `observed_epoch` tracks individual observed IDs.
///
/// `populated` coalesces concurrent initial dumps. Once populated, each call
/// may run its supplied operation (dump_repo promises unconditional re-dump).
pub struct FieldMap {
    name_to_id: scc::HashMap<String, u64, THasher>,
    id_to_name: scc::HashMap<u64, String, THasher>,
    epoch: AtomicU64,
    complete_epoch: AtomicU64,
    /// Set once the first full dump has merged. Concurrent first dump callers
    /// share one initializer, and failures leave it available for retry.
    populated: OnceCell<()>,
}

impl FieldMap {
    /// New empty field map with epoch 0 (nothing observed yet).
    pub fn new() -> Self {
        FieldMap {
            name_to_id: scc::HashMap::with_hasher(THasher::default()),
            id_to_name: scc::HashMap::with_hasher(THasher::default()),
            epoch: AtomicU64::new(0),
            complete_epoch: AtomicU64::new(0),
            populated: OnceCell::new(),
        }
    }

    pub fn complete_epoch(&self) -> u64 {
        self.complete_epoch.load(Ordering::Acquire)
    }

    /// Advance the completeness watermark only on a full or delta server response.
    pub fn set_complete_epoch(&self, candidate: u64) {
        let mut current = self.complete_epoch.load(Ordering::Acquire);
        while candidate > current {
            match self.complete_epoch.compare_exchange(
                current,
                candidate,
                Ordering::Release,
                Ordering::Acquire,
            ) {
                Ok(_) => break,
                Err(observed) => current = observed,
            }
        }
        self.advance_epoch(candidate);
    }

    /// Return the contiguous completeness watermark used for delta refresh.
    pub fn epoch(&self) -> u64 {
        self.complete_epoch()
    }

    /// Highest observed id from any source.
    pub fn observed_epoch(&self) -> u64 {
        self.epoch.load(Ordering::Acquire)
    }

    pub fn set_observed_epoch(&self, candidate: u64) {
        self.advance_epoch(candidate);
    }

    /// Explicitly set the completeness watermark from a full dump or delta.
    pub fn set_epoch(&self, candidate: u64) {
        self.set_complete_epoch(candidate);
    }

    /// CAS-max the observed high-water from any server mapping.
    fn advance_epoch(&self, candidate: u64) {
        let mut current = self.epoch.load(Ordering::Acquire);
        while candidate > current {
            match self.epoch.compare_exchange(
                current,
                candidate,
                Ordering::Release,
                Ordering::Acquire,
            ) {
                Ok(_) => break,
                Err(observed) => current = observed,
            }
        }
    }

    /// Insert a `(name, id)` pair into both directions.
    ///
    /// Idempotent: re-inserting an existing pair is a no-op. A conflicting
    /// re-insert (same name, different id) keeps the FIRST mapping — ids are
    /// monotonic + append-only, so a conflict is a server contract violation
    /// that we surface by keeping the stable first-seen value.
    pub fn insert_entry(&self, name: &str, id: u64) {
        let _ = self.name_to_id.insert_sync(name.to_string(), id);
        let _ = self.id_to_name.insert_sync(id, name.to_string());
        self.advance_epoch(id);
    }

    /// Insert a pair from a sparse server response without advancing the epoch.
    pub fn insert_sparse_entry(&self, name: &str, id: u64) {
        let _ = self.name_to_id.insert_sync(name.to_string(), id);
        let _ = self.id_to_name.insert_sync(id, name.to_string());
    }

    /// Look up the interner id for a field name. Returns `None` if the name
    /// is not yet cached.
    ///
    /// §9.4 guard: `name` is treated as an opaque STRING. The literal "42" is
    /// looked up as the field whose name is the three characters '4','2'; it
    /// is NEVER parsed into the integer 42 and used as an id. Ids come ONLY
    /// from server responses (inserted via [`insert_entry`]).
    pub fn id_of(&self, name: &str) -> Option<u64> {
        // `read` returns `Option<R>` where R is the closure return; the outer
        // Option is the key-presence signal, so the closure returns the bare
        // id and the outer Option IS our result. `name` (the key) is the
        // STRING being resolved — §9.4: we never parse it as a number.
        self.name_to_id.read_sync(name, |_, id| *id)
    }

    /// Reverse lookup: id → name.
    pub fn name_of(&self, id: u64) -> Option<String> {
        self.id_to_name.read_sync(&id, |_, name| name.clone())
    }

    /// Collect the names from `input` that are NOT yet cached.
    ///
    /// O(N) lock-free reads over the input. The returned `Vec` preserves
    /// input order and deduplicates; it is the candidate set for a single
    /// `interner_touch` roundtrip.
    pub fn missing_names<'a, I>(&self, input: I) -> Vec<String>
    where
        I: IntoIterator<Item = &'a str>,
    {
        let mut out = Vec::with_capacity(4);
        let mut seen = TFxSet::default();
        for name in input {
            if seen.insert(name) && self.id_of(name).is_none() {
                out.push(name.to_string());
            }
        }
        out
    }

    /// Number of cached entries (both maps are kept in lockstep).
    #[allow(clippy::disallowed_methods)] // O(N) ack: cache cardinality accessor, off hot path
    pub fn len(&self) -> usize {
        // `id_to_name.len()` avoids a transient inconsistency if a concurrent
        // insert hit only one map; in practice both are inserted back-to-back.
        self.id_to_name.len()
    }

    /// Whether the cache holds any entries.
    pub fn is_empty(&self) -> bool {
        self.id_to_name.is_empty()
    }

    /// `true` once the first full dump has completed.
    pub fn is_populated(&self) -> bool {
        self.populated.initialized()
    }

    /// Run a full dump, coalescing concurrent first callers while allowing
    /// subsequent calls to fetch again. Failed first attempts remain retryable.
    pub async fn ensure_populated<F, Fut>(&self, dump: F) -> Result<(), crate::error::ClientError>
    where
        F: FnOnce() -> Fut,
        Fut: std::future::Future<Output = Result<(), crate::error::ClientError>>,
    {
        if !self.populated.initialized() {
            self.populated.get_or_try_init(dump).await?;
            return Ok(());
        }
        dump().await
    }
}

impl Default for FieldMap {
    fn default() -> Self {
        FieldMap::new()
    }
}

/// Per-`(db, repo)` registry of [`FieldMap`]s.
///
/// Lock-free: a single `scc::HashMap` keyed by `(db, repo)`. Lookups and
/// inserts are CAS-based; the value is an `Arc<FieldMap>` so callers can
/// hold a stable reference across an `.await` without re-locking.
pub struct InternerCacheRegistry {
    maps: scc::HashMap<(String, String), Arc<FieldMap>, THasher>,
}

impl InternerCacheRegistry {
    /// New empty registry.
    pub fn new() -> Self {
        InternerCacheRegistry {
            maps: scc::HashMap::with_hasher(THasher::default()),
        }
    }

    /// Get the [`FieldMap`] for `(db, repo)`, creating an empty one on first
    /// access. The returned `Arc` is stable across calls — concurrent
    /// `get_or_create`s race the insert benignly; the loser discards its
    /// empty `FieldMap` and reuses the winner's.
    pub fn get_or_create(&self, db: &str, repo: &str) -> Arc<FieldMap> {
        let key = (db.to_string(), repo.to_string());
        // Fast path: already present. `read` returns `Option<R>`; the closure
        // clones the inner Arc, so the outer Option is the presence signal.
        if let Some(fm) = self.maps.read_sync(&key, |_, v| Arc::clone(v)) {
            return fm;
        }
        // Slow path: race the insert. Loser gets back the winner's Arc.
        let new_fm = Arc::new(FieldMap::new());
        match self.maps.insert_sync(key, Arc::clone(&new_fm)) {
            Ok(()) => new_fm,
            Err((recovered_key, _)) => {
                // Another caller won the race — hand back theirs.
                self.maps
                    .read_sync(&recovered_key, |_, v| Arc::clone(v))
                    // scc insert failed AND the read missed — should be
                    // unreachable given the prior insert. Fall back to a fresh
                    // map rather than panicking on a cache miss.
                    .unwrap_or(new_fm)
            }
        }
    }
}

impl Default for InternerCacheRegistry {
    fn default() -> Self {
        InternerCacheRegistry::new()
    }
}
