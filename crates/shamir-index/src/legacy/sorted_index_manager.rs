//! Sorted (B-tree-by-value) index manager.
//!
//! Parallel to the hash-based `IndexManager`. Where hash indexes
//! answer **equality** lookups (`field == value`), sorted indexes
//! answer **range / order / min** queries by encoding the indexed
//! value into bytes that sort the same way the value does (see
//! `shamir_types::core::sort_codec`) and storing one info-store
//! record per `(value, record_id)` pair.
//!
//! What's supported in this first cut:
//! - Single-field index over a scalar column (Int / Float / String /
//!   Bool / U64).
//! - Range queries: between / gt / gte / lt / lte.
//! - `order by field asc + limit K` (forward scan, stop after K).
//! - `min(field)` (first record from prefix scan).
//!
//! Not yet:
//! - `max(field)`, `order by desc` — needs reverse iteration on the
//!   Store trait (next).
//! - Composite sorted index over multiple columns.

use bytes::Bytes;
use futures::StreamExt;
use smallvec::SmallVec;
use std::collections::BTreeSet;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
// Re-export so existing callers that import `SortedIndexDefinition` from this
// module continue to compile unchanged after the type moved to its own file.
pub use crate::legacy::sorted_index_definition::SortedIndexDefinition;
use crate::legacy::sorted_index_definition::{
    SortedIndexDefinitionNoState, SortedIndexDefinitionV1, SORTED_TAG,
};
use crate::write_ops::IndexWriteOp;
use shamir_storage::error::DbResult;
use shamir_storage::types::RecordKey;
use shamir_storage::types::Store;
use shamir_tunables::store_defaults::MAINT_SCAN_BATCH;
use shamir_types::core::interner::{Interner, InternerKey};
use shamir_types::core::sort_codec;
use shamir_types::record_view::RecordRef;
use shamir_types::record_view::ScalarRef;
use shamir_types::types::common::THasher;
use shamir_types::types::record_id::RecordId;
use shamir_types::types::value::{InnerValue, QueryValue};

/// Manages a set of sorted indexes for one table.
///
/// # Storage
///
/// Definitions live in a `NodeReplicated<Vec<SortedIndexDefinition>>` — a
/// NUMA-aware, read-mostly RCU-style snapshot. Reads (`iter_indexes`/
/// `has_indexes`/`find_by_*`/`has_covering_indexes`) are lock-free against
/// the calling thread's node-local `Arc<Vec<...>>` replica; writes
/// (`register`/`drop_index`/`rename_definition`/`intern_included_paths`)
/// copy-on-write via `NodeReplicated::rcu` and mirror to all per-node
/// replicas. On single-socket machines (dev, Windows, CI) there is exactly
/// one replica, giving identical performance to a bare `ArcSwap`. On
/// multi-socket NUMA machines each node reads its own replica without
/// crossing a socket interconnect.
///
/// Replaces the previous sharded `DashMap` whose per-shard read-locks
/// fired on every `plan_record_*` (singular insert path) and every
/// batch — mirrors the refactor that `IndexInfo` got in N3.
///
/// # Persistence
///
/// Persisted as a single system record under
/// `RecordId::system("sorted_indexes")` so we can reload on restart.
///
/// # Cardinality assumption
///
/// Typical workloads have ≤ ~10 sorted indexes per table — linear scan
/// over the Vec is cache-friendly and beats DashMap shard locks; matches
/// N3's profile of IndexInfo.
pub struct SortedIndexManager {
    info_store: Arc<dyn Store>,
    /// NUMA-aware RCU snapshot of `Vec<SortedIndexDefinition>`.
    /// Each NUMA node owns its own cache-padded replica so reads never
    /// cross a socket interconnect. Writes copy-on-write on node 0 and
    /// mirror to all other nodes.
    ///
    /// **Shared across clones via `Arc`.** `TableManager` (and hence this
    /// manager) is cloned on every `get_table()` — the DDL path
    /// (`create_sorted_index*` → `register`) and the read path
    /// (`iter_indexes`) each hold their own `TableManager` clone. A
    /// per-clone `NodeReplicated` (the previous design) desynced: a
    /// `register` on the DDL clone COW-updated only that clone's replicas,
    /// so the next read clone (snapshotted from the OnceCell primary, whose
    /// replicas were never touched) saw zero indexes. Wrapping in `Arc`
    /// makes every clone observe the same `NodeReplicated`, so any clone's
    /// `register`/`drop_index`/`rename`/`intern_included_paths` is visible
    /// to every other clone — mirroring how the sibling `IndexManager`
    /// shares its `Arc<IndexInfo>` and how `TableManager` shares
    /// `bindings_len`/`validator_bindings` through `Arc`.
    indexes: Arc<shamir_numa::NodeReplicated<Vec<SortedIndexDefinition>>>,
    /// F-50 Step 2 (#870, Part D): monotonic generation counter bumped on
    /// every `register` / `drop_index` (the two operations that change the
    /// queryable def set). Mirrors `IndexRegistry::generation` for the
    /// legacy sorted-index path: a tx captures this at stage time
    /// (`note_sorted_stage_gen`) and, at commit Phase 2.7, re-derives
    /// sorted posting ops for tables whose generation advanced. The zero-
    /// overhead gate value: a single Acquire load short-circuits when no
    /// sorted DDL happened between stage and commit.
    ///
    /// `Arc`-shared across clones for the SAME reason `indexes` is: a
    /// `register` / `drop_index` on one clone must be visible to every
    /// other clone's `generation()` read, or a commit-pipeline clone
    /// (snapshotted from the OnceCell primary) would see a stale
    /// generation and skip re-derivation. Mirrors how `indexes` itself is
    /// shared.
    generation: Arc<AtomicU64>,

    /// F-67 (#893): PER-INDEX "last mutation version" high-water — the MVCC
    /// commit version of the most recent write that applied a posting
    /// (create / update / delete) to THIS SPECIFIC index (keyed by
    /// `name_interned`), not to the manager as a whole.
    ///
    /// Was a single manager-wide `AtomicU64` (F-53b Step 2, #878) until an
    /// independent readonly review (snapshot `e145b1d3`, section P1-4)
    /// flagged that the AsOf cursor index-seek fast path
    /// (`read_as_of_keyset_seek`) always plans against ONE specific sorted
    /// index, so a manager-wide high-water disabled the fast path for every
    /// cursor on the table whenever ANY sorted index mutated — not just the
    /// one the cursor actually reads. This is a scope-narrowing refactor
    /// only: the safety argument is unchanged, just re-derived per-index
    /// (see [`Self::last_mutation_version`] / [`Self::note_mutation_at_version`]
    /// docs below).
    ///
    /// The gate for the AsOf-aware cursor index-seek fast path
    /// (`read_as_of_keyset_seek`): a cursor pinned at `pinned_version`,
    /// planned against index `name_interned`, may ONLY use the seek when
    /// `last_mutation_version(name_interned) <= pinned`, because only then
    /// does the current-state index provably mirror the pinned snapshot's
    /// postings for THAT index. When a concurrent write to THAT SAME index
    /// has advanced this past the pin, a current-state index CANNOT
    /// correctly place a row whose pinned-version posting was MOVED (UPDATE
    /// to the indexed field) or REMOVED (DELETE) after the pin — proven by
    /// F-53b Step 1's two negative tests. The seek declines and the existing
    /// full-rescan `read_as_of` path handles the page instead (correct, just
    /// O(N) not O(page_size)). A false-negative gate (bumping when the SAME
    /// index changed but not in a way that affects this cursor's page)
    /// costs ONE fallback to the already-correct scan, never a correctness
    /// bug. Mutating an UNRELATED index no longer bumps this cursor's gate
    /// at all — that's the whole point of keying by `name_interned`.
    ///
    /// **Bumped at APPLY time, NOT stage time.** Sorted-index ops are staged
    /// into `tx.index_write_set` at STAGE time and applied at commit Phase
    /// 5c (`apply_index_batch`); bumping at stage time would let an
    /// uncommitted (possibly-aborting) tx disable the fast path for
    /// unrelated cursors. The non-tx direct CRUD path bumps inside
    /// `on_record_*` (that path's own apply point). Both wire points pass
    /// the write's MVCC commit version, never a stage-time placeholder.
    ///
    /// An index that has never been mutated has no entry in the map and
    /// reads as epoch `0` — the same default-empty semantics the old
    /// manager-wide counter had at construction.
    ///
    /// F-71 (#898): a fresh in-memory `scc::HashMap` (below) has NO entries
    /// at construction — including right after [`load`](Self::load) hydrates
    /// `indexes` from disk on a restart. Left alone this would mean every
    /// restarted index reads epoch `0` regardless of its real mutation
    /// history, wrongly opening the AsOf seek gate for any pinned version.
    /// `load()` closes this by seeding this map from each loaded
    /// definition's durable `ready_at_version` field (set once at
    /// backfill-completion time by [`mark_ready_at`](Self::mark_ready_at)) —
    /// see that method's and `load`'s docs for the full restart / CREATE /
    /// RENAME epoch-initialization story.
    ///
    /// `scc::HashMap` (lock-free, sharded) per this repo's NORMATIVE
    /// concurrency invariants (CLAUDE.md pillar 5, "shared registry"
    /// row) — `Arc`-shared across clones for the same reason `generation`
    /// is: a write applied through one clone's commit pipeline must advance
    /// the high-water for every other clone's `read_as_of` gate read.
    last_mutation_version: Arc<scc::HashMap<u64, AtomicU64, THasher>>,
}

impl Clone for SortedIndexManager {
    fn clone(&self) -> Self {
        // Share the SAME NodeReplicated + generation across clones so a
        // register/drop on any clone is visible to every other clone (see the
        // field docs for the read-after-write desync this prevents). A
        // snapshot-copy here would silently drop DDL-registered indexes from
        // later read clones.
        Self {
            info_store: Arc::clone(&self.info_store),
            indexes: Arc::clone(&self.indexes),
            generation: Arc::clone(&self.generation),
            last_mutation_version: Arc::clone(&self.last_mutation_version),
        }
    }
}

