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
//!
//! R0-A (#1006): `insert()`'s per-entry tag and the published watermark are
//! the SAME counter (`generation.fetch_add(1, Release) + 1`) — see
//! [`IndexRegistry::insert`] for why a decoupled ticket counter (the P1/#992
//! design this replaces) is no longer needed now that every registry-
//! mutating DDL op (CREATE/DROP/RENAME across all four index families) holds
//! `TableManager::ddl_admission` for its ENTIRE critical section, INCLUDING
//! the `insert`/`remove_by_id` call and the generation bump
//! (`crates/shamir-engine/src/table/table_manager.rs`'s `begin_write_barrier`
//! — see that method's doc and R0-A's brief,
//! `docs/dev-artifacts/prompts/ddl-lifecycle/03-registry-watermark-and-full-admission.md`,
//! for the full derivation). With at most one registry mutation ever in
//! flight per table, a single monotonic counter can serve both roles without
//! reintroducing the read-then-write race `insert_ticket` was built to avoid.

use crate::backend::{IndexBackend, IndexError};
use crate::state::IndexState;
use shamir_collections::THasher;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::Arc;

/// Authoritative per-backend record held in [`IndexRegistry::by_id`].
///
/// The fields here are the SINGLE SOURCE OF TRUTH for a live backend's
/// mutable identity and lifecycle state. The backend's own
/// `IndexDescriptor` (returned by [`IndexBackend::descriptor`]) is an
/// immutable snapshot built at construction time; its `state`, `name`, and
/// `name_interned` fields are OVERRIDDEN from these slots at persistence time
/// (see [`IndexRegistry::all_descriptors`]) so the persisted blob always
/// reflects the registry's current truth, not the stale construction-time
/// snapshot.
///
/// This mirrors the proven `state`-override pattern (F-50 Step 3b) extended
/// to `name`/`name_interned` (P0-5a / #961): a RENAME INDEX updates these
/// slots so the new name round-trips through `save_index2_metadata` and
/// survives a restart. Without the name slots, the persisted descriptor
/// carried the stale original name and the rename was silently reverted on
/// reopen.
struct BackendEntry {
    backend: Arc<dyn IndexBackend>,
    /// Generation at which this backend was inserted (F-50). Set by
    /// [`insert`](IndexRegistry::insert) to the value it bumps `generation`
    /// to, so [`backends_newer_than`](IndexRegistry::backends_newer_than) can
    /// filter without a second map lookup (and without the
    /// two-maps-out-of-sync hazard a parallel side-map would introduce).
    gen: u64,
    /// Authoritative lifecycle state — overrides `descriptor().state`.
    /// [`set_state`](IndexRegistry::set_state) flips `Building → Ready` after
    /// a successful backfill without per-backend interior mutability (the
    /// Step 3a memo explicitly rejected that as more invasive).
    state: IndexState,
    /// Authoritative human-readable name — overrides `descriptor().name`
    /// (P0-5a / #961).
    name: String,
    /// Authoritative interned name — overrides `descriptor().name_interned`
    /// (P0-5a / #961). Kept in lockstep with [`name`](Self::name) and with the
    /// `by_name` reverse index.
    name_interned: u64,
    /// R0-D (#1013): human-readable reason the backend was moved to
    /// [`IndexState::Failed`], if any. Process-local, in-memory only — NOT
    /// persisted (`all_descriptors()` does not read it; `IndexDescriptor`
    /// carries no matching field, so a restart loses the specific message
    /// but not the `Failed` state itself, which self-heals via the same
    /// recovery path that set it). `None` for every state other than
    /// `Failed`. Surfaced by `doctor::verify()`'s `Index2Health::message`.
    failure_reason: Option<String>,
}

pub struct IndexRegistry {
    by_id: scc::HashMap<u32, BackendEntry, THasher>,
    by_name: scc::HashMap<u64, u32, THasher>,
    next_id: AtomicU32,
    /// F-50: bumped on every successful `insert` / `remove_by_id`. Read with
    /// [`generation`](Self::generation) to gate commit-time re-derivation.
    ///
    /// R0-A (#1006): also THE per-entry generation tag `insert()` stamps on
    /// a new `BackendEntry` — see [`insert`](Self::insert)'s doc for why one
    /// counter now safely serves both roles (the caller's admission guard
    /// guarantees at most one registry mutation is ever in flight per
    /// table, so the old decoupled-ticket scheme's concurrency concern no
    /// longer applies).
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

