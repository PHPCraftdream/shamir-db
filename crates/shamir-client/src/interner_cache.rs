//! Client-side registered-numeric field-map cache (Stage 5 minimal).
//!
//! The server interns field names to monotonic `u64` ids per repo. Ids are
//! append-only and NEVER reused or deleted (see
//! `shamir-types::core::interner`), so the client can mirror a repo's
//! `(name ↔ id)` mapping without invalidation logic — only fill-missing.
//!
//! This module owns two lock-free types:
//!
//! * [`FieldMap`] — one per `(db, repo)`. Two `scc::HashMap`s (forward +
//!   reverse) plus an `AtomicU64` epoch. A `tokio::sync::OnceCell<()>`
//!   guards the FIRST full dump so concurrent first-users don't stampede
//!   the server with N identical dump requests; later writers race the
//!   insert benignly (ids are stable).
//! * [`InternerCacheRegistry`] — `scc::HashMap<(db, repo), Arc<FieldMap>>`.
//!
//! Concurrency model: every access is lock-free (`scc` + atomics). The ONLY
//! sanctioned `tokio::sync` primitive is the `OnceCell` dump guard, because
//! it must hold across the `.await` on the server roundtrip — that is the
//! single justified async-mutex-style use in this module (CLAUDE.md pillar 1
//! exception: "Guard across `.await`, bounded contention → tokio::sync").

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use shamir_collections::{TFxSet, THasher};
use tokio::sync::OnceCell;

/// One name↔id mirror for a single `(db, repo)` interner.
///
/// Both directions are kept lock-free via `scc::HashMap`. The epoch is the
/// highest gap-free id the cache has observed from the server; it is the
/// cursor passed to `interner_dump(since=epoch)` for delta refreshes.
///
/// `populated` is set once the first full `interner_dump` succeeds. Concurrent
/// callers that reach `ensure_populated` before the first dump completes await
/// the same `OnceCell` future rather than each firing their own dump
/// (stampede guard).
pub struct FieldMap {
    name_to_id: scc::HashMap<String, u64, THasher>,
    id_to_name: scc::HashMap<u64, String, THasher>,
    epoch: AtomicU64,
    /// Set once the first full dump has merged. Held across the dump
    /// roundtrip `.await`, so `tokio::sync` is the sanctioned primitive
    /// (see module docs). `()` carries no data — the maps themselves are
    /// the state.
    populated: OnceCell<()>,
}

impl FieldMap {
    /// New empty field map with epoch 0 (nothing observed yet).
    pub fn new() -> Self {
        FieldMap {
            name_to_id: scc::HashMap::with_hasher(THasher::default()),
            id_to_name: scc::HashMap::with_hasher(THasher::default()),
            epoch: AtomicU64::new(0),
            populated: OnceCell::new(),
        }
    }

    /// Highest gap-free id the cache has observed.
    pub fn epoch(&self) -> u64 {
        self.epoch.load(Ordering::Acquire)
    }

    /// Explicitly set the epoch (e.g. from a server dump/touch response's
    /// `epoch` field, which is the gap-free high-water mark and may exceed
    /// individual ids). CAS-maxes so it never decreases.
    pub fn set_epoch(&self, candidate: u64) {
        self.advance_epoch(candidate);
    }

    /// CAS-max the epoch: advance to `candidate` only if it is greater than
    /// the current value. Monotonic — never decreases.
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
        // Forward direction. Ignore the (name, id) return on collision —
        // first writer wins (see doc comment).
        let _ = self.name_to_id.insert(name.to_string(), id);
        let _ = self.id_to_name.insert(id, name.to_string());
        self.advance_epoch(id);
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
        self.name_to_id.read(name, |_, id| *id)
    }

    /// Reverse lookup: id → name.
    pub fn name_of(&self, id: u64) -> Option<String> {
        self.id_to_name.read(&id, |_, name| name.clone())
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

    /// Stampede guard for the first dump.
    ///
    /// Returns a `&OnceCell<()>` the caller initials with the dump
    /// roundtrip: `fieldmap.ensure_populated(|| async { dump }).await`.
    /// The first caller runs the closure; concurrent callers await the same
    /// future. After it resolves, `is_populated()` is `true`.
    pub async fn ensure_populated<F, Fut>(&self, dump: F)
    where
        F: FnOnce() -> Fut,
        Fut: std::future::Future<Output = ()>,
    {
        // OnceCell::get_or_init takes the closure; the closure must NOT
        // borrow `self` (it would create a self-referential future). The
        // caller-owned closure performs the roundtrip + merge.
        self.populated.get_or_init(dump).await;
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
        if let Some(fm) = self.maps.read(&key, |_, v| Arc::clone(v)) {
            return fm;
        }
        // Slow path: race the insert. Loser gets back the winner's Arc.
        let new_fm = Arc::new(FieldMap::new());
        match self.maps.insert(key, Arc::clone(&new_fm)) {
            Ok(()) => new_fm,
            Err((recovered_key, _)) => {
                // Another caller won the race — hand back theirs.
                self.maps
                    .read(&recovered_key, |_, v| Arc::clone(v))
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
