use crate::repo::RepoWriteDomain;

use std::hash::{Hash, Hasher};
use std::sync::Arc;

use super::interner_manager::InternerManager;
use super::persistable::PersistRegistry;
use super::record_counter::RecordCounter;
use super::table::Table;
use crate::index::index_manager::IndexManager;
use crate::index::sorted_index_manager::SortedIndexManager;
use shamir_storage::error::{DbError, DbResult};
use shamir_storage::types::Store;

// S5c-X — `#[path]`-wired impl-block files. Declared HERE (not in
// `table/mod.rs`) so they become CHILD modules of `table_manager` and can
// see its private/`pub(super)` fields — that is what makes the
// line-budget-driven move of the admitted twins out of this file legal
// without widening visibility. Same pattern as `repo/changelog_store.rs`.
#[path = "table_manager/admission/domain/barrier/admitted_ops.rs"]
mod admitted_ops;
#[path = "table_manager/admission/views/read_only_views.rs"]
mod read_only_views;
#[path = "table_manager/admission/domain/barrier/write_domain_barrier.rs"]
mod write_domain_barrier;
#[path = "table_manager/admission/domain/index_admission/write_domain_index_admitted.rs"]
mod write_domain_index_admitted;
#[path = "table_manager/admission/domain/index_admission/write_domain_index_core.rs"]
mod write_domain_index_core;
#[path = "table_manager/admission/domain/index_admission/write_domain_online_index.rs"]
mod write_domain_online_index;
#[path = "table_manager/admission/domain/barrier/write_domain_rename_admitted.rs"]
mod write_domain_rename_admitted;

// Engine sweep T1, chunk W4: the `table_manager_*` implementation files, the
// tx-staging split of the former `table_manager_tx_ops.rs`, the diagnostics
// files (`doctor.rs` / `degraded_index_count.rs`) and the guard/tokenizer
// tail split out of this file now live under `table_manager/`. They are
// mounted HERE so they stay CHILD modules of `table_manager` and keep
// private/`pub(super)` field access — same pattern as the admission mounts
// above.
#[path = "table_manager/accessors/mod.rs"]
pub(in crate::table) mod accessors;
#[path = "table_manager/barriers/mod.rs"]
mod barriers;
#[path = "table_manager/diagnostics/mod.rs"]
pub mod diagnostics;
#[path = "table_manager/operations/mod.rs"]
pub(in crate::table) mod operations;
#[path = "table_manager/transactions/mod.rs"]
pub(in crate::table) mod transactions;

// Kept name re-export: `WriteBarrierGuard` used to be defined in this file;
// it now lives in `barriers/guard.rs`.
pub use barriers::guard::WriteBarrierGuard;

// Kept path re-export (W4 split): `fts_tokenizer_from_dsl` used to be defined
// in this file; tests and the index-admission impl reach it via this path.
pub(crate) use accessors::tokenizer::fts_tokenizer_from_dsl;

pub use read_only_views::{
    Index2ReadView, InternerReadView, TableDataStoreReadView, TableInfoStoreReadView,
    ValidatorRegistryReadView,
};

/// Compute the deterministic token for a table name.
/// Same hash as `TableManager::table_token` (instance method).
pub fn table_token_for(name: &str) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    name.hash(&mut h);
    h.finish()
}