    /// R0-A (#1006) test-only: read the insertion-generation tag recorded
    /// for the backend registered under `id`. Exercised by
    /// `concurrent_inserts_get_distinct_generation_tags` (still meaningful
    /// post-merge: `generation.fetch_add` guarantees distinct tags even
    /// though it's now the same counter as the published watermark).
    #[cfg(test)]
    pub(crate) async fn entry_gen(&self, id: u32) -> Option<u64> {
        self.by_id.read_async(&id, |_, e| e.gen).await
    }

    /// Register `backend` under both `by_id` and `by_name`.
    ///
    /// # Precondition (R0-C / #1009): caller must hold `ddl_admission`
    ///
    /// This method is check-then-act: it reads `by_name` to reject a name
    /// collision BEFORE mutating either map (see below), which is safe from
    /// a concurrent second `insert()` on the SAME table ONLY because R0-A
    /// (#1006) already guarantees `insert()` is called EXCLUSIVELY while the
    /// caller holds `TableManager::ddl_admission` for that table — every
    /// registry-mutating DDL op (CREATE/DROP/RENAME, all four index
    /// families) takes that lock for its entire critical section, including
    /// this call. That serializes every caller of `insert` on a given table
    /// to at most one in flight at a time, so this check-then-act cannot be
    /// beaten by a second concurrent `insert()` racing the same name (the
    /// client-driven CREATE paths all go through admission; open-path
    /// recovery in `TableManager::create` is single-task sequential by
    /// construction, so no concurrent caller exists there either). Calling
    /// this directly on a bare `IndexRegistry` with no external admission
    /// guard (as some registry-level tests do) reintroduces a TOCTOU race —
    /// acceptable there only because those tests don't exercise concurrent
    /// callers against the same name.
    ///
    /// # Atomicity (#1009 fix)
    ///
    /// Before this fix, `insert()` published to `by_id` FIRST and `by_name`
    /// SECOND — if the `by_name` publish failed (name already taken), the
    /// function returned `Err` WITHOUT rolling back `by_id`, leaving an
    /// orphan backend visible by id (to `all_backends()`/
    /// `backends_newer_than()` and any planner path that iterates by id) but
    /// unreachable by name (a `DROP` by that name would never find it). The
    /// fix checks name availability FIRST via a cheap `contains_async`, and
    /// returns `Err` without touching `by_id` at all if the name is taken —
    /// "check before mutate" instead of "mutate then maybe roll back".
    pub async fn insert(&self, backend: Arc<dyn IndexBackend>) -> Result<(), IndexError> {
        let d = backend.descriptor();
        let id = d.id;
        let name_interned = d.name_interned;

        // #1009: reject a name collision BEFORE touching `by_id` at all —
        // see this method's doc for why this is safe under the caller's
        // `ddl_admission` guarantee.
        if self.by_name.contains_async(&name_interned).await {
            return Err(IndexError::Backend(format!(
                "index name {name_interned} already registered"
            )));
        }

        // R0-A (#1006): `my_gen` — this entry's per-entry tag AND, once
        // published below, the new watermark `generation()` returns — is
        // now drawn from THE SAME counter, not a decoupled `insert_ticket`.
        //
        // Why a plain `load() + 1` is safe here (it was NOT, under the
        // P0-2/#958-2b code this directly restores, nor under the P1/#992
        // ticket scheme this replaces): every registry-mutating DDL op —
        // CREATE/DROP/RENAME across all four index families — now holds
        // `TableManager::ddl_admission` (via `begin_write_barrier`) for its
        // ENTIRE critical section, INCLUDING this `insert()` call (see
        // `begin_write_barrier`'s doc in
        // `crates/shamir-engine/src/table/table_manager.rs`). That
        // serializes every caller of `insert`/`remove_by_id` on a given
        // table to at most one in flight at a time — so there is no second
        // concurrent caller left to race this `load()` against and collide
        // on the same computed tag (the exact hazard `insert_ticket`'s
        // `fetch_add` existed to close). The old two-counter split was
        // needed ONLY because a decoupled `fetch_add` could remain correct
        // under true concurrent callers while `generation.load() + 1`
        // could not; now that admission rules out concurrent callers
        // entirely, that extra machinery is dead weight, not a safety net.
        //
        // Publish order is UNCHANGED from P0-2 (#958, sub-bug 2b) and MUST
        // stay this way even though single-counter-safe: publish to
        // `by_id`/`by_name` FIRST, advance `generation` to `my_gen` LAST
        // (via `fetch_max`, below) — a reader observing `generation() == N`
        // must still be guaranteed every entry tagged `<= N` is already
        // visible in `by_id`. Bumping the counter here (before publish)
        // would reintroduce the exact TOCTOU window P0-2 closed: a stage-
        // time reader could observe the advanced generation with the entry
        // still missing from `by_id`.
        let my_gen = self.generation.load(Ordering::Acquire) + 1;

        // F-50 Step 3b + P0-5a (#961): capture the descriptor's `state`,
        // `name`, and `name_interned` into the authoritative entry slots. For
        // `create_index_v2` `state` is `Building` at insert time (flipped to
        // `Ready` via `set_state` once the backfill completes); for the
        // table-open path a `Ready` descriptor carries `Ready`, a `Building`
        // descriptor carries `Building` (the open-path self-heal flips it to
        // `Ready` after re-backfill). The entry — NOT
        // `IndexDescriptor.state`/`.name`/`.name_interned` — is the live source
        // of truth; `rename_entry` later mutates the name slots here so the
        // rename survives persistence.
        let state = d.state;
        let name = d.name.clone();
        self.by_id
            .insert_async(
                id,
                BackendEntry {
                    backend: backend.clone(),
                    gen: my_gen,
                    state,
                    name,
                    name_interned,
                    failure_reason: None,
                },
            )
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
            .update_async(&id, |_, e| {
                e.state = state;
                // A transition to any state OTHER than `Failed` clears a
                // stale failure reason from a previous failed attempt (e.g.
                // `doctor::repair()` successfully re-healing a `Failed`
                // backend back to `Ready`). `set_failed` is the only path
                // that populates this field going forward.
                if state != IndexState::Failed {
                    e.failure_reason = None;
                }
            })
            .await
            .is_some()
    }

