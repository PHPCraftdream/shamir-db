use std::sync::Arc;

use crate::index::sorted_index_manager::SortedIndexManager;
use crate::repo::RepoWritePermit;
use crate::table::interner_manager::InternerManager;
use crate::table::migration_freeze_guard::MigrationFreezeGuard;
use crate::tx::CommitError;
use shamir_storage::error::DbResult;

use super::super::{NonTxChangefeed, TableManager};
use super::guard::WriteBarrierGuard;

impl TableManager {
    /// F-70 (#897, P0) — THE canonical acquisition path for a DDL-side write
    /// barrier: raise `bit`, [`drain_writers`](Self::drain_writers), THEN
    /// acquire `unique_write_lock` — in that order,
    /// and no other. See [`writer_drain_barrier`](crate::table::writer_drain_barrier)'s
    /// module doc ("F-70 — THE canonical lock-order hierarchy") for the full
    /// derivation of why this order (not lock-then-drain, which F-57, #883,
    /// wired into every DDL create path and which this task found genuinely
    /// deadlocks against the tx-commit path's own drain-guard-then-lock
    /// shape).
    ///
    /// Every DDL call site that used to hand-roll
    /// `unique_write_lock().lock().await` followed by
    /// `IndexCreateBarrierGuard::set(..)` + `drain_writers().await` now calls
    /// this instead, so the order cannot silently drift back at a new call
    /// site (exactly how F-57 introduced the inversion in the first place —
    /// the hierarchy was never written down anywhere shared, only implied
    /// per-call-site).
    ///
    /// Returns the RAII [`WriteBarrierGuard`] (clears `bit` on drop) and the
    /// owned `unique_write_lock` guard. Both must be held for the ENTIRE
    /// snapshot/backfill/register/persist body that follows — drop order
    /// (guard before lock, achieved by dropping in the order returned/
    /// declared) mirrors every existing DDL site's discipline.
    ///
    /// `pub` (not `pub(crate)`): the schema-activation DDL handler
    /// (`shamir-db::execute::admin_schema`) is a DIFFERENT crate and must use
    /// this SAME entry point for `SCHEMA_ACTIVATION` — see
    /// `write_barrier_flags`'s bit constants, all of which are `pub`.
    /// S5c-X — public DDL entry point: acquire repository admission FIRST,
    /// then run the canonical raise→drain→lock sequence under it. The repo
    /// permit is retained inside the returned [`WriteBarrierGuard`] (and
    /// released only when the guard drops, AFTER the barrier bit is
    /// cleared), so the entire caller critical section holds repo
    /// admission. Prefer the `_admitted` twin when a permit is already
    /// held (see [`begin_write_barrier_admitted`](Self::begin_write_barrier_admitted)).
    pub async fn begin_write_barrier(
        &self,
        bit: u8,
    ) -> DbResult<(WriteBarrierGuard, tokio::sync::OwnedMutexGuard<()>)> {
        let permit = self.admit().await?;
        let (guard, lock) = self.begin_write_barrier_admitted_impl(bit).await;
        Ok((guard.retaining_permit(permit), lock))
    }

    /// S5c-X — acquire per-REPOSITORY write admission. Lock order (BINDING):
    /// Freeze source writes until the returned guard is dropped.
    /// Preparation order: ddl_admission → raise bit → drain → lock fence.
    /// Admission is released before returning; DDL remains allowed during
    /// the migration, and the unique write lock is only a preparation fence.
    ///
    /// cancel-safe: yes — admission acquisition has no side effect; after
    /// the bit is claimed its owner exists before drain or lock can suspend.
    /// Cancelling either wait drops that owner and clears only the freeze bit.
    pub async fn begin_migration_freeze(&self) -> Result<MigrationFreezeGuard, CommitError> {
        let permit = self.admit().await.map_err(CommitError::Storage)?;
        self.begin_migration_freeze_admitted(&permit).await
    }