/// S5c-X P2 (Round 6, split Round 7): these compile-fail doctests are the
/// ENFORCED boundary gate. Doctests are enabled on this crate solely so they
/// run in CI (see `Cargo.toml` `[lib]` and the `cargo test --doc` workflow
/// step — nextest cannot run doctests). They must KEEP failing to compile
/// downstream: the raw component handles and the pre-F2 registry escape are
/// unreachable. One block per accessor, so reopening any single accessor
/// independently fails the gate.
///
/// ```compile_fail
/// use shamir_engine::table::TableManager;
///
/// fn downstream(manager: &TableManager) {
///     let _ = manager.info_store();
/// }
/// ```
///
/// ```compile_fail
/// use shamir_engine::table::TableManager;
///
/// fn downstream(manager: &TableManager) {
///     let _ = manager.data_store();
/// }
/// ```
///
/// ```compile_fail
/// use shamir_engine::table::TableManager;
///
/// fn downstream(manager: &TableManager) {
///     let _ = manager.interner();
/// }
/// ```
///
/// ```compile_fail
/// use shamir_engine::table::TableManager;
///
/// fn downstream(manager: &TableManager) {
///     let _ = manager.validator_registry();
/// }
/// ```
///
/// Round 8: the context capability routes. `FilterContext.interner` is
/// `pub(crate)` — downstream holders of the read-view `filter_context`
/// cannot reach the interior-mutable dictionary through it:
///
/// ```compile_fail
/// use shamir_engine::query::filter::FilterContext;
///
/// fn downstream(ctx: &FilterContext) {
///     let _ = ctx.interner.touch_ind("x");
/// }
/// ```
///
/// And `InternerReadView` exposes no mutators at all (its cell is private;
/// only `get_ind`/`get_str`/`generation`/decode accessors exist):
///
/// ```compile_fail
/// use shamir_engine::table::InternerReadView;
///
/// fn downstream(view: &InternerReadView) {
///     let _ = view.touch_ind("x");
/// }
/// ```
pub struct TableManager {
    pub(super) name: String,
    pub(super) table: Arc<Table>,
    /// Direct handle to the info_store the sub-managers were
    /// built on. Kept so DDL (buffer-config get/set, future
    /// per-table settings) can hit the same store without going
    /// through any sub-manager's surface.
    pub(super) info_store: Arc<dyn Store>,
    pub(super) interner: InternerManager,
    pub(super) counter: Arc<RecordCounter>,
    /// Registry of metadata blobs that are flushed together at the end
    /// of each write operation via `flush_metadata()`.
    pub(super) persist_registry: PersistRegistry,
    pub(super) index_manager: IndexManager,
    pub(super) sorted_indexes: SortedIndexManager,
    /// Monotonic counter of mutating operations since open. The
    /// auto-verify background watchdog samples this; every
    /// `AUTO_VERIFY_EVERY_N_WRITES` operations it spawns a verify
    /// pass and logs anything unhealthy. See `bump_write_counter`.
    pub(super) write_counter: Arc<std::sync::atomic::AtomicU64>,
    /// `true` when a background verify is in flight — prevents
    /// multiple concurrent verifies piling up.
    pub(super) verify_running: Arc<std::sync::atomic::AtomicBool>,
    /// Serialises validate + write + index-update for tables that have
    /// unique indexes. Tables without unique indexes hit the fast path
    /// (no lock). `tokio::sync::Mutex` because the guard lives across
    /// `.await` points.
    pub(super) unique_write_lock: Arc<tokio::sync::Mutex<()>>,
    /// F-69 (#896, P0): the SINGLE packed atomic backing the whole
    /// write-barrier predicate — see
    /// [`write_barrier_flags`](crate::index::write_barrier_flags) for the
    /// full rationale (why one `Arc<AtomicU16>` replaces what used to be seven
    /// independent atomics: two from `IndexManager` plus five DDL-intent flags here).
    ///
    /// This is the SAME `Arc<AtomicU16>` [`IndexManager`] uses for its
    /// `UNIQUE_INDEX_EXISTS` and `REGULAR_INDEX_EXISTS` bits — obtained via
    /// [`IndexManager::write_barrier_flags`] at construction time and
    /// stored here so `TableManager` can fold its own five DDL-intent bits
    /// into the identical word:
    ///
    /// - `INDEX2_CREATE` — `true` (under `unique_write_lock`) for the
    ///   duration of an index2 (`fts`/`functional`/`vector`)
    ///   `create_index_v2` backfill→register sequence. While set, EVERY
    ///   writer path also acquires `unique_write_lock`, so no row can be
    ///   written in the backfill→register window where it would be seen by
    ///   NEITHER the backfill snapshot (cursor already past it) NOR the live
    ///   `index2_on_insert` hook (backend not yet registered) — the
    ///   lost-write race (#534, finding 1) that `unique_write_lock` alone
    ///   does not close because writers only took that lock when a base_index
    ///   unique index existed, and an index2-only table has none.
    /// - `SCHEMA_ACTIVATION` (F-37, #845) — raised for the duration of a
    ///   schema-activation DDL sequence (`set_table_schema` /
    ///   `add_schema_rule`) whose `keyset_safe` proof reads
    ///   `table.count() == 0` and then persists + activates a new schema
    ///   rule. While set, EVERY writer path also acquires
    ///   `unique_write_lock`, so no row can land between the count proof and
    ///   the schema's activation — the F-37 race that `lock_schema_rmw` does
    ///   NOT close (that lock only serializes schema DDL against OTHER
    ///   schema DDL in `admin_user_locks`, never against the write path in
    ///   `table_manager_crud.rs`). Independently settable/clearable from
    ///   `INDEX2_CREATE`. Set/cleared by the shamir-db DDL handler via
    ///   [`set_schema_activation_barrier`](Self::set_schema_activation_barrier).
    /// - `REGULAR_INDEX_CREATE` (F-57, #883) — raised for the duration of a
    ///   regular (non-unique) hash-index `create_index`
    ///   snapshot→backfill→register sequence, closing the lost-write race
    ///   where a row written between the backfill snapshot and registration
    ///   is seen by NEITHER the backfill NOR the live `index_manager` write
    ///   hook (`needs_write_barrier()` previously never considered regular
    ///   indexes at all). Set/cleared by [`WriteBarrierGuard`] (F-70, #897).
    /// - `UNIQUE_INDEX_CREATE` (F-57, #883) — raised for the duration of a
    ///   unique-index `create_unique_index` snapshot→backfill→register
    ///   sequence. The `UNIQUE_INDEX_EXISTS` bit already forces the barrier
    ///   once a unique index is registered, but for the FIRST unique index on
    ///   a table that bit is unset until the index IS registered — so every
    ///   concurrent fast-path writer would otherwise bypass the lock
    ///   entirely. Set/cleared by [`WriteBarrierGuard`] (F-70, #897).
    /// - `SORTED_INDEX_CREATE` (F-57, #883) — raised for the duration of a
    ///   sorted-index `create_sorted_index_with_include` register→backfill
    ///   sequence. Sorted indexes register BEFORE backfill (load-bearing: the
    ///   backfill calls `on_record_created` through the manager, which needs
    ///   the registered definition). Without this bit, a concurrent writer
    ///   could interleave with the backfill and the row could be missed or
    ///   double-posted. Set/cleared by [`WriteBarrierGuard`] (F-70, #897).
    ///
    /// Every bit-set/clear is `SeqCst` (F-56's total-order argument, now
    /// covering the full barrier predicate — see
    /// `writer_drain_barrier`'s module doc, updated by this task).
    ///
    /// Note: `REGULAR_INDEX_EXISTS` (owned by `IndexManager`) is excluded from
    /// the barrier predicate via `BARRIER_BITS` — a table with ONLY a regular
    /// index keeps the lock-free fast path (see #1102 round 2 brief for why).
    pub(super) write_barrier_flags: crate::index::write_barrier_flags::WriteBarrierFlags,
    /// F-95 (#957, P0) — per-table DDL admission mutex. Serializes
    /// concurrent DDL operations that would share the same write-barrier
    /// bit (e.g. two `CREATE INDEX` both raising `REGULAR_INDEX_CREATE`).
    /// Acquired FIRST inside
    /// [`begin_write_barrier`](Self::begin_write_barrier) (before the
    /// raise→drain→lock sequence) and held for the caller's ENTIRE
    /// critical section (the guard is embedded in the returned
    /// [`WriteBarrierGuard`]).
    ///
    /// Without this, two concurrent DDLs sharing a bit race: DDL-A drops
    /// its `WriteBarrierGuard` (unconditionally clearing the bit via
    /// `fetch_and`) while DDL-B has already drained and is parked on
    /// `unique_write_lock`. When B acquires the lock the bit is 0 and B
    /// never re-raises it, so a brand-new writer fast-paths past B's
    /// still-in-progress backfill — the exact hazard the barrier exists
    /// to prevent.
    ///
    /// The admission mutex ensures DDL-B does its OWN raise→drain→lock
    /// from scratch, AFTER DDL-A's entire critical section (including
    /// guard drop) has completed. Regular writers
    /// ([`needs_write_barrier`](Self::needs_write_barrier)) do NOT take
    /// this mutex — only `begin_write_barrier` callers do — so the F-70
    /// (#897) lock-order invariant (writers never block behind a
    /// DDL-held lock in a way that could deadlock) is preserved.
    ///
    /// Lock-order hierarchy (extending F-70's canonical order):
    /// `ddl_admission` → (raise bit) → `drain_writers` →
    /// `unique_write_lock`. No writer or committer ever acquires
    /// `ddl_admission`, so no cycle involving it can form.
    pub(super) ddl_admission: Arc<tokio::sync::Mutex<()>>,
    /// S5c-X — per-REPOSITORY write-admission domain, shared with every
    /// other table in the same repository and the repo's registry/counter
    /// flushers. DDL entry points acquire the repo permit FIRST (before
    /// `ddl_admission` and every per-table lock) via
    /// [`admit`](Self::admit); the permit rides inside the returned
    /// [`WriteBarrierGuard`] (or the caller's own binding for `_admitted`
    /// twins) for the whole critical section.
    pub(super) write_domain: Arc<RepoWriteDomain>,
    /// F-48 (#859, P0) — reusable writer-drain barrier. Fast-path writers
    /// (those that read `needs_write_barrier() == false`) bump this counter
    /// before their flag check and drop it after their full
    /// validate→write→index sequence. A drainer (schema DDL today; index2
    /// create in F-50) calls [`drain_writers`](Self::drain_writers) after
    /// raising its intent flag + acquiring `unique_write_lock` to wait for
    /// any in-flight fast-path writer that read `false` before the flag went
    /// up — the check-then-act gap `unique_write_lock` + the flag alone
    /// cannot close. Shared across clones via `Arc` (same rationale as the
    /// sibling barrier flags). See `writer_drain_barrier` for the full
    /// memory-model + reusability rationale.
    pub(super) writer_drain: super::writer_drain_barrier::WriterDrainBarrier,
    /// #1003 (follow-up to #984) — process-local set of index-name ids
    /// (interned) whose `CREATE INDEX` is currently in flight, across all
    /// four index families (regular, unique, sorted, index2).
    /// `degraded_index_count()` skips a non-`Ready` definition ONLY if its
    /// OWN name is in this set — a per-identity check, not a scalar
    /// subtraction (an earlier scalar-counter version of this fix could
    /// mask an unrelated genuinely-stuck index; see
    /// `in_flight_create_guard`'s module doc for the full writeup). A
    /// healthy, currently-in-progress backfill (which sets
    /// `IndexState::Building` BEFORE the backfill even starts, per
    /// F-72/#899) therefore does not read as "degraded" for its OWN index,
    /// while an index left `Building` by a crash in a PAST process (this
    /// set starts empty on every restart) still counts. Shared across
    /// clones via `Arc` (same rationale as the sibling barrier/counter
    /// fields).
    pub(super) in_flight_creates: super::in_flight_create_guard::InFlightCreateSet,
    pub(super) index2_registry: Arc<crate::index2::IndexRegistry>,
    /// Per-manager planner thresholds for cheap filtered-ANN tests. Set before
    /// reads; copied by Clone, with no global or thread-local async state.
    /// None uses the production constants, including in ordinary unit tests.
    #[cfg(test)]
    pub(super) filtered_ann_thresholds: Option<(usize, f64)>,
    pub(super) mvcc_store: Option<Arc<shamir_tx::MvccStore>>,
    /// Per-table validator bindings (S2). Lock-free reads via
    /// `ArcSwap`; the S3 write path reads this on every write.
    /// DDL (`add_validator_binding` / `remove_validator_binding`)
    /// mutates + persists to the info-twin.
    pub(super) validator_bindings: Arc<arc_swap::ArcSwap<Vec<crate::validator::ValidatorBinding>>>,
    /// DDL-only contention across clones; held across persist awaits, never on writes.
    pub(super) bindings_lock: Arc<tokio::sync::Mutex<()>>,
    /// Handle to the global validator registry (S3). `None` for system
    /// tables / tests that don't need validation. The S3 write path
    /// reads this to resolve `ValidatorBinding.validator_id` to a
    /// compiled `ShamirFunction`.
    pub(super) validator_registry: Option<Arc<crate::validator::ValidatorRegistry>>,
    /// SSI gate handle — wires the non-tx write path to the per-repo
    /// [`RepoTxGate`](shamir_tx::RepoTxGate) so that Serializable
    /// transactions see non-tx writes in their Phase 2-bis predicate-conflict
    /// check. `None` for system tables / tests that have no gate wired.
    /// Attached by `RepoInstance::create_table_context` via
    /// [`with_changefeed`](Self::with_changefeed).
    pub(super) changefeed: Option<NonTxChangefeed>,
    /// Per-DB scalar resolver (user + builtin layers). Lock-free reads
    /// via `ArcSwap`; defaults to builtins-only until
    /// [`set_scalar_resolver`](Self::set_scalar_resolver) is called.
    /// Used by `create_index_v2` for the `.trusted_pure()` index-safety gate.
    pub(super) scalar_resolver:
        Arc<arc_swap::ArcSwap<shamir_funclib::scalar_resolver::ScalarResolver>>,
    /// Test-only deterministic pause point for `create_index_v2`, installed
    /// between the index2 backfill and the `index2_registry.insert`. Lets a
    /// #534-finding-1 regression test drive a concurrent writer INTO the exact
    /// lost-write window (backfill done, backend not yet registered) so the
    /// test fails deterministically against the pre-fix (barrier-less) code and
    /// passes after. `None` in every non-test build and by default in tests —
    /// zero cost on the real create path.
    #[cfg(test)]
    pub(super) create_index2_backfill_hook:
        Arc<arc_swap::ArcSwapOption<super::index2_backfill_hook::BackfillPauseHook>>,
    /// F-72 (#899, P0) test-only deterministic pause point: parks
    /// `create_sorted_index_with_include`'s backfill loop mid-stream (after
    /// registering the definition at `Building`, before it flips to
    /// `Ready`). Lets a regression test drive a concurrent READ into the
    /// exact planner-invisibility window this task closes — see
    /// `f72_planner_invisibility_tests.rs`. Reuses the same
    /// `BackfillPauseHook` primitive as `create_index2_backfill_hook`.
    /// `None` in every non-test build and by default in tests.
    #[cfg(test)]
    pub(super) create_sorted_index_backfill_hook:
        Arc<arc_swap::ArcSwapOption<super::index2_backfill_hook::BackfillPauseHook>>,
    /// F-76 (#903) test-only deterministic pause point: parks `drop_index2`
    /// between the registry retirement and the posting sweep (the exact
    /// visibility window this task closes). Lets a regression test drive a
    /// concurrent READ into the window and assert it falls back to a full
    /// scan instead of observing a registered-but-emptied backend. Reuses
    /// the same `BackfillPauseHook` primitive. `None` in every non-test
    /// build and by default in tests — see `f76_drop_visibility_tests.rs`.
    #[cfg(test)]
    pub(super) drop_index2_pause_hook:
        Arc<arc_swap::ArcSwapOption<super::index2_backfill_hook::BackfillPauseHook>>,
    /// P0-3b (#988) test-only deterministic pause point: parks `drop_index2`
    /// AFTER the posting sweep but BEFORE the reduced metadata is persisted —
    /// the exact crash window sub-bug 3c exercises. A regression test
    /// installs this hook, parks `drop_index2` mid-drop, drops the manager
    /// (simulating a crash), then constructs a fresh `TableManager::create`
    /// against the SAME stores and asserts the recovery path finishes the
    /// drop. Mirrors sorted's `drop_index_post_sweep_hook` (#972). `None` in
    /// every non-test build and by default in tests.
    #[cfg(test)]
    pub(super) drop_index2_post_sweep_hook:
        Arc<arc_swap::ArcSwapOption<super::index2_backfill_hook::BackfillPauseHook>>,
    /// #1066 test-only deterministic pause point: parks `rename_index`'s
    /// `is_index2` branch AFTER the durable RENAME tombstone is written but
    /// BEFORE `rename_entry` mutates the live registry. Lets a regression
    /// test observe/crash in the exact window where the tombstone is durable
    /// while both registry and disk still show the OLD name (matrix row:
    /// "crash before rename_entry" → recovery re-runs the rename from
    /// scratch). `None` in every non-test build and by default in tests.
    #[cfg(test)]
    pub(super) rename_index2_early_pause_hook:
        Arc<arc_swap::ArcSwapOption<super::index2_backfill_hook::BackfillPauseHook>>,
    /// #1066 test-only deterministic pause point: parks `rename_index`'s
    /// `is_index2` branch AFTER `rename_entry` mutates the live registry but
    /// BEFORE `save_index2_metadata` persists it — the exact crash window
    /// this task's tombstone closes (registry shows the NEW name, disk still
    /// has the OLD name, tombstone present). A regression test installs this
    /// hook, parks the rename mid-flight, drops the manager (simulating a
    /// crash), then reopens against the SAME stores and asserts recovery
    /// (`recover_index2_renames`) finishes the rename. `None` in every
    /// non-test build and by default in tests.
    #[cfg(test)]
    pub(super) rename_index2_mid_pause_hook:
        Arc<arc_swap::ArcSwapOption<super::index2_backfill_hook::BackfillPauseHook>>,
    /// #1003 test-only deterministic pause point: parks `create_index_v2`'s
    /// non-btree branch AFTER `index2_registry.insert` (the backend is now
    /// live in the registry, still `Building`) but BEFORE `set_state(Ready)`
    /// — the narrow window where `degraded_index_count()` can actually see
    /// this specific index as non-`Ready`. Needed because the EXISTING
    /// `create_index2_backfill_hook` parks strictly BEFORE the registry
    /// insert, where the raw tally can't see the index at all regardless of
    /// the in-flight-set fix — a review found the original test using that
    /// hook was vacuous for exactly this reason. `None` in every non-test
    /// build and by default in tests.
    #[cfg(test)]
    pub(super) index2_registered_before_ready_hook:
        Arc<arc_swap::ArcSwapOption<super::index2_backfill_hook::BackfillPauseHook>>,
    /// #1087 test-only deterministic pause point: parks `phase_b_a_backfill`
    /// mid-scan (after registering at Building, before completing Phase A).
    /// Lets a regression test drive a concurrent writer into the exact
    /// dirty-set capture window this task enables — confirming that
    /// `mark_build_in_flight` is called BEFORE Phase A starts scanning.
    /// Reuses the same `BackfillPauseHook` primitive as other index hooks.
    /// `None` in every non-test build and by default in tests.
    #[cfg(test)]
    pub(super) online_index_backfill_hook:
        Arc<arc_swap::ArcSwapOption<super::index2_backfill_hook::BackfillPauseHook>>,
    /// P-1060: test-only pause hook for Phase B crash simulation.
    /// Fires after `mark_build_in_flight` but before dropping barrier guards.
    /// At this point: Building is durably persisted, in-flight flag is set,
    /// barrier is still held. Proves the index stays Building after crash.
    #[cfg(test)]
    pub(super) phase_b_pause_hook:
        Arc<arc_swap::ArcSwapOption<super::index2_backfill_hook::BackfillPauseHook>>,
    /// P-1060: test-only pause hook for Phase C crash simulation.
    /// Fires at the top of the catch-up loop (first iteration), before the
    /// first dirty-set drain. At this point: Building is on disk, Phase A
    /// postings are durable, SnapshotGuard is held, no barrier.
    #[cfg(test)]
    pub(super) phase_c_pause_hook:
        Arc<arc_swap::ArcSwapOption<super::index2_backfill_hook::BackfillPauseHook>>,
    /// P-1060: test-only pause hook for Phase D crash simulation.
    /// Fires after the publish barrier is acquired but before the final
    /// residual drain. At this point: Building is STILL on disk (the flip
    /// is the last step of Phase D), barrier is held.
    #[cfg(test)]
    pub(super) phase_d_pause_hook:
        Arc<arc_swap::ArcSwapOption<super::index2_backfill_hook::BackfillPauseHook>>,
    /// Task group 10 (panic-safety audit) test-only deterministic panic
    /// injection point for `verify()`. When set, `verify()` panics
    /// immediately instead of running its normal audit — used to prove that
    /// `bump_write_counter`'s spawned background-verify task still clears
    /// `verify_running` when `verify()` panics instead of returning `Err`
    /// (see [`install_verify_panic_hook`](Self::install_verify_panic_hook)).
    /// `false` in every non-test build and by default in tests — zero cost
    /// on the real verify path.
    #[cfg(test)]
    pub(super) verify_panic_hook: Arc<std::sync::atomic::AtomicBool>,
    /// Task group 10 test-only counter: bumped at the top of every
    /// `verify()` call (panicking or not). Lets a regression test prove a
    /// SECOND `bump_write_counter`-triggered verify actually ran (rather
    /// than being silently skipped because `verify_running` was left
    /// stranded `true` by a prior panicking verify).
    #[cfg(test)]
    pub(super) verify_call_count: Arc<std::sync::atomic::AtomicUsize>,
}