    /// R0-D (#1013): set the authoritative lifecycle state to
    /// [`IndexState::Failed`] and record `reason` as the operator-facing
    /// diagnostic (surfaced by `doctor::verify()`). Used by the table-open
    /// recovery path when a `drop_all` (Building self-heal) or
    /// `restore_on_open` call genuinely fails — fail CLOSED instead of
    /// leaving the backend at whatever state it had before the failed
    /// recovery attempt. Returns `true` if the backend was found and
    /// updated, `false` if no backend is registered under `id` (no-op).
    pub async fn set_failed(&self, id: u32, reason: impl Into<String>) -> bool {
        let reason = reason.into();
        self.by_id
            .update_async(&id, |_, e| {
                e.state = IndexState::Failed;
                e.failure_reason = Some(reason.clone());
            })
            .await
            .is_some()
    }

    /// F-50 Step 3b: the authoritative lifecycle `state` for the backend
    /// registered under `id`, or `None` if no backend is registered. Reads
    /// the tuple slot (not the descriptor clone) — this is the value the
    /// planner Ready-gate and the doctor consult.
    pub async fn state_of(&self, id: u32) -> Option<IndexState> {
        self.by_id.read_async(&id, |_, e| e.state).await
    }

    /// R0-D (#1013): the recorded failure reason for the backend registered
    /// under `id`, if it is (or was) `Failed`. `None` if no backend is
    /// registered, or if the backend has never been marked `Failed` (or was
    /// healed back to a non-`Failed` state since — see [`set_state`]'s
    /// clearing behavior). Consulted by `doctor::verify()`'s
    /// `Index2Health::message`.
    pub async fn failure_reason_of(&self, id: u32) -> Option<String> {
        self.by_id
            .read_async(&id, |_, e| e.failure_reason.clone())
            .await
            .flatten()
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
            .iter_async(|_, entry| {
                if entry.gen > threshold_gen {
                    out.push(entry.backend.clone());
                }
                true
            })
            .await;
        out
    }

    pub async fn get_by_id(&self, id: u32) -> Option<Arc<dyn IndexBackend>> {
        self.by_id.read_async(&id, |_, e| e.backend.clone()).await
    }

    pub async fn get_by_name(&self, name_interned: u64) -> Option<Arc<dyn IndexBackend>> {
        let id = self.by_name.read_async(&name_interned, |_, v| *v).await?;
        self.get_by_id(id).await
    }

    pub async fn remove_by_id(&self, id: u32) -> Option<Arc<dyn IndexBackend>> {
        let removed = self.by_id.remove_async(&id).await;
        if let Some((_, entry)) = removed {
            // P0-5a (#961): unlink `by_name` using the AUTHORITATIVE
            // interned name from the entry (NOT `backend.descriptor()`'s
            // construction-time snapshot). A backend renamed after
            // construction has its current name only here; reading the
            // backend's own `descriptor().name_interned` would try to remove
            // the STALE old key from `by_name`, leaving a dangling entry
            // under the new name.
            let _ = self.by_name.remove_async(&entry.name_interned).await;
            // F-50: a removal changes the queryable backend set, so advance
            // the generation — a tx that staged against the now-removed
            // backend will re-derive (and find the backend absent, so it
            // contributes no ops; the now-orphan posting is a separate
            // drop-during-tx concern, scoped to Step 3's DDL cancellation).
            self.generation.fetch_add(1, Ordering::AcqRel);
            Some(entry.backend)
        } else {
            None
        }
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
            .iter_async(|_, entry| {
                out.push(entry.backend.clone());
                true
            })
            .await;
        out
    }