impl SortedIndexManager {
    /// Construct empty; caller must `load()` to hydrate.
    pub async fn new(info_store: Arc<dyn Store>) -> DbResult<Self> {
        let m = Self {
            info_store,
            indexes: Arc::new(shamir_numa::NodeReplicated::new(
                shamir_numa::detect(),
                Vec::new(),
            )),
            generation: Arc::new(AtomicU64::new(0)),
            last_mutation_version: Arc::new(scc::HashMap::with_hasher(THasher::default())),
        };
        m.load().await?;
        Ok(m)
    }

    /// F-50 Step 2 (#870): current sorted-index generation. Bumped (monotonic)
    /// whenever the set of queryable defs changes (`register` / `drop_index`).
    /// The zero-overhead gate value for commit-time ops-plan re-derivation: a
    /// tx captures this at stage time and, at commit, skips re-derivation
    /// entirely unless it has advanced.
    pub fn generation(&self) -> u64 {
        self.generation.load(Ordering::Acquire)
    }

    /// F-67 (#893): the mutation high-water for ONE specific index (keyed by
    /// `name_interned`) — the MVCC commit version of the most recent write
    /// that applied a posting to THAT index. An index with no recorded
    /// mutation (never written, or this manager was just constructed) reads
    /// as `0`, matching the old manager-wide counter's default.
    ///
    /// The AsOf cursor seek gate compares this against the cursor's
    /// `pinned_version` FOR THE INDEX THE SEEK IS PLANNED AGAINST: when
    /// `<= pinned`, the current-state index provably mirrors the pinned
    /// snapshot and the seek fast path is safe; when `> pinned`, a
    /// concurrent write to THIS index may have moved/removed a pinned
    /// posting and the seek MUST decline (falling back to the full scan).
    /// Mutating a DIFFERENT index no longer affects this read at all — the
    /// scope-narrowing this task exists for.
    ///
    /// Acquire-load pairs with the `fetch_max`-equivalent upsert (AcqRel) in
    /// [`note_mutation_at_version`]. The same monotonic-counter proof F-58
    /// established for the manager-wide counter applies unchanged per-index:
    /// any mutation the scan could have observed for THIS index necessarily
    /// bumped THIS index's counter before the observation, and this
    /// Acquire load is guaranteed to see it (or a later value).
    pub fn last_mutation_version(&self, name_interned: u64) -> u64 {
        match self.last_mutation_version.get_sync(&name_interned) {
            Some(entry) => entry.get().load(Ordering::Acquire),
            None => 0,
        }
    }

    /// F-67 (#893): advance the mutation high-water for ONE specific index
    /// (`name_interned`) to `version` if it is newer than that index's
    /// current value. Called at APPLY time only — inside `on_record_*` (the
    /// non-tx direct apply path, bumping only the indexes whose planner
    /// actually produced an op for this call) and at commit Phase 5c
    /// (`apply_index_batch`, bumping only the indexes with at least one op
    /// in the commit batch), always with the write's MVCC commit version.
    /// Never called at stage time: bumping before commit would let an
    /// uncommitted (possibly-aborting) tx disable the fast path for
    /// unrelated cursors.
    ///
    /// Lazily creates the per-index entry on first mutation (initialized to
    /// `version`); an existing entry is advanced via a `fetch_max`-style
    /// AcqRel RMW, so the bump is monotonic and race-free under concurrent
    /// committers targeting the SAME index without needing a CAS loop: the
    /// maximum of all concurrent commit versions is always the correct
    /// high-water for that index.
    pub fn note_mutation_at_version(&self, name_interned: u64, version: u64) {
        match self.last_mutation_version.entry_sync(name_interned) {
            scc::hash_map::Entry::Occupied(entry) => {
                entry.get().fetch_max(version, Ordering::AcqRel);
            }
            scc::hash_map::Entry::Vacant(entry) => {
                entry.insert_entry(AtomicU64::new(version));
            }
        }
    }

    /// F-71 (#898): mark index `name_interned` READY as of `table_version` —
    /// call exactly once, right after a successful CREATE INDEX backfill
    /// finishes streaming the table into the new index.
    ///
    /// Fixes vector 2 of the F-67 regression: the backfill call site
    /// (`TableManager::create_sorted_index_with_include`) drives
    /// `on_record_created(&id, &record, 0)` for every existing row — a
    /// literal version-`0` placeholder, because the backfill isn't a real
    /// MVCC write and has no commit version of its own. Left alone, that
    /// leaves the freshly built index's epoch at `0` even though its
    /// postings mirror everything up to the table's CURRENT version, so an
    /// `AsOf` query pinned to any version BEFORE the create would wrongly
    /// see `0 <= pinned` and take the fast path against an index that in
    /// fact reflects newer state (silently omitting a row deleted between
    /// the pin and the backfill).
    ///
    /// Sets BOTH:
    /// - the durable `ready_at_version` on the persisted definition (COW via
    ///   `rcu`, `max` with any existing value so a re-run — e.g. a doctor
    ///   repair rebuild — never moves the floor backward), persisted
    ///   immediately so a restart right after backfill restores the exact
    ///   epoch via [`load`](Self::load) rather than falling back to `0`;
    /// - the in-memory [`last_mutation_version`](Self::last_mutation_version)
    ///   high-water for this index, via the same monotonic
    ///   [`note_mutation_at_version`](Self::note_mutation_at_version) used by
    ///   every other bump site, so the gate is correct immediately —
    ///   without waiting for the NEXT write to bump it.
    ///
    /// `table_version` MUST be the table's `last_committed_version` (or
    /// equivalent snapshot-ceiling watermark) sampled AFTER the backfill
    /// stream has fully drained, never `0` — passing `0` here would just
    /// reproduce the bug this method exists to close. An index whose
    /// backfill observed an EMPTY table is still "ready" as of the current
    /// version, not as of the dawn of time, so callers must call this
    /// unconditionally (not only when the backfill touched at least one
    /// row).
    ///
    /// F-72 (#899, P0): ALSO flips the definition's lifecycle `state` from
    /// `Building` to `Ready` in the SAME copy-on-write `rcu` pass as the
    /// `ready_at_version` bump — one atomic publication, mirroring index2's
    /// `IndexRegistry::set_state`. This is the ONLY point a concurrent
    /// planner read (`find_by_field_ready`) may start observing the index:
    /// before this call the definition is registered (closing the
    /// lost-write race against concurrent writers, unchanged from F-57) but
    /// planner-invisible; after it, both the postings AND the planner
    /// visibility are complete. Idempotent: called on an already-`Ready`
    /// definition (e.g. `doctor::repair`'s rebuild path never calls this
    /// directly, but a future retry safely could) is a no-op state-wise.
    pub async fn mark_ready_at(&self, name_interned: u64, table_version: u64) -> DbResult<()> {
        self.note_mutation_at_version(name_interned, table_version);
        let mut found = false;
        self.indexes.rcu(|cur| {
            let mut new_vec: Vec<SortedIndexDefinition> = (*cur).clone();
            if let Some(def) = new_vec
                .iter_mut()
                .find(|d| d.name_interned == name_interned)
            {
                def.ready_at_version = def.ready_at_version.max(table_version);
                def.state = crate::state::IndexState::Ready;
                found = true;
            }
            new_vec
        });
        if !found {
            return Err(shamir_storage::error::DbError::Internal(
                "sorted index definition disappeared before backfill completed".to_string(),
            ));
        }
        self.persist_defs().await
    }

    /// True if at least one sorted index exists.
    pub fn has_indexes(&self) -> bool {
        !self.indexes.load_local().is_empty()
    }

    /// True if at least one sorted index has non-empty `included_fields`
    /// (i.e. is a covering index). Used to skip early interner
    /// initialization on open when no covering projections are needed.
    pub fn has_covering_indexes(&self) -> bool {
        self.indexes
            .load_local()
            .iter()
            .any(|d| !d.included_fields.is_empty())
    }

    /// Iterate over all sorted-index definitions.
    pub fn iter_indexes(&self) -> Vec<SortedIndexDefinition> {
        // Snapshot the current node-local Arc<Vec<...>> and clone its contents.
        // load_local() → Guard<Arc<T>>; *guard → Arc<T>; **guard → Vec<...>.
        // Callers consume by-value; for hot-path planners that just
        // need a borrow, see future `snapshot()` accessor.
        (**self.indexes.load_local()).clone()
    }

    /// Look up a definition whose `field_path` matches.
    ///
    /// F-72 (#899, P0): NOT state-filtered — returns a `Building` definition
    /// just as readily as a `Ready` one. This is the DDL/introspection-shaped
    /// lookup (mirrors index2's `get_by_name`, intentionally unfiltered so
    /// paths like `rename_index`/`drop_sorted_index` can still resolve an
    /// in-flight CREATE). PLANNER call sites (anything that decides whether a
    /// query can use this index for a scan) MUST use
    /// [`find_by_field_ready`](Self::find_by_field_ready) instead — see that
    /// method's doc for the correctness reason.
    pub fn find_by_field(&self, field_path: &[u64]) -> Option<SortedIndexDefinition> {
        self.indexes
            .load_local()
            .iter()
            .find(|d| d.field_path == field_path)
            .cloned()
    }

    /// Planner Ready-gate sibling of [`find_by_field`](Self::find_by_field):
    /// returns `None` for a definition whose `state` is `Building`, exactly
    /// as if the index did not exist yet.
    ///
    /// F-72 (#899, P0): closes the planner-invisibility gap — CREATE INDEX
    /// registers the definition BEFORE the backfill loop populates its
    /// postings (closing a DIFFERENT, pre-existing lost-write race against
    /// concurrent writers; see `create_sorted_index_with_include`'s doc), so
    /// a concurrent range/`Between`/`Gte`/`Lte`/keyset-seek/ORDER-BY-LIMIT-K
    /// query planned against the raw (unfiltered) `find_by_field` could be
    /// routed to a half-populated index and silently return fewer rows than
    /// actually exist. Every PLANNER call site
    /// (`TableManager::try_plan_sorted_index_scan`,
    /// `try_plan_order_limit_fast_path`, `try_plan_keyset_seek`) uses this
    /// method instead, so a `Building` index is invisible to query planning
    /// and those queries safely fall back to a full scan until the backfill
    /// flips the definition to `Ready` (mirrors index2's
    /// `IndexRegistry::find_by_field_and_kind` Ready-gate).
    pub fn find_by_field_ready(&self, field_path: &[u64]) -> Option<SortedIndexDefinition> {
        self.find_by_field(field_path)
            .filter(|d| d.state == crate::state::IndexState::Ready)
    }