/// Bundle wiring the non-tx write path to the SSI commit-write log.
///
/// Holds the per-repo [`RepoTxGate`](shamir_tx::RepoTxGate) (the version
/// source — shared with the tx commit pipeline so versions stay monotonic
/// across both paths). The `gate` is used by `record_nontx_ssi_footprint`
/// to append `CommitWriteRecord`s so that Serializable transactions see
/// non-tx writes in their Phase 2-bis predicate-conflict window.
/// Cloned cheaply (`Arc`).
#[derive(Clone)]
pub(super) struct NonTxChangefeed {
    pub(super) gate: Arc<shamir_tx::RepoTxGate>,
}

/// Extract a human-readable message from a caught panic payload. Panics
/// raised via `panic!("...")` / `.expect("...")` carry `&str` or `String`;
/// anything else (a custom payload type) falls back to a fixed label.
fn panic_payload_message(payload: &(dyn std::any::Any + Send)) -> String {
    if let Some(s) = payload.downcast_ref::<&str>() {
        (*s).to_string()
    } else if let Some(s) = payload.downcast_ref::<String>() {
        s.clone()
    } else {
        "non-string panic payload".to_string()
    }
}

/// How often the background watchdog runs a `verify` pass.
/// Coarse — once per ~thousand mutating ops, regardless of batch
/// size. Tuned to "noticeable problem within seconds" without
/// noticeable overhead.
pub(super) const AUTO_VERIFY_EVERY_N_WRITES: u64 = 1024;

