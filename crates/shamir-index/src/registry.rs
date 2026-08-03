//! Lock-free index registry.
//!
//! Uses `scc::HashMap` (CAS-based) for both `id → backend` and
//! `name_interned → id` lookups. `next_id` is an `AtomicU32` —
//! `fetch_add(Relaxed)` is enough for unique-id generation since the
//! counter is single-source (no cross-process coordination).
//!
//! F-50 (#869, spike): `generation` is a monotonic `AtomicU64` bumped on
//! every successful `insert` / `remove_by_id`. It lets a tx cheaply detect
//! "an index2 backend was registered between my stage-time `all_backends()`
//! snapshot and my commit" without storing the full snapshot — capture the
//! generation once at stage, compare at commit. Each `by_id` entry also
//! records the generation at which THAT backend was inserted, so a commit
//! can ask for exactly the backends newer than its stage-time generation
//! (`backends_newer_than`) and re-derive posting ops for just those — never
//! re-planning (and thus never double-counting `BumpFtsStats` for) backends
//! the tx already planned against at stage time.

use crate::backend::{IndexBackend, IndexError};
use crate::state::IndexState;
use shamir_collections::THasher;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::Arc;

pub struct IndexRegistry {
    /// `(backend, inserted_at_generation, lifecycle_state)`. The generation
    /// tag is set by [`insert`](Self::insert) to the value it bumps
    /// `generation` to, so [`backends_newer_than`](Self::backends_newer_than)
    /// can filter without a second map lookup (and without the
    /// two-maps-out-of-sync hazard a parallel side-map would introduce). The
    /// `IndexState` slot (F-50 Step 3b) is the AUTHORITATIVE lifecycle state
    /// for a live backend: `IndexDescriptor.state` is a pure serialization
    /// carrier, and [`all_descriptors`](Self::all_descriptors) overwrites the
    /// cloned descriptor's `state` from this tuple entry so persistence always
    /// emits the registry's current truth. [`set_state`](Self::set_state)
    /// flips `Building → Ready` after a successful backfill without
    /// per-backend interior mutability (the Step 3a memo explicitly rejected
    /// that as more invasive — this is the F-50 Step 1 generation-tag pattern
    /// extended by one field).
    by_id: scc::HashMap<u32, (Arc<dyn IndexBackend>, u64, IndexState), THasher>,
    by_name: scc::HashMap<u64, u32, THasher>,
    next_id: AtomicU32,
    /// F-50: bumped on every successful `insert` / `remove_by_id`. Read with
    /// [`generation`](Self::generation) to gate commit-time re-derivation.
    generation: AtomicU64,
}

impl IndexRegistry {
    pub fn new() -> Self {
        Self {
            by_id: scc::HashMap::with_hasher(THasher::default()),
            by_name: scc::HashMap::with_hasher(THasher::default()),
            next_id: AtomicU32::new(1),
            generation: AtomicU64::new(0),
        }
    }

    /// F-50: current registry generation. Bumped (monotonic) whenever the
    /// set of queryable backends changes. The zero-overhead gate value for
    /// commit-time ops-plan re-derivation: a tx captures this at stage time
    /// and, at commit, skips re-derivation entirely unless it has advanced.
    pub fn generation(&self) -> u64 {
        self.generation.load(Ordering::Acquire)
    }

    /// Atomically allocate the next monotonic ID. Lock-free.
    pub fn allocate_id(&self) -> u32 {
        self.next_id.fetch_add(1, Ordering::Relaxed)
    }