    /// Look up a definition by its interned name id.
    /// Used by the index-only read path (slice A3) to check
    /// whether the scanned index is a covering index.
    ///
    /// NOT state-filtered — see [`find_by_field`](Self::find_by_field)'s doc;
    /// this is a name-keyed lookup used by rename/introspection, not planning.
    pub fn find_by_name_interned(&self, name_interned: u64) -> Option<SortedIndexDefinition> {
        self.indexes
            .load_local()
            .iter()
            .find(|d| d.name_interned == name_interned)
            .cloned()
    }

    /// Register a new sorted index (copy-on-write under a CAS loop).
    /// Persists the updated definitions blob, but does NOT backfill —
    /// the caller scans the table and calls `insert_entry` for each
    /// existing record.
    ///
    /// Last-write-wins matches the previous `DashMap::insert` semantics:
    /// if a definition with the same `name_interned` exists, it is
    /// replaced in-place; otherwise appended.
    pub async fn register(&self, def: SortedIndexDefinition) -> DbResult<()> {
        self.indexes.rcu(|cur| {
            let mut new_vec: Vec<SortedIndexDefinition> = (*cur).clone();
            match new_vec
                .iter()
                .position(|d| d.name_interned == def.name_interned)
            {
                Some(pos) => new_vec[pos] = def.clone(),
                None => new_vec.push(def.clone()),
            }
            new_vec
        });
        // F-50 Step 2 (#870): the queryable def set changed — advance the
        // generation so the commit pipeline's re-derivation gate fires for
        // any tx that staged before this register.
        self.generation.fetch_add(1, Ordering::AcqRel);
        self.persist_defs().await
    }

    /// Drop a sorted index definition AND every entry written under
    /// it. O(I) where I is the size of the index.
    ///
    /// F-76 (#903): this family is ALREADY safe — the RCU `indexes` swap
    /// (definition retirement) runs BEFORE the posting sweep, so a
    /// concurrent reader can never observe a registered-but-emptied sorted
    /// index. No reorder was needed here (unlike the regular / unique /
    /// index2 families, whose DROP paths swept postings BEFORE retiring the
    /// definition). See `shamir_index::state`'s lifecycle doc for the
    /// unified per-family contract.
    pub async fn drop_index(&self, name_interned: u64) -> DbResult<bool> {
        let mut existed = false;
        self.indexes.rcu(|cur| {
            let initial_len = cur.len();
            let new_vec: Vec<SortedIndexDefinition> = cur
                .iter()
                .filter(|d| d.name_interned != name_interned)
                .cloned()
                .collect();
            existed = new_vec.len() != initial_len;
            new_vec
        });
        if !existed {
            return Ok(false);
        }
        // F-50 Step 2 (#870): the queryable def set changed — advance the
        // generation so the commit pipeline's re-derivation gate fires for
        // any tx that staged before this drop.
        self.generation.fetch_add(1, Ordering::AcqRel);
        // Sweep entries.
        let prefix = self.entry_prefix(name_interned);
        let stream = self.info_store.scan_prefix_stream(prefix, MAINT_SCAN_BATCH);
        futures::pin_mut!(stream);
        // `RecordKey` (scan yields store keys, consumed by `remove_many`).
        let mut to_drop: Vec<RecordKey> = Vec::new();
        while let Some(batch) = stream.next().await {
            for (k, _) in batch? {
                to_drop.push(k);
            }
        }
        if !to_drop.is_empty() {
            // Ok-value (removed entries) intentionally discarded; ? propagates errors.
            let _ = self.info_store.remove_many(to_drop).await?;
        }
        self.persist_defs().await?;
        Ok(true)
    }

    /// Re-key an in-memory sorted-index definition from `old_id` to `new_id`
    /// and persist the updated metadata.
    ///
    /// This is the metadata half of RENAME INDEX for sorted indexes — the
    /// physical posting entries are re-keyed separately by the engine
    /// (`rekey_sorted_prefix`). Here we only swap the in-memory entry and
    /// re-save the definitions blob.
    ///
    /// Note: `drop_index` would delete the physical entries we just moved, so
    /// we bypass it and manipulate the `indexes` snapshot directly via `rcu`.
    ///
    /// F-71 (#898): fixes vector 3 of the F-67 regression. The definition's
    /// `ready_at_version` travels for free (the `rcu` below mutates
    /// `name_interned` IN PLACE on the same struct, so its `ready_at_version`
    /// field is untouched and is persisted under the new key by the
    /// `persist_defs` call below). But the in-memory
    /// [`last_mutation_version`](Self::last_mutation_version) high-water is a
    /// SEPARATE map keyed by `name_interned` — without an explicit carry, the
    /// rename would silently leave the OLD key's entry behind (never read
    /// again under `new_id`) and `new_id` would read as epoch `0`, resetting
    /// the AsOf gate to wide-open for the renamed index for the remainder of
    /// this process's lifetime (a restart would then re-seed correctly from
    /// the persisted `ready_at_version`, but that's not good enough — the
    /// gate must not go wrong even without a restart in between). We `remove`
    /// the old entry and re-insert its value under `new_id` (last-write-wins
    /// `max` against any value already sitting under `new_id`, though that
    /// should be impossible since the caller already checked no index is
    /// registered under `new_id`).
    pub async fn rename_definition(&self, old_id: u64, new_id: u64) -> DbResult<()> {
        let mut not_found = false;
        self.indexes.rcu(|cur| {
            let mut new_vec: Vec<SortedIndexDefinition> = (*cur).clone();
            match new_vec.iter().position(|d| d.name_interned == old_id) {
                Some(pos) => {
                    new_vec[pos].name_interned = new_id;
                    not_found = false;
                }
                None => {
                    not_found = true;
                }
            }
            new_vec
        });
        if not_found {
            return Err(shamir_storage::error::DbError::Internal(
                "sorted index definition disappeared mid-rename".to_string(),
            ));
        }
        // Carry the in-memory mutation-epoch entry from old_id to new_id —
        // see the doc above. `remove_sync` returns the removed AtomicU64
        // (if any); an index that was never mutated has no entry, which is
        // fine — `new_id` correctly starts at the freshly-carried
        // `ready_at_version` floor `load()` would seed on the next restart,
        // and at `0` in-memory until the next mutation or `mark_ready_at`
        // call, matching a never-mutated index's normal semantics.
        if let Some((_, old_epoch)) = self.last_mutation_version.remove_sync(&old_id) {
            self.note_mutation_at_version(new_id, old_epoch.load(Ordering::Acquire));
        }
        self.persist_defs().await
    }

    // ============================================================================
    // Covering-index helpers
    // ============================================================================

    /// Resolve `included_fields` string paths to interned u64 ids for every
    /// definition that has at least one included field. Call this:
    ///   1. After `register()` when the caller already has an interner, OR
    ///   2. After construction (load from disk) to rebuild the transient
    ///      `included_fields_interned` caches.
    ///
    /// Unknown strings are silently skipped (they produce an empty inner vec
    /// for that path, which `build_covering_projection` will treat as absent).
    pub fn intern_included_paths(&self, interner: &Interner) {
        // Single COW pass: clone the current snapshot, mutate each def
        // in-place, store the new Arc. Replaces the previous per-key
        // DashMap::alter loop. Off hot path (called after register with
        // interner OR after load on bootstrap) so the one-shot Vec clone
        // is acceptable.
        self.indexes.rcu(|cur| {
            let mut new_vec: Vec<SortedIndexDefinition> = (*cur).clone();
            for def in new_vec.iter_mut() {
                if def.included_fields.is_empty() {
                    continue;
                }
                def.included_fields_interned = def
                    .included_fields
                    .iter()
                    .map(|path_segs| {
                        path_segs
                            .iter()
                            .filter_map(|seg| {
                                interner.touch_ind(seg.as_str()).ok().map(|t| t.key().id())
                            })
                            .collect::<Vec<u64>>()
                    })
                    .collect();
            }
            new_vec
        });
    }

    // ============================================================================
    // Planner methods — return Vec<IndexWriteOp> without side effects
    // ============================================================================

    /// Plan index entries for a newly created record.
    ///
    /// F-67 (#893): a PURE planner — no side effects, including no epoch
    /// bump. This is called at BOTH stage time (tx path, `version == 0`
    /// placeholder — see `table_manager_tx_ops.rs`) and apply time (non-tx
    /// direct path, via [`on_record_created`]'s real `version`), so bumping
    /// here would incorrectly fire at stage time for an uncommitted
    /// (possibly-aborting) tx. The per-index bump lives in
    /// [`on_record_created`], the actual apply-time entry point, derived
    /// from the ops this planner returns.
    pub fn plan_record_created(
        &self,
        record_id: &RecordId,
        record: &(impl RecordRef + ?Sized),
        version: u64,
    ) -> DbResult<Vec<IndexWriteOp>> {
        if self.indexes.load_local().is_empty() {
            return Ok(Vec::new());
        }
        let defs: Vec<SortedIndexDefinition> = self.iter_indexes();
        let mut ops = Vec::with_capacity(4);
        for def in &defs {
            if let Some(encoded) = extract_and_encode(record, &def.field_path)? {
                let key = self.build_entry_key(def.name_interned, &encoded, record_id);
                let value = if def.is_covering() {
                    build_covering_projection(record, def, version)
                } else {
                    Bytes::new()
                };
                ops.push(IndexWriteOp::SetPosting { key, value });
            }
        }
        Ok(ops)
    }