impl Clone for TableManager {
    fn clone(&self) -> Self {
        Self {
            name: self.name.clone(),
            table: Arc::clone(&self.table),
            info_store: Arc::clone(&self.info_store),
            interner: self.interner.clone(),
            counter: Arc::clone(&self.counter),
            persist_registry: self.persist_registry.clone(),
            index_manager: self.index_manager.clone(),
            sorted_indexes: self.sorted_indexes.clone(),
            write_counter: Arc::clone(&self.write_counter),
            verify_running: Arc::clone(&self.verify_running),
            unique_write_lock: Arc::clone(&self.unique_write_lock),
            // F-69 (#896) — shared across clones (Arc<AtomicU8> inside) so a
            // create/DDL in flight on any clone forces writers on every clone
            // onto the barrier, same rationale the five separate flags this
            // replaces each carried individually.
            write_barrier_flags: self.write_barrier_flags.clone(),
            // F-95 (#957) — shared across clones (Arc<tokio::sync::Mutex>
            // inside) so a DDL admission in flight on any clone blocks every
            // clone's begin_write_barrier, same rationale as the other
            // shared-Arc fields.
            ddl_admission: Arc::clone(&self.ddl_admission),
            // S5c-X — the SAME per-repo domain is shared across clones.
            write_domain: Arc::clone(&self.write_domain),
            // F-48 — shared across clones (Arc<AtomicUsize> inside).
            writer_drain: self.writer_drain.clone(),
            // #1003 — shared across clones (Arc<Mutex<BTreeMap<..>>> inside),
            // same rationale as `writer_drain`: a create in flight on any
            // clone must be visible to `degraded_index_count()` called on
            // another.
            in_flight_creates: self.in_flight_creates.clone(),
            index2_registry: Arc::clone(&self.index2_registry),
            #[cfg(test)]
            filtered_ann_thresholds: self.filtered_ann_thresholds,
            mvcc_store: self.mvcc_store.clone(),
            validator_bindings: Arc::clone(&self.validator_bindings),
            bindings_lock: Arc::clone(&self.bindings_lock),
            validator_registry: self.validator_registry.clone(),
            changefeed: self.changefeed.clone(),
            scalar_resolver: Arc::clone(&self.scalar_resolver),
            #[cfg(test)]
            create_index2_backfill_hook: Arc::clone(&self.create_index2_backfill_hook),
            #[cfg(test)]
            create_sorted_index_backfill_hook: Arc::clone(&self.create_sorted_index_backfill_hook),
            #[cfg(test)]
            drop_index2_pause_hook: Arc::clone(&self.drop_index2_pause_hook),
            #[cfg(test)]
            drop_index2_post_sweep_hook: Arc::clone(&self.drop_index2_post_sweep_hook),
            #[cfg(test)]
            rename_index2_early_pause_hook: Arc::clone(&self.rename_index2_early_pause_hook),
            #[cfg(test)]
            rename_index2_mid_pause_hook: Arc::clone(&self.rename_index2_mid_pause_hook),
            #[cfg(test)]
            index2_registered_before_ready_hook: Arc::clone(
                &self.index2_registered_before_ready_hook,
            ),
            #[cfg(test)]
            online_index_backfill_hook: Arc::clone(&self.online_index_backfill_hook),
            #[cfg(test)]
            phase_b_pause_hook: Arc::clone(&self.phase_b_pause_hook),
            #[cfg(test)]
            phase_c_pause_hook: Arc::clone(&self.phase_c_pause_hook),
            #[cfg(test)]
            phase_d_pause_hook: Arc::clone(&self.phase_d_pause_hook),
            #[cfg(test)]
            verify_panic_hook: Arc::clone(&self.verify_panic_hook),
            #[cfg(test)]
            verify_call_count: Arc::clone(&self.verify_call_count),
        }
    }
}

