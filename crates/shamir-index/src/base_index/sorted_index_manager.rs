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
use std::collections::{BTreeMap, BTreeSet};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
// Re-export so existing callers that import `SortedIndexDefinition` from this
// module continue to compile unchanged after the type moved to its own file.
pub use crate::base_index::sorted_index_definition::SortedIndexDefinition;
use crate::base_index::sorted_index_definition::{
    SortedIndexDefinitionNoState, SortedIndexDefinitionV1, SORTED_TAG,
};
use crate::write_ops::IndexWriteOp;
use shamir_storage::error::{DbError, DbResult};
use shamir_storage::types::{KvOp, RecordKey, Store};
use shamir_tunables::store_defaults::{FULL_SCAN_BATCH, MAINT_SCAN_BATCH};
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
    dropping_sorted: Arc<Mutex<BTreeSet<u64>>>,

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
    renaming_sorted: Arc<Mutex<BTreeMap<u64, u64>>>,

    /// P0-3b (#972): test-only deterministic pause point that fires AFTER the
    /// definition is retired but BEFORE the posting sweep (the pre-sweep
    /// window). A regression test installs this hook, parks `drop_index`
    /// mid-drop (tombstone written + def retired, pre-sweep), then asserts
    /// the namespace-reuse guard rejects a concurrent `register`. NOT
    /// `#[cfg(test)]`-gated — cross-crate test consumer, same reason as
    /// #959's `drop_index_pause_hook`. `None` on every real path.
    drop_index_pause_hook:
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
    drop_index_post_sweep_hook:
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
    rename_rekey_pause_hook:
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

    // ========================================================================
    // P0-3b (#972) — DROP INDEX durable tombstone + crash recovery (sorted)
    // ========================================================================
    //
    // Mirrors #959's base_index-family fix for the sorted family.
    //
    // Sub-bug 3c (crash-resurrection): the OLD `drop_index` had retire →
    // sweep → `persist_defs` ordering. A crash between the sweep and the
    // `persist_defs` call left the on-disk defs listing the index as present
    // while its postings were gone — on restart `load()` would restore the
    // stale `Ready` definition and the planner would route range/order/min
    // queries to an index with zero postings, silently returning wrong
    // (empty / missing) results with no error. The durable tombstone (written
    // BEFORE the sweep, cleared AFTER the persist) closes this: on restart,
    // `recover_in_progress_drops` sees the tombstone, resumes the sweep
    // (idempotent), removes the stale definition, persists, and clears the
    // tombstone.
    //
    // Sub-bug 3b (name-reuse ghost postings): a name in the tombstone set is
    // rejected by `register` (the sorted CREATE entry point) until the
    // tombstone clears, preventing ghost postings from namespace reuse.
    //
    // Tombstone shape: `Vec<u64>` serialized via bincode under
    // `system:sidx_drop`. A separate key was chosen (mirroring #959's
    // `idx_drop`/`uidx_drop`) to avoid touching `persist_defs`/`load`'s
    // three-tier bincode forward-compat fallback chain (current-shape →
    // pre-`state` → V1). An absent key or empty vec means "no in-progress
    // drops".
    //
    // **CRITICAL** (the hard-won #959 collision lesson): the key name
    // `"sidx_drop"` is short by design — `RecordId::system(name)` truncates
    // `name` to 12 bytes (see `shamir-types::types::record_id::system`). The
    // only other sorted system key is `"sorted_indexes"` (truncates to
    // `"sorted_index"`), which differs from `"sidx_drop"` at byte index 1
    // (`o` vs `i`) — NO collision. Verified against every
    // `RecordId::system(...)` call site in the workspace (the #959 review
    // caught a real `"indexes_unique_dropping"` ↔ `"indexes_unique"` clash
    // that both truncate to `"indexes_uniq"`).

    /// P0-3b (#972): persist the current in-memory `dropping_sorted` set to
    /// `info_store` under `system:sidx_drop`. Serializes the set as a
    /// `Vec<u64>` (deterministic order via `BTreeSet`). An empty set writes
    /// an empty `Vec<u64>` (NOT a key deletion) so the load path handles
    /// both `NotFound` and empty-vec uniformly. Mirrors #959's
    /// `save_dropping_regular`.
    pub(super) async fn save_dropping_sorted(&self) -> DbResult<()> {
        let snapshot: Vec<u64> = {
            let set = self.dropping_sorted.lock().unwrap();
            set.iter().copied().collect()
        };
        let key = RecordId::system("sidx_drop").to_bytes();
        let bytes = bincode::serialize(&snapshot)
            .map_err(|e| shamir_storage::error::DbError::Codec(e.to_string()))?;
        self.info_store.set(key.into(), Bytes::from(bytes)).await?;
        Ok(())
    }

    /// P0-3b (#972): load a persisted dropping set from `info_store`. Returns
    /// an empty `Vec` if the key is absent (`NotFound`) or contains an empty
    /// vec — both mean "no in-progress drops". Mirrors #959's
    /// `load_dropping_set`.
    pub(super) async fn load_dropping_sorted(&self) -> DbResult<Vec<u64>> {
        let key = RecordId::system("sidx_drop").to_bytes();
        match self.info_store.get(key.into()).await {
            Ok(bytes) => {
                if bytes.is_empty() {
                    return Ok(Vec::new());
                }
                bincode::deserialize::<Vec<u64>>(&bytes).map_err(|e| {
                    shamir_storage::error::DbError::Codec(format!(
                        "system:sidx_drop decode failed: {e}"
                    ))
                })
            }
            Err(shamir_storage::error::DbError::NotFound(_)) => Ok(Vec::new()),
            Err(e) => Err(e),
        }
    }

    /// P0-3b (#972): add `name_interned` to the in-memory dropping set, then
    /// persist the updated set durably. MUST be called BEFORE the sweep in
    /// `drop_index` so a crash at any subsequent point is recoverable.
    ///
    /// In-memory update happens FIRST (so a concurrent `register`
    /// immediately sees the name as guarded), then the persist follows. If
    /// the persist fails, the in-memory update is rolled back and the caller
    /// receives `Err` — the drop MUST NOT proceed without a durable
    /// tombstone. A crash between the in-memory update and the persist is
    /// safe: nothing has been swept or persisted yet, so the on-disk state
    /// is unchanged (old defs, postings intact). Mirrors #959's
    /// `add_to_dropping`.
    pub(super) async fn add_to_dropping_sorted(&self, name_interned: u64) -> DbResult<()> {
        {
            let mut set = self.dropping_sorted.lock().unwrap();
            set.insert(name_interned);
        }
        let result = self.save_dropping_sorted().await;
        if result.is_err() {
            // Roll back the in-memory set so the guard stays consistent.
            let mut set = self.dropping_sorted.lock().unwrap();
            set.remove(&name_interned);
        }
        result
    }

    /// P0-3b (#972): clear `name_interned` from the persisted tombstone, then
    /// from the in-memory set. Persist-first ordering ensures the on-disk
    /// state is always at least as advanced as the in-memory state: a crash
    /// between persist and in-memory update leaves a stale in-memory entry
    /// that dies with the process, while the on-disk tombstone is already
    /// correct.
    ///
    /// MUST be called AFTER `persist_defs` (the reduced defs must be durable
    /// first). If the process crashes between the defs persist and this
    /// clear, recovery sees the tombstone but the def is already gone from
    /// the persisted defs — it just clears the tombstone (a no-op sweep).
    /// Mirrors #959's `clear_from_dropping`.
    pub(super) async fn clear_from_dropping_sorted(&self, name_interned: u64) -> DbResult<()> {
        // Compute the snapshot without the entry (do NOT modify in-memory yet).
        let snapshot: Vec<u64> = {
            let set = self.dropping_sorted.lock().unwrap();
            set.iter()
                .filter(|&&k| k != name_interned)
                .copied()
                .collect()
        };
        // Persist first.
        let key = RecordId::system("sidx_drop").to_bytes();
        let bytes = bincode::serialize(&snapshot)
            .map_err(|e| shamir_storage::error::DbError::Codec(e.to_string()))?;
        self.info_store.set(key.into(), Bytes::from(bytes)).await?;
        // Only now update in-memory.
        {
            let mut set = self.dropping_sorted.lock().unwrap();
            set.remove(&name_interned);
        }
        Ok(())
    }

    /// P0-3b (#972): sweep all posting entries for the given index prefix.
    /// Extracted from `drop_index` so the recovery path can re-run it
    /// idempotently. Removing already-removed keys is a no-op on every
    /// `Store` backend. Mirrors #959's `sweep_index_postings`.
    pub(super) async fn sweep_sorted_postings(&self, name_interned: u64) -> DbResult<()> {
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

    /// Drop a sorted index definition AND every entry written under
    /// it. O(I) where I is the size of the index.
    ///
    /// Process (F-76 / #903 — definition retired BEFORE the posting sweep,
    /// the mirror image of F-72's CREATE gate; P0-3b / #972 — durable
    /// tombstone closes the crash-resurrection gap):
    /// 1. Fast existence check.
    /// 2. **P0-3b**: persist a durable tombstone (`system:sidx_drop`)
    ///    recording that `name_interned` is being dropped — MUST succeed
    ///    before the sweep, so a crash at any later point is recoverable.
    /// 3. Retires the definition from the in-memory Vec (planner-invisible
    ///    from this moment — RCU swap publishes a Vec without this def).
    /// 4. Sweeps all posting entries from `info_store` (now orphan,
    ///    planner-invisible).
    /// 5. Persists the reduced defs (def removed).
    /// 6. **P0-3b**: clears the tombstone.
    ///
    /// F-76 (#903): the RCU `indexes` swap (definition retirement) runs
    /// BEFORE the posting sweep, so a concurrent reader can never observe a
    /// registered-but-emptied sorted index (the in-flight-reader race this
    /// closes is documented on #959's `IndexManager::drop_index` doc — the
    /// same 3a residual applies here).
    ///
    /// P0-3b (#972) sub-bug 3c — crash-resurrection: the OLD code had a gap
    /// between the sweep (step 4) and the defs persist (step 5). If the
    /// process crashed in that gap, the on-disk defs still listed the index
    /// as `Ready`, but all its postings were gone — `SortedIndexManager::new`
    /// would `load()` the stale `Ready` definition and the planner would
    /// route queries to an index with zero postings, silently returning
    /// wrong (empty / missing) results. The durable tombstone (step 2)
    /// closes this: on restart, `recover_in_progress_drops` sees the
    /// tombstone, resumes the sweep (idempotent), removes the stale
    /// definition, persists, and clears the tombstone.
    ///
    /// #1067: accepts `op_id`/`index_name` so the terminal `Succeeded`
    /// status can be written BEFORE the tombstone clear (mirrors #1069's
    /// write-order fix for the base_index `IndexManager::drop_index`/
    /// `drop_unique_index` — see this same function's step 6 below).
    pub async fn drop_index(
        &self,
        name_interned: u64,
        op_id: Option<String>,
        index_name: Option<&str>,
    ) -> DbResult<bool> {
        // Fast existence check. TOCTOU-safe under the engine's write barrier
        // (drop_sorted_index is serialized via begin_write_barrier); mirrors
        // #959's base_index `if !self.indexes.contains(...)`.
        if self.find_by_name_interned(name_interned).is_none() {
            return Ok(false);
        }

        // P0-3b (#972): write a durable tombstone BEFORE retiring the
        // definition or sweeping postings. If the process crashes after the
        // sweep but before the reduced defs are persisted, the on-disk
        // metadata still lists the index — but the tombstone tells
        // `recover_in_progress_drops` to finish the drop rather than
        // resurrecting a broken "Ready but no postings" index. MUST succeed
        // before proceeding; `add_to_dropping_sorted` rolls back the
        // in-memory set on persist failure.
        self.add_to_dropping_sorted(name_interned).await?;

        // P0-3a (#1037) step 2.5: raise the reader-drain gate's intent flag
        // (SeqCst) BEFORE the RCU retire below. From this point every NEW
        // sorted-index reader observes the flag and backs off (empty result
        // → caller falls back to a full scan). The flag stays up until
        // `drain_guard` is dropped (step 4.5 + RAII safety net on early `?`).
        // See `crate::reader_drain_gate`'s module doc for the memory-model
        // proof and the deadlock-exclusion placement invariant.
        let drain_guard = self.reader_gate.begin_drop();

        // F-76 (#903): retire the definition from the planner-visible Vec
        // FIRST (RCU swap publishes a Vec without this definition atomically).
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
            // Race: the def vanished between our pre-check and the rcu (only
            // possible if a concurrent un-serialized caller violated the
            // engine's write-barrier contract). Undo the tombstone we just
            // wrote and report not-found — nothing was swept or persisted.
            // The RAII guard is dropped here on early return (step 4.5's safety
            // net), clearing the flag.
            self.clear_from_dropping_sorted(name_interned).await?;
            return Ok(false);
        }
        // F-50 Step 2 (#870): the queryable def set changed — advance the
        // generation so the commit pipeline's re-derivation gate fires for
        // any tx that staged before this drop.
        self.generation.fetch_add(1, Ordering::AcqRel);

        // P0-3b (#972) test seam — park here (definition already retired,
        // postings not yet swept) if a test installed the pre-sweep hook.
        // This is the window sub-bug 3b's namespace-reuse test parks in
        // (tombstone written + def retired, sweep pending). NOT
        // `#[cfg(test)]`-gated — cross-crate test consumer.
        if let Some(hook) = self.drop_index_pause_hook.load_full() {
            hook.wait_at_window().await;
        }

        // P0-3a (#1037) step 3.5: drain every reader that entered one of the
        // 8 sorted-index read chokepoints BEFORE the flag went up (the gate's
        // memory model guarantees such a reader is counted in `in_flight`,
        // so this wait terminates — see the livelock-freedom argument in the
        // gate's module doc). AFTER the RCU retire and the pause-hook park,
        // BEFORE the sweep, so the physical sweep cannot start until no
        // reader is mid-scan of the shared keyspace. Bounded by the gate's
        // escalating log; this wait sits inside the already-held
        // `ddl_admission` critical section exactly as the mirror-image
        // `WriterDrainBarrier::drain` does.
        drain_guard.wait_for_drain().await;

        // Sweep the (now orphan, planner-invisible) posting entries.
        // P0-3b (#972): extracted to `sweep_sorted_postings` so the recovery
        // path can re-run it idempotently.
        // P1-2 (#967): a durable tombstone is already persisted — enrich
        // the error if this sweep fails.
        // NOTE: Cannot write DdlOpState::Failed here because this layer
        // (SortedIndexManager) does not have op_id in scope.
        self.sweep_sorted_postings(name_interned)
            .await
            .map_err(|e| {
                shamir_storage::error::DbError::Internal(format!(
                    "DROP SORTED INDEX '{name_interned}': a durable drop tombstone \
                 was persisted and the definition was retired from the planner, \
                 but the posting sweep failed: {e}. On restart, recovery will \
                 resume the sweep idempotently and finish the drop. Call \
                 TableManager::verify() to confirm state."
                ))
            })?;

        // P0-3a (#1037) step 4.5: the sweep finished — explicitly release the
        // drain guard now so the (unrelated) persist / tombstone-clear steps
        // below run with the gate already clear. RAII (`drain_guard`'s `Drop`
        // clears the flag) remains the safety net for every early `?` return
        // above (including the `!existed` rollback branch), but an explicit
        // early drop here keeps the drain window as tight as possible.
        drop(drain_guard);

        // P0-3b (#972) test seam — park here (sweep complete, reduced defs
        // NOT yet persisted) if a test installed the post-sweep hook. This is
        // the exact crash window sub-bug 3c exercises: a "crash" here
        // (dropping the manager) leaves the tombstone on disk but the old
        // defs, and the recovery path in `new()` must finish the drop. NOT
        // `#[cfg(test)]`-gated — cross-crate test consumer.
        if let Some(hook) = self.drop_index_post_sweep_hook.load_full() {
            hook.wait_at_window().await;
        }

        // Persist the reduced defs (definition removed).
        // P1-2 (#967): the tombstone is still in place — if this persist
        // fails, recovery will see the tombstone and finish the drop.
        // NOTE: Cannot write DdlOpState::Failed here because this layer
        // (SortedIndexManager) does not have op_id in scope.
        self.persist_defs().await.map_err(|e| {
            shamir_storage::error::DbError::Internal(format!(
                "DROP SORTED INDEX '{name_interned}': a durable drop tombstone \
                 was persisted, the definition was retired, and the posting sweep \
                 completed, but persisting the reduced definitions failed: {e}. \
                 On restart, recovery will finish the drop idempotently. Call \
                 TableManager::verify() to confirm state."
            ))
        })?;

        // #1067: Write terminal Succeeded status BEFORE clearing the
        // tombstone. This is the exact same crash-safety write-order fix
        // #1069 applied to the base_index `IndexManager::drop_index`/
        // `drop_unique_index`: if we crash after this write but before the
        // tombstone clear, the status is durable and recovery will find it
        // (the tombstone clear is idempotent, so recovery re-clearing is
        // safe).
        if let (Some(op_id_str), Some(index_name_str)) = (op_id.as_deref(), index_name) {
            let op_id_parsed =
                <RecordId as std::str::FromStr>::from_str(op_id_str).map_err(|e| {
                    shamir_storage::error::DbError::Codec(format!("Invalid op_id: {e}"))
                })?;
            let status = shamir_query_types::read::DdlOpStatus {
                op_id: op_id_parsed,
                kind: shamir_query_types::read::DdlOpKind::DropSortedIndex {
                    index_name: index_name_str.to_string(),
                },
                state: shamir_query_types::read::DdlOpState::Succeeded {
                    completed_at: std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap()
                        .as_millis() as u64,
                },
            };
            // Do not swallow status-write errors — log loudly.
            if let Err(e) =
                crate::base_index::ddl_op_log::write_op_status(&self.info_store, &status).await
            {
                log::error!(
                    "#1067: DROP INDEX (sorted) '{}' succeeded, but failed to write \
                     Succeeded status: {}. The drop completed successfully, but polling by \
                     op_id {:?} will return Unknown. Call TableManager::verify() to confirm \
                     the index was dropped.",
                    index_name_str,
                    e,
                    op_id_parsed
                );
            }
        }

        // P0-3b (#972): clear the tombstone AFTER the reduced defs are
        // durably persisted. `clear_from_dropping_sorted` persists first,
        // then updates in-memory — see that method's doc for the ordering
        // rationale.
        // P1-2 (#967): if this fails, the tombstone remains — recovery
        // will just clear it (a no-op on the already-finished drop).
        // NOTE: Cannot write DdlOpState::Failed here because this layer
        // (SortedIndexManager) does not have op_id in scope.
        self.clear_from_dropping_sorted(name_interned)
            .await
            .map_err(|e| {
                shamir_storage::error::DbError::Internal(format!(
                    "DROP SORTED INDEX '{name_interned}': the drop is essentially \
                 complete (tombstone persisted, definition retired, sweep done, \
                 reduced definitions persisted), but clearing the drop tombstone \
                 failed: {e}. On restart, recovery will clear the tombstone as \
                 a no-op. Call TableManager::verify() to confirm state."
                ))
            })?;

        Ok(true)
    }

    /// Re-key an in-memory sorted-index definition from `old_id` to `new_id`
    /// and persist the updated metadata.
    ///
    /// This is the metadata half of RENAME INDEX for sorted indexes — the
    /// physical posting entries are re-keyed by [`rekey_postings`](Self::rekey_postings)
    /// (driven by [`rename_index_sorted`](Self::rename_index_sorted)). Here we
    /// only swap the in-memory entry and re-save the definitions blob.
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
                    // R0-B (#1008): bump the in-memory instance epoch on
                    // RENAME — the SAME semantic Part 1's `generation` bump
                    // establishes at manager granularity, applied here at
                    // per-definition granularity. This is what lets
                    // `pre_commit.rs`'s reconcile distinguish "a staged op
                    // planned against this definition BEFORE the rename"
                    // (stale — the op still carries the OLD name_interned
                    // AND the OLD epoch) from "a freshly re-derived op
                    // planned against the renamed definition" (carries the
                    // NEW name_interned and the NEW epoch). See
                    // `SortedIndexDefinition::instance_epoch`'s doc.
                    new_vec[pos].instance_epoch =
                        crate::base_index::sorted_index_definition::next_instance_epoch();
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
        // F-50 Step 2 (#870, Part D) / #1007: a RENAME changes the queryable
        // def set's identity (a tx staged against the OLD name must be
        // treated as stale after this point — see the class doc above), so
        // it must advance `generation` exactly like `register`/`drop_index`
        // already do. Before this fix, `rename_definition` performed the RCU
        // swap + epoch-carry + persist but never bumped `generation`, so
        // `pre_commit.rs`'s sorted rederive gate (`sorted_mgr.generation() ==
        // stage_gen`) never fired for a tx that staged before a rename —
        // the exact NP-2 gap #1007 closes. Placed AFTER the RCU swap
        // succeeds, mirroring `register`/`drop_index`'s existing call-site
        // placement (immediately after their own mutating RCU/removal).
        self.generation.fetch_add(1, Ordering::AcqRel);
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

    // ========================================================================
    // P0-5b (#962) — RENAME INDEX durable tombstone + crash recovery (sorted)
    // ========================================================================
    //
    // Mirrors #972's DROP-INDEX durable-tombstone pattern, adapted for rename.
    //
    // The REAL gap this closes (re-read against CURRENT source): the per-batch
    // atomicity of the physical rekey is ALREADY handled — `rekey_postings`
    // (below) is a settle re-scan loop that applies each batch as one
    // `Store::transact` call, the accepted non-atomic-backend tolerance pattern
    // (F-85/#913; a partial-batch visibility is fine because the NEXT loop
    // iteration picks up whatever wasn't yet moved). What was NOT handled is
    // RESUMABILITY: the engine's `rename_index` previously swapped the
    // definition `old_id → new_id` FIRST (`rename_definition`) and THEN ran the
    // rekey. If the rekey returned `Err` (a genuine backend failure), the error
    // propagated to the client while the catalog already believed the rename
    // had succeeded — a client RETRY of the same command then failed
    // immediately (the source name no longer resolved), leaving postings
    // permanently orphaned under the `SORTED_TAG || old_id` prefix (silent
    // storage leak + ghost-posting collision if `old_id` is ever reused). The
    // durable "Renaming" tombstone (written BEFORE `rename_definition`, cleared
    // AFTER the rekey completes) makes the interrupted rekey RESUMABLE on
    // restart: `recover_in_progress_renames` re-runs the (idempotent) settle
    // loop for each recorded `(old_id, new_id)` pair.
    //
    // Tombstone shape: `Vec<(u64, u64)>` (the `old_id → new_id` pairs),
    // serialized via bincode under `system:sidx_ren`. A separate key was chosen
    // (mirroring #972's `system:sidx_drop`) to avoid touching `persist_defs`/
    // `load`'s three-tier bincode forward-compat fallback chain. An absent key
    // or empty vec means "no in-progress renames".
    //
    // **CRITICAL** (the hard-won #959 collision lesson): `RecordId::system(name)`
    // truncates `name` to 12 bytes (see `shamir_types::types::record_id::system`).
    // `"sidx_ren"` (8 bytes) was verified collision-free against every other
    // sorted system key: it differs from `"sorted_indexes"` (truncates to
    // `"sorted_index"`) at byte index 1 (`i` vs `o`), and from `"sidx_drop"` at
    // byte index 5 (`r` vs `d`) — NO collision.

    /// P0-5b (#962): persist the current in-memory `renaming_sorted` map to
    /// `info_store` under `system:sidx_ren`. Serializes the map as a
    /// `Vec<(u64, u64)>` (deterministic order via `BTreeMap`). An empty map
    /// writes an empty `Vec` (NOT a key deletion) so the load path handles
    /// both `NotFound` and empty-vec uniformly. Mirrors #972's
    /// `save_dropping_sorted`.
    pub(super) async fn save_renaming_sorted(&self) -> DbResult<()> {
        let snapshot: Vec<(u64, u64)> = {
            let map = self.renaming_sorted.lock().unwrap();
            map.iter().map(|(&o, &n)| (o, n)).collect()
        };
        let key = RecordId::system("sidx_ren").to_bytes();
        let bytes = bincode::serialize(&snapshot)
            .map_err(|e| shamir_storage::error::DbError::Codec(e.to_string()))?;
        self.info_store.set(key.into(), Bytes::from(bytes)).await?;
        Ok(())
    }

    /// P0-5b (#962): load a persisted renaming map from `info_store`. Returns
    /// an empty `Vec` if the key is absent (`NotFound`) or contains an empty
    /// vec — both mean "no in-progress renames". Mirrors #972's
    /// `load_dropping_sorted`.
    pub(super) async fn load_renaming_sorted(&self) -> DbResult<Vec<(u64, u64)>> {
        let key = RecordId::system("sidx_ren").to_bytes();
        match self.info_store.get(key.into()).await {
            Ok(bytes) => {
                if bytes.is_empty() {
                    return Ok(Vec::new());
                }
                bincode::deserialize::<Vec<(u64, u64)>>(&bytes).map_err(|e| {
                    shamir_storage::error::DbError::Codec(format!(
                        "system:sidx_ren decode failed: {e}"
                    ))
                })
            }
            Err(shamir_storage::error::DbError::NotFound(_)) => Ok(Vec::new()),
            Err(e) => Err(e),
        }
    }

    /// P0-5b (#962): record `(old_id → new_id)` in the in-memory renaming map,
    /// then persist the updated map durably. MUST be called BEFORE
    /// `rename_definition` so a crash at any subsequent point is recoverable.
    ///
    /// In-memory update happens FIRST (so a concurrent observer immediately
    /// sees the pair as in-flight), then the persist follows. If the persist
    /// fails, the in-memory update is rolled back and the caller receives
    /// `Err` — the rename MUST NOT proceed without a durable tombstone.
    /// Mirrors #972's `add_to_dropping_sorted`.
    pub(super) async fn add_to_renaming_sorted(&self, old_id: u64, new_id: u64) -> DbResult<()> {
        {
            let mut map = self.renaming_sorted.lock().unwrap();
            map.insert(old_id, new_id);
        }
        let result = self.save_renaming_sorted().await;
        if result.is_err() {
            // Roll back the in-memory map so the state stays consistent.
            let mut map = self.renaming_sorted.lock().unwrap();
            map.remove(&old_id);
        }
        result
    }

    /// P0-5b (#962): clear `old_id` from the persisted tombstone, then from
    /// the in-memory map. Persist-first ordering ensures the on-disk state is
    /// always at least as advanced as the in-memory state: a crash between
    /// persist and in-memory update leaves a stale in-memory entry that dies
    /// with the process, while the on-disk tombstone is already correct.
    ///
    /// MUST be called AFTER the rekey completes (the moved postings must be
    /// durable under `new_id` first). If the process crashes between the rekey
    /// and this clear, recovery sees the tombstone and re-runs the (idempotent,
    /// no-op-if-finished) rekey, then clears the tombstone. Mirrors #972's
    /// `clear_from_dropping_sorted`.
    pub(super) async fn clear_from_renaming_sorted(&self, old_id: u64) -> DbResult<()> {
        // Compute the snapshot without the entry (do NOT modify in-memory yet).
        let snapshot: Vec<(u64, u64)> = {
            let map = self.renaming_sorted.lock().unwrap();
            map.iter()
                .filter(|(&o, _)| o != old_id)
                .map(|(&o, &n)| (o, n))
                .collect()
        };
        // Persist first.
        let key = RecordId::system("sidx_ren").to_bytes();
        let bytes = bincode::serialize(&snapshot)
            .map_err(|e| shamir_storage::error::DbError::Codec(e.to_string()))?;
        self.info_store.set(key.into(), Bytes::from(bytes)).await?;
        // Only now update in-memory.
        {
            let mut map = self.renaming_sorted.lock().unwrap();
            map.remove(&old_id);
        }
        Ok(())
    }

    /// P0-5b (#962): rekey all posting entries of a sorted index from
    /// `old_id` to `new_id`. MOVED here from the engine crate's free function
    /// `rekey_sorted_prefix` so the open-time recovery path can re-run it
    /// idempotently (it now lives next to its `sweep_sorted_postings` sibling).
    ///
    /// Sorted-index physical key layout:
    ///   `[SORTED_TAG: 1][name_interned BE: 8][encoded_value: var][record_id: 16]`
    /// For each key found under the old prefix, bytes [1..9] are replaced with
    /// `new_id.to_be_bytes()` (big-endian).
    ///
    /// # Concurrency / atomicity
    ///
    /// Settle re-scan loop: after each pass applies one `Store::transact` batch
    /// (one mixed-op atomic unit — a crash/failure between a `set_many` +
    /// `remove_many` pair was the pre-#616 bug), we re-scan the old-id prefix
    /// and loop until a pass finds nothing to move. This is the accepted,
    /// F-85/#913-documented way production callers tolerate
    /// `supports_atomic_transact() == false`: a non-atomic backend's
    /// partial-batch visibility is fine because the NEXT iteration picks up
    /// whatever wasn't yet moved. A concurrent writer that lands an entry
    /// under `old_id` mid-sweep (only possible in the brief window before the
    /// `rename_definition` RCU swap publishes `new_id` to the write-hook) is
    /// caught by the next settle pass. Idempotent: calling it again after the
    /// rekey finished just finds nothing to move and returns `Ok(())`.
    pub(super) async fn rekey_postings(&self, old_id: u64, new_id: u64) -> DbResult<()> {
        let old_prefix = self.entry_prefix(old_id);
        let new_id_bytes = new_id.to_be_bytes();

        // Loop until a full scan of the old-id prefix finds no entries to move.
        loop {
            let stream = self
                .info_store
                .scan_prefix_stream(old_prefix.clone(), FULL_SCAN_BATCH);
            futures::pin_mut!(stream);

            // One mixed-op batch per pass — `Store::transact` commits the whole
            // batch as one atomic unit (the pre-#616 two-call `set_many` +
            // `remove_many` shape was NOT atomic).
            let mut ops: Vec<KvOp> = Vec::new();
            let mut moved_any = false;

            while let Some(batch) = stream.next().await {
                for (key, value) in batch? {
                    if key.len() < 9 {
                        continue; // malformed; skip
                    }
                    let mut new_key = key.to_vec();
                    new_key[1..9].copy_from_slice(&new_id_bytes);
                    ops.push(KvOp::Set(Bytes::from(new_key).into(), value));
                    ops.push(KvOp::Remove(key));
                    moved_any = true;
                }
            }

            if !moved_any {
                // No old-id entries found this pass — settle complete.
                break;
            }

            self.info_store.transact(ops).await?;
        }

        Ok(())
    }

    /// P0-5b (#962): open-time recovery for RENAME INDEX operations
    /// interrupted by a crash. Called from `SortedIndexManager::new` AFTER
    /// definitions are loaded and AFTER `recover_in_progress_drops` runs (a
    /// drop and a rename for the same name cannot both be in flight — the
    /// engine's write barrier serializes DDL).
    ///
    /// # Crash-state matrix
    ///
    /// | crash point                                  | def under? | tombstone? | recovery action                         |
    /// |----------------------------------------------|------------|------------|-----------------------------------------|
    /// | after tombstone, before rename_definition    | old_id     | yes        | rename def, rekey (moves postings)      |
    /// | after rename_definition, before/during rekey | new_id     | yes        | rekey (finishes moving remainder)       |
    /// | after rekey, before tombstone clear          | new_id     | yes        | rekey (no-op), clear tombstone          |
    ///
    /// In every case recovery leaves the manager consistent: the definition is
    /// under `new_id`, ALL postings are under `new_id`, and the tombstone is
    /// cleared. The rekey is idempotent, so calling recovery twice (two
    /// restart attempts) is a clean no-op on the second call.
    pub(super) async fn recover_in_progress_renames(&self) -> DbResult<()> {
        let renaming = self.load_renaming_sorted().await?;
        if renaming.is_empty() {
            return Ok(());
        }

        log::info!(
            "P0-5b (#962): recovering {} in-progress sorted RENAME(s)",
            renaming.len()
        );

        for &(old_id, new_id) in &renaming {
            // If the definition is still under old_id, the crash happened
            // before rename_definition's persist finalized — finish the def
            // rename now. If it is already under new_id (or absent under
            // old_id), `find_by_name_interned(old_id).is_none()` short-
            // circuits and we skip straight to the (idempotent) rekey.
            if self.find_by_name_interned(old_id).is_some() {
                // rename_definition carries the in-memory mutation epoch and
                // persists; if the def raced away between the check and the
                // call (only under a violated write-barrier contract), treat
                // its Err as non-fatal and let the rekey run regardless.
                let _ = self.rename_definition(old_id, new_id).await;
            }
            // Always run the rekey (idempotent). Covers both the "rekey never
            // ran" and "rekey ran partially" cases.
            self.rekey_postings(old_id, new_id).await?;
        }

        // Clear the tombstone (write empty Vec; the load path treats empty-vec
        // and NotFound identically — cheaper than a remove).
        let empty = bincode::serialize(&Vec::<(u64, u64)>::new())
            .map_err(|e| shamir_storage::error::DbError::Codec(e.to_string()))?;
        let key = RecordId::system("sidx_ren").to_bytes();
        self.info_store.set(key.into(), Bytes::from(empty)).await?;

        log::info!(
            "P0-5b (#962): recovery complete — {} sorted RENAME(s) finalized",
            renaming.len()
        );

        Ok(())
    }

    /// P0-5b (#962): the FULL sorted-RENAME orchestration — definition swap +
    /// physical posting rekey, wrapped in a durable "Renaming" tombstone so an
    /// interrupted rekey is resumable on restart. The engine's `rename_index`
    /// delegates its sorted branch to this method (mirroring how
    /// `drop_sorted_index` delegates to `drop_index`).
    ///
    /// Sequence:
    /// 1. Write a durable "Renaming" tombstone `(old_id → new_id)` — MUST
    ///    succeed before anything mutates, so a crash at any later point is
    ///    recoverable by `recover_in_progress_renames`.
    /// 2. `rename_definition(old_id, new_id)` — swap the in-memory definition
    ///    (RCU) + carry the mutations epoch + persist.
    /// 3. `rekey_postings(old_id, new_id)` — settle-loop move of the physical
    ///    postings to the new prefix.
    /// 4. Clear the tombstone (persist-first).
    ///
    /// # Error / resume semantics
    ///
    /// If step 2 or 3 returns `Err`, the tombstone is LEFT IN PLACE and the
    /// error propagates to the caller. The catalog may already believe the
    /// rename succeeded (step 2 swapped + persisted), so a client RETRY of the
    /// same command is expected to fail — but a restart runs
    /// `recover_in_progress_renames`, which finishes the rekey (the whole
    /// point of the tombstone). This matches #972's model: a crashed DDL op is
    /// finished by restart-recovery, not by re-issuing the command.
    ///
    /// #1067: accepts `op_id`/`old_name`/`new_name` (resolved by the caller,
    /// which already has the string names in scope — this layer only ever
    /// dealt in interned ids before) so the terminal `Succeeded` status can
    /// be written AFTER step 3 (`rekey_postings`) but BEFORE step 4 (the
    /// tombstone clear) — the same before-the-clear write-order discipline
    /// as `drop_index` above and the base_index `IndexManager` family.
    pub async fn rename_index_sorted(
        &self,
        old_id: u64,
        new_id: u64,
        op_id: Option<String>,
        old_name: Option<&str>,
        new_name: Option<&str>,
    ) -> DbResult<()> {
        // 1. Durable tombstone BEFORE the definition swap.
        self.add_to_renaming_sorted(old_id, new_id).await?;

        // 2. Swap + persist the definition (carries the mutations epoch too).
        // If this fails AFTER the tombstone was written, the tombstone is left
        // in place — recovery will reconcile on restart (it checks which id the
        // definition is actually under before deciding to rename vs. just rekey).
        // P1-2 (#967): enrich the error with partial-state context.
        // NOTE: Cannot write DdlOpState::Failed here because this layer
        // (SortedIndexManager) does not have op_id in scope.
        self.rename_definition(old_id, new_id).await.map_err(|e| {
            shamir_storage::error::DbError::Internal(format!(
                "RENAME SORTED INDEX ({old_id} → {new_id}): a durable rename \
                 tombstone was persisted, but the definition swap + persist \
                 failed: {e}. The tombstone is left in place — on restart, \
                 recovery will reconcile the rename. Call TableManager::verify() \
                 to confirm state."
            ))
        })?;

        // P0-5b (#962) test seam — park here (definition already swapped +
        // persisted under new_id, postings NOT yet rekeyed) if a test
        // installed the pause hook. This is the EXACT crash window the bug
        // describes. NOT `#[cfg(test)]`-gated — cross-crate test consumer.
        if let Some(hook) = self.rename_rekey_pause_hook.load_full() {
            hook.wait_at_window().await;
        }

        // 3. Rekey the physical postings (settle loop).
        // P1-2 (#967): enrich — the definition swap already succeeded, so
        // the rename is partially done (definition under new_id, postings
        // still under old_id).
        // NOTE: Cannot write DdlOpState::Failed here because this layer
        // (SortedIndexManager) does not have op_id in scope.
        self.rekey_postings(old_id, new_id).await.map_err(|e| {
            shamir_storage::error::DbError::Internal(format!(
                "RENAME SORTED INDEX ({old_id} → {new_id}): the definition was \
                 swapped and persisted under {new_id}, but rekeying the physical \
                 postings from {old_id} failed: {e}. The tombstone is left in \
                 place — on restart, recovery will finish the rekey. Call \
                 TableManager::verify() to confirm state."
            ))
        })?;

        // #1067: Write terminal Succeeded status AFTER the rekey but BEFORE
        // clearing the tombstone — same before-the-clear write-order
        // discipline as `drop_index` above and the base_index `IndexManager`
        // family's #1069 fix. `op_id`/`old_name`/`new_name` are now
        // parameters (previously this layer had no op_id in scope at all).
        if let (Some(op_id_str), Some(old_name_str), Some(new_name_str)) =
            (op_id.as_deref(), old_name, new_name)
        {
            let op_id_parsed =
                <RecordId as std::str::FromStr>::from_str(op_id_str).map_err(|e| {
                    shamir_storage::error::DbError::Codec(format!("Invalid op_id: {e}"))
                })?;
            let status = shamir_query_types::read::DdlOpStatus {
                op_id: op_id_parsed,
                kind: shamir_query_types::read::DdlOpKind::RenameSortedIndex {
                    old_name: old_name_str.to_string(),
                    new_name: new_name_str.to_string(),
                },
                state: shamir_query_types::read::DdlOpState::Succeeded {
                    completed_at: std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap()
                        .as_millis() as u64,
                },
            };
            // Do not swallow status-write errors — log loudly.
            if let Err(e) =
                crate::base_index::ddl_op_log::write_op_status(&self.info_store, &status).await
            {
                log::error!(
                    "#1067: RENAME INDEX (sorted) '{}' -> '{}' succeeded, but failed to \
                     write Succeeded status: {}. The rename completed successfully, but \
                     polling by op_id {:?} will return Unknown. Call \
                     TableManager::verify() to confirm the index was renamed.",
                    old_name_str,
                    new_name_str,
                    e,
                    op_id_parsed
                );
            }
        }

        // 4. Clear the tombstone now that the rekey is durable.
        // P1-2 (#967): if this fails, the tombstone remains — recovery
        // will just clear it (the rename is fully done).
        // NOTE: Cannot write DdlOpState::Failed here because this layer
        // (SortedIndexManager) does not have op_id in scope for a FAILURE —
        // by the time op_id/old_name/new_name are available (as of #1067),
        // every fallible step above already returned via `?` before this
        // point, so there is no reachable failure site left in this
        // function to attach a Failed status to.
        self.clear_from_renaming_sorted(old_id).await.map_err(|e| {
            shamir_storage::error::DbError::Internal(format!(
                "RENAME SORTED INDEX ({old_id} → {new_id}): the rename is fully \
                 complete (definition swapped, postings rekeyed), but clearing \
                 the rename tombstone failed: {e}. On restart, recovery will \
                 clear the tombstone as a no-op. Call TableManager::verify() \
                 to confirm state."
            ))
        })?;

        Ok(())
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
                ops.push(IndexWriteOp::SetPosting {
                    key,
                    value,
                    provenance: def.provenance(),
                });
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
            let provenance = def.provenance();
            for (rid, value) in items.clone() {
                if let Some(encoded) = extract_and_encode(value, &def.field_path)? {
                    let key = self.build_entry_key(def.name_interned, &encoded, rid);
                    let pv = if def.is_covering() {
                        build_covering_projection(value, def, version)
                    } else {
                        Bytes::new()
                    };
                    ops.push(IndexWriteOp::SetPosting {
                        key,
                        value: pv,
                        provenance,
                    });
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
            let provenance = def.provenance();
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
                    ops.push(IndexWriteOp::RemovePosting { key, provenance });
                }
                if let Some(ref nv) = new_enc {
                    let key = self.build_entry_key(def.name_interned, nv, record_id);
                    let value = new_proj.clone().unwrap_or(Bytes::new());
                    ops.push(IndexWriteOp::SetPosting {
                        key,
                        value,
                        provenance,
                    });
                }
            } else {
                // Key is the same but projection changed — overwrite in place.
                if let Some(ref nv) = new_enc {
                    let key = self.build_entry_key(def.name_interned, nv, record_id);
                    let value = new_proj.clone().unwrap_or(Bytes::new());
                    ops.push(IndexWriteOp::SetPosting {
                        key,
                        value,
                        provenance,
                    });
                }
            }
        }
        Ok(ops)
    }

    /// Plan index entry removals for a deleted record.
    ///
    /// F-67 (#893): a PURE planner — no epoch bump (see
    /// [`plan_record_created`]'s doc for why: this is also called at stage
    /// time, e.g. `table_manager_tx_ops.rs::plan_base_index_delete_ops`).
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
                ops.push(IndexWriteOp::RemovePosting {
                    key,
                    provenance: def.provenance(),
                });
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
                IndexWriteOp::SetPosting { key, value, .. } => {
                    self.info_store
                        .set(key.clone().into(), value.clone())
                        .await?;
                }
                IndexWriteOp::RemovePosting { key, .. } => {
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
                IndexWriteOp::RemovePosting { key, .. } => key,
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
        // P0-3a (#1037): acquire the reader-drain guard as the FIRST statement.
        // `None` means a DROP INDEX is currently in its raise→sweep window: the
        // caller MUST NOT read the physical store and MUST fall back (a full
        // table scan). See `crate::reader_drain_gate`'s module doc for the
        // placement invariant: this guard is always the INNERMOST
        // synchronization primitive on the read path.
        let Some(_guard) = self.reader_gate.enter() else {
            // We don't have the human-readable name available (only name_interned),
            // so use the numeric ID in the error message.
            let index_name = format!("sorted_index_{}", name_interned);
            return Err(DbError::IndexDrainInProgress(index_name));
        };

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
        // P0-3a (#1037): acquire the reader-drain guard as the FIRST statement.
        let Some(_guard) = self.reader_gate.enter() else {
            // We don't have the human-readable name available (only name_interned),
            // so use the numeric ID in the error message.
            let index_name = format!("sorted_index_{}", name_interned);
            return Err(DbError::IndexDrainInProgress(index_name));
        };

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
        // P0-3a (#1037): acquire the reader-drain guard as the FIRST statement.
        let Some(_guard) = self.reader_gate.enter() else {
            // We don't have the human-readable name available (only name_interned),
            // so use the numeric ID in the error message.
            let index_name = format!("sorted_index_{}", name_interned);
            return Err(DbError::IndexDrainInProgress(index_name));
        };

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
        // P0-3a (#1037): acquire the reader-drain guard as the FIRST statement.
        let Some(_guard) = self.reader_gate.enter() else {
            // We don't have the human-readable name available (only name_interned),
            // so use the numeric ID in the error message.
            let index_name = format!("sorted_index_{}", name_interned);
            return Err(DbError::IndexDrainInProgress(index_name));
        };

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
        // P0-3a (#1037): acquire the reader-drain guard as the FIRST statement.
        let Some(_guard) = self.reader_gate.enter() else {
            // We don't have the human-readable name available (only name_interned),
            // so use the numeric ID in the error message.
            let index_name = format!("sorted_index_{}", name_interned);
            return Err(DbError::IndexDrainInProgress(index_name));
        };

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
        // P0-3a (#1037): acquire the reader-drain guard as the FIRST statement.
        let Some(_guard) = self.reader_gate.enter() else {
            // We don't have the human-readable name available (only name_interned),
            // so use the numeric ID in the error message.
            let index_name = format!("sorted_index_{}", name_interned);
            return Err(DbError::IndexDrainInProgress(index_name));
        };

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
        // P0-3a (#1037): acquire the reader-drain guard as the FIRST statement.
        let Some(_guard) = self.reader_gate.enter() else {
            // We don't have the human-readable name available (only name_interned),
            // so use the numeric ID in the error message.
            let index_name = format!("sorted_index_{}", name_interned);
            return Err(DbError::IndexDrainInProgress(index_name));
        };

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
        // P0-3a (#1037): acquire the reader-drain guard as the FIRST statement.
        // `None` means a DROP INDEX is currently in its raise→sweep window:
        // the caller must report an error (fall back to full scan or similar).
        let Some(_guard) = self.reader_gate.enter() else {
            // We don't have the human-readable name available (only name_interned),
            // so use the numeric ID in the error message.
            let index_name = format!("sorted_index_{}", name_interned);
            return Err(DbError::IndexDrainInProgress(index_name));
        };

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
/// SAME `sort_codec::encode_*` primitives the base_index `&InnerValue` path
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