    /// Planner variant of [`on_records_created_batch`] — collects
    /// entry ops for N records across all sorted indexes in one
    /// pass, snapshotting `iter_indexes()` ONCE (the per-row
    /// `plan_record_created` re-snapshots every call). Used by the
    /// tx batch insert path.
    ///
    /// F-67 (#893): a PURE planner — no epoch bump (see
    /// [`plan_record_created`]'s doc for why: this is also called at stage
    /// time with a `version == 0` placeholder).
    pub fn plan_records_created_batch<'a, R, I>(
        &self,
        items: I,
        version: u64,
    ) -> DbResult<Vec<IndexWriteOp>>
    where
        R: RecordRef + ?Sized + 'a,
        I: IntoIterator<Item = (&'a RecordId, &'a R)> + Clone,
    {
        if self.indexes.load_local().is_empty() {
            return Ok(Vec::new());
        }
        let defs: Vec<SortedIndexDefinition> = self.iter_indexes();
        let mut ops = Vec::new();
        for def in &defs {
            for (rid, value) in items.clone() {
                if let Some(encoded) = extract_and_encode(value, &def.field_path)? {
                    let key = self.build_entry_key(def.name_interned, &encoded, rid);
                    let pv = if def.is_covering() {
                        build_covering_projection(value, def, version)
                    } else {
                        Bytes::new()
                    };
                    ops.push(IndexWriteOp::SetPosting { key, value: pv });
                }
            }
        }
        Ok(ops)
    }

    /// Plan index entry changes when a record is updated.
    ///
    /// F-67 (#893): a PURE planner — no epoch bump (see
    /// [`plan_record_created`]'s doc for why: this is also called at stage
    /// time with a `version == 0` placeholder, e.g.
    /// `table_manager_tx_ops.rs`'s tx-stage path).
    pub fn plan_record_updated(
        &self,
        record_id: &RecordId,
        old: &(impl RecordRef + ?Sized),
        new: &(impl RecordRef + ?Sized),
        version: u64,
    ) -> DbResult<Vec<IndexWriteOp>> {
        if self.indexes.load_local().is_empty() {
            return Ok(Vec::new());
        }
        let defs: Vec<SortedIndexDefinition> = self.iter_indexes();
        let mut ops = Vec::new();
        for def in &defs {
            let old_enc = extract_and_encode(old, &def.field_path)?;
            let new_enc = extract_and_encode(new, &def.field_path)?;
            // For covering indexes, also rewrite the posting when the
            // projected values changed even if the indexed key did not.
            // Both old and new are built with the same `version` so that
            // the version bytes are identical and do not spuriously trigger
            // a rewrite when only the version changed.
            let old_proj = if def.is_covering() {
                Some(build_covering_projection(old, def, version))
            } else {
                None
            };
            let new_proj = if def.is_covering() {
                Some(build_covering_projection(new, def, version))
            } else {
                None
            };
            let key_changed = old_enc != new_enc;
            let proj_changed = old_proj != new_proj;
            if !key_changed && !proj_changed {
                continue;
            }
            if key_changed {
                if let Some(ref ov) = old_enc {
                    let key = self.build_entry_key(def.name_interned, ov, record_id);
                    ops.push(IndexWriteOp::RemovePosting { key });
                }
                if let Some(ref nv) = new_enc {
                    let key = self.build_entry_key(def.name_interned, nv, record_id);
                    let value = new_proj.clone().unwrap_or(Bytes::new());
                    ops.push(IndexWriteOp::SetPosting { key, value });
                }
            } else {
                // Key is the same but projection changed — overwrite in place.
                if let Some(ref nv) = new_enc {
                    let key = self.build_entry_key(def.name_interned, nv, record_id);
                    let value = new_proj.clone().unwrap_or(Bytes::new());
                    ops.push(IndexWriteOp::SetPosting { key, value });
                }
            }
        }
        Ok(ops)
    }

    /// Plan index entry removals for a deleted record.
    ///
    /// F-67 (#893): a PURE planner — no epoch bump (see
    /// [`plan_record_created`]'s doc for why: this is also called at stage
    /// time, e.g. `table_manager_tx_ops.rs::plan_legacy_delete_ops`).
    pub fn plan_record_deleted(
        &self,
        record_id: &RecordId,
        record: &(impl RecordRef + ?Sized),
    ) -> DbResult<Vec<IndexWriteOp>> {
        if self.indexes.load_local().is_empty() {
            return Ok(Vec::new());
        }
        let defs: Vec<SortedIndexDefinition> = self.iter_indexes();
        let mut ops = Vec::new();
        for def in &defs {
            if let Some(encoded) = extract_and_encode(record, &def.field_path)? {
                let key = self.build_entry_key(def.name_interned, &encoded, record_id);
                ops.push(IndexWriteOp::RemovePosting { key });
            }
        }
        Ok(ops)
    }

    // ============================================================================
    // Apply ops
    // ============================================================================

    /// Apply a slice of `IndexWriteOp` against `self.info_store`.
    async fn apply_ops(&self, ops: &[IndexWriteOp]) -> DbResult<()> {
        for op in ops {
            match op {
                IndexWriteOp::SetPosting { key, value } => {
                    self.info_store
                        .set(key.clone().into(), value.clone())
                        .await?;
                }
                IndexWriteOp::RemovePosting { key } => {
                    let _ = self.info_store.remove(key.clone().into()).await?;
                }
                IndexWriteOp::BumpFtsStats { .. } => {
                    // Not relevant for SortedIndexManager.
                }
            }
        }
        Ok(())
    }

    /// F-67 (#893): bump the per-index mutation high-water to `version` for
    /// every DISTINCT sorted-index `name_interned` touched by `ops`.
    ///
    /// Called at APPLY time only:
    /// - Internally, right after a planner's pure `Vec<IndexWriteOp>` is
    ///   produced by one of the `on_record_*` wrappers below (the non-tx
    ///   direct path's own apply point) — never inside the planners
    ///   themselves, which are also invoked at STAGE time (tx path,
    ///   `version == 0` placeholder) where bumping would incorrectly disable
    ///   the fast path for an uncommitted, possibly-aborting tx.
    /// - Externally, by `commit_phases.rs::apply_index_batch` (the tx-commit
    ///   path's Phase 5c apply point) against the flat `ops: &[IndexWriteOp]`
    ///   batch for one table. F-74 (#901): called BEFORE the postings land
    ///   (mirroring the non-tx path above), not after — `tx.
    ///   index_write_set` is `Vec<(table_token, IndexWriteOp)>`, grouped only
    ///   by table (never by index), so this is the only per-index-precise
    ///   entry point available at that call site.
    ///
    /// `ops` may legitimately contain entries this manager did NOT produce
    /// (other index families — hash/FTS/functional — share the same flat
    /// `IndexWriteOp` enum and the same commit-time batch; `IndexWriteOp`
    /// carries no index-id field of its own). Each key is decoded via
    /// [`decode_sorted_index_name`] (the `[SORTED_TAG | name_interned |
    /// ...]` layout [`Self::build_entry_key`] writes); a `None` (foreign key
    /// from a different index family, or a malformed key) is skipped rather
    /// than treated as an error. F-74 (#901): a decode miss on a key that
    /// genuinely WAS a sorted-index posting is NOT harmless — the gate this
    /// counter drives is `epoch <= pinned ⟹ take the fast path`, so a missed
    /// bump leaves the epoch LOW and the gate OPEN, i.e. it KEEPS the fast
    /// path enabled for an index whose postings just changed, which is the
    /// unsafe direction (a false-negative here risks a wrong AsOf page, not
    /// a spurious full-scan fallback). Before F-67 (#893) the bump was
    /// UNCONDITIONAL, so the only possible error was OVER-bumping (safe:
    /// closes the gate, forces a fallback); F-67 made the bump conditional on
    /// `decode_sorted_index_name` returning `Some`, introducing the
    /// possibility of UNDER-bumping this doc previously mis-described as
    /// harmless. `decode_sorted_index_name`'s own decode correctness is out
    /// of scope here — this note only corrects the description of what a
    /// miss actually costs.
    /// De-duplicates via a small on-stack scan (`SmallVec`) since a table
    /// typically has ≤ ~10 sorted indexes (see the manager's cardinality
    /// doc) — cheaper than allocating a `HashSet` for single-digit N.
    pub fn bump_touched_indexes(&self, ops: &[IndexWriteOp], version: u64) {
        let mut touched: SmallVec<[u64; 8]> = SmallVec::new();
        for op in ops {
            let key = match op {
                IndexWriteOp::SetPosting { key, .. } => key,
                IndexWriteOp::RemovePosting { key } => key,
                IndexWriteOp::BumpFtsStats { .. } => continue,
            };
            if let Some(name_interned) = decode_sorted_index_name(key.as_ref()) {
                if !touched.contains(&name_interned) {
                    touched.push(name_interned);
                    self.note_mutation_at_version(name_interned, version);
                }
            }
        }
    }

    // ============================================================================
    // on_record_* wrappers — plan + apply
    // ============================================================================

    /// Add an index entry for a record. Called from
    /// `TableManager::insert` and `set` (create branch).
    pub async fn on_record_created(
        &self,
        record_id: &RecordId,
        record: &(impl RecordRef + ?Sized),
        version: u64,
    ) -> DbResult<()> {
        let ops = self.plan_record_created(record_id, record, version)?;
        // F-67 (#893): bump at APPLY time (this method is the non-tx direct
        // path's apply point), only for the index(es) `ops` actually touched.
        self.bump_touched_indexes(&ops, version);
        self.apply_ops(&ops).await
    }

    /// Update entries when a record changes.
    pub async fn on_record_updated(
        &self,
        record_id: &RecordId,
        old: &(impl RecordRef + ?Sized),
        new: &(impl RecordRef + ?Sized),
        version: u64,
    ) -> DbResult<()> {
        let ops = self.plan_record_updated(record_id, old, new, version)?;
        // F-67 (#893): bump at APPLY time (see on_record_created).
        self.bump_touched_indexes(&ops, version);
        self.apply_ops(&ops).await
    }

    /// Batched version of `on_record_created` — collects all entry
    /// writes across all sorted indexes for N records into one
    /// `Store::set_many` call. Borrow-only — no `InnerValue` clones
    /// except for covering-index projection (unavoidable: projection
    /// requires a deep clone of the leaf value).
    pub async fn on_records_created_batch<'a, R, I>(&self, items: I, version: u64) -> DbResult<()>
    where
        R: RecordRef + ?Sized + 'a,
        I: IntoIterator<Item = (&'a RecordId, &'a R)> + Clone,
    {
        if self.indexes.load_local().is_empty() {
            return Ok(());
        }
        let defs: Vec<SortedIndexDefinition> = self.iter_indexes();
        // `RecordKey` keys (fed to the store `set_many`); entry keys are
        // built as `Bytes` and converted byte-identically at each push.
        let mut writes: Vec<(RecordKey, Bytes)> = Vec::new();
        for def in &defs {
            let mut touched = false;
            for (rid, value) in items.clone() {
                if let Some(encoded) = extract_and_encode(value, &def.field_path)? {
                    let key = self.build_entry_key(def.name_interned, &encoded, rid);
                    let pv = if def.is_covering() {
                        build_covering_projection(value, def, version)
                    } else {
                        Bytes::new()
                    };
                    writes.push((key.into(), pv));
                    touched = true;
                }
            }
            // F-67 (#893): bump at APPLY time (this method is the non-tx
            // direct batch path's apply point) — only for a `def` at least
            // one item in the batch actually produced a posting for.
            if touched {
                self.note_mutation_at_version(def.name_interned, version);
            }
        }
        if writes.is_empty() {
            return Ok(());
        }
        self.info_store.set_many(writes).await?;
        Ok(())
    }

    /// Drop entries for a deleted record.
    ///
    /// `version` is the DELETE's MVCC commit version, used to bump the
    /// per-index mutation high-water at APPLY time (F-67, #893: only for the
    /// index(es) that actually carried a posting for this record — see
    /// [`Self::bump_touched_indexes`]). A DELETE after a cursor's pin
    /// REMOVES the posting for that index — the index never yields the
    /// record id again, so a seek planned against THAT SAME index would
    /// silently miss a row the pinned snapshot must show. The gate (bumped
    /// here) is what makes the AsOf seek decline in that case and fall back
    /// to the full scan; mutating an UNRELATED index no longer bumps this
    /// counter at all.
    pub async fn on_record_deleted(
        &self,
        record_id: &RecordId,
        record: &(impl RecordRef + ?Sized),
        version: u64,
    ) -> DbResult<()> {
        let ops = self.plan_record_deleted(record_id, record)?;
        // F-67 (#893): bump at APPLY time (see on_record_created).
        self.bump_touched_indexes(&ops, version);
        self.apply_ops(&ops).await
    }

    /// Range lookup: return all record IDs whose indexed value is in
    /// `[start, end]` (both inclusive). `start` / `end` are the
    /// already-encoded value bytes (call sites use
    /// `sort_codec::encode_*` to produce them).
    ///
    /// Builds the lower / upper bounds in the physical-key space and
    /// delegates to `Store::iter_range_stream` — on B-tree-backed
    /// stores (sled, redb, fjall, persy, canopy) this seeks straight
    /// to `lower` and stops at `upper`, doing zero wasted work
    /// outside the range. In-memory / cached fall back to
    /// `iter_range_stream`'s default filter wrapper, still correct.
    pub async fn lookup_range(
        &self,
        name_interned: u64,
        start_encoded: Option<&[u8]>,
        end_encoded: Option<&[u8]>,
    ) -> DbResult<BTreeSet<RecordId>> {
        let prefix = self.entry_prefix(name_interned);
        let (lower, upper) = self.range_bounds(&prefix, start_encoded, end_encoded);

        let stream = self
            .info_store
            .iter_range_stream(Some(lower), Some(upper), MAINT_SCAN_BATCH);
        futures::pin_mut!(stream);

        let mut out: BTreeSet<RecordId> = BTreeSet::new();
        while let Some(batch) = stream.next().await {
            for (k, _) in batch? {
                if let Some(id) = decode_record_id_suffix(k.as_ref()) {
                    out.insert(id);
                }
            }
        }
        Ok(out)
    }

    /// Range lookup with physical values: identical to [`lookup_range`] but
    /// returns `(RecordId, Bytes)` pairs (preserving scan / value order in a
    /// `Vec`, NOT de-duplicating into a `BTreeSet`).  The `Bytes` is the
    /// raw physical_value stored in the index entry — for covering indexes
    /// that is the versioned projection envelope written by
    /// `build_covering_projection`; for non-covering indexes it is empty.
    ///
    /// Used by the index-only read path (slice A3).
    pub async fn lookup_range_with_values(
        &self,
        name_interned: u64,
        start_encoded: Option<&[u8]>,
        end_encoded: Option<&[u8]>,
    ) -> DbResult<Vec<(RecordId, Bytes)>> {
        let prefix = self.entry_prefix(name_interned);
        let (lower, upper) = self.range_bounds(&prefix, start_encoded, end_encoded);

        let stream = self
            .info_store
            .iter_range_stream(Some(lower), Some(upper), MAINT_SCAN_BATCH);
        futures::pin_mut!(stream);

        let mut out: Vec<(RecordId, Bytes)> = Vec::new();
        while let Some(batch) = stream.next().await {
            for (k, v) in batch? {
                if let Some(id) = decode_record_id_suffix(k.as_ref()) {
                    out.push((id, v));
                }
            }
        }
        Ok(out)
    }

    /// Min lookup — the first record under the sorted prefix.
    /// `iter_range_stream` with batch_size=1 reads exactly the first
    /// entry on B-tree backends; in-memory falls back to its default.
    pub async fn lookup_min(&self, name_interned: u64) -> DbResult<Option<RecordId>> {
        let prefix = self.entry_prefix(name_interned);
        let (lower, upper) = self.range_bounds(&prefix, None, None);
        let stream = self
            .info_store
            .iter_range_stream(Some(lower), Some(upper), 1);
        futures::pin_mut!(stream);
        if let Some(batch) = stream.next().await {
            if let Some((k, _)) = batch?.into_iter().next() {
                return Ok(decode_record_id_suffix(k.as_ref()));
            }
        }
        Ok(None)
    }

    /// Max lookup — the last record under the sorted prefix.
    /// Uses `iter_range_stream_reverse` so disk backends seek
    /// straight to the upper bound and walk one entry backwards.
    pub async fn lookup_max(&self, name_interned: u64) -> DbResult<Option<RecordId>> {
        let prefix = self.entry_prefix(name_interned);
        let (lower, upper) = self.range_bounds(&prefix, None, None);
        let stream = self
            .info_store
            .iter_range_stream_reverse(Some(lower), Some(upper), 1);
        futures::pin_mut!(stream);
        if let Some(batch) = stream.next().await {
            if let Some((k, _)) = batch?.into_iter().next() {
                return Ok(decode_record_id_suffix(k.as_ref()));
            }
        }
        Ok(None)
    }

    /// Last K record ids under the sorted prefix, in value-DESC order.
    /// Mirror of `lookup_first_k` using `iter_range_stream_reverse`.
    pub async fn lookup_last_k(&self, name_interned: u64, k: usize) -> DbResult<Vec<RecordId>> {
        if k == 0 {
            return Ok(Vec::new());
        }
        let prefix = self.entry_prefix(name_interned);
        let (lower, upper) = self.range_bounds(&prefix, None, None);
        let stream =
            self.info_store
                .iter_range_stream_reverse(Some(lower), Some(upper), k.min(256));
        futures::pin_mut!(stream);
        let mut out = Vec::with_capacity(k);
        while let Some(batch) = stream.next().await {
            for (key, _) in batch? {
                if out.len() == k {
                    return Ok(out);
                }
                if let Some(id) = decode_record_id_suffix(key.as_ref()) {
                    out.push(id);
                }
            }
        }
        Ok(out)
    }

    /// Keyset-seek helper: the first `k` record ids in the half-plane
    /// beyond `seek_encoded`, returned in value order, EXCLUDING every
    /// entry whose indexed value equals the seek value (the already-seen
    /// page boundary).
    ///
    /// `forward == true`  → ASC : walk `[seek, +∞)` low→high, skip `== seek`.
    /// `forward == false` → DESC: walk `(-∞, seek]` high→low, skip `== seek`.
    ///
    /// This is the ordered early-stop replacement for
    /// `lookup_range` + full fetch + full sort + truncate on the keyset
    /// path: the physical key is `[tag|name|encoded_value|record_id]`, so
    /// the backend stream is already value-ordered — we compare the
    /// encoded-value slice of each key against `seek_encoded` to drop the
    /// boundary rows and stop the walk the instant `k` survivors are
    /// collected. Per-page cost is O(k + |rows == seek|), not
    /// O(remaining table).
    pub async fn lookup_range_first_k(
        &self,
        name_interned: u64,
        seek_encoded: &[u8],
        k: usize,
        forward: bool,
    ) -> DbResult<Vec<RecordId>> {
        // First page only; the continuation cursor is discarded. Callers
        // that must survive stale index entries (dead postings whose record
        // body is gone) use `lookup_range_first_k_page` to resume the walk.
        // No tie-breaker (`after_id = None`) → today's skip-all-ties behavior.
        let (ids, _cursor) = self
            .lookup_range_first_k_page(name_interned, seek_encoded, None, None, k, forward)
            .await?;
        Ok(ids)
    }

    /// Continuation-aware sibling of [`lookup_range_first_k`]: collects the
    /// next `k` **physical** index entries in value order and also returns a
    /// resume cursor so the caller can drive a live-row-aware page loop.
    ///
    /// Returns `(ids, cursor)`:
    /// * `ids` — up to `k` record ids in ORDER BY direction, skipping every
    ///   entry whose indexed value equals `seek_encoded` (the page boundary).
    /// * `cursor` — `Some(last_physical_key)` when the walk stopped because it
    ///   reached `k` survivors (there may be MORE range beyond); `None` when
    ///   the underlying stream was exhausted (the range is truly done — a
    ///   genuine last page, never resume past this).
    ///
    /// `after_key`:
    /// * `None` → first page: walk the half-plane bounded by `seek_encoded`.
    /// * `Some(prev_cursor)` → resume STRICTLY after (ASC) / before (DESC) the
    ///   physical key returned as the previous page's cursor, so already-seen
    ///   entries are never re-emitted.
    ///
    /// `after_id` (task #537 — the record-id tie-breaker):
    /// * `None` → the boundary rows (value == `seek_encoded`) are ALL skipped —
    ///   today's exact, backward-compatible behavior. Correct only for the one
    ///   row that established the boundary; every OTHER row tied on the same
    ///   ORDER BY value is silently dropped (the pre-existing #537 limitation
    ///   an old client — one that doesn't echo a tie-breaker — still gets).
    /// * `Some(id)` → the physical `(value, record_id)` order is used to skip
    ///   ONLY the rows at the boundary value that are not-strictly-past `id`:
    ///   for ASC, value == seek AND record_id <= id; for DESC, value == seek
    ///   AND record_id >= id. Rows tied on the seek value but strictly past the
    ///   client's last-seen id are RETURNED, so no tied row is ever lost.
    ///
    /// Note: the caller (`TableManager::read_keyset_seek`) passes the SAME
    /// `after_id` on every internal-loop call this function drives across a
    /// single client request (e.g. when a stale posting forces another
    /// `after_key`-cursored page). This is safe and necessary: `after_id`
    /// only ever affects rows whose encoded value still equals
    /// `seek_encoded` — once `after_key` has moved the walk past the tied
    /// value, `key_value_slice(kb) == seek_encoded` is false and the
    /// boundary check never fires, so `after_id` is a no-op for those rows.
    /// Dropping `after_id` to `None` on continuation calls (an earlier,
    /// buggy version of the caller) made every remaining boundary-value row
    /// unconditionally skipped, silently losing tied rows sitting behind a
    /// stale posting on the SAME request — an adversarial review caught
    /// this empirically before it shipped.
    ///
    /// This keeps the index layer purely about ordered physical traversal:
    /// liveness (fetching record bodies, dropping dead ids) lives entirely in
    /// the engine's read path, which loops here until it has `limit` LIVE rows
    /// or this method reports the range exhausted (`cursor == None`).
    #[allow(clippy::too_many_arguments)] // ordered-traversal cursor + tie-breaker params
    pub async fn lookup_range_first_k_page(
        &self,
        name_interned: u64,
        seek_encoded: &[u8],
        after_key: Option<&[u8]>,
        after_id: Option<&RecordId>,
        k: usize,
        forward: bool,
    ) -> DbResult<(Vec<RecordId>, Option<Bytes>)> {
        if k == 0 {
            return Ok((Vec::new(), None));
        }
        let prefix = self.entry_prefix(name_interned);
        let value_start = prefix.len();

        // Bounds. On a continuation page the moving bound is `after_key`
        // itself (inclusive in the store API); we drop the entry equal to it
        // below so the resume is STRICTLY past the previous cursor.
        let (lower, upper) = if forward {
            match after_key {
                // `after_key` is a full physical key → use it verbatim as the
                // (inclusive) lower bound; the open upper is the same
                // `prefix||0xFF*64` as the first page. We do NOT re-wrap
                // through `range_bounds`, which carries seek-value semantics.
                Some(ak) => {
                    let (_, up) = self.range_bounds(&prefix, None, None);
                    (Bytes::copy_from_slice(ak), up)
                }
                None => self.range_bounds(&prefix, Some(seek_encoded), None),
            }
        } else {
            match after_key {
                // Resume high→low: `after_key` becomes the inclusive upper
                // bound; the lower stays the prefix (start of this index).
                Some(ak) => (prefix.clone(), Bytes::copy_from_slice(ak)),
                None => self.range_bounds(&prefix, None, Some(seek_encoded)),
            }
        };

        let mut out = Vec::with_capacity(k);
        let mut cursor: Option<Bytes> = None;
        let mut exhausted = true;
        let batch = k.min(MAINT_SCAN_BATCH);

        if forward {
            let stream = self
                .info_store
                .iter_range_stream(Some(lower), Some(upper), batch);
            futures::pin_mut!(stream);
            'outer: while let Some(b) = stream.next().await {
                for (key, _) in b? {
                    let kb = key.as_ref();
                    // Resume strictly-after: skip the entry equal to the
                    // previous cursor (inclusive lower bound).
                    if let Some(ak) = after_key {
                        if kb == ak {
                            continue;
                        }
                    }
                    // Boundary-value skip (task #537): with a tie-breaker,
                    // skip only the tied rows not-strictly-past `after_id`
                    // (ASC: record_id <= after_id); without one, skip all.
                    if key_value_slice(kb, value_start) == seek_encoded
                        && skip_boundary_row(kb, after_id, true)
                    {
                        continue;
                    }
                    if let Some(id) = decode_record_id_suffix(kb) {
                        out.push(id);
                        if out.len() == k {
                            cursor = Some(Bytes::copy_from_slice(kb));
                            exhausted = false;
                            break 'outer;
                        }
                    }
                }
            }
        } else {
            let stream = self
                .info_store
                .iter_range_stream_reverse(Some(lower), Some(upper), batch);
            futures::pin_mut!(stream);
            'outer: while let Some(b) = stream.next().await {
                for (key, _) in b? {
                    let kb = key.as_ref();
                    if let Some(ak) = after_key {
                        if kb == ak {
                            continue;
                        }
                    }
                    // Boundary-value skip (task #537), DESC direction: skip
                    // only tied rows not-strictly-before `after_id`
                    // (record_id >= after_id); without one, skip all.
                    if key_value_slice(kb, value_start) == seek_encoded
                        && skip_boundary_row(kb, after_id, false)
                    {
                        continue;
                    }
                    if let Some(id) = decode_record_id_suffix(kb) {
                        out.push(id);
                        if out.len() == k {
                            cursor = Some(Bytes::copy_from_slice(kb));
                            exhausted = false;
                            break 'outer;
                        }
                    }
                }
            }
        }

        // If the stream ran dry before we hit `k`, the range is exhausted and
        // there is nothing to resume from → cursor stays `None`.
        if exhausted {
            cursor = None;
        }
        Ok((out, cursor))
    }

    /// First K record ids under the sorted prefix, in value-asc order.
    pub async fn lookup_first_k(&self, name_interned: u64, k: usize) -> DbResult<Vec<RecordId>> {
        if k == 0 {
            return Ok(Vec::new());
        }
        let prefix = self.entry_prefix(name_interned);
        let (lower, upper) = self.range_bounds(&prefix, None, None);
        let stream =
            self.info_store
                .iter_range_stream(Some(lower), Some(upper), k.min(MAINT_SCAN_BATCH));
        futures::pin_mut!(stream);
        let mut out = Vec::with_capacity(k);
        while let Some(batch) = stream.next().await {
            for (key, _) in batch? {
                if out.len() == k {
                    return Ok(out);
                }
                if let Some(id) = decode_record_id_suffix(key.as_ref()) {
                    out.push(id);
                }
            }
        }
        Ok(out)
    }

    /// tx-aware variant of [`lookup_range`].
    ///
    /// Phase C (Step 5): records an `IndexRange` predicate dependency
    /// for Serializable txs BEFORE forwarding to the non-tx method.
    /// Zero-overhead: Snapshot / non-tx callers skip the recording
    /// block entirely (single tag-compare on `Option<&TxContext>`).
    pub async fn lookup_range_tx(
        &self,
        table_token: u64,
        name_interned: u64,
        start_encoded: Option<&[u8]>,
        end_encoded: Option<&[u8]>,
        tx: Option<&shamir_tx::TxContext>,
    ) -> DbResult<BTreeSet<RecordId>> {
        if let Some(t) = tx {
            if t.isolation == shamir_tx::IsolationLevel::Serializable {
                let prefix = self.entry_prefix(name_interned);
                let (lower, upper) = self.range_bounds(&prefix, start_encoded, end_encoded);
                t.record_predicate_shared(shamir_tx::predicate_set::PredicateDep::IndexRange {
                    table_token,
                    index_id: name_interned,
                    lo: std::ops::Bound::Included(lower),
                    hi: std::ops::Bound::Included(upper),
                });
            }
        }
        self.lookup_range(name_interned, start_encoded, end_encoded)
            .await
    }

    /// tx-aware variant of [`lookup_min`].
    ///
    /// Phase C (Step 5): records a full-prefix `IndexRange` predicate
    /// dependency (the entire sorted index) for Serializable txs.
    pub async fn lookup_min_tx(
        &self,
        table_token: u64,
        name_interned: u64,
        tx: Option<&shamir_tx::TxContext>,
    ) -> DbResult<Option<RecordId>> {
        if let Some(t) = tx {
            if t.isolation == shamir_tx::IsolationLevel::Serializable {
                let prefix = self.entry_prefix(name_interned);
                let (lower, upper) = self.range_bounds(&prefix, None, None);
                t.record_predicate_shared(shamir_tx::predicate_set::PredicateDep::IndexRange {
                    table_token,
                    index_id: name_interned,
                    lo: std::ops::Bound::Included(lower),
                    hi: std::ops::Bound::Included(upper),
                });
            }
        }
        self.lookup_min(name_interned).await
    }

    /// tx-aware variant of [`lookup_max`].
    ///
    /// Phase C (Step 5): records a full-prefix `IndexRange` predicate
    /// dependency for Serializable txs.
    pub async fn lookup_max_tx(
        &self,
        table_token: u64,
        name_interned: u64,
        tx: Option<&shamir_tx::TxContext>,
    ) -> DbResult<Option<RecordId>> {
        if let Some(t) = tx {
            if t.isolation == shamir_tx::IsolationLevel::Serializable {
                let prefix = self.entry_prefix(name_interned);
                let (lower, upper) = self.range_bounds(&prefix, None, None);
                t.record_predicate_shared(shamir_tx::predicate_set::PredicateDep::IndexRange {
                    table_token,
                    index_id: name_interned,
                    lo: std::ops::Bound::Included(lower),
                    hi: std::ops::Bound::Included(upper),
                });
            }
        }
        self.lookup_max(name_interned).await
    }

    /// tx-aware variant of [`lookup_last_k`].
    ///
    /// Phase C (Step 5): records a full-prefix `IndexRange` predicate
    /// dependency for Serializable txs. The interval does not depend on
    /// `k` — every entry the scan could reach is in the full-prefix range.
    pub async fn lookup_last_k_tx(
        &self,
        table_token: u64,
        name_interned: u64,
        k: usize,
        tx: Option<&shamir_tx::TxContext>,
    ) -> DbResult<Vec<RecordId>> {
        if let Some(t) = tx {
            if t.isolation == shamir_tx::IsolationLevel::Serializable {
                let prefix = self.entry_prefix(name_interned);
                let (lower, upper) = self.range_bounds(&prefix, None, None);
                t.record_predicate_shared(shamir_tx::predicate_set::PredicateDep::IndexRange {
                    table_token,
                    index_id: name_interned,
                    lo: std::ops::Bound::Included(lower),
                    hi: std::ops::Bound::Included(upper),
                });
            }
        }
        self.lookup_last_k(name_interned, k).await
    }

    /// tx-aware variant of [`lookup_first_k`].
    ///
    /// Phase C (Step 5): records a full-prefix `IndexRange` predicate
    /// dependency for Serializable txs.
    pub async fn lookup_first_k_tx(
        &self,
        table_token: u64,
        name_interned: u64,
        k: usize,
        tx: Option<&shamir_tx::TxContext>,
    ) -> DbResult<Vec<RecordId>> {
        if let Some(t) = tx {
            if t.isolation == shamir_tx::IsolationLevel::Serializable {
                let prefix = self.entry_prefix(name_interned);
                let (lower, upper) = self.range_bounds(&prefix, None, None);
                t.record_predicate_shared(shamir_tx::predicate_set::PredicateDep::IndexRange {
                    table_token,
                    index_id: name_interned,
                    lo: std::ops::Bound::Included(lower),
                    hi: std::ops::Bound::Included(upper),
                });
            }
        }
        self.lookup_first_k(name_interned, k).await
    }

    /// Build inclusive (lower, upper) physical-key bounds for one
    /// sorted-index range query.
    ///
    /// - `start_encoded = None` → lower = `prefix` itself (start of
    ///   the index's keyspace).
    /// - `end_encoded = None` → upper = `prefix || [0xFF; 64]`,
    ///   strictly greater than any real entry in this prefix and
    ///   strictly less than the start of the next prefix
    ///   (`name_interned + 1`), so it correctly bounds "everything in
    ///   this index" without leaking into neighbours.
    /// - Otherwise the bounds are `prefix || encoded[ || 0xFF×16]`.
    fn range_bounds(
        &self,
        prefix: &Bytes,
        start_encoded: Option<&[u8]>,
        end_encoded: Option<&[u8]>,
    ) -> (Bytes, Bytes) {
        let lower = match start_encoded {
            Some(enc) => {
                let mut k = Vec::with_capacity(prefix.len() + enc.len());
                k.extend_from_slice(prefix);
                k.extend_from_slice(enc);
                Bytes::from(k)
            }
            None => prefix.clone(),
        };
        let upper = match end_encoded {
            Some(enc) => {
                let mut k = Vec::with_capacity(prefix.len() + enc.len() + 16);
                k.extend_from_slice(prefix);
                k.extend_from_slice(enc);
                // Cover all record_id tiebreakers at the upper value.
                k.extend_from_slice(&[0xFFu8; 16]);
                Bytes::from(k)
            }
            None => {
                let mut k = Vec::with_capacity(prefix.len() + 64);
                k.extend_from_slice(prefix);
                k.extend_from_slice(&[0xFFu8; 64]);
                Bytes::from(k)
            }
        };
        (lower, upper)
    }

    // ----- internals --------------------------------------------------------

    /// Count of entries currently in the sorted index — used by the
    /// doctor's verify pass. O(K) where K is the entry count.
    pub async fn entry_count(&self, name_interned: u64) -> DbResult<u64> {
        let prefix = self.entry_prefix(name_interned);
        let mut count: u64 = 0;
        let stream = self.info_store.scan_prefix_stream(prefix, 1024);
        futures::pin_mut!(stream);
        while let Some(batch) = stream.next().await {
            count += batch?.len() as u64;
        }
        Ok(count)
    }

    /// True iff `record` carries a value at `field_path` that the
    /// sort codec can encode (i.e. an entry for this record *should*
    /// exist in a sorted index keyed on this path).
    pub fn has_indexable_value(record: &(impl RecordRef + ?Sized), field_path: &[u64]) -> bool {
        matches!(extract_and_encode(record, field_path), Ok(Some(_)))
    }

    /// Prefix common to every entry of one sorted index.
    fn entry_prefix(&self, name_interned: u64) -> Bytes {
        let mut buf = Vec::with_capacity(9);
        buf.push(SORTED_TAG);
        buf.extend_from_slice(&name_interned.to_be_bytes());
        Bytes::from(buf)
    }

    /// Full entry key for one (value, record_id) pair.
    fn build_entry_key(
        &self,
        name_interned: u64,
        encoded_value: &[u8],
        record_id: &RecordId,
    ) -> Bytes {
        let mut buf = Vec::with_capacity(1 + 8 + encoded_value.len() + 16);
        buf.push(SORTED_TAG);
        buf.extend_from_slice(&name_interned.to_be_bytes());
        buf.extend_from_slice(encoded_value);
        buf.extend_from_slice(&record_id.to_bytes());
        Bytes::from(buf)
    }

    async fn persist_defs(&self) -> DbResult<()> {
        let defs: Vec<SortedIndexDefinition> = self.iter_indexes();
        let bytes = bincode::serialize(&defs).map_err(|e| {
            shamir_storage::error::DbError::Codec(format!("sorted-index defs encode: {e}"))
        })?;
        let sys_id = RecordId::system("sorted_indexes");
        self.info_store
            .set(sys_id.to_bytes().into(), Bytes::from(bytes))
            .await?;
        Ok(())
    }

    async fn load(&self) -> DbResult<()> {
        let sys_id = RecordId::system("sorted_indexes");
        let bytes = match self.info_store.get(sys_id.to_bytes().into()).await {
            Ok(b) => b,
            Err(_) => return Ok(()),
        };
        if bytes.is_empty() {
            return Ok(());
        }
        // F-72 (#899): three-tier decode, try current shape first, falling
        // back to progressively older shapes — mirrors
        // `persistence::load_index2_metadata`'s pattern (see
        // `sorted_index_definition.rs`'s `state` field doc for why
        // `#[serde(default)]` alone does NOT rescue a pre-`state` blob under
        // this workspace's pinned bincode). Tier 2 (`SortedIndexDefinitionNoState`)
        // handles a blob written after covering indexes but before F-72's
        // `state` field; tier 3 (`SortedIndexDefinitionV1`) handles the
        // original pre-covering-index shape. A legacy tier's definitions are
        // lifted to `state = Ready` — every pre-`state` persisted index was,
        // by definition, fully built.
        let defs: Vec<SortedIndexDefinition> =
            match bincode::deserialize::<Vec<SortedIndexDefinition>>(bytes.as_ref()) {
                Ok(d) => d,
                Err(new_err) => {
                    match bincode::deserialize::<Vec<SortedIndexDefinitionNoState>>(bytes.as_ref())
                    {
                        Ok(no_state) => {
                            log::warn!(
                                "sorted-index defs: decoded with pre-`state` legacy fallback \
                                 ({} definition(s) lifted to state=Ready). New-shape decode \
                                 error: {}",
                                no_state.len(),
                                new_err
                            );
                            no_state
                                .into_iter()
                                .map(SortedIndexDefinition::from)
                                .collect()
                        }
                        Err(_) => {
                            let v1s: Vec<SortedIndexDefinitionV1> =
                                bincode::deserialize(bytes.as_ref()).map_err(|e| {
                                    shamir_storage::error::DbError::Codec(format!(
                                        "sorted-index defs decode: {e}"
                                    ))
                                })?;
                            v1s.into_iter().map(SortedIndexDefinition::from).collect()
                        }
                    }
                }
            };
        // Last-write-wins dedup by name_interned (matches the previous
        // DashMap::insert loop, on disk Vec is already deduped via
        // persist_defs / iter_indexes but we defensively dedup here too).
        let mut deduped: std::collections::BTreeMap<u64, SortedIndexDefinition> =
            std::collections::BTreeMap::new();
        for d in defs {
            deduped.insert(d.name_interned, d);
        }
        let new_vec: Vec<SortedIndexDefinition> = deduped.into_values().collect();
        // F-71 (#898): fixes vector 1 of the F-67 regression — seed the
        // in-memory AsOf-gate high-water for EVERY loaded index from its
        // durable `ready_at_version` (set once at backfill-completion time by
        // `mark_ready_at`), so a restart restores the EXACT epoch instead of
        // `last_mutation_version` defaulting to `0` for an index this fresh
        // `SortedIndexManager` has not yet observed a mutation for in THIS
        // process. Pre-fix, `load()` hydrated only `self.indexes`
        // (definitions) and never touched this map, so `0` was every
        // restarted index's epoch regardless of how much mutation history it
        // actually carried — `0 <= pinned` then held for every AsOf query,
        // wrongly opening the seek fast path against an index that might not
        // mirror the pinned snapshot at all. `note_mutation_at_version`'s
        // `fetch_max` semantics make this call idempotent and order-
        // independent with any other seed. A definition persisted before
        // F-71 decodes with `ready_at_version == 0` (`#[serde(default)]`),
        // which reproduces exactly the OLD (permissive but not regressed
        // further) default for data written before this fix shipped.
        for def in &new_vec {
            self.note_mutation_at_version(def.name_interned, def.ready_at_version);
        }
        self.indexes.store(new_vec);
        Ok(())
    }
}