    pub async fn insert(&self, backend: Arc<dyn IndexBackend>) -> Result<(), IndexError> {
        let d = backend.descriptor();
        let id = d.id;
        let name_interned = d.name_interned;

        // P0-2 (#958, sub-bug 2b) TOCTOU fix: publish to `by_id` / `by_name`
        // FIRST, then advance `generation` LAST via `fetch_max(Release)`.
        //
        // The OLD order (`fetch_add(generation)` BEFORE `insert_async`)
        // created a TOCTOU window: a reader that loaded `generation()`
        // AFTER the bump but iterated `by_id` BEFORE the `insert_async`
        // completed saw an advanced generation with a missing backend —
        // at commit, `current_gen == staged_gen` and re-derivation was
        // skipped entirely, permanently missing the backend's posting.
        //
        // The new order, combined with the stage-time reader reading
        // `generation()` FIRST (Acquire) THEN calling `all_backends()`,
        // establishes the invariant: any backend whose `insert()` advanced
        // `generation` to a value ≤ the reader's observed generation is
        // GUARANTEED already visible in `by_id` (by Release-Acquire: the
        // writer's `insert_async` happens-before its `fetch_max(Release)`,
        // which synchronizes-with the reader's `load(Acquire)`).
        //
        // The per-backend tag (`my_gen`) is computed as `current_gen + 1`.
        // Two concurrent inserts that read the same `current_gen` compute
        // the same `my_gen` — both publish with that tag and both
        // `fetch_max(my_gen)` (idempotent). `backends_newer_than(threshold)`
        // uses strict-greater-than, so both are returned for any
        // `threshold < my_gen`. A tag slightly below the final generation
        // value (due to a concurrent insert's `fetch_max` landing between
        // our load and our `fetch_max`) is harmless: it just means the
        // backend is included in a slightly wider `backends_newer_than`
        // filter — the resulting ops are idempotent (`SetPosting`
        // overwrites, `RemovePosting` is a no-op on absent keys).
        let my_gen = self.generation.load(Ordering::Acquire) + 1;

        // F-50 Step 3b: capture the descriptor's persisted `state` into the
        // authoritative tuple slot. For `create_index_v2` this is `Building`
        // at insert time (flipped to `Ready` via `set_state` once the backfill
        // completes); for the table-open path a `Ready` descriptor carries
        // `Ready`, a `Building` descriptor carries `Building` (the open-path
        // self-heal flips it to `Ready` after re-backfill). The tuple — NOT
        // `IndexDescriptor.state` — is the live source of truth.
        let state = d.state;
        self.by_id
            .insert_async(id, (backend.clone(), my_gen, state))
            .await
            .map_err(|_| IndexError::Backend(format!("index id {id} already registered")))?;
        self.by_name
            .insert_async(name_interned, id)
            .await
            .map_err(|_| {
                IndexError::Backend(format!("index name {name_interned} already registered"))
            })?;
        // P0-2 (2b): advance generation AFTER successful publish.
        self.generation.fetch_max(my_gen, Ordering::Release);
        Ok(())
    }

    /// F-50 Step 3b: set the authoritative lifecycle `state` for the backend
    /// registered under `id`. Used by `create_index_v2` (and the table-open
    /// self-heal) to flip `Building → Ready` once a backfill has completed —
    /// the backend's own `IndexDescriptor.state` is immutable and stays as
    /// the serialization carrier; this tuple slot is what
    /// [`all_descriptors`](Self::all_descriptors) reads when persisting.
    ///
    /// Returns `true` if the backend was found and updated, `false` if no
    /// backend is registered under `id` (no-op). Idempotent: setting `Ready`
    /// on an already-`Ready` backend is a cheap no-op write.
    pub async fn set_state(&self, id: u32, state: IndexState) -> bool {
        self.by_id
            .update_async(&id, |_, v| {
                v.2 = state;
            })
            .await
            .is_some()
    }

    /// F-50 Step 3b: the authoritative lifecycle `state` for the backend
    /// registered under `id`, or `None` if no backend is registered. Reads
    /// the tuple slot (not the descriptor clone) — this is the value the
    /// planner Ready-gate and the doctor consult.
    pub async fn state_of(&self, id: u32) -> Option<IndexState> {
        self.by_id.read_async(&id, |_, v| v.2).await
    }

    /// F-50: every backend whose insertion generation is strictly greater
    /// than `threshold_gen` — i.e. every backend registered AFTER the caller
    /// captured `threshold_gen` (typically at tx stage time). Used by the
    /// commit pipeline to re-derive posting ops for precisely the backends a
    /// stale stage-time `all_backends()` snapshot missed, without re-planning
    /// (and thus without double-applying `BumpFtsStats` for) backends the tx
    /// already planned against.
    #[allow(clippy::disallowed_methods)] // O(N) ack: filtered snapshot, off hot path (only when generation advanced)
    pub async fn backends_newer_than(&self, threshold_gen: u64) -> Vec<Arc<dyn IndexBackend>> {
        let mut out = Vec::new();
        self.by_id
            .iter_async(|_, (backend, gen, _state)| {
                if *gen > threshold_gen {
                    out.push(backend.clone());
                }
                true
            })
            .await;
        out
    }

    pub async fn get_by_id(&self, id: u32) -> Option<Arc<dyn IndexBackend>> {
        self.by_id.read_async(&id, |_, v| v.0.clone()).await
    }

    pub async fn get_by_name(&self, name_interned: u64) -> Option<Arc<dyn IndexBackend>> {
        let id = self.by_name.read_async(&name_interned, |_, v| *v).await?;
        self.get_by_id(id).await
    }

    pub async fn remove_by_id(&self, id: u32) -> Option<Arc<dyn IndexBackend>> {
        let removed = self.by_id.remove_async(&id).await.map(|(_, v)| v.0);
        if let Some(ref backend) = removed {
            let name_interned = backend.descriptor().name_interned;
            let _ = self.by_name.remove_async(&name_interned).await;
            // F-50: a removal changes the queryable backend set, so advance
            // the generation — a tx that staged against the now-removed
            // backend will re-derive (and find the backend absent, so it
            // contributes no ops; the now-orphan posting is a separate
            // drop-during-tx concern, scoped to Step 3's DDL cancellation).
            self.generation.fetch_add(1, Ordering::AcqRel);
        }
        removed
    }