    pub async fn begin_migration_freeze_admitted(
        &self,
        permit: &RepoWritePermit,
    ) -> Result<MigrationFreezeGuard, CommitError> {
        self.write_domain.debug_check_permit(permit);
        let admission = self.ddl_admission.lock().await;
        let guard =
            MigrationFreezeGuard::try_set(self.write_barrier_flags.clone()).ok_or_else(|| {
                CommitError::MigrationFrozen {
                    table: self.name.clone(),
                }
            })?;
        self.drain_writers().await;
        let fence = self.unique_write_lock.lock().await;
        drop(fence);
        drop(admission);
        Ok(guard)
    }

    /// F-37 (#845) — drive the schema-activation write barrier from outside
    /// the engine crate (the shamir-db schema DDL handler in
    /// `admin_schema.rs`). When `on == true`, every writer consulting
    /// `needs_write_barrier` returns `true` and
    /// serializes on `unique_write_lock` — closing
    /// the `keyset_safe` count-proof race (a concurrent INSERT/UPDATE can no
    /// longer land between the `count() == 0` read and the schema's persist +
    /// activate).
    ///
    /// `SeqCst`-ordered (F-56): the store must participate in the SAME single
    /// SeqCst total order as the writer's `SeqCst` flag load in
    /// `needs_write_barrier` and the writer's
    /// `SeqCst` `active`-counter `fetch_add` — the cross-atomic dependency the
    /// drain proof relies on cannot be carried by `Release`/`Acquire` alone
    /// (see `writer_drain_barrier`).
    ///
    /// **This is a raw, test-facing flag setter, NOT the production entry
    /// point.** It does NOT need to be (and in production is NOT) called
    /// under `unique_write_lock`: F-70 (#897) established the canonical
    /// drain-then-lock order — raise the intent bit FIRST, then drain, then
    /// acquire the lock (see [`begin_write_barrier`](Self::begin_write_barrier)
    /// for the sequence and [`writer_drain_barrier`](crate::table::writer_drain_barrier)'s
    /// "F-70 — THE canonical lock-order hierarchy" module doc for why the
    /// prior lock-then-set discipline is a deadlock). Production DDL callers
    /// MUST go through [`begin_write_barrier`](Self::begin_write_barrier),
    /// which performs the raise→drain→lock sequence in this exact order and
    /// returns an RAII [`WriteBarrierGuard`] that clears the bit on drop
    /// (every exit path). This raw setter remains only for tests that
    /// exercise the flag transition directly without the full barrier
    /// sequence.
    pub fn set_schema_activation_barrier(&self, on: bool) {
        self.write_barrier_flags
            .set_to(crate::index::write_barrier_flags::SCHEMA_ACTIVATION, on);
    }

    /// F-48 (#859, P0) — drain every in-flight fast-path writer before the
    /// caller proceeds past a snapshot/proof point (the `keyset_safe`
    /// count-proof for schema activation; the backfill snapshot for index2
    /// create in F-50).
    ///
    /// The caller MUST have already raised its intent bit
    /// (`SCHEMA_ACTIVATION` / `INDEX2_CREATE` / ... — see
    /// `write_barrier_flags` field doc) so NEW writers take the slow
    /// (locked) path. Then this catches any writer that read `false` before
    /// the flag went up and is still in its validate→write→index sequence.
    /// Returns immediately (one `SeqCst` load) when no fast-path writer is
    /// active.
    ///
    /// **F-70 (#897) order — call this BEFORE acquiring `unique_write_lock`,
    /// NOT after.** This is the canonical drain-then-lock order; the prior
    /// lock-then-drain order (F-57) deadlocks against the tx-commit path's
    /// drain-guard-then-lock shape — see
    /// [`writer_drain_barrier`](crate::table::writer_drain_barrier)'s "F-70 —
    /// THE canonical lock-order hierarchy" module doc for the full 3-party
    /// derivation. Holding the lock is NOT required for `drain_writers()` to
    /// be correct: what matters is that the intent bit was raised first (so
    /// every NEW writer takes the slow path and cannot re-enter the drain
    /// set) — the lock only serializes those NEW slow-path writers against
    /// the caller's own body, a separate concern from the drain wait itself.
    ///
    /// Production code MUST go through [`begin_write_barrier`](Self::begin_write_barrier)
    /// (which performs the raise→drain→lock sequence in this exact order)
    /// instead of calling this method directly — `drain_writers()` is a raw
    /// primitive that `begin_write_barrier` (and the legacy direct callers
    /// that pre-date it) delegate to, not the sanctioned entry point. See
    /// [`writer_drain_barrier`](crate::table::writer_drain_barrier) for the
    /// full memory-model rationale and how F-50 reuses this same call.
    pub async fn drain_writers(&self) {
        self.writer_drain.drain().await;
    }