/// Serialised covering-index projection type: a list of
/// `(field_path_dotted, QueryValue)` pairs, encoded with MessagePack.
///
/// S9: changed from `Vec<(String, InnerValue)>` to `Vec<(String, QueryValue)>`
/// as part of the InnerValue-elimination campaign. `QueryValue = Value<String>`
/// has NO `InternerKey` dependency.
///
/// The field_path_dotted key is the segments joined with "." so the
/// read-side (S3.3) can reconstruct the projection without the interner.
///
/// Format: `rmp_serde::to_vec_named(&Vec<(String, QueryValue)>)` — bincode is
/// not usable because `Value`'s `Deserialize` relies on `deserialize_any`,
/// which bincode does not support.
///
/// The wire format for SCALAR leaves (Null, Bool, Int, F64, Str, Bin, Dec, Big)
/// is byte-identical between `QueryValue` and `InnerValue`, so the decode side
/// (`decode_covering_projection`) can deserialize as `Vec<(String, InnerValue)>`
/// without conversion. Container leaves are skipped at encode time (they would
/// differ due to key types).
type CoveringProjection = Vec<(String, QueryValue)>;

/// Build the covering-index projection value for one record and one
/// `SortedIndexDefinition` that has non-empty `included_fields_interned`.
///
/// S9: produces `Vec<(String, QueryValue)>` instead of `Vec<(String, InnerValue)>`.
/// For each included field path:
///   - Walk `record` by the interned path segments via `RecordRef::materialize_at`.
///   - Convert the leaf to `QueryValue` (scalar-only; containers are skipped).
///   - If the leaf is present, push `(path_joined_with_dots, leaf)` (owned).
///   - Missing / container paths are silently skipped.
///
/// Returns `Bytes::new()` when no fields could be resolved (backward-compat:
/// write side acts as if no projection; read side sees empty value).
///
/// When the projection is non-empty the returned bytes are a **versioned
/// envelope**:
/// ```text
/// [8 bytes: version as u64 little-endian] ++ [msgpack: Vec<(String, QueryValue)>]
/// ```
/// The `version` parameter should be the MVCC write version for the record
/// being indexed (pass `0` when no MVCC store is attached).
fn build_covering_projection(
    record: &(impl RecordRef + ?Sized),
    def: &SortedIndexDefinition,
    version: u64,
) -> Bytes {
    let mut projection: CoveringProjection = Vec::new();
    for (path_strs, path_ids) in def
        .included_fields
        .iter()
        .zip(def.included_fields_interned.iter())
    {
        if path_ids.is_empty() {
            continue;
        }
        let ipath: SmallVec<[InternerKey; 4]> =
            path_ids.iter().map(|&id| InternerKey::new(id)).collect();
        if let Some(leaf) = record.materialize_at(&ipath) {
            if let Some(qv) = inner_value_to_query_scalar(&leaf) {
                let key_str = path_strs.join(".");
                projection.push((key_str, qv));
            }
            // Container leaves (Map/List/Set) are skipped — their QueryValue
            // wire format differs from InnerValue due to key types, and the
            // decode side reads as InnerValue. Scalar leaves are wire-identical.
        }
    }
    if projection.is_empty() {
        return Bytes::new();
    }
    // Use MessagePack (rmp_serde) because Value's Deserialize impl
    // relies on `deserialize_any`, which bincode does not support.
    match rmp_serde::to_vec_named(&projection) {
        Ok(msgpack) => {
            let mut out = version.to_le_bytes().to_vec();
            out.extend_from_slice(&msgpack);
            Bytes::from(out)
        }
        Err(_) => Bytes::new(),
    }
}