    #[allow(clippy::disallowed_methods)] // O(N) ack: cardinality accessor, off hot path
    pub fn len(&self) -> usize {
        self.by_id.len()
    }

    pub fn is_empty(&self) -> bool {
        self.by_id.is_empty()
    }

    pub fn peek_next_id(&self) -> u32 {
        self.next_id.load(Ordering::Relaxed)
    }

    pub fn set_next_id(&self, id: u32) {
        self.next_id.store(id, Ordering::Relaxed);
    }

    /// Collect all registered backends (snapshot).
    #[allow(clippy::disallowed_methods)] // O(N) ack: Vec-capacity sizing at snapshot, off hot path
    pub async fn all_backends(&self) -> Vec<Arc<dyn IndexBackend>> {
        let mut out = Vec::with_capacity(self.by_id.len());
        self.by_id
            .iter_async(|_, v| {
                out.push(v.0.clone());
                true
            })
            .await;
        out
    }

    /// Collect all descriptors (for persistence). F-50 Step 3b: the cloned
    /// descriptor's `state` field is OVERWRITTEN from the authoritative tuple
    /// slot — `IndexDescriptor.state` as read from disk (or set at backend
    /// construction) is a pure serialization carrier; the registry tuple is
    /// the single source of truth for a LIVE backend's current state. This
    /// keeps `create_index_v2`'s `Building`-at-construction →
    /// `set_state(Ready)` flip correctly reflected in the persisted blob
    /// without any per-backend interior mutability.
    #[allow(clippy::disallowed_methods)] // O(N) ack: Vec-capacity sizing at snapshot, off hot path
    pub async fn all_descriptors(&self) -> Vec<crate::descriptor::IndexDescriptor> {
        let mut out = Vec::with_capacity(self.by_id.len());
        self.by_id
            .iter_async(|_, v| {
                let mut desc = v.0.descriptor().clone();
                desc.state = v.2;
                out.push(desc);
                true
            })
            .await;
        out
    }

    /// Update the `by_name` mapping from `old_name_interned` to `new_name_interned`
    /// without touching the physical posting entries (they are keyed by `index_id`, not
    /// by name). Returns `true` if the entry was found and updated, `false` otherwise.
    ///
    /// This is the rekey primitive for RENAME INDEX on index2 backends: since
    /// posting keys embed the compact `u32` id (not the interned string id), the
    /// stored data survives a rename without any scan/copy.
    pub async fn rename_entry(&self, old_name_interned: u64, new_name_interned: u64) -> bool {
        // Look up the numeric id behind the old name.
        let id = match self.by_name.read_async(&old_name_interned, |_, v| *v).await {
            Some(v) => v,
            None => return false,
        };
        // Remove old name mapping.
        let _ = self.by_name.remove_async(&old_name_interned).await;
        // Insert new name mapping. If insertion fails (new_name already registered)
        // re-insert the old mapping to keep the registry consistent, then return false.
        if self
            .by_name
            .insert_async(new_name_interned, id)
            .await
            .is_err()
        {
            // Restore old entry on conflict.
            let _ = self.by_name.insert_async(old_name_interned, id).await;
            return false;
        }
        true
    }

    /// Find a backend whose first field path matches and whose kind
    /// matches the given tag ("fts", "functional", "vector", "btree").
    ///
    /// F-50 Step 3b Ready-gate: a backend in `Building` state is INVISIBLE
    /// to this lookup — the planner and every read path that dispatches via
    /// `find_by_field_and_kind` fall through to a full scan as if the
    /// half-built index did not exist. This is the correctness anchor for
    /// restart-from-scratch: a `Building` backend's partial postings are
    /// safely droppable because no reader can have depended on them. DDL
    /// paths that need to reach a `Building` backend (e.g. `drop_index2`)
    /// resolve by name via [`get_by_name`](Self::get_by_name), which is
    /// intentionally NOT state-filtered.
    pub async fn find_by_field_and_kind(
        &self,
        field_path: &[u64],
        kind_tag: &str,
    ) -> Option<Arc<dyn IndexBackend>> {
        let mut found = None;
        self.by_id
            .iter_async(|_, (backend, _gen, state)| {
                // Planner Ready-gate: skip a Building backend so reads never
                // observe partial postings.
                if *state != IndexState::Ready {
                    return true;
                }
                let desc = backend.descriptor();
                let kind_matches = matches!(
                    (&desc.kind, kind_tag),
                    (crate::kind::IndexKind::Fts { .. }, "fts")
                        | (crate::kind::IndexKind::Functional(_), "functional")
                        | (crate::kind::IndexKind::Vector(_), "vector")
                        | (crate::kind::IndexKind::Btree { .. }, "btree")
                );
                if kind_matches && !desc.paths.is_empty() && desc.paths[0] == field_path {
                    found = Some(backend.clone());
                    return false;
                }
                true
            })
            .await;
        found
    }
}

impl Default for IndexRegistry {
    fn default() -> Self {
        Self::new()
    }
}