impl TableManager {
    #[cfg(any(test, feature = "test-util"))]
    pub fn validator_registry_ref_for_test(
        &self,
    ) -> Option<&Arc<crate::validator::ValidatorRegistry>> {
        self.validator_registry.as_ref()
    }

    /// R0-C (#1009): build the table-open-wide error for a persisted index2
    /// descriptor whose name collides with an already-registered backend.
    ///
    /// Once [`IndexRegistry::insert`](crate::index2::IndexRegistry::insert)
    /// checks name availability before mutating (the #1009 fix), an `Err`
    /// from `insert()` during open-path recovery means the ON-DISK METADATA
    /// itself is contradictory — two persisted descriptors claim the same
    /// name. There is no single "broken backend" to mark `Failed` the way
    /// R0-D's fix does (that mechanism is for a backend that registered fine
    /// but failed to RESTORE its content; this is a backend that can't even
    /// be assigned a name slot) — so the correct response is stronger: fail
    /// the WHOLE table open rather than silently drop one of the two
    /// colliding descriptors and proceed with a partially-loaded table.
    fn duplicate_persisted_index2_name_error(
        name: &str,
        colliding_id: u32,
        source: &crate::index2::IndexError,
    ) -> shamir_storage::error::DbError {
        shamir_storage::error::DbError::Internal(format!(
            "table open failed: persisted index2 metadata is corrupt — descriptor \
             '{name}' (id={colliding_id}) could not be registered because its name is \
             already taken by another persisted descriptor: {source}. On-disk metadata \
             contains two descriptors claiming the same name; resolve the duplicate \
             (rename or remove one of the colliding entries in the persisted index2 \
             metadata) before this table can be reopened."
        ))
    }