/// Convert an `InnerValue` to `QueryValue` for SCALAR types only.
/// Returns `None` for Map/List/Set containers (whose wire format would
/// differ due to InternerKey vs String keys).
fn inner_value_to_query_scalar(v: &InnerValue) -> Option<QueryValue> {
    match v {
        InnerValue::Null => Some(QueryValue::Null),
        InnerValue::Bool(b) => Some(QueryValue::Bool(*b)),
        InnerValue::Int(i) => Some(QueryValue::Int(*i)),
        InnerValue::F64(f) => Some(QueryValue::F64(*f)),
        InnerValue::Dec(d) => Some(QueryValue::Dec(*d)),
        InnerValue::Big(b) => Some(QueryValue::Big(b.clone())),
        InnerValue::Str(s) => Some(QueryValue::Str(s.clone())),
        InnerValue::Bin(b) => Some(QueryValue::Bin(b.clone())),
        // Container leaves are skipped — see doc on build_covering_projection.
        InnerValue::List(_) | InnerValue::Set(_) | InnerValue::Map(_) => None,
    }
}

/// Decode a versioned covering-projection envelope written by
/// `build_covering_projection`. Returns `None` for an empty value, a
/// value shorter than 8 bytes, or one whose msgpack body fails to
/// decode (callers treat `None` as "fall back to a full fetch").
///
/// S9: the encode side writes `Vec<(String, QueryValue)>` but the wire
/// format for scalar leaves is byte-identical to `Vec<(String, InnerValue)>`,
/// so this function can deserialize as `InnerValue` without conversion.
/// The return type stays `InnerValue` for engine API compatibility.
///
/// Used by slice A3 (index-only read path).
pub fn decode_covering_projection(value: &[u8]) -> Option<(u64, Vec<(String, InnerValue)>)> {
    if value.len() < 8 {
        return None;
    }
    let version = u64::from_le_bytes(value[..8].try_into().ok()?);
    let projection: Vec<(String, InnerValue)> = rmp_serde::from_slice(&value[8..]).ok()?;
    Some((version, projection))
}