    /// F-48b (#867) — the writer-side twin of `drain_writers`: bump the
    /// drain-set counter and return an RAII guard that decrements on drop.
    ///
    /// Called by the tx-commit pipeline's Phase 2.5 prelock
    /// (`tx::pre_commit::pre_commit_prelock`) for EVERY table in `tx.write_set`
    /// BEFORE reading [`needs_write_barrier`](Self::needs_write_barrier) — the
    /// ordering is load-bearing (see `writer_drain_barrier` for the
    /// happens-before chain). If the flag is `true` (slow path), the caller
    /// drops the returned guard BEFORE taking `unique_write_lock` (the lock
    /// serializes the slow path; staying in the drain set while blocking on
    /// the lock would deadlock against a DDL holding the lock and waiting on
    /// `drain_writers`). If the flag is `false`, the caller keeps the guard
    /// alive until its Phase 5c materialize write has landed — so a DDL that
    /// raises the barrier AFTER the flag read genuinely waits for this tx.
    ///
    /// Visibility mirrors `needs_write_barrier` (`pub(crate)`): the only
    /// caller is the engine's own tx-commit prelock. The non-tx writer methods
    /// in `table_manager_crud.rs` access `self.writer_drain` directly (same
    /// module); this accessor exists for the cross-module prelock call site.
    pub(crate) fn enter_writer_drain(
        &self,
    ) -> crate::table::writer_drain_barrier::WriterDrainGuard {
        self.writer_drain.enter_writer()
    }

    /// Borrow the table's sorted-index manager — used by the planner
    /// for range / order / min queries, and by DDL when a
    /// `create_index { sorted: true }` op lands.
    pub(crate) fn sorted_indexes(&self) -> &SortedIndexManager {
        &self.sorted_indexes
    }

    /// O(1) composite check: does this table have ANY index across all
    /// three subsystems (index2 registry, base_index hash/unique, sorted)?
    ///
    /// Used as a fast-path guard on the insert hot path to skip the
    /// `all_backends().await` scan + 3 base_index planner calls when the
    /// table has zero indexes. Each sub-check is O(1): `is_empty()`
    /// on `scc::HashMap`, an `AtomicBool` load, a packed-word bit test
    /// (F-69, #896), `DashMap::is_empty`.
    pub fn has_any_index(&self) -> bool {
        !self.index2_registry.is_empty()
            || self.index_manager.has_indexes()
            || self.index_manager.has_unique_indexes()
            || self.sorted_indexes.has_indexes()
    }