    /// Collect all descriptors (for persistence). F-50 Step 3b + P0-5a (#961):
    /// the cloned descriptor's `state`, `name`, and `name_interned` fields are
    /// OVERWRITTEN from the authoritative entry slots — the backend's own
    /// `IndexDescriptor` (as read from disk or set at construction) is a pure
    /// serialization carrier; the registry entry is the single source of truth
    /// for a LIVE backend's current identity/state. This keeps
    /// `create_index_v2`'s `Building`-at-construction → `set_state(Ready)` flip
    /// AND `rename_entry`'s name change correctly reflected in the persisted
    /// blob without any per-backend interior mutability.
    #[allow(clippy::disallowed_methods)] // O(N) ack: Vec-capacity sizing at snapshot, off hot path
    pub async fn all_descriptors(&self) -> Vec<crate::descriptor::IndexDescriptor> {
        let mut out = Vec::with_capacity(self.by_id.len());
        self.by_id
            .iter_async(|_, entry| {
                let mut desc = entry.backend.descriptor().clone();
                desc.state = entry.state;
                desc.name = entry.name.clone();
                desc.name_interned = entry.name_interned;
                out.push(desc);
                true
            })
            .await;
        out
    }

    /// Update the `by_name` mapping from `old_name_interned` to
    /// `new_name_interned` without touching the physical posting entries (they
    /// are keyed by `index_id`, not by name). Returns `true` if the entry was
    /// found and updated, `false` otherwise.
    ///
    /// This is the rekey primitive for RENAME INDEX on index2 backends: since
    /// posting keys embed the compact `u32` id (not the interned string id), the
    /// stored data survives a rename without any scan/copy.
    ///
    /// `new_name` (the human-readable string) and `new_name_interned` are BOTH
    /// written to the authoritative `by_id` entry so that
    /// [`all_descriptors`](Self::all_descriptors) — the persistence path —
    /// emits the new name. Without this, the persisted descriptor carried the
    /// stale construction-time name and the rename was silently reverted on
    /// restart (P0-5a / #961).
    ///
    /// # Consistency / failure model
    ///
    /// `by_name` is updated first (with its existing conflict-rollback for the
    /// destination-already-taken case); the `by_id` entry update runs LAST via
    /// `update_async`. `scc::HashMap::update_async` takes `FnOnce` and returns
    /// `None` ONLY when the key is absent — and the key (`id`) was just
    /// resolved from `by_name`, so it is guaranteed present unless a concurrent
    /// [`remove_by_id`](Self::remove_by_id) unlinked it in the interim. That
    /// remove-vs-rename race is pre-existing (it already leaves a dangling
    /// `by_name` entry under the current code) and is out of scope for this
    /// fix; under the engine's DDL `rename_index` path the backend is
    /// guaranteed present. No additional rollback logic is needed: if the
    /// entry was concurrently removed there is nothing left to persist for it,
    /// and the backend's own immutable descriptor never changed.
    pub async fn rename_entry(
        &self,
        old_name_interned: u64,
        new_name: String,
        new_name_interned: u64,
    ) -> bool {
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
        // P0-5a (#961): update the authoritative name/name_interned slots in
        // the by_id entry so all_descriptors() (the persistence path) emits
        // the NEW name. Mirrors set_state's update_async pattern. See the
        // method doc for why this cannot fail in practice and needs no
        // rollback.
        self.by_id
            .update_async(&id, |_, entry| {
                entry.name = new_name;
                entry.name_interned = new_name_interned;
            })
            .await;
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
            .iter_async(|_, entry| {
                // Planner Ready-gate: skip a Building backend so reads never
                // observe partial postings.
                if entry.state != IndexState::Ready {
                    return true;
                }
                let desc = entry.backend.descriptor();
                let kind_matches = matches!(
                    (&desc.kind, kind_tag),
                    (crate::kind::IndexKind::Fts { .. }, "fts")
                        | (crate::kind::IndexKind::Functional(_), "functional")
                        | (crate::kind::IndexKind::Vector(_), "vector")
                        | (crate::kind::IndexKind::Btree { .. }, "btree")
                );
                if kind_matches && !desc.paths.is_empty() && desc.paths[0] == field_path {
                    found = Some(entry.backend.clone());
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