/// Extract the value at `field_path` from a record and encode it via
/// `sort_codec`. Returns `None` if the field is missing or has a type
/// we don't index (we intentionally skip such records — they won't
/// surface in sorted lookups).
///
/// Reads the scalar via `RecordRef::scalar_at` and dispatches to the
/// SAME `sort_codec::encode_*` primitives the legacy `&InnerValue` path
/// used. `scalar_at` yields exactly {Null, Bool, Int, F64, Str, Bin} for
/// comparable scalars and `None` for Dec/Big/containers/absent —
/// byte-identical to the previous `resolve_path_ref` + InnerValue match,
/// including all the skip cases.
fn extract_and_encode(
    rec: &(impl RecordRef + ?Sized),
    field_path: &[u64],
) -> DbResult<Option<Vec<u8>>> {
    let ipath: SmallVec<[InternerKey; 4]> =
        field_path.iter().map(|&id| InternerKey::new(id)).collect();
    let Some(sr) = rec.scalar_at(&ipath) else {
        return Ok(None);
    };
    let mut buf = Vec::with_capacity(16);
    match sr {
        ScalarRef::Null => sort_codec::encode_null(&mut buf),
        ScalarRef::Bool(b) => sort_codec::encode_bool(&mut buf, b),
        ScalarRef::Int(i) => sort_codec::encode_i64(&mut buf, i),
        ScalarRef::F64(f) => {
            if sort_codec::encode_f64(&mut buf, f).is_err() {
                return Ok(None);
            }
        }
        ScalarRef::Str(s) => sort_codec::encode_str(&mut buf, s),
        ScalarRef::Bin(b) => sort_codec::encode_bytes(&mut buf, b),
    }
    Ok(Some(buf))
}