    /// O(1) predicate: must this writer acquire `unique_write_lock` before
    /// its validate→write→index sequence?
    ///
    /// `true` when ANY of:
    /// - the table has a base_index unique index (the original reason the barrier
    ///   exists — atomic unique-check + posting-write),
    /// - an index2 `create_index_v2` is currently in flight (#534 finding 1),
    ///   OR
    /// - a schema-activation DDL (`set_table_schema` / `add_schema_rule`) is
    ///   currently in its `keyset_safe` count-proof → persist → activate
    ///   window (F-37, #845), OR
    /// - a regular hash-index `create_index` (F-57, #883), unique-index
    ///   `create_unique_index` (F-57), or sorted-index
    ///   `create_sorted_index_with_include` (F-57) snapshot→backfill→register
    ///   sequence is currently in flight.
    ///
    /// Note: a table with ONLY a regular (non-unique) hash index and zero
    /// other barrier conditions keeps the lock-free fast path. The
    /// `REGULAR_INDEX_EXISTS` steady-state flag is excluded from the barrier
    /// predicate via `BARRIER_BITS` (see #1102 round 2 brief for why this is
    /// necessary).
    ///
    /// F-69 (#896, P0): this is now EXACTLY ONE atomic load —
    /// `write_barrier_flags.any_set()` — instead of seven independent loads
    /// (two from `IndexManager` plus five DDL-intent flags here). A single
    /// atomic load can never be torn regardless of memory ordering, so the
    /// compound predicate is now a genuine point-in-time snapshot; see
    /// `shamir_index::base_index::write_barrier_flags`'s module doc for the
    /// full before/after writeup and why this fixes a real duplicate-past-unique-
    /// constraint race. The load is `SeqCst` (F-56, #882): it is the writer
    /// half of a cross-atomic dependency with the `active` drain counter — a
    /// writer that reads `false` must have its prior `active.fetch_add`
    /// (also `SeqCst`) be observable by the drainer's `active.load` (also
    /// `SeqCst`). Only a single `SeqCst` total order across both atomics can
    /// carry that edge — `Release`/`Acquire` alone cannot (see
    /// `writer_drain_barrier`'s memory-model proof, which this task
    /// extends to state it now covers the full predicate).
    ///
    /// Consulted by the non-tx writer methods in `table_manager_crud.rs`
    /// (`insert`/`insert_many_returning_version`/`delete_returning_version`/
    /// `set`), AND (task #538, Part A) by the tx-commit pipeline's Phase 2.5
    /// prelock (`tx::pre_commit::pre_commit_prelock`), which now acquires
    /// `unique_write_lock` for every table this tx wrote to that returns
    /// `true` here — not just tables with base_index unique guards. This closes
    /// the commit-time serialization gap for an index2-only table (no base_index
    /// unique index) under an in-flight `create_index_v2`. It does NOT close
    /// #538's Part B (stage-time index2 ops-plan staleness against a
    /// create that starts/finishes entirely between this tx's stage and
    /// commit) — see `TableManager::backfill_index2_backend`'s doc comment
    /// for the full accounting of what #534 and #538 each close and leave
    /// open.
    pub(crate) fn needs_write_barrier(&self) -> bool {
        self.write_barrier_flags.any_set()
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    /// Attach an MvccStore for tx-aware reads. Returns `self` so callers
    /// can chain after `create()`. When attached, `read_one_tx(rid, Some(tx))`
    /// reads through the MvccStore at `tx.snapshot_version`. Without an
    /// attached MvccStore, tx-aware reads fall through to the non-tx
    /// fast path (same as `get`).
    pub fn with_mvcc_store(mut self, mvcc: Arc<shamir_tx::MvccStore>) -> Self {
        self.mvcc_store = Some(mvcc);
        self
    }

    /// Stage I — replace this table's per-table [`InternerManager`] with the
    /// shared per-repo one. `RepoInstance::create_table_context` calls this
    /// so every table in a repo shares ONE live
    /// [`Interner`](shamir_types::core::interner::Interner) and id-namespace
    /// (a field name resolves to the SAME id across tables). Returns `self`
    /// for chaining after [`create`](Self::create), mirroring
    /// [`with_mvcc_store`](Self::with_mvcc_store).
    ///
    /// `InternerManager::clone` Arc-shares the live `Interner` (the
    /// `OnceCell<Interner>`, the chunk-persist atomics, and the persist
    /// mutex), so the per-table handle returned by `interner`
    /// is the SAME manager the repo owns — a write through any table's
    /// interner is visible to every other table's reads. The
    /// [`PersistRegistry`] keeps a separate clone of the per-table manager it
    /// was built with in [`create`](Self::create); that clone shares the same
    /// Arc state, so `flush_metadata` / `flush_buffers` persist the shared
    /// interner through any registered handle. Idempotent: re-attaching the
    /// same manager is a cheap clone.
    pub fn with_interner(mut self, interner: InternerManager) -> Self {
        self.interner = interner;
        self
    }

    /// Borrow the attached [`MvccStore`](shamir_tx::MvccStore), if any.
    /// Used by the index-only read path (slice A3) to validate covering-index
    /// posting freshness without fetching the record from the data store.
    pub(crate) fn mvcc_store_ref(&self) -> Option<&Arc<shamir_tx::MvccStore>> {
        self.mvcc_store.as_ref()
    }

    /// Public accessor for the attached MvccStore (the version-log handle).
    /// Used by the migration coordinator (Q1) to read the source snapshot
    /// through the log seam (`current_stream`) instead of the raw data_store.
    pub(crate) fn mvcc_store(&self) -> Option<Arc<shamir_tx::MvccStore>> {
        self.mvcc_store_ref().cloned()
    }

    /// Acquire a snapshot guard for Phase A of online CREATE INDEX.
    ///
    /// Returns `Some(guard)` if this table has a changefeed attached (i.e.,
    /// if it was created through `RepoInstance::create_table_context`, which
    /// always attaches both `mvcc_store` and `changefeed`). Returns `None`
    /// for system tables or direct-constructed test tables that lack a changefeed.
    ///
    /// The guard pins the MVCC version it protects via `gate.active_snapshots`,
    /// preventing GC from reclaiming any version ≥ `guard.version()` until the
    /// guard is dropped. Phase A's backfill scan must hold this guard for its
    /// entire lifetime — dropping it mid-scan would leave the scan vulnerable to
    /// GC removing versions it still needs, resulting in data loss.
    ///
    /// **Contract:** The caller MUST ensure the guard outlives the scan it
    /// protects. Acquire the guard FIRST, then use `guard.version()` as the
    /// pin version — this is the version provably protected from GC. The
    /// caller must hold the writer-drain barrier, after draining this table's
    /// writers, before opening the allocation-high-water snapshot.
    ///
    /// Returns an async `SnapshotGuard` — `await` is required.
    ///
    /// #1057 (slice 1b): primitive added, not yet wired into create_index path
    /// (that's slice 1d). Allow dead-code until then.
    #[allow(dead_code)]
    pub(crate) async fn open_index_build_snapshot(&self) -> Option<shamir_tx::SnapshotGuard> {
        match self.changefeed.as_ref() {
            Some(cf) => {
                let pin = cf.gate.version_allocation_high_water_mark();
                Some(cf.gate.open_snapshot_at(pin).await)
            }
            None => None,
        }
    }

    /// Wire this table's non-tx write path to the SSI commit-write log.
    ///
    /// Returns `self` so callers can chain after `create()` (mirrors
    /// [`with_mvcc_store`](Self::with_mvcc_store)). `gate` MUST be the same
    /// per-repo [`RepoTxGate`](shamir_tx::RepoTxGate) the tx commit pipeline
    /// uses — that is what keeps non-tx and tx `commit_version`s on one
    /// monotonic sequence per repo, AND ensures non-tx writes are visible
    /// to Serializable transactions' Phase 2-bis predicate-conflict check.
    ///
    /// Attached by `RepoInstance::create_table_context`. When absent, the
    /// non-tx write methods skip SSI footprint recording entirely (system
    /// tables / direct-constructed test tables).
    pub fn with_changefeed(mut self, gate: Arc<shamir_tx::RepoTxGate>) -> Self {
        self.changefeed = Some(NonTxChangefeed { gate });
        self
    }
}