    /// Create a new TableManager with all internal components.
    ///
    /// This is the preferred way to create a TableManager - it handles
    /// internal Table creation and all component initialization.
    ///
    /// S5c-X: constructs a fresh, private per-repo write-admission domain
    /// and delegates to [`create_with_domain`](Self::create_with_domain).
    pub async fn create(
        name: String,
        data_store: Arc<dyn Store>,
        info_store: Arc<dyn Store>,
    ) -> DbResult<Self> {
        let write_domain = RepoWriteDomain::new(name.clone());
        Self::create_with_domain(name, data_store, info_store, write_domain).await
    }

    /// S5c-X: like [`create`](Self::create), but sharing a CALLER-SUPPLIED
    /// per-repo [`RepoWriteDomain`] — the normal production path, where a
    /// repository's tables must serialize DDL against each other and the
    /// registry flushers through the SAME domain.
    ///
    /// Admits ONCE for the whole open/create sequence, then delegates to
    /// [`create_with_domain_admitted`](Self::create_with_domain_admitted)
    /// (the full body lives in `write_receipt/write_domain_barrier.rs`).
    pub async fn create_with_domain(
        name: String,
        data_store: Arc<dyn Store>,
        info_store: Arc<dyn Store>,
        write_domain: Arc<RepoWriteDomain>,
    ) -> DbResult<Self> {
        let permit = write_domain.admit().await.map_err(|e| {
            DbError::Internal(format!(
                "repository write admission failed during table open: {e}"
            ))
        })?;
        Self::create_with_domain_admitted(name, data_store, info_store, write_domain, &permit).await
    }