/// The encoded-value slice of a sorted-index physical key
/// `[tag|name|encoded_value|record_id]`: everything between the
/// `value_start` offset (== prefix length) and the trailing 16-byte
/// record_id. Returns `&[]` for a malformed / too-short key.
fn key_value_slice(key_bytes: &[u8], value_start: usize) -> &[u8] {
    if key_bytes.len() < value_start + 16 {
        return &[];
    }
    &key_bytes[value_start..key_bytes.len() - 16]
}

/// Decide whether a boundary-value physical entry (one whose encoded value
/// equals the seek value) must be SKIPPED, given the optional record-id
/// tie-breaker `after_id` (task #537).
///
/// * `after_id == None` → skip every boundary row (today's backward-compatible
///   skip-all-ties behavior — an old client that sent no tie-breaker).
/// * `after_id == Some(a)`, `forward == true` (ASC) → skip only rows whose
///   record_id is `<= a` (already seen / the boundary row itself). Rows tied
///   on the seek value with `record_id > a` are RETURNED.
/// * `after_id == Some(a)`, `forward == false` (DESC) → skip only rows whose
///   record_id is `>= a`. Rows tied on the seek value with `record_id < a`
///   are RETURNED.
///
/// A key whose record-id suffix can't be decoded is skipped (malformed).
fn skip_boundary_row(key_bytes: &[u8], after_id: Option<&RecordId>, forward: bool) -> bool {
    match after_id {
        None => true,
        Some(a) => match decode_record_id_suffix(key_bytes) {
            Some(rid) => {
                if forward {
                    // ASC: strictly past means record_id > a; skip otherwise.
                    rid <= *a
                } else {
                    // DESC: strictly past means record_id < a; skip otherwise.
                    rid >= *a
                }
            }
            None => true,
        },
    }
}

fn decode_record_id_suffix(key_bytes: &[u8]) -> Option<RecordId> {
    if key_bytes.len() < 16 {
        return None;
    }
    let tail = &key_bytes[key_bytes.len() - 16..];
    let mut arr = [0u8; 16];
    arr.copy_from_slice(tail);
    Some(RecordId(arr))
}

/// F-67 (#893): decode the `name_interned` (u64, big-endian) out of a
/// physical sorted-index key `[SORTED_TAG (1) | name_interned (8 BE) |
/// encoded_value | record_id (16)]` — the exact layout [`SortedIndexManager
/// ::build_entry_key`]/[`SortedIndexManager::entry_prefix`] write.
///
/// Used by [`SortedIndexManager::bump_touched_indexes`] to recover per-index
/// identity from the `IndexWriteOp::{SetPosting, RemovePosting}` keys the
/// manager's own planners produced — `IndexWriteOp` itself carries no
/// separate index-id field (it's a pure-data enum shared across every index
/// family: hash, sorted, FTS, functional), so the physical key is the only
/// place the identity survives once ops are collected into a flat
/// `Vec<IndexWriteOp>` (see `commit_phases.rs::apply_index_batch`, which
/// resolves this the same way for the tx-commit path).
///
/// Returns `None` for a key that is too short or is not `SORTED_TAG`-prefixed
/// (a foreign key from a different index family sharing the same
/// `info_store` — should never reach this manager's own planner-produced
/// ops, but decoding defensively rather than panicking).
fn decode_sorted_index_name(key_bytes: &[u8]) -> Option<u64> {
    if key_bytes.len() < 1 + 8 || key_bytes[0] != SORTED_TAG {
        return None;
    }
    let mut arr = [0u8; 8];
    arr.copy_from_slice(&key_bytes[1..9]);
    Some(u64::from_be_bytes(arr))
}
