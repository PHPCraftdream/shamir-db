use bytes::Bytes;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use super::SortedIndexDefinition;
use crate::base_index::sorted_index_definition::{
    SortedIndexDefinitionNoState, SortedIndexDefinitionV1,
};
use shamir_storage::error::DbResult;
use shamir_storage::types::Store;
use shamir_types::types::common::THasher;
use shamir_types::types::record_id::RecordId;

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
    pub(super) info_store: Arc<dyn Store>,
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
    pub(super) indexes: Arc<shamir_numa::NodeReplicated<Vec<SortedIndexDefinition>>>,
    /// F-50 Step 2 (#870): monotonic generation counter bumped on
    /// every `register` / `drop_index` (the two operations that change the
    /// queryable def set). Mirrors `IndexRegistry::generation` for the
    /// base_index sorted-index path: a tx captures this at stage time
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
    pub(super) generation: Arc<AtomicU64>,

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
    pub(super) last_mutation_version: Arc<scc::HashMap<u64, AtomicU64, THasher>>,

    /// P0-3b (#972): in-memory mirror of the persisted tombstone set for
    /// in-progress DROP INDEX operations on the SORTED family. When
    /// `drop_index` starts, it adds `name_interned` here AND persists it
    /// durably (see `add_to_dropping_sorted`) BEFORE sweeping postings; when
    /// the drop completes (sweep + persist reduced defs), it removes the
    /// entry (see `clear_from_dropping_sorted`). On restart, `new` loads the
    /// persisted tombstone, runs the recovery sweep, and clears both the
    /// persisted and in-memory sets. Also consulted by `register` to reject
    /// a CREATE that would reuse a name whose DROP is still in flight
    /// (sub-bug 3b). Mirrors #959's `dropping_regular`/`dropping_unique`
    /// exactly for the sorted family.
    ///
    /// `std::sync::Mutex` is the sanctioned low-frequency fallback here
    /// (CLAUDE.md: "only low-frequency/setup fallbacks, justified inline").
    /// DROP INDEX is a DDL operation — contention is nil in normal
    /// operation and the set is empty 99.999 % of the time. The lock is
    /// NEVER held across an `.await` point.
    pub(super) dropping_sorted: Arc<Mutex<BTreeSet<u64>>>,

    /// P0-5b (#962): in-memory mirror of the persisted "Renaming" tombstone set
    /// for in-progress RENAME INDEX operations on the SORTED family. Each entry
    /// maps `old_id → new_id`. When `rename_index_sorted` starts, it adds the
    /// `(old_id, new_id)` pair here AND persists it durably (see
    /// `add_to_renaming_sorted`) BEFORE `rename_definition` runs; when the
    /// rename completes (definition swapped + postings rekeyed), it removes the
    /// entry (see `clear_from_renaming_sorted`). On restart, `new` loads the
    /// persisted tombstone and `recover_in_progress_renames` resumes the
    /// (idempotent) rekey settle loop for each pair, then clears the tombstone.
    ///
    /// This closes the REAL remaining gap from the P0-5 review: `rename_index`
    /// previously swapped the definition to `new_id` FIRST and then ran
    /// `rekey_sorted_prefix`; if the rekey returned `Err` (a real backend
    /// failure, not just non-atomic partial visibility — the per-batch
    /// atomicity is already handled by the settle loop + `Store::transact`,
    /// F-85/#913), the error propagated to the client while the catalog
    /// already believed the rename succeeded. A client RETRY of the same
    /// command then failed immediately (the source name no longer resolved),
    /// leaving orphan postings permanently stranded under the old
    /// `SORTED_TAG || old_id` prefix (never queried under either name, silent
    /// storage leak, and ghost-posting collision if `old_id` is ever reused).
    /// The durable tombstone makes the interrupted rekey RESUMABLE on restart
    /// instead of orphaned. Mirrors #972's `dropping_sorted` for rename.
    ///
    /// `std::sync::Mutex` is the sanctioned low-frequency fallback here
    /// (CLAUDE.md: "only low-frequency/setup fallbacks, justified inline").
    /// RENAME INDEX is a DDL operation — contention is nil in normal
    /// operation and the set is empty 99.999 % of the time. The lock is
    /// NEVER held across an `.await` point.
    pub(super) renaming_sorted: Arc<Mutex<BTreeMap<u64, u64>>>,

    /// P0-3b (#972): test-only deterministic pause point that fires AFTER the
    /// definition is retired but BEFORE the posting sweep (the pre-sweep
    /// window). A regression test installs this hook, parks `drop_index`
    /// mid-drop (tombstone written + def retired, pre-sweep), then asserts
    /// the namespace-reuse guard rejects a concurrent `register`. NOT
    /// `#[cfg(test)]`-gated — cross-crate test consumer, same reason as
    /// #959's `drop_index_pause_hook`. `None` on every real path.
    pub(super) drop_index_pause_hook:
        Arc<arc_swap::ArcSwapOption<crate::base_index::backfill_pause_hook::BackfillPauseHook>>,

    /// P0-3b (#972): test-only deterministic pause point that fires AFTER the
    /// posting sweep but BEFORE the reduced defs are persisted — the exact
    /// crash window sub-bug 3c tests. A regression test installs this hook,
    /// parks `drop_index` mid-drop, drops the manager (simulating a crash),
    /// then constructs a fresh `SortedIndexManager::new` against the SAME
    /// `info_store` and asserts the recovery path finishes the drop instead
    /// of resurrecting a broken `Ready` index. NOT `#[cfg(test)]`-gated —
    /// cross-crate test consumer. `None` on every real path. Mirrors #959's
    /// `drop_index_post_sweep_hook`.
    pub(super) drop_index_post_sweep_hook:
        Arc<arc_swap::ArcSwapOption<crate::base_index::backfill_pause_hook::BackfillPauseHook>>,

    /// P0-5b (#962): test-only deterministic pause point that fires AFTER
    /// `rename_definition` has swapped-and-persisted the definition
    /// `old_id → new_id` but BEFORE `rekey_postings` runs — the EXACT crash
    /// window the P0-5b bug describes (definition already under `new_id`,
    /// postings still stranded under `old_id`). A regression test installs
    /// this hook, parks `rename_index_sorted` mid-rename, drops the manager
    /// (simulating a crash), then constructs a fresh
    /// `SortedIndexManager::new` against the SAME `info_store` and asserts
    /// the recovery path finishes the rekey. NOT `#[cfg(test)]`-gated —
    /// cross-crate test consumer. `None` on every real path. Mirrors #972's
    /// `drop_index_post_sweep_hook`.
    pub(super) rename_rekey_pause_hook:
        Arc<arc_swap::ArcSwapOption<crate::base_index::backfill_pause_hook::BackfillPauseHook>>,

    /// P0-3a (#1037) — reader-vs-DROP mutual exclusion gate for the sorted
    /// index family. Mirrors `IndexManager::reader_gate` (slice 1, #1011).
    /// Closes the race where a reader that resolves an index definition BEFORE
    /// `drop_index` retires it can scan the shared `info_store` keyspace WHILE
    /// the sweep runs, observing a partially-swept keyspace. See
    /// `crate::reader_drain_gate`'s module doc for the full design (flag +
    /// counter + reader back-off, not epoch-parity RCU).
    pub(super) reader_gate: crate::reader_drain_gate::ReaderDrainGate,
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
            dropping_sorted: Arc::clone(&self.dropping_sorted),
            drop_index_pause_hook: Arc::clone(&self.drop_index_pause_hook),
            drop_index_post_sweep_hook: Arc::clone(&self.drop_index_post_sweep_hook),
            renaming_sorted: Arc::clone(&self.renaming_sorted),
            rename_rekey_pause_hook: Arc::clone(&self.rename_rekey_pause_hook),
            reader_gate: self.reader_gate.clone(),
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
            dropping_sorted: Arc::new(Mutex::new(BTreeSet::new())),
            drop_index_pause_hook: Arc::new(arc_swap::ArcSwapOption::empty()),
            drop_index_post_sweep_hook: Arc::new(arc_swap::ArcSwapOption::empty()),
            renaming_sorted: Arc::new(Mutex::new(BTreeMap::new())),
            rename_rekey_pause_hook: Arc::new(arc_swap::ArcSwapOption::empty()),
            reader_gate: crate::reader_drain_gate::ReaderDrainGate::new(),
        };
        m.load().await?;
        // P0-3b (#972): resume any in-progress DROP INDEX operations that were
        // interrupted by a crash between the tombstone write and the final
        // defs persist. The tombstone is persisted under a separate
        // `system:sidx_drop` key; if present, the recovery path re-runs the
        // (idempotent) sweep and removes the stale definition so the planner
        // never resurrects a fully-broken "Ready but no postings" index. See
        // `recover_in_progress_drops`'s doc for the full crash-state matrix.
        // Mirrors #959's base_index-family open-time recovery.
        m.recover_in_progress_drops().await?;
        // P0-5b (#962): resume any in-progress RENAME INDEX operations that
        // were interrupted by a crash between the tombstone write and the
        // rekey's completion. The tombstone is persisted under a separate
        // `system:sidx_ren` key recording `(old_id, new_id)` pairs; if
        // present, the recovery path re-runs the (idempotent) rekey settle
        // loop for each pair, ensuring no postings are left orphaned under
        // the old prefix, then clears the tombstone. See
        // `recover_in_progress_renames`'s doc for the full crash-state
        // matrix. Mirrors #972's drop-recovery sibling.
        m.recover_in_progress_renames().await?;
        Ok(m)
    }

    /// P0-3b (#972) test-only: install (or clear with `None`) the deterministic
    /// pre-sweep pause hook (fires after the definition is retired, before the
    /// posting sweep — the window sub-bug 3b's namespace-reuse test parks in).
    /// Not `#[cfg(test)]`-gated — cross-crate test consumer, same reason as
    /// #959's `set_drop_index_pause_hook`.
    pub fn set_drop_index_pause_hook(
        &self,
        hook: Option<Arc<crate::base_index::backfill_pause_hook::BackfillPauseHook>>,
    ) {
        self.drop_index_pause_hook.store(hook);
    }

    /// P0-3b (#972) test-only: install (or clear with `None`) the deterministic
    /// post-sweep pause hook (fires after the sweep, before the reduced defs
    /// are persisted — the exact crash window sub-bug 3c exercises). Not
    /// `#[cfg(test)]`-gated — cross-crate test consumer.
    pub fn set_drop_index_post_sweep_hook(
        &self,
        hook: Option<Arc<crate::base_index::backfill_pause_hook::BackfillPauseHook>>,
    ) {
        self.drop_index_post_sweep_hook.store(hook);
    }

    /// P0-5b (#962) test-only: install (or clear with `None`) the deterministic
    /// pause hook that fires AFTER `rename_definition` swaps-and-persists the
    /// definition `old_id → new_id` but BEFORE `rekey_postings` runs — the
    /// exact crash window this fix targets (definition already under `new_id`,
    /// postings still under `old_id`). Not `#[cfg(test)]`-gated — cross-crate
    /// test consumer, same reason as #972's hooks.
    pub fn set_rename_rekey_pause_hook(
        &self,
        hook: Option<Arc<crate::base_index::backfill_pause_hook::BackfillPauseHook>>,
    ) {
        self.rename_rekey_pause_hook.store(hook);
    }

    /// P0-3a (#1037) test/telemetry oracle: how many `drop_index` drains
    /// genuinely had to wait for at least one in-flight reader. Passthrough to
    /// [`reader_drain_gate::ReaderDrainGate::drain_waits`]. The drain-waits
    /// counter is the test oracle that discriminates "gate genuinely blocked a
    /// drain" from "gate was wired but never contended" — a lone `== 0`
    /// assertion passes vacuously if the counter is never incremented, so test
    /// suites always pair it with the contended `== 1` case (see
    /// `p1037_sorted_reader_drain_tests`).
    pub fn reader_drain_waits(&self) -> usize {
        self.reader_gate.drain_waits()
    }

    /// P0-3a (#1037) test-only: expose the reader gate for in-flight count
    /// checks. Used by regression tests to prove a reader is counted mid-flight
    /// (e.g., to verify `in_flight_count() == 1` while a read is parked). Not
    /// `#[cfg(test)]`-gated — cross-crate test consumer, same reason as the
    /// other hooks.
    pub fn reader_gate(&self) -> &crate::reader_drain_gate::ReaderDrainGate {
        &self.reader_gate
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
        // P0-3b (#972) sub-bug 3b: reject CREATE for a name whose DROP is
        // still in flight — a tombstoned name has had (or is having) its
        // postings swept; a fresh create would inherit orphan ghost postings
        // keyed by the same `name_interned`. Mirrors #959's guard in
        // `create_index[_from_records][_from_stream]`.
        if self
            .dropping_sorted
            .lock()
            .unwrap()
            .contains(&def.name_interned)
        {
            return Err(shamir_storage::error::DbError::Internal(format!(
                "Cannot create sorted index '{}': \
                 a DROP INDEX for this name is still in progress",
                def.name_interned
            )));
        }
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

    // ============================================================================
    // Persistence internals
    // ============================================================================

    pub(super) async fn persist_defs(&self) -> DbResult<()> {
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

    /// P0-3b (#972): open-time recovery for DROP INDEX operations interrupted
    /// by a crash. Called from `SortedIndexManager::new` AFTER definitions are
    /// loaded but BEFORE the manager is returned to the caller.
    ///
    /// # Crash-state matrix (mirrors #959's `recover_in_progress_drops`)
    ///
    /// | crash point                       | defs has def? | tombstone? | recovery action            |
    /// |-----------------------------------|---------------|------------|----------------------------|
    /// | after tombstone-write, pre-sweep  | yes (Ready)   | yes        | remove def, sweep, persist |
    /// | after sweep, pre-persist          | yes (Ready)   | yes        | remove def, sweep (no-op), persist |
    /// | after persist, pre-clear          | no            | yes        | sweep (no-op), clear tombstone |
    ///
    /// In every case the recovery leaves the manager in a consistent state:
    /// the def is gone from the defs blob, its postings are swept, and the
    /// tombstone is cleared. The sweep is idempotent, so calling recovery
    /// twice (two restart attempts) is a clean no-op on the second call.
    pub(super) async fn recover_in_progress_drops(&self) -> DbResult<()> {
        let dropping = self.load_dropping_sorted().await?;
        if dropping.is_empty() {
            return Ok(());
        }

        log::info!(
            "P0-3b (#972): recovering {} in-progress sorted DROP(s)",
            dropping.len()
        );

        let mut changed = false;
        for &name_interned in &dropping {
            // If the def is still present, the crash happened before
            // `persist_defs` finalized the removal — retire it now.
            let mut removed_here = false;
            self.indexes.rcu(|cur| {
                let before = cur.len();
                let new_vec: Vec<SortedIndexDefinition> = cur
                    .iter()
                    .filter(|d| d.name_interned != name_interned)
                    .cloned()
                    .collect();
                removed_here = new_vec.len() != before;
                new_vec
            });
            if removed_here {
                self.generation.fetch_add(1, Ordering::AcqRel);
                changed = true;
            }
            // Always run the sweep (idempotent). Covers both the
            // "sweep never ran" and "sweep ran but persist failed" cases.
            self.sweep_sorted_postings(name_interned).await?;
        }
        if changed {
            self.persist_defs().await?;
        }

        // Clear the tombstone (write empty Vec<u64>; the load path treats
        // empty-vec and NotFound identically — cheaper than a remove).
        let empty = bincode::serialize(&Vec::<u64>::new())
            .map_err(|e| shamir_storage::error::DbError::Codec(e.to_string()))?;
        let key = RecordId::system("sidx_drop").to_bytes();
        self.info_store.set(key.into(), Bytes::from(empty)).await?;

        log::info!(
            "P0-3b (#972): recovery complete — {} sorted DROP(s) finalized",
            dropping.len()
        );

        Ok(())
    }
}