    /// Create a TableManager from existing components.
    ///
    /// This is primarily for testing or advanced use cases.
    #[cfg(test)]
    pub fn new(
        name: String,
        table: Table,
        interner: InternerManager,
        counter: Arc<RecordCounter>,
        index_manager: IndexManager,
    ) -> Self {
        // Tests that construct TableManager directly don't exercise
        // sorted indexes — give them an empty manager that shares
        // info_store... but we don't have it here. The simplest
        // thing: construct an "orphan" sorted manager backed by an
        // in-memory store. Its persisted defs blob then lives in a
        // throwaway store, which is fine because these tests never
        // call sorted-index methods.
        let info_store: Arc<dyn Store> =
            Arc::new(shamir_storage::storage_in_memory::InMemoryStore::new());
        // Construct synchronously: SortedIndexManager::new() is async
        // but the empty-state path doesn't await any real work.
        let sorted_indexes =
            futures::executor::block_on(SortedIndexManager::new(info_store.clone()))
                .expect("sorted index manager init for test");
        let mut persist_registry = PersistRegistry::new();
        persist_registry
            .register(Arc::new(interner.clone()) as Arc<dyn super::persistable::Persistable>);
        persist_registry.register(Arc::clone(&counter) as Arc<dyn super::persistable::Persistable>);
        // F-69 (#896): clone the SAME packed word `index_manager` uses for its
        // `UNIQUE_INDEX_EXISTS` bit before `index_manager` moves into `Self`.
        let write_barrier_flags = index_manager.write_barrier_flags();
        // Bind the domain BEFORE the struct literal: `name` is MOVED into the
        // literal below, so `name.clone()` after that move would not compile.
        let domain = RepoWriteDomain::new(name.clone());
        Self {
            name,
            table: Arc::new(table),
            info_store,
            interner,
            counter,
            persist_registry,
            index_manager,
            sorted_indexes,
            write_counter: Arc::new(std::sync::atomic::AtomicU64::new(0)),
            verify_running: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            unique_write_lock: Arc::new(tokio::sync::Mutex::new(())),
            write_barrier_flags,
            ddl_admission: Arc::new(tokio::sync::Mutex::new(())),
            // S5c-X — tests constructing directly get a private domain.
            write_domain: domain,
            writer_drain: super::writer_drain_barrier::WriterDrainBarrier::new(),
            in_flight_creates: super::in_flight_create_guard::InFlightCreateSet::new(),
            index2_registry: Arc::new(crate::index2::IndexRegistry::new()),
            #[cfg(test)]
            filtered_ann_thresholds: None,
            mvcc_store: None,
            validator_bindings: Arc::new(arc_swap::ArcSwap::from_pointee(Vec::new())),
            bindings_lock: Arc::new(tokio::sync::Mutex::new(())),
            validator_registry: None,
            changefeed: None,
            scalar_resolver: Arc::new(arc_swap::ArcSwap::new(std::sync::Arc::new(
                shamir_funclib::scalar_resolver::ScalarResolver::builtins_only(),
            ))),
            #[cfg(test)]
            create_index2_backfill_hook: Arc::new(arc_swap::ArcSwapOption::empty()),
            #[cfg(test)]
            create_sorted_index_backfill_hook: Arc::new(arc_swap::ArcSwapOption::empty()),
            #[cfg(test)]
            drop_index2_pause_hook: Arc::new(arc_swap::ArcSwapOption::empty()),
            #[cfg(test)]
            drop_index2_post_sweep_hook: Arc::new(arc_swap::ArcSwapOption::empty()),
            #[cfg(test)]
            rename_index2_early_pause_hook: Arc::new(arc_swap::ArcSwapOption::empty()),
            #[cfg(test)]
            rename_index2_mid_pause_hook: Arc::new(arc_swap::ArcSwapOption::empty()),
            #[cfg(test)]
            index2_registered_before_ready_hook: Arc::new(arc_swap::ArcSwapOption::empty()),
            #[cfg(test)]
            online_index_backfill_hook: Arc::new(arc_swap::ArcSwapOption::empty()),
            #[cfg(test)]
            phase_b_pause_hook: Arc::new(arc_swap::ArcSwapOption::empty()),
            #[cfg(test)]
            phase_c_pause_hook: Arc::new(arc_swap::ArcSwapOption::empty()),
            #[cfg(test)]
            phase_d_pause_hook: Arc::new(arc_swap::ArcSwapOption::empty()),
            #[cfg(test)]
            verify_panic_hook: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            #[cfg(test)]
            verify_call_count: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
        }
    }
}
