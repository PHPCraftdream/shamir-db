use std::collections::BTreeSet;
use std::str::FromStr;
use std::sync::Arc;

use futures::StreamExt;
use shamir_query_types::read::{DdlOpKind, DdlOpState, DdlOpStatus};
use shamir_storage::error::DbResult;
use shamir_types::core::interner::TouchInd;
use shamir_types::types::record_id::RecordId;
use shamir_types::types::value::InnerValue;

use super::table_manager::TableManager;
use crate::index::index_definition::IndexDefinition;
use crate::index::index_info_item::IndexInfoItem;

/// Result structure for #1088 Phase C/D: the SnapshotGuard and pin version
/// must be returned from Phase B+A and kept alive through Phase C/D.
#[allow(dead_code)]
pub(crate) struct PhaseBAResult {
    pub guard: shamir_tx::SnapshotGuard,
    pub pin: u64,
}

impl TableManager {
    #[allow(dead_code)]
    const CATCHUP_ITERATION_CAP: usize = 10; // RFC v3 §2.4/§6.2 — conservative
                                             // fixed cap, no tunables precedent
                                             // for this yet; local const is fine.

    /// Create a regular or specialized (fts/vector/functional) index.
    ///
    /// Routes `btree` + `unique` variants through the base_index `IndexManager`
    /// path (`create_index` / `create_unique_index`). All other index types
    /// go through the `index2` backend pipeline.
    pub async fn create_index_v2(
        &self,
        op: &shamir_query_types::admin::CreateIndexOp,
    ) -> DbResult<()> {
        use crate::index2::backend::IndexBackend;
        use crate::index2::descriptor::IndexDescriptor;
        use crate::index2::kind::*;
        use smallvec::SmallVec;

        let index_type = op.index_type.as_deref().unwrap_or("btree");
        if index_type == "btree" {
            let paths: Vec<String> = op.fields.iter().map(|segs| segs.join(".")).collect();
            let path_refs: Vec<&str> = paths.iter().map(|s| s.as_str()).collect();
            return if op.unique {
                self.create_unique_index(&op.create_index, &path_refs).await
            } else {
                self.create_index(&op.create_index, &path_refs).await
            };
        }

        // P0-3b (#988) sub-bug 3b: namespace-reuse guard. Reject CREATE for
        // a name whose DROP is still in flight. The tombstone stores descriptor
        // ids (`Vec<u32>`), so we resolve each tombstoned id's name via the
        // persisted metadata (the durable record) and check for a match with
        // `op.create_index`. Mirrors `SortedIndexManager::register`'s guard
        // (#972) — adapted for index2's id-based tombstone. On the normal path
        // (no tombstone), this is a single cheap load that returns an empty vec.
        //
        // R0-A (#1012): this check MUST run BEFORE `begin_write_barrier`
        // below, not after (moved here from immediately after the barrier
        // acquisition). Reason: `drop_index2` (R0-A) now holds
        // `ddl_admission` for its ENTIRE critical section, including while
        // parked at its pre-sweep test hook — so a `create_index_v2` that
        // took the barrier FIRST would block on the SAME per-table
        // `ddl_admission` mutex until the in-flight drop fully completes,
        // never reaching this tombstone check at all. That reintroduces a
        // deadlock against the P0-3b (#988) test design, which requires
        // `create_index_v2` to observe an in-flight drop's tombstone and
        // reject QUICKLY (without waiting for the drop to finish) —
        // confirmed by `p03b_index2_name_reuse_rejected_during_drop`
        // (`table/tests/p03b_index2_drop_durability_tests.rs`), which timed
        // out under nextest's 180s bound when this check ran after the
        // barrier. Running the check first, cheaply, without holding
        // admission, restores that fast-reject behavior.
        //
        // This reordering does not weaken the guard: the check still runs
        // again for free effectively, because if a DROP starts and takes
        // `ddl_admission` between this pre-check and our own
        // `begin_write_barrier` call below, OUR barrier call simply blocks
        // until that drop's entire critical section (including its
        // tombstone clear) has completed — by the time we're admitted, the
        // tombstone is gone and there is nothing left to reject against.
        // The only residual is a benign race in caller-ordering (a CREATE
        // issued microseconds before a DROP starts may "win" and proceed
        // once the DROP releases admission, rather than being rejected) —
        // not a correctness bug: no double-registration, no orphaned
        // postings, no stale tombstone state.
        let dropping_ids =
            crate::index2::persistence::load_dropping_index2(&self.info_store).await?;
        if !dropping_ids.is_empty() {
            if let Some(persisted) =
                crate::index2::persistence::load_index2_metadata(&self.info_store).await?
            {
                for d in &persisted.descriptors {
                    if dropping_ids.iter().any(|(id, _, _)| *id == d.id)
                        && d.name == op.create_index
                    {
                        return Err(shamir_storage::error::DbError::Internal(format!(
                            "Cannot create index '{}': a DROP INDEX for this name is \
                             still in progress. Retry after the drop completes.",
                            op.create_index
                        )));
                    }
                }
            }
        }

        // #534 finding 1 (write-barrier, Option B — mirrors
        // `create_unique_index`'s own audit-A9 precedent). Hold the table-wide
        // `unique_write_lock` across the ENTIRE reserve-id → backfill →
        // register → persist sequence for the index2 branch.
        // The `INDEX2_CREATE` bit (set below, under this guard) flips
        // `needs_write_barrier()` to `true` on every NON-TX writer path
        // (`insert`/`insert_many_returning_version`/`delete_returning_version`/
        // `set` in `table_manager_crud.rs`) — so even an index2-only table
        // (no base_index unique index, normally lock-free) now serializes THOSE
        // writers against this create.
        //
        // PARTIAL FIX, honestly scoped (see `backfill_index2_backend`'s doc
        // comment below for the full writeup, added after an `@fl`
        // adversarial pass): this barrier does NOT reach the tx-commit path
        // (`execute_insert_tx`/`execute_update_tx`/`execute_delete_tx`/
        // `execute_set_tx` → the commit pipeline), which is how every real
        // client DML statement is actually served — that path plans index2
        // ops against an `all_backends()` snapshot at STAGE time and
        // materializes the row later at commit Phase 5a, neither of which
        // consults this flag. Closing that gap needs the commit pipeline's
        // own prelock/materialize ordering to respect the barrier — tracked
        // as a separate follow-up task, not implemented here. What IS
        // correctly closed by this guard: the non-tx write paths (used by
        // replication-apply and directly by tests).
        //
        // Callers of `create_index_v2` (grep-verified: DDL admin path in
        // shamir-db + engine tests) never already hold `unique_write_lock`, so
        // acquiring it here cannot self-deadlock (`tokio::sync::Mutex` is NOT
        // reentrant).
        //
        // F-70 (#897, P0): acquired via the canonical drain-then-lock path —
        // raise `INDEX2_CREATE`, drain in-flight fast-path writers, THEN take
        // `unique_write_lock` — NOT lock-then-drain (F-57, #883's original
        // order here, which this task found deadlocks against
        // `pre_commit_prelock`'s drain-guard-then-lock shape). See
        // `TableManager::begin_write_barrier` and
        // `writer_drain_barrier`'s "F-70 — THE canonical lock-order
        // hierarchy" doc section for the full derivation. RAII: `_barrier`
        // clears the bit on EVERY exit path (including the `?` early-returns
        // in the backend-build match and the backfill), so a failed create
        // never leaves writers stuck on the barrier forever.
        let (_barrier, _uwl_guard) = self
            .begin_write_barrier(crate::index::write_barrier_flags::INDEX2_CREATE)
            .await;

        // R0-C (#1010): cross-family name-uniqueness preflight, done WHILE
        // holding `ddl_admission` (via the barrier above) so no other
        // family's CREATE can interleave between this check and this
        // method's eventual registration — see `any_index_exists`'s doc for
        // why the admission-guarded window (not a handler-layer check
        // before admission) is what closes the TOCTOU gap.
        if self.any_index_exists(&op.create_index).await {
            return Err(shamir_storage::error::DbError::KeyExists(format!(
                "index '{}' already exists on this table (possibly in a different \
                 index family — names are unique per table across all families)",
                op.create_index
            )));
        }

        let interner = self.interner.get().await?;
        let mut interned_paths: SmallVec<[Vec<u64>; 2]> = SmallVec::new();
        for field_path in &op.fields {
            let mut seg_ids = Vec::with_capacity(field_path.len());
            for seg in field_path {
                let key = match interner
                    .touch_ind(seg)
                    .map_err(|e| shamir_storage::error::DbError::Internal(e.to_string()))?
                {
                    TouchInd::Exists(k) | TouchInd::New(k) => k.id(),
                };
                seg_ids.push(key);
            }
            interned_paths.push(seg_ids);
        }

        let id = self.index2_registry.allocate_id();

        let name_key = match interner
            .touch_ind(&op.create_index)
            .map_err(|e| shamir_storage::error::DbError::Internal(e.to_string()))?
        {
            TouchInd::Exists(k) | TouchInd::New(k) => k.id(),
        };

        // #1003: mark THIS index's name as in-flight for the rest of the
        // method body — see `create_index`'s matching guard +
        // `in_flight_create_guard`'s module doc. Installed AFTER the "btree"
        // early-return above (that branch delegates to
        // `create_index`/`create_unique_index`, which install their OWN
        // guard keyed on the SAME name — installing one here too would just
        // be a harmless refcount bump on an identity already covered, but
        // this call site never reaches here for a btree index anyway).
        let _in_flight = self.in_flight_creates.enter(name_key);

        let first_path = interned_paths.first().cloned().unwrap_or_default();

        let (_kind, backend): (IndexKind, Arc<dyn IndexBackend>) = match index_type {
            "fts" => {
                // DSL names for fts_tokenizer:
                //   "whitespace"          → plain whitespace split
                //   "unicode"             → unicode-aware split
                //   "stemmed_<lang>"      → Full { <lang>, stopwords=true, stem=true }
                //   "ngram"               → Ngram { n: 3 } (default trigram)
                //   "ngram2".."ngram9"    → Ngram { n: <digit> }
                let tok = crate::table::table_manager::fts_tokenizer_from_dsl(
                    op.fts_tokenizer.as_deref(),
                );
                let kind = IndexKind::Fts {
                    tokenizer: tok,
                    language: op.fts_language.clone(),
                };
                // F-50 Step 3b: construct with state=Building so the first
                // `save_index2_metadata_with_pending` (after this match)
                // persists a durable crash-restart marker BEFORE the backfill
                // runs. Flipped to Ready via `set_state` after the backfill.
                let mut desc = IndexDescriptor::new(
                    id,
                    &op.create_index,
                    name_key,
                    interned_paths.clone(),
                    kind.clone(),
                );
                desc.state = crate::index2::state::IndexState::Building;
                let backend: Arc<dyn IndexBackend> =
                    Arc::new(crate::index2::fts_ranked_backend::FtsRankedBackend::new(
                        desc,
                        first_path,
                        Arc::clone(self.info_store()),
                    ));
                (kind, backend)
            }
            "functional" => {
                let expr_op = op.functional_op.as_deref().unwrap_or("lower");
                let base = crate::index2::expr::IndexExpr::Field(first_path.clone());
                let expr = match expr_op {
                    "lower" => crate::index2::expr::IndexExpr::Lower(Box::new(base)),
                    "upper" => crate::index2::expr::IndexExpr::Upper(Box::new(base)),
                    "trim" => crate::index2::expr::IndexExpr::Trim(Box::new(base)),
                    "length" => crate::index2::expr::IndexExpr::Length(Box::new(base)),
                    user_scalar_name => {
                        // User-registered scalar: check the ScalarResolver for
                        // a trusted_pure vouch. Non-vouched scalars are rejected
                        // from the functional-index path (index-safety gate).
                        let resolver = self.scalar_resolver.load_full();
                        let entry = resolver.get(user_scalar_name).ok_or_else(|| {
                            shamir_storage::error::DbError::Internal(format!(
                                "functional_op '{user_scalar_name}' is not a known built-in or registered scalar"
                            ))
                        })?;
                        if !entry.is_indexable() {
                            return Err(shamir_storage::error::DbError::Internal(format!(
                                "scalar '{user_scalar_name}' is not trusted_pure — cannot back a functional index. \
                                 Call .trusted_pure() on the FnEntry when registering to vouch it is pure + deterministic."
                            )));
                        }
                        crate::index2::expr::IndexExpr::Scalar {
                            name: user_scalar_name.to_string(),
                            inner: Box::new(base),
                        }
                    }
                };
                let kind = IndexKind::Functional(Box::new(FunctionalConfig { expr: expr.clone() }));
                // F-50 Step 3b: state=Building (see the fts arm's matching
                // comment) — persisted by the first save before backfill,
                // flipped to Ready after.
                let mut desc = IndexDescriptor::new(
                    id,
                    &op.create_index,
                    name_key,
                    interned_paths.clone(),
                    kind.clone(),
                );
                desc.state = crate::index2::state::IndexState::Building;
                let backend: Arc<dyn IndexBackend> =
                    if matches!(expr, crate::index2::expr::IndexExpr::Scalar { .. }) {
                        Arc::new(
                            crate::index2::functional_backend::FunctionalBackend::with_resolver(
                                desc,
                                expr,
                                Arc::clone(self.info_store()),
                                self.scalar_resolver.load_full().as_ref().clone(),
                            ),
                        )
                    } else {
                        Arc::new(crate::index2::functional_backend::FunctionalBackend::new(
                            desc,
                            expr,
                            Arc::clone(self.info_store()),
                        ))
                    };
                (kind, backend)
            }
            "vector" => {
                // VR-10 (#432) — one vector index per table.
                //
                // `TxContext::staged_vectors` is keyed by the TABLE token (not
                // per-index), and `promote_vectors` / `apply_vector_batch`
                // (`commit_phases.rs`) fan the SAME batch out to every vector
                // backend on the table. A second vector index with a different
                // `dim` therefore hits `DimMismatch` and fails the post-commit
                // promote. Until the staging/promote pipeline is reworked to key
                // vectors per-index, the DDL must refuse a second vector index
                // on a table that already has one. (Full multi-vector-index
                // support is tracked in BACKLOG.)
                let has_vector = self
                    .index2_registry
                    .all_backends()
                    .await
                    .iter()
                    .any(|b| matches!(b.descriptor().kind, IndexKind::Vector(_)));
                if has_vector {
                    return Err(shamir_storage::error::DbError::Validation(format!(
                        "table '{}' already has a vector index; only ONE vector index per table \
                         is supported (staged-vector promote is keyed per-table, not per-index — \
                         see docs/guide-docs/guide/06-search.md). Drop the existing vector index first, or \
                         track full multi-vector-index support in BACKLOG.md.",
                        self.name()
                    )));
                }
                let dim = op.vector_dim.unwrap_or(384);
                let metric = match op.vector_metric.as_deref() {
                    Some("l2") => VectorMetric::L2,
                    Some("dot") => VectorMetric::Dot,
                    _ => VectorMetric::Cosine,
                };
                // V5.2 (#411) — opt-in SQ8 quantization. `op.vector_quantization`
                // is a wire string ("sq8"); `None` (old messages, or omitted)
                // → unquantized f32 path, bit-for-bit identical to pre-#411.
                let quantization = op
                    .vector_quantization
                    .as_deref()
                    .and_then(VectorQuantization::from_dsl);
                let kind = IndexKind::Vector(Box::new(VectorConfig {
                    dim,
                    metric,
                    backend: VectorBackendRef::InProcessHnsw {
                        ef_construct: 200,
                        m: 16,
                    },
                    quantization,
                }));
                // F-50 Step 3b: state=Building (see the fts arm's matching
                // comment) — persisted by the first save before backfill,
                // flipped to Ready after.
                let mut desc = IndexDescriptor::new(
                    id,
                    &op.create_index,
                    name_key,
                    interned_paths.clone(),
                    kind.clone(),
                );
                desc.state = crate::index2::state::IndexState::Building;
                let adapter = Arc::new(
                    crate::index2::vector::hnsw_adapter::HnswAdapter::new_with_quantization(
                        dim,
                        metric,
                        crate::index2::vector::hnsw_adapter::HnswConfig {
                            max_elements: 100_000,
                            m: 16,
                            ef_construction: 200,
                            ef_search: 50,
                            ..Default::default()
                        },
                        quantization,
                    ),
                );
                let backend: Arc<dyn IndexBackend> = Arc::new(
                    crate::index2::vector::VectorBackend::new(desc, first_path, adapter),
                );
                (kind, backend)
            }
            _ => {
                return Err(shamir_storage::error::DbError::Internal(format!(
                    "unknown index_type: {index_type}"
                )))
            }
        };

        // #534 finding 2 (crash-orphan-id-reuse) + F-50 Step 3b
        // (crash-restart marker). `allocate_id()` is a plain in-memory
        // `AtomicU32::fetch_add` with NO durability, and the backend's
        // descriptor is not yet in the live registry (the backfill runs
        // before register — see `backfill_index2_backend`'s doc comment).
        // This persist runs BEFORE the backfill to:
        //  (a) durably advance the `next_id` watermark past the reserved id
        //      so a crash can never reallocate it to a different index
        //      definition (#534 finding 2); AND
        //  (b) make the in-flight `Building` descriptor visible on disk so a
        //      restart between this point and the final `Ready` persist can
        //      DETECT the interrupted build and self-heal (F-50 Step 3b's
        //      table-open restart-from-scratch — the `Building` state is the
        //      durable marker that distinguishes "interrupted build" from
        //      "never attempted").
        // The `pending` arg passes the descriptor WITHOUT inserting the
        // backend into the live registry — preserving the backfill-before-
        // register invariant the live `index2_on_insert` hook relies on.
        crate::index2::persistence::save_index2_metadata_with_pending(
            &self.index2_registry,
            &self.info_store,
            Some(backend.descriptor().clone()),
        )
        .await?;

        // Backfill the new backend from records that already exist in the
        // table BEFORE it was registered. Without this, a functional / fts /
        // vector index created on a non-empty table silently omits every
        // pre-existing row: only rows written AFTER the index exists get a
        // posting via the `index2_on_insert` write-hook. (The base_index btree
        // path backfills via `create_index` → `create_index_from_records`;
        // the index2 pipeline had no equivalent, so `create_index_v2` was the
        // divergence — a query using the index would then miss every row that
        // predated it.) We backfill the single new `backend` here rather than
        // re-running `bulk_populate_index2` over ALL backends: postings are
        // idempotent, but re-touching already-populated backends is needless
        // O(N·backends) work on every CREATE INDEX.
        self.backfill_index2_backend(backend.as_ref())
            .await
            .map_err(|e| {
                shamir_storage::error::DbError::Internal(format!(
                    "CREATE INDEX '{}': the index was durably persisted as \
                     Building, but the backfill failed: {e}. The index is NOT \
                     queryable — it remains permanently Building until rebuilt. \
                     On restart, the table-open self-heal will detect the \
                     Building state and re-backfill. Call TableManager::verify() \
                     to confirm state, or TableManager::repair() to rebuild it.",
                    op.create_index
                ))
            })?;

        // #534 finding-1 regression hook: park here (backfill done, backend NOT
        // yet registered) if a test installed a pause hook. Zero cost in the
        // real path (`None`), compiled out of non-test builds. See
        // `index2_backfill_hook`.
        #[cfg(test)]
        if let Some(hook) = self.create_index2_backfill_hook.load_full() {
            hook.wait_at_window().await;
        }

        self.index2_registry.insert(backend).await.map_err(|e| {
            shamir_storage::error::DbError::Internal(format!(
                "CREATE INDEX '{}': the index was durably persisted as \
                     Building and the backfill completed, but registering the \
                     backend in the live registry failed: {e}. The index is \
                     NOT queryable in THIS process — on restart, the table-open \
                     self-heal will detect the Building state and re-backfill. \
                     Call TableManager::verify() to confirm state, or \
                     TableManager::repair() to rebuild it.",
                op.create_index
            ))
        })?;

        // #1003 test-only pause point: park here (backend now LIVE in the
        // registry, still `Building`) if a test installed the hook. This is
        // the narrow window where `degraded_index_count()` can actually see
        // THIS index as non-`Ready` — the pre-existing
        // `create_index2_backfill_hook` above parks strictly before this
        // point, where the raw tally can't see the index at all regardless
        // of the in-flight-set fix. Zero cost in the real path (`None`),
        // compiled out of non-test builds.
        #[cfg(test)]
        if let Some(hook) = self.index2_registered_before_ready_hook.load_full() {
            hook.wait_at_window().await;
        }

        // F-50 Step 3b: the backfill completed and the backend is now
        // registered, so flip its authoritative lifecycle state from
        // `Building` (set at descriptor construction above and captured into
        // the registry tuple by `insert`) to `Ready`. The FINAL
        // `save_index2_metadata` below reads `all_descriptors()`, which
        // merges the tuple's state into the cloned descriptor — so the
        // persisted blob now carries `Ready`, atomically replacing the
        // `Building` marker from the first save. A crash before this point
        // leaves `Building` on disk → the table-open self-heal re-backfills.
        self.index2_registry
            .set_state(id, crate::index2::state::IndexState::Ready)
            .await;

        crate::index2::persistence::save_index2_metadata(&self.index2_registry, &self.info_store)
            .await
            .map_err(|e| {
                shamir_storage::error::DbError::Internal(format!(
                    "CREATE INDEX '{}': the backfill completed and the index was \
                     flipped to Ready in memory, but the final durable persist of \
                     the Ready state failed: {e}. The index is queryable in THIS \
                     process but durably Building on disk — on restart, the \
                     table-open self-heal will detect the Building state and \
                     re-backfill. Call TableManager::verify() to confirm state, \
                     or TableManager::repair() to rebuild it.",
                    op.create_index
                ))
            })?;

        Ok(())
    }

    /// Populate a single freshly-created index2 backend from the records that
    /// already exist in this table, by streaming the current record set and
    /// running `plan_insert` + `apply_index_ops` for each row.
    ///
    /// This is the index2 analogue of the base_index btree
    /// `create_index_from_records` backfill. It is scoped to ONE backend (the
    /// one just created) so a CREATE INDEX on a table that already carries
    /// other index2 backends does not needlessly re-touch them.
    ///
    /// Called from BOTH `create_index_v2` (the normal CREATE INDEX path) AND
    /// the F-50 Step 3b table-open self-healing restart-from-scratch
    /// (`TableManager::create`'s `Building`-descriptor recovery loop).
    ///
    /// Runs BEFORE the backend is registered, which avoids a double-write
    /// (the live `index2_on_insert` hook can't yet route to an unregistered
    /// backend).
    ///
    /// # Lost-write race — CLOSED for the non-tx write path (#534), PARTIALLY
    /// # closed on the tx-commit path (#538 Part A), Part B still OPEN
    ///
    /// The caller (`create_index_v2`) holds `unique_write_lock` AND has set
    /// the `INDEX2_CREATE` bit across this backfill → the subsequent
    /// `index2_registry.insert`. While that bit is up, every **non-tx**
    /// writer path (`TableManager::insert`/`insert_many_returning_version`/
    /// `delete_returning_version`/`set`, gated by `needs_write_barrier()`)
    /// also takes `unique_write_lock`, so no row reaching the store through
    /// those methods can land in the window between this backfill's stream
    /// cursor passing a key and the backend becoming live-registered.
    ///
    /// **#534 `@fl` adversarial pass** found the tx-commit path (the primary
    /// production DML route) was NOT covered at all. **#538** closes part of
    /// that gap and leaves the harder part honestly open:
    ///
    /// 1. **Part A — CLOSED.** Every client INSERT/UPDATE/DELETE/SET runs
    ///    through an implicit or interactive tx (`execute_insert_tx` et al.
    ///    in `write_exec.rs` → `insert_tx_many_bytes`/`update_tx_bytes`/
    ///    `delete_tx` in `table_manager_tx_ops.rs` for STAGING → the commit
    ///    pipeline's Phase 2.5 prelock / Phase 5a-5c for MATERIALIZE). The
    ///    commit-time prelock (`pre_commit.rs`'s `pre_commit_prelock`, Phase
    ///    2.5) originally took `unique_write_lock` only for tables in
    ///    `tx.unique_guards` (tables with a base_index UNIQUE index) — an
    ///    index2-only table under an in-flight `create_index_v2` contributed
    ///    nothing, so that tx's commit could freely interleave with this
    ///    backfill. #538 Part A extends Phase 2.5 to ALSO acquire
    ///    `unique_write_lock` for every table the tx wrote to
    ///    (`tx.write_set` keys) whose `needs_write_barrier()` is `true` — so
    ///    a tx's COMMIT now serializes against this backfill exactly like
    ///    the non-tx writers do, via the same lock held for this whole
    ///    backfill → register sequence.
    /// 2. **Part B — CLOSED for the functional/INSERT case by F-50 (#869
    ///    spike); full closure (vector/fts/update/delete/sorted-index) is
    ///    Step 2.** Closing Part A only fixed the commit's TIMING; it does
    ///    not fix WHAT gets committed. The index2 ops-PLAN
    ///    (`tx.index_write_set`) is built at STAGE time (inside
    ///    `insert_tx_many_bytes` etc.) against an `all_backends()` snapshot
    ///    taken well before Phase 2.5 ever runs — in the worst case, before
    ///    this `create_index_v2` even started. If a tx stages before this
    ///    backend exists (in any form) and then commits after this backfill
    ///    has already completed and registered it, that tx's row IS correctly
    ///    serialized into the store by Part A's lock — but its ops-plan was
    ///    built with zero ops for this backend, so Phase 5c has nothing to
    ///    write for it. This was a GUARANTEED miss (not a rare race),
    ///    independent of Part A. F-50 closes it by re-deriving the ops-plan
    ///    against the LIVE backend set at commit time (in `pre_commit_prelock`
    ///    Phase 2.7, gated by `IndexRegistry`'s generation counter, BEFORE the
    ///    WAL entry is built so recovery replays the fresh plan) — see the
    ///    decision memo `docs/dev-artifacts/research/f50-index-lifecycle-spike.md`
    ///    for the full design + the Step 2 scope (the remaining backend kinds
    ///    and the sorted-index residual).
    /// 3. **Check-then-act, not a drain (pre-existing #534 residual,
    ///    unaffected by #538).** A writer/committer that observes
    ///    `needs_write_barrier() == false` (before `create_index_v2` has set
    ///    the flag) proceeds fully lock-free; nothing here waits for such an
    ///    already-in-flight writer to finish before the backfill takes its
    ///    snapshot. This narrows the original whole-backfill-duration race
    ///    down to the duration of one writer's already-in-flight
    ///    validate+write step — a real improvement, not a full close.
    ///
    /// Net effect: #534 was strictly better than the pre-#534 state, and
    /// fully correct for the non-tx/replication-apply write path. #538 Part A
    /// closes the commit-time serialization gap on the tx-commit path. F-50
    /// (#869 spike) closes Part B's stage-time ops-plan staleness for the
    /// functional/INSERT case by re-deriving ops in `pre_commit_prelock`
    /// Phase 2.7 — see
    /// `crate::table::tests::index2_create_barrier_tests::stage_and_commit_inside_window_now_indexes_new_index_part_b_closed`
    /// for the regression test confirming the row is now indexed. The full
    /// closure (vector/fts/update/delete + sorted-index + crash/restart
    /// continuation) is Step 2 / Step 3 per the F-50 memo — do not read this
    /// comment as "the tx-commit-path lost-write race is fully closed for all
    /// backend kinds" without that qualification.
    pub(super) async fn backfill_index2_backend(
        &self,
        backend: &dyn crate::index2::backend::IndexBackend,
    ) -> DbResult<()> {
        let stream = self.list_stream(1000);
        futures::pin_mut!(stream);
        while let Some(batch_result) = stream.next().await {
            let batch = batch_result?;
            for (rid, cow) in batch {
                let val = cow.into_inner()?;
                let ops = backend
                    .plan_insert(rid, &val)
                    .await
                    .map_err(|e| shamir_storage::error::DbError::Internal(e.to_string()))?;
                crate::index2::apply_index_ops(&ops, &self.info_store, backend)
                    .await
                    .map_err(|e| shamir_storage::error::DbError::Internal(e.to_string()))?;
            }
        }
        Ok(())
    }

    /// Create a regular index on specified paths.
    ///
    /// # Concurrency (F-57, #883)
    ///
    /// Holds `unique_write_lock` across the ENTIRE snapshot→backfill→register
    /// sequence and raises the `REGULAR_INDEX_CREATE` bit so
    /// [`needs_write_barrier`](Self::needs_write_barrier) returns `true` for
    /// every writer path — closing the lost-write race where a row written
    /// between the backfill snapshot and registration is seen by NEITHER the
    /// backfill NOR the live `index_manager` write hook. Pre-F-57 this path had
    /// ZERO protection: `needs_write_barrier()` never considered regular
    /// indexes at all. The drain (via
    /// [`begin_write_barrier`](Self::begin_write_barrier), BEFORE the lock —
    /// F-70, #897) closes the check-then-act gap for a writer that read
    /// `false` a moment before the flag went up — same pattern as
    /// `create_index_v2`.
    pub async fn create_index(&self, name: &str, paths: &[&str]) -> DbResult<()> {
        let mut index_def = self.build_index_definition(name, paths).await?;
        // #1003: mark THIS index's name as in-flight for the rest of the
        // method body (RAII guard, dropped on every exit path including an
        // early `?` return or a panic) so `degraded_index_count()` excludes
        // the `Building` state this create is about to publish below — see
        // `in_flight_create_guard`'s module doc for the false-positive this
        // closes (and why it's a per-identity set, not a scalar count).
        let _in_flight = self.in_flight_creates.enter(index_def.name_interned);
        // F-72 (#899, P0): register at `Building` — `IndexManager::
        // create_index_from_records` persists this marker BEFORE the
        // backfill runs and flips it to `Ready` (with its own durable
        // persist) only once the backfill fully completes. See that
        // method's doc for the full publish/backfill/flip sequence this
        // closes (a concurrent planner read must never observe a
        // half-populated index).
        index_def.state = crate::index2::state::IndexState::Building;
        // F-70 (#897, P0): canonical drain-then-lock acquisition — raise
        // `REGULAR_INDEX_CREATE`, drain in-flight fast-path writers, THEN
        // take `unique_write_lock`. F-57 (#883) originally acquired the lock
        // FIRST here, which this task found deadlocks against
        // `pre_commit_prelock`'s drain-guard-then-lock shape on a second
        // table. See `TableManager::begin_write_barrier` and
        // `writer_drain_barrier`'s "F-70" doc section.
        let (_barrier, _uwl_guard) = self
            .begin_write_barrier(crate::index::write_barrier_flags::REGULAR_INDEX_CREATE)
            .await;
        // R0-C (#1010): cross-family name-uniqueness preflight, done WHILE
        // holding `ddl_admission` (via the barrier above) — see
        // `any_index_exists`'s doc and `create_index_v2`'s matching check for
        // why the admission-guarded window (not a handler-layer check before
        // admission) is what closes the TOCTOU gap. `index_exists` (this
        // family's own occupancy) was already implicitly enforced by
        // `IndexManager::create_index_from_stream`'s own registration; this
        // additionally rejects a name already used by ANY OTHER family.
        if self.any_index_exists(name).await {
            return Err(shamir_storage::error::DbError::KeyExists(format!(
                "index '{name}' already exists on this table (possibly in a different \
                 index family — names are unique per table across all families)"
            )));
        }
        // F-42 (#850): persist the interner's newly-touched ids BEFORE the
        // index goes live — a persist failure must abort BEFORE publish, not
        // after. `build_index_definition` already interned the index NAME and
        // every field-path segment in-memory (`intern_string`/`intern_path` →
        // `touch_ind`); the ONLY thing deferred to this point is the DURABLE
        // flush of those already-assigned ids, so moving it ahead of the
        // streaming backfill below is a pure ordering change with no
        // functional side effect. Confirmed by reading the actual signatures:
        // `create_index_from_stream(index_def, stream)` consumes the
        // already-built `IndexDefinition` (whose `name_interned` u64 was set
        // in-memory by `build_index_definition`) and a stream built
        // independently from `list_stream` — neither depends on the interner
        // having been durably persisted. Pre-F-42 a persist failure here
        // returned `Err` but left the just-registered index LIVE in
        // `index_manager` — a live index whose interner ids may not survive a
        // restart, the exact F-33 corruption class reopened at the failure
        // path. Reordering means a persist failure aborts before any publish,
        // so no rollback is needed.
        self.interner.persist().await?;
        // F-78 (#905): stream the backfill instead of materializing the WHOLE
        // table into a `Vec<(RecordId, InnerValue)>`. `list_stream` already
        // yields batches in O(batch) memory; we adapt each `RecordCow` to a
        // decoded `InnerValue` here (the same decode `collect_all_current_records`
        // did, but per-batch instead of for the whole table) and hand the
        // stream to `IndexManager::create_index_from_stream`, which batch-
        // writes postings via `set_many` per batch (one transactional commit
        // per batch) instead of one giant `set_many` at the end. Peak memory
        // drops from O(table) to O(batch).
        //
        // `collect_all_current_records` is intentionally LEFT in place — it
        // still has other callers (`doctor::repair()` and two parity tests).
        //
        // Write-delta catch-up is already FREE: the barrier+lock above
        // (`begin_write_barrier(REGULAR_INDEX_CREATE)`) serializes every
        // concurrent writer for the whole create (in both the old materialize
        // shape and this streaming shape), and Phase 1's register-at-Building
        // (inside `create_index_from_stream`) activates the live write-hook so
        // a row written at the registration boundary gets its posting
        // maintained by that hook — the SAME mechanism the old path relied on.
        // Streaming adds no new lost-write window and needs no new mechanism.
        let stream = self.list_stream(1000).map(|batch| {
            batch.and_then(|rows| {
                rows.into_iter()
                    .map(|(id, cow)| cow.into_inner().map(|v| (id, v)))
                    .collect()
            })
        });
        self.index_manager
            .create_index_from_stream(index_def, stream)
            .await
    }

    /// Create a unique index on specified paths.
    ///
    /// # Concurrency (audit A9 — write-barrier for unique CREATE)
    ///
    /// Holds the table-wide `unique_write_lock` across the ENTIRE
    /// snapshot→backfill→register sequence. This closes the
    /// duplicate-slip-through window at its root: while the unique index
    /// is between "not yet registered" and "registered", no writer can
    /// insert ANY row (let alone a duplicate). Unique-index uniqueness
    /// validation during backfill is NOT safely idempotent-double-writable
    /// (a duplicate is a correctness violation, not a harmless double-write),
    /// so the write-barrier (Option B) is used instead of the register-first
    /// approach (Option A) applied to regular indexes.
    ///
    /// # Errors
    /// Returns `DbError::UniqueIndexCreationFailed` if duplicate values exist.
    pub async fn create_unique_index(&self, name: &str, paths: &[&str]) -> DbResult<()> {
        // #1003: the in-flight guard is installed inside `create_unique_index_body`
        // (right after the name is interned), not here — see that method's
        // matching comment. This wrapper only acquires the barrier+lock.
        // F-70 (#897, P0): canonical drain-then-lock acquisition — raise
        // `UNIQUE_INDEX_CREATE`, drain in-flight fast-path writers, THEN take
        // `unique_write_lock`. F-57 (#883) originally took the lock FIRST
        // (here, in the caller), with the flag+drain inside
        // `create_unique_index_locked` — this task found that order
        // deadlocks against `pre_commit_prelock`'s drain-guard-then-lock
        // shape on a second table. See `TableManager::begin_write_barrier`
        // and `writer_drain_barrier`'s "F-70" doc section. This is the SAME
        // lock non-tx writers (`insert`) and the tx commit pipeline
        // (Phase 2.5) acquire, so it unifies DDL against all writer classes.
        // Tables without unique indexes pay this harmlessly (no contention);
        // this is a low-frequency DDL operation.
        let (_barrier, _uwl_guard) = self
            .begin_write_barrier(crate::index::write_barrier_flags::UNIQUE_INDEX_CREATE)
            .await;
        self.create_unique_index_body(name, paths).await
    }

    /// Inner unique-index create body: snapshot→backfill→register, with NO
    /// flag/lock acquisition of its own. ASSUMES the caller already holds
    /// BOTH `unique_write_lock` AND the `UNIQUE_INDEX_CREATE` barrier bit
    /// (via [`begin_write_barrier`](Self::begin_write_barrier)) for the
    /// caller's own required span. Used by:
    ///   - [`create_unique_index`](Self::create_unique_index) (acquires
    ///     barrier+lock itself, immediately above, then calls this body), and
    ///   - [`rename_index`](Self::rename_index)'s unique-index branch, which
    ///     acquires the SAME barrier+lock ONCE and holds it across the entire
    ///     drop→create span (a DIFFERENT requirement — uniqueness-gap
    ///     atomicity across drop+create, audit A9 — not drain ordering; see
    ///     that call site).
    ///
    /// # Concurrency (F-57, #883; reordered by F-70, #897)
    ///
    /// The FIRST unique index on a table is the critical case:
    /// `has_unique_indexes()` (and therefore `UNIQUE_INDEX_EXISTS`) is
    /// `false` until this create registers it, so without the caller's
    /// `UNIQUE_INDEX_CREATE` bit + drain, every concurrent fast-path writer
    /// would bypass `unique_write_lock` entirely — the exact race the review
    /// flagged. The bit + drain (done by the caller, BEFORE the lock — F-70)
    /// make the FIRST unique-index create as safe as the second-and-later.
    async fn create_unique_index_body(&self, name: &str, paths: &[&str]) -> DbResult<()> {
        // R0-C (#1010): cross-family name-uniqueness preflight, done WHILE
        // holding `ddl_admission` — both callers (`create_unique_index` and
        // `rename_index`'s unique branch) already acquired the barrier
        // before reaching this body (see this method's doc). See
        // `any_index_exists`'s doc and `create_index_v2`'s matching check
        // for why the admission-guarded window is what closes the TOCTOU
        // gap. `unique_index_exists` (this family's own occupancy) is
        // enforced separately by `IndexManager`'s own registration; this
        // additionally rejects a name already used by ANY OTHER family.
        if self.any_index_exists(name).await {
            return Err(shamir_storage::error::DbError::KeyExists(format!(
                "index '{name}' already exists on this table (possibly in a different \
                 index family — names are unique per table across all families)"
            )));
        }
        let index_def = self.build_index_definition(name, paths).await?;
        // #1003: mark THIS index's name as in-flight for the rest of the
        // method body — see `create_index`'s matching guard +
        // `in_flight_create_guard`'s module doc. The unique family's real-path
        // definition is never observably `Building` today (see
        // `create_unique_index_from_records`'s doc) — since it's never
        // iterated over as non-`Ready` during its own backfill in the first
        // place, this guard is a no-op for THIS family's own count, but
        // (being per-identity, not a scalar) it can never bleed into an
        // unrelated index's count either. Installed uniformly across all four
        // families, and covers a future/crash-recovery path that DOES
        // register at `Building`. Also covers `rename_index`'s unique branch,
        // which calls this body directly.
        let _in_flight = self.in_flight_creates.enter(index_def.name_interned);
        // F-42 (#850) — see `create_index`'s matching comment: durably
        // persist the index-name/field-path ids BEFORE registering the
        // unique index. A persist failure aborts before publish, so no
        // rollback is needed (nothing was published yet). Signature check:
        // `create_unique_index_from_records(index_def, records)` consumes
        // the already-built `IndexDefinition` and a records Vec collected
        // independently from `list_stream` — neither depends on the
        // interner having been durably persisted.
        self.interner.persist().await?;
        // Always use the seam: collect_all_current_records routes
        // attached→log / unattached→data_store, so it is correct for
        // both cases. Collected UNDER the lock (held by caller) so no
        // writer can interpose.
        //
        // F-78 (#905) — DEFERRED for the unique family: unlike the regular
        // `create_index` path (which now streams via
        // `create_index_from_stream`), the unique path still materializes the
        // whole table here because duplicate detection needs global knowledge.
        // See `create_unique_index_from_records`'s F-78 doc for the full
        // rationale + the bounded-memory approaches tracked as follow-up. The
        // regular-hash streaming fix landed fully tested + benchmarked; this
        // unique-family gap is the documented escape-hatch deferral.
        //
        // P1-4 (#969): the unique family has no per-batch progress point (it
        // materializes-then-writes in one shot), so at minimum log start and
        // completion of the table scan so an operator can see the DDL is
        // progressing.
        log::info!(
            "CREATE UNIQUE INDEX '{}': starting backfill (scanning whole table — \
             unique family materializes all rows, no streaming)",
            index_def.name_interned
        );
        let backfill_start = std::time::Instant::now();
        let records = self.collect_all_current_records().await?;
        log::info!(
            "CREATE UNIQUE INDEX '{}': scanned {} records in {:.1}s, \
             writing unique index...",
            index_def.name_interned,
            records.len(),
            backfill_start.elapsed().as_secs_f64()
        );
        self.index_manager
            .create_unique_index_from_records(index_def, records)
            .await
    }

    /// Drop a regular index by name.
    ///
    /// P0-3 (#957/#959): wrapped in `begin_write_barrier(REGULAR_INDEX_CREATE)`
    /// — same drain-then-lock pattern as `create_index`. This serializes DROP
    /// against concurrent WRITERS (an in-flight fast-path writer that read
    /// `needs_write_barrier() == false` before the bit went up is drained
    /// before the sweep begins). It does NOT fully close the in-flight
    /// READER race (sub-bug 3a — a reader holding an `Arc` snapshot of the
    /// old definition can still observe a partially-swept keyspace); that
    /// residual is documented on `IndexManager::drop_index`'s method doc.
    ///
    /// # Returns
    /// `true` if index existed and was removed, `false` if not found.
    ///
    /// #1051: accepts `op_id` minted at dispatch time for crash recovery status writes.
    pub async fn drop_index(&self, name: &str, op_id: Option<RecordId>) -> DbResult<bool> {
        let name_id = self.intern_string(name).await?;
        // P0-3 (#959): drain-then-lock — raise the REGULAR_INDEX_CREATE bit,
        // drain in-flight fast-path writers, then take `unique_write_lock`.
        // See `create_index`'s matching acquisition for the F-70 ordering
        // rationale.
        let (_barrier, _uwl_guard) = self
            .begin_write_barrier(crate::index::write_barrier_flags::REGULAR_INDEX_CREATE)
            .await;
        let op_id_str = op_id.map(|id| id.to_string());
        self.index_manager.drop_index(name_id, op_id_str).await
    }

    /// Drop a unique index by name.
    ///
    /// P0-3 (#957/#959): wrapped in `begin_write_barrier(UNIQUE_INDEX_CREATE)`
    /// — same drain-then-lock pattern as `create_unique_index`. See
    /// `drop_index`'s doc for what this does and does not close.
    ///
    /// # Returns
    /// `true` if index existed and was removed, `false` if not found.
    ///
    /// #1051: accepts `op_id` minted at dispatch time for crash recovery status writes.
    pub async fn drop_unique_index(&self, name: &str, op_id: Option<RecordId>) -> DbResult<bool> {
        let name_id = self.intern_string(name).await?;
        // P0-3 (#959): drain-then-lock — see `drop_index`'s doc.
        let (_barrier, _uwl_guard) = self
            .begin_write_barrier(crate::index::write_barrier_flags::UNIQUE_INDEX_CREATE)
            .await;
        let op_id_str = op_id.map(|id| id.to_string());
        self.index_manager
            .drop_unique_index(name_id, op_id_str)
            .await
    }

    /// Drop an index2 backend (`fts` / `functional` / `vector`) by name.
    ///
    /// This is the standalone `DROP INDEX` counterpart to
    /// [`create_index_v2`](Self::create_index_v2). Unlike the
    /// `DROP TABLE ... CASCADE` path (which can skip persistence + posting
    /// cleanup because the whole table dies with the index), a standalone
    /// drop must do both so the surviving table never observes a stale
    /// descriptor or orphan postings.
    ///
    /// # Crash-safety sequence (P0-3b / #988 — durable tombstone)
    ///
    /// The order tombstone → retire → sweep → persist → clear-tombstone
    /// closes the crash-resurrection gap that the retire → sweep → persist
    /// order had (a crash between sweep and persist left the on-disk
    /// `PersistedIndexes` still listing the index as `Ready` with zero
    /// postings — the planner would route queries to a dead index, silently
    /// returning empty/missing results). This mirrors the durable-tombstone
    /// pattern from the base_index regular/unique family (#959) and the
    /// sorted family (#972):
    ///   1. resolve the backend by interned name (`Ok(false)` if absent);
    ///   2. **persist a durable tombstone** (`add_to_dropping_index2`) recording
    ///      that this descriptor id is being dropped — MUST succeed before
    ///      the sweep, so a crash at any later point is recoverable;
    ///   3. `registry.remove_by_id(id)` to retire it from the planner-visible
    ///      live set FIRST (F-76 / #903 — mirror image of F-72's `Building`
    ///      gate for CREATE). Also advances the F-50 generation counter;
    ///   4. `backend.drop_all()` to sweep its (now orphan, planner-invisible)
    ///      posting entries — `backend` is held locally so it survives the
    ///      registry removal;
    ///   5. `save_index2_metadata` to persist the reduced registry — it
    ///      re-derives `PersistedIndexes` from the LIVE registry's
    ///      `all_descriptors()`, so calling it AFTER `remove_by_id`
    ///      naturally persists the removal;
    ///   6. **clear the tombstone** (`clear_from_dropping_index2`) — the
    ///      reduced metadata is now durable, so the tombstone is no longer
    ///      needed.
    ///
    /// On restart, [`recover_index2_drops`](Self::recover_index2_drops) in
    /// `TableManager::create` sees any tombstone left by an interrupted drop
    /// and finishes it idempotently (remove backend, sweep postings, persist,
    /// clear tombstone). See that method's crash-state matrix for the full
    /// recovery reasoning.
    ///
    /// F-76 (#903): the OLD order was `drop_all` → `remove_by_id`, which left
    /// a window in which a concurrent reader could still resolve the backend
    /// via `find_by_field_and_kind` (it was still `Ready` in the registry)
    /// while its postings were mid-sweep — silently wrong/incomplete results,
    /// the mirror image of the F-72 CREATE bug. Retiring the registry entry
    /// FIRST closes that window: every NEW reader after the removal resolves
    /// the backend as absent and falls back to a full scan. A reader that
    /// already holds an `Arc<dyn IndexBackend>` snapshot keeps working
    /// against its own consistent view (RCU — the `Arc` keeps the backend and
    /// its already-read postings alive).
    ///
    /// # Returns
    /// `true` if a backend existed and was removed, `false` if no index2
    /// backend is registered under `name`.
    ///
    /// R0-A (#1012): wrapped in `begin_write_barrier(INDEX2_CREATE)` — reuses
    /// the family's existing CREATE bit (mirrors `drop_sorted_index`'s
    /// reasoning: no legitimate case needs a concurrent CREATE/DROP/RENAME
    /// pair racing the SAME index2 backend, and a dedicated DROP bit would
    /// only let an unrelated CREATE proceed unserialized against this drop's
    /// registry mutation). Held across the ENTIRE tombstone → registry
    /// `remove_by_id` → posting-sweep → metadata-persist → tombstone-clear
    /// sequence below — before this fix, NOTHING serialized this method
    /// against a concurrent CREATE/DROP/RENAME on the same table, so two
    /// registry-mutating DDL ops could race `IndexRegistry`'s ticket/
    /// generation bookkeeping (see `IndexRegistry`'s doc for the scenario
    /// this closes).
    ///
    /// #1051: accepts `op_id` minted at dispatch time for crash recovery status writes.
    pub async fn drop_index2(&self, name: &str, op_id: Option<RecordId>) -> DbResult<bool> {
        let interner = self.interner.get().await?;
        // `get_ind` is a pure lookup (does NOT mint a new id), so dropping a
        // name that was never interned cannot pollute the interner.
        let Some(name_key) = interner.get_ind(name) else {
            return Ok(false);
        };
        let (_barrier, _uwl_guard) = self
            .begin_write_barrier(crate::index::write_barrier_flags::INDEX2_CREATE)
            .await;
        let Some(backend) = self.index2_registry.get_by_name(name_key.id()).await else {
            return Ok(false);
        };
        let drop_id = backend.descriptor().id;
        let drop_name = backend.descriptor().name.clone();
        let op_id_str = op_id.map(|id| id.to_string());

        // P0-3b (#988 / #1051): write a durable tombstone BEFORE retiring the backend
        // or sweeping postings. If the process crashes after the sweep but
        // before the reduced metadata is persisted, the on-disk metadata
        // still lists the index — but the tombstone tells
        // `recover_index2_drops` to finish the drop rather than resurrecting
        // a broken "Ready but no postings" index. MUST succeed before
        // proceeding; if the persist fails the on-disk tombstone is unchanged
        // and we propagate `Err` without touching the registry or postings.
        crate::index2::persistence::add_to_dropping_index2(
            drop_id,
            drop_name,
            op_id_str,
            &self.info_store,
        )
        .await
        .map_err(|e| {
            shamir_storage::error::DbError::Internal(format!(
                "DROP INDEX '{name}': failed to persist the durable drop tombstone: {e}. \
                     The backend was NOT retired and postings were NOT swept — the \
                     index is still fully intact. Retry the DROP."
            ))
        })?;

        // P0-3a (#1038) step 2.5: raise the reader-drain gate's intent flag
        // (SeqCst) BEFORE the retire below. From this point every NEW
        // index2 reader acquires a lease that observes the flag and backs off
        // (Err(IndexDrainInProgress) → caller falls back to a full scan).
        // The flag stays up until `drain_guard` is dropped (step 4.5 + RAII
        // safety net on early `?`). See `reader_drain_gate`'s module doc for
        // the memory-model proof and the deadlock-exclusion placement invariant.
        let drain_guard = self.index2_registry.reader_gate().begin_drop();

        // F-76 (#903): retire the backend from the planner-visible registry
        // BEFORE sweeping its postings. See the method doc for the full
        // rationale. `backend` is held locally, so `drop_all` below still
        // runs after this removal.
        self.index2_registry.remove_by_id(drop_id).await;
        // F-76 test seam: park here (backend already retired from the
        // registry, postings not yet swept) if a test installed a pause hook.
        // With the fix, a concurrent read issued while parked here must fall
        // back to a full scan (the backend is gone from the planner). Zero
        // cost on the real path (`None`), compiled out of non-test builds.
        #[cfg(test)]
        if let Some(hook) = self.drop_index2_pause_hook.load_full() {
            hook.wait_at_window().await;
        }

        // P0-3a (#1038) step 3.5: drain every reader that entered
        // `lease_by_field_and_kind` BEFORE the flag went up (the gate's
        // memory model guarantees such a reader is counted in `in_flight`,
        // so this wait terminates — see the livelock-freedom argument in the
        // gate's module doc). AFTER the retire and the pause-hook park, BEFORE
        // the sweep, so the physical sweep cannot start until no reader is
        // holding a lease against the backend.
        drain_guard.wait_for_drain().await;

        // Sweep the (now orphan, planner-invisible) posting entries.
        backend.drop_all().await.map_err(|e| {
            shamir_storage::error::DbError::Internal(format!(
                "DROP INDEX '{name}': a durable drop tombstone was persisted and \
                     the backend was retired from the planner-visible registry, but \
                     the posting sweep failed: {e}. On restart, recovery will resume \
                     the sweep idempotently and finish the drop. Call \
                     TableManager::verify() to confirm state."
            ))
        })?;

        // P0-3a (#1038) step 4.5: the sweep finished — explicitly release the
        // drain guard now so the (unrelated) persist / tombstone-clear steps
        // below run with the gate already clear. RAII (`drain_guard`'s `Drop`
        // clears the flag) remains the safety net for every early `?` return
        // above, but an explicit early drop here keeps the drain window as
        // tight as possible.
        drop(drain_guard);

        // P0-3b (#988) test seam — park here (sweep complete, reduced metadata
        // NOT yet persisted) if a test installed the post-sweep hook. This is
        // the exact crash window sub-bug 3c exercises: a "crash" here (dropping
        // the manager) leaves the tombstone on disk and the old metadata, and
        // the recovery path in `TableManager::create` must finish the drop.
        // NOT `#[cfg(test)]`-gated cost-wise — zero cost (`None`) on real path.
        #[cfg(test)]
        if let Some(hook) = self.drop_index2_post_sweep_hook.load_full() {
            hook.wait_at_window().await;
        }

        // Persist the reduced metadata (backend removed from registry).
        crate::index2::persistence::save_index2_metadata(&self.index2_registry, &self.info_store)
            .await
            .map_err(|e| {
                shamir_storage::error::DbError::Internal(format!(
                    "DROP INDEX '{name}': a durable drop tombstone was persisted, \
                     the backend was retired, and the posting sweep completed, but \
                     persisting the reduced index metadata failed: {e}. On restart, \
                     recovery will finish the drop idempotently. Call \
                     TableManager::verify() to confirm state."
                ))
            })?;

        // P0-3b (#988): clear the tombstone AFTER the reduced metadata is
        // durably persisted. If this fails, the tombstone remains — recovery
        // will just clear it (a no-op on the already-finished drop).
        crate::index2::persistence::clear_from_dropping_index2(drop_id, &self.info_store)
            .await
            .map_err(|e| {
                shamir_storage::error::DbError::Internal(format!(
                    "DROP INDEX '{name}': the drop is essentially complete (tombstone \
                     persisted, backend retired, sweep done, reduced metadata persisted), \
                     but clearing the drop tombstone failed: {e}. On restart, recovery \
                     will clear the tombstone as a no-op. Call TableManager::verify() \
                     to confirm state."
                ))
            })?;
        Ok(true)
    }

    // #1048: write SucceededViaCrashRecovery status for recovered hash DROP operations.
    // This is a public helper called from TableManager::create.
    pub async fn write_hash_drop_recovery_status(
        dropping_regular: &[(u64, Option<String>)],
        dropping_unique: &[(u64, Option<String>)],
        interner: &super::interner_manager::InternerManager,
        info_store: Arc<dyn shamir_storage::types::Store>,
    ) -> Result<(), shamir_storage::error::DbError> {
        // Resolve name_interned back to string names using the interner
        // and write SucceededViaCrashRecovery for each recovered operation.
        let interner_guard = interner.get().await.map_err(|e| {
            shamir_storage::error::DbError::Internal(format!(
                "#1048: failed to get interner for hash DROP recovery status: {e}"
            ))
        })?;

        // Regular family
        for &(name_interned, ref op_id_str) in dropping_regular {
            if let Some(name) = interner_guard.with_str(
                &shamir_types::core::interner::InternerKey::new(name_interned),
                |s| s.to_string(),
            ) {
                // Skip status write if op_id is None (pre-#1051 tombstone or no-op caller)
                if let Some(op_id_str) = op_id_str {
                    // A corrupt op_id string is a best-effort miss on the
                    // status log, not a reason to fail the whole table open
                    // — the actual index recovery (this function's caller)
                    // has already succeeded by the time this runs.
                    let op_id = match std::str::FromStr::from_str(op_id_str) {
                        Ok(id) => id,
                        Err(e) => {
                            log::error!(
                                "#1051: failed to parse op_id '{op_id_str}' for recovered \
                                 hash DROP (regular) '{name}': {e} — skipping status write"
                            );
                            continue;
                        }
                    };
                    let status = DdlOpStatus {
                        op_id,
                        kind: DdlOpKind::DropHashIndex {
                            index_name: name.clone(),
                        },
                        state: DdlOpState::SucceededViaCrashRecovery {
                            completed_at_restart: std::time::SystemTime::now()
                                .duration_since(std::time::UNIX_EPOCH)
                                .unwrap()
                                .as_millis()
                                as u64,
                        },
                    };
                    if let Err(e) =
                        crate::table::ddl_op_log::write_op_status(&info_store, &status).await
                    {
                        log::error!(
                            "#1048: failed to write SucceededViaCrashRecovery status for \
                             recovered hash DROP (regular) '{}': {e}",
                            name
                        );
                    }
                }
            }
        }

        // Unique family
        for &(name_interned, ref op_id_str) in dropping_unique {
            if let Some(name) = interner_guard.with_str(
                &shamir_types::core::interner::InternerKey::new(name_interned),
                |s| s.to_string(),
            ) {
                // Skip status write if op_id is None (pre-#1051 tombstone or no-op caller)
                if let Some(op_id_str) = op_id_str {
                    // See the regular-family arm above for why this is
                    // best-effort (log + skip), not a hard failure.
                    let op_id = match std::str::FromStr::from_str(op_id_str) {
                        Ok(id) => id,
                        Err(e) => {
                            log::error!(
                                "#1051: failed to parse op_id '{op_id_str}' for recovered \
                                 hash DROP (unique) '{name}': {e} — skipping status write"
                            );
                            continue;
                        }
                    };
                    let status = DdlOpStatus {
                        op_id,
                        kind: DdlOpKind::DropUniqueHashIndex {
                            index_name: name.clone(),
                        },
                        state: DdlOpState::SucceededViaCrashRecovery {
                            completed_at_restart: std::time::SystemTime::now()
                                .duration_since(std::time::UNIX_EPOCH)
                                .unwrap()
                                .as_millis()
                                as u64,
                        },
                    };
                    if let Err(e) =
                        crate::table::ddl_op_log::write_op_status(&info_store, &status).await
                    {
                        log::error!(
                            "#1048: failed to write SucceededViaCrashRecovery status for \
                             recovered hash DROP (unique) '{}': {e}",
                            name
                        );
                    }
                }
            }
        }

        Ok(())
    }

    /// P0-3b (#988): open-time recovery for index2 DROP INDEX operations
    /// interrupted by a crash. Called from `TableManager::create` AFTER the
    /// F-50 Step 3b self-heal block (which loaded persisted metadata and
    /// inserted backends into the registry) and BEFORE the `restore_on_open`
    /// loop (so we don't waste time rebuilding a backend recovery removes).
    ///
    /// # Crash-state matrix (mirrors #972's `recover_in_progress_drops`)
    ///
    /// The drop sequence is: tombstone → `remove_by_id` → sweep → persist →
    /// clear-tombstone. On restart, the F-50 Step 3b block has already loaded
    /// persisted metadata and inserted backends, so the registry reflects
    /// whatever was on disk.
    ///
    /// | crash point                              | registry has backend? | postings? | tombstone? | recovery action                       |
    /// |------------------------------------------|----------------------|-----------|------------|---------------------------------------|
    /// | after tombstone-write, before sweep      | yes (from persisted)  | present   | yes        | remove_by_id, sweep, persist, clear   |
    /// | after sweep, before persist              | yes (from persisted)  | gone      | yes        | remove_by_id, sweep (no-op), persist, clear |
    /// | after persist, before clear              | no (not in persisted) | gone      | yes        | sweep (no-op), clear                  |
    ///
    /// In every case the recovery leaves the manager in a consistent state:
    /// the backend is gone from the registry, its postings are swept, the
    /// reduced metadata is persisted, and the tombstone is cleared. The sweep
    /// is idempotent (prefix-scan + remove_many on already-removed keys is a
    /// no-op), so calling recovery twice (two restart attempts) is a clean
    /// no-op on the second call. Mirrors #972's `recover_in_progress_drops`.
    pub(crate) async fn recover_index2_drops(&self) -> DbResult<()> {
        let dropping = crate::index2::persistence::load_dropping_index2(&self.info_store).await?;
        if dropping.is_empty() {
            return Ok(());
        }

        log::info!(
            "P0-3b (#988): recovering {} in-progress index2 DROP(s)",
            dropping.len()
        );

        let mut changed = false;
        for (id, _name, _op_id) in &dropping {
            // If the backend is still in the registry (crash happened before
            // `save_index2_metadata` finalized the removal), retire it now.
            // If it's already gone (crash after persist), this is a no-op.
            if self.index2_registry.remove_by_id(*id).await.is_some() {
                changed = true;
            }
            // Always run the sweep (idempotent). Covers both the "sweep never
            // ran" and "sweep ran but persist failed" cases. The sweep is a
            // 4-byte prefix scan on `id.to_le_bytes()` — no backend Arc needed.
            crate::index2::persistence::sweep_index2_postings_by_id(*id, &self.info_store).await?;
        }
        if changed {
            crate::index2::persistence::save_index2_metadata(
                &self.index2_registry,
                &self.info_store,
            )
            .await?;
        }

        // Clear the entire tombstone (write empty Vec; the load path
        // treats empty-vec and NotFound identically).
        let empty = bincode::serialize(&Vec::<(u32, String, Option<String>)>::new())
            .map_err(|e| shamir_storage::error::DbError::Codec(e.to_string()))?;
        let key = shamir_types::types::record_id::RecordId::system("_m.idx.drop").to_bytes();
        self.info_store
            .set(key.into(), bytes::Bytes::from(empty))
            .await?;

        log::info!(
            "P0-3b (#988): recovery complete — {} index2 DROP(s) finalized",
            dropping.len()
        );

        Ok(())
    }

    // #1048 / #1051: write SucceededViaCrashRecovery status for recovered index2 DROP operations.
    // This is a public helper called from TableManager::create.
    pub async fn write_index2_drop_recovery_status(
        dropping_entries: &[(u32, String, Option<String>)],
        info_store: Arc<dyn shamir_storage::types::Store>,
    ) -> Result<(), shamir_storage::error::DbError> {
        for &(_id, ref name, ref op_id_str) in dropping_entries {
            // Skip status write if op_id is None (pre-#1051 tombstone or no-op caller)
            if let Some(op_id_str) = op_id_str {
                // A corrupt op_id string is a best-effort miss on the
                // status log, not a reason to fail the whole table open —
                // the actual index2 recovery (this function's caller) has
                // already succeeded by the time this runs.
                let op_id = match RecordId::from_str(op_id_str) {
                    Ok(id) => id,
                    Err(e) => {
                        log::error!(
                            "#1051: failed to parse op_id '{op_id_str}' for recovered \
                             index2 DROP '{name}': {e} — skipping status write"
                        );
                        continue;
                    }
                };
                let status = DdlOpStatus {
                    op_id,
                    kind: DdlOpKind::DropIndex2 {
                        index_name: name.clone(),
                    },
                    state: DdlOpState::SucceededViaCrashRecovery {
                        completed_at_restart: std::time::SystemTime::now()
                            .duration_since(std::time::UNIX_EPOCH)
                            .unwrap()
                            .as_millis() as u64,
                    },
                };
                if let Err(e) =
                    crate::table::ddl_op_log::write_op_status(&info_store, &status).await
                {
                    log::error!(
                        "#1048 / #1051: failed to write SucceededViaCrashRecovery status for \
                         recovered index2 DROP '{}' (id={}): {e}",
                        name,
                        _id
                    );
                }
            }
        }

        Ok(())
    }

    /// #997: open-time recovery for RENAME INDEX operations on the
    /// base_index REGULAR and UNIQUE (hash) families interrupted by a
    /// crash. Called from `TableManager::create` AFTER
    /// `recover_index2_drops` (same position in the open sequence as the
    /// index2 drop recovery). Lives here (not on `IndexManager`) because
    /// recovery needs the record stream + interner for a backfill, which
    /// only `TableManager` has access to.
    ///
    /// Unlike the sorted family's RENAME recovery (#962, which re-runs an
    /// idempotent rekey settle loop), a hash rename is a drop+rebuild. The
    /// tombstone payload (`HashRenameTombstone`) carries the resolved
    /// string names + paths recovery needs to rebuild from nothing — this
    /// is ESSENTIAL for the unique path, which drops the OLD definition
    /// FIRST, so by the time a crash can strand the tombstone the old
    /// `IndexDefinition` is already gone from both memory and disk.
    ///
    /// # Crash-state matrix — REGULAR (create-new → drop-old)
    ///
    /// Tombstone written BEFORE `create_index`; cleared AFTER `drop_index`.
    ///
    /// | crash point                              | old present? | new state         | recovery action                                              |
    /// |------------------------------------------|--------------|-------------------|--------------------------------------------------------------|
    /// | after tombstone, before create           | yes          | absent            | create new, drop old, clear tombstone                        |
    /// | during create (Phase 1 done, not Phase 3)| yes          | Building          | drop new (sweep partial), re-create new, drop old, clear     |
    /// | after create, before drop                | yes          | Ready             | drop old, clear tombstone                                    |
    /// | after drop, before clear                 | no           | Ready             | clear tombstone (already done)                               |
    ///
    /// # Crash-state matrix — UNIQUE (drop-old → create-new)
    ///
    /// Tombstone written BEFORE the barrier+drop; cleared AFTER create.
    ///
    /// | crash point                              | old present? | new state         | recovery action                                              |
    /// |------------------------------------------|--------------|-------------------|--------------------------------------------------------------|
    /// | after tombstone, before drop             | yes          | absent            | create new, drop old, clear tombstone                        |
    /// | after drop, before/during create         | no           | absent            | create new (rebuild from paths), clear tombstone (SEVERE)    |
    /// | during create (Phase 1 done, not Phase 3)| no           | Building          | drop new (sweep partial), re-create new, clear tombstone     |
    /// | after create, before clear               | no           | Ready             | clear tombstone (already done)                               |
    ///
    /// # #966 self-heal ownership resolution
    ///
    /// #966's F-50 Step 3b self-heal (in `TableManager::create`, right before
    /// `recover_index2_drops`) is **index2-only**: it iterates
    /// `load_index2_metadata().descriptors` and heals Building backends. The
    /// base_index `IndexManager` family has **NO** automatic Building
    /// self-heal (grep-verified: `IndexManager::new` only loads definitions
    /// via `IndexInfo::decode_bytes`, it does not re-run any backfill — see
    /// the explicit note at `create_index_from_records`'s doc). Therefore:
    ///
    /// - A crashed regular `create_index` that leaves `new` as `Building` is
    ///   **NOT** healed by #966. THIS recovery owns it: it drops the partial
    ///   Building index (sweeping its stale postings) and re-creates fresh.
    /// - A crashed unique `create_unique_index_body` that leaves `new` as
    ///   `Building` is also **NOT** healed by #966. Same fix here.
    /// - No overlap, no uncovered state. The two mechanisms own disjoint
    ///   families (index2 vs base_index).
    ///
    /// # Unique-duplicate-during-recovery hazard
    ///
    /// A recovery-time unique-index rebuild runs a backfill that checks
    /// uniqueness. In theory, writes could have landed while the unique
    /// index didn't exist (the window this bug opens). In practice this is
    /// **impossible** for this specific scenario: recovery runs during
    /// `TableManager::create`, BEFORE any writer can access the table
    /// (writers require a fully-open `TableManager`). The data is exactly as
    /// it was when the original unique index was enforcing uniqueness, so
    /// the backfill's duplicate check passes. If it somehow fails (data
    /// corruption, bug), the error propagates and **fails the table open**
    /// with a clear diagnostic — the table does NOT silently accept
    /// duplicates. This is the safest of the defensible options: "fail the
    /// open with a clear diagnostic" over "leave absent + surface via
    /// doctor", because a silently-absent unique constraint that the
    /// operator isn't warned about is the exact class of bug this task
    /// closes.
    ///
    /// # Idempotence
    ///
    /// Every recovery action is idempotent: a double restart after recovery
    /// finds an empty tombstone (the first recovery cleared it) and is a
    /// clean no-op. The `create_index` / `drop_index` calls re-register /
    /// re-sweep idempotently. Calling recovery twice in a row (without a
    /// restart in between) is also a no-op on the second call because the
    /// first call cleared the tombstone.
    pub(crate) async fn recover_hash_renames(&self) -> DbResult<()> {
        let regular_renames = self.index_manager.load_renaming_list(false).await?;
        let unique_renames = self.index_manager.load_renaming_list(true).await?;

        if regular_renames.is_empty() && unique_renames.is_empty() {
            return Ok(());
        }

        log::info!(
            "#997: recovering {} regular + {} unique in-progress hash RENAME(s)",
            regular_renames.len(),
            unique_renames.len()
        );

        // ── Regular family ──────────────────────────────────────────────
        for entry in &regular_renames {
            let path_refs: Vec<&str> = entry.paths.iter().map(|s| s.as_str()).collect();

            // Step 1: ensure new index is in a clean, Ready state.
            if self.index_exists(&entry.new_name).await {
                let new_id = self.intern_string(&entry.new_name).await?;
                if let Some(def) = self.index_manager.get_index_definition(new_id) {
                    if def.state == crate::index2::state::IndexState::Building {
                        log::warn!(
                            "#997: regular rename target '{}' was left in Building \
                             state by a crashed create — dropping partial and re-creating",
                            entry.new_name
                        );
                        self.drop_index(&entry.new_name, None).await?;
                        self.create_index(&entry.new_name, &path_refs).await?;
                    }
                    // else Ready — already done, leave as-is
                }
            } else {
                // New doesn't exist — crash before/during create. Re-run.
                self.create_index(&entry.new_name, &path_refs).await?;
            }

            // Step 2: drop old if still present.
            if self.index_exists(&entry.old_name).await {
                self.drop_index(&entry.old_name, None).await?;
            }

            // #1000 test seam — park here (this entry fully reconciled, the
            // tombstone list not yet cleared) if a test installed the
            // between-entries pause hook. This is the exact window a
            // regression test uses to prove a not-yet-processed sibling
            // entry's tombstone survives an interruption here.
            self.index_manager
                .maybe_pause_recover_renames_between_entries()
                .await;
        }

        // Step 3: clear the WHOLE regular tombstone list once, after every
        // loaded entry has been reconciled — NOT per-entry via
        // `clear_from_renaming` (see `IndexManager::clear_all_renaming`'s doc
        // comment for why a per-entry clear during recovery would silently
        // discard not-yet-processed entries: `renaming_regular` is empty at
        // open time, so `clear_from_renaming`'s snapshot-from-in-memory-map
        // would persist `[]` after the FIRST entry, stranding any later
        // entry's tombstone if recovery then failed or crashed again).
        if !regular_renames.is_empty() {
            self.index_manager.clear_all_renaming(false).await?;

            // #1048: write SucceededViaCrashRecovery for each recovered regular rename
            // that has an op_id. Skip silently for None (backward compat).
            for entry in &regular_renames {
                if let Some(ref op_id_str) = entry.op_id {
                    if let Ok(op_id) = RecordId::from_str(op_id_str) {
                        let status = DdlOpStatus {
                            op_id,
                            kind: DdlOpKind::RenameHashIndex {
                                old_name: entry.old_name.clone(),
                                new_name: entry.new_name.clone(),
                            },
                            state: DdlOpState::SucceededViaCrashRecovery {
                                completed_at_restart: std::time::SystemTime::now()
                                    .duration_since(std::time::UNIX_EPOCH)
                                    .unwrap()
                                    .as_millis()
                                    as u64,
                            },
                        };
                        if let Err(e) =
                            crate::table::ddl_op_log::write_op_status(&self.info_store, &status)
                                .await
                        {
                            log::error!(
                                "#1048: failed to write SucceededViaCrashRecovery status for \
                                 recovered regular rename '{} → '{}': {e}",
                                entry.old_name,
                                entry.new_name
                            );
                        }
                    }
                }
            }
        }

        // ── Unique family ───────────────────────────────────────────────
        for entry in &unique_renames {
            let path_refs: Vec<&str> = entry.paths.iter().map(|s| s.as_str()).collect();

            // Step 1: ensure new unique index is in a clean, Ready state.
            if self.unique_index_exists(&entry.new_name).await {
                let new_id = self.intern_string(&entry.new_name).await?;
                if let Some(def) = self.index_manager.get_unique_index_definition(new_id) {
                    if def.state == crate::index2::state::IndexState::Building {
                        log::warn!(
                            "#997: unique rename target '{}' was left in Building \
                             state by a crashed create — dropping partial and re-creating",
                            entry.new_name
                        );
                        // Drop + re-create under the barrier (mirrors the
                        // rename path's barrier+lock span).
                        let (_barrier, _uwl_guard) = self
                            .begin_write_barrier(
                                crate::index::write_barrier_flags::UNIQUE_INDEX_CREATE,
                            )
                            .await;
                        self.index_manager.drop_unique_index(new_id, None).await?;
                        self.create_unique_index_body(&entry.new_name, &path_refs)
                            .await?;
                    }
                    // else Ready — already done
                }
            } else {
                // New doesn't exist. Either crash before drop (old still
                // present) or crash after drop (SEVERE: both absent).
                // Either way: create new. The backfill checks uniqueness —
                // see the doc comment above for why this is safe.
                self.create_unique_index(&entry.new_name, &path_refs)
                    .await?;
            }

            // Step 2: drop old if still present.
            if self.unique_index_exists(&entry.old_name).await {
                self.drop_unique_index(&entry.old_name, None).await?;
            }

            // #1000 test seam — same between-entries pause point as the
            // regular family above.
            self.index_manager
                .maybe_pause_recover_renames_between_entries()
                .await;
        }

        // Step 3: clear the WHOLE unique tombstone list once, after every
        // loaded entry has been reconciled — same reasoning as the regular
        // family above.
        if !unique_renames.is_empty() {
            self.index_manager.clear_all_renaming(true).await?;

            // #1048: write SucceededViaCrashRecovery for each recovered unique rename
            // that has an op_id. Skip silently for None (backward compat).
            for entry in &unique_renames {
                if let Some(ref op_id_str) = entry.op_id {
                    if let Ok(op_id) = RecordId::from_str(op_id_str) {
                        let status = DdlOpStatus {
                            op_id,
                            kind: DdlOpKind::RenameUniqueHashIndex {
                                old_name: entry.old_name.clone(),
                                new_name: entry.new_name.clone(),
                            },
                            state: DdlOpState::SucceededViaCrashRecovery {
                                completed_at_restart: std::time::SystemTime::now()
                                    .duration_since(std::time::UNIX_EPOCH)
                                    .unwrap()
                                    .as_millis()
                                    as u64,
                            },
                        };
                        if let Err(e) =
                            crate::table::ddl_op_log::write_op_status(&self.info_store, &status)
                                .await
                        {
                            log::error!(
                                "#1048: failed to write SucceededViaCrashRecovery status for \
                                 recovered unique rename '{} → '{}': {e}",
                                entry.old_name,
                                entry.new_name
                            );
                        }
                    }
                }
            }
        }

        log::info!(
            "#997: recovery complete — {} regular + {} unique hash RENAME(s) finalized",
            regular_renames.len(),
            unique_renames.len()
        );

        Ok(())
    }

    /// Check if a sorted index exists by name.
    ///
    /// Mirrors [`index_exists`](Self::index_exists) /
    /// [`unique_index_exists`](Self::unique_index_exists): a pure lookup that
    /// does NOT mint a new interned id when the name is absent.
    pub async fn sorted_index_exists(&self, name: &str) -> bool {
        if let Ok(interner) = self.interner.get().await {
            if let Some(key) = interner.get_ind(name) {
                return self
                    .sorted_indexes
                    .find_by_name_interned(key.id())
                    .is_some();
            }
        }
        false
    }

    /// Check if an index2 backend (`fts` / `functional` / `vector`) exists
    /// by name.
    ///
    /// Mirrors [`index_exists`](Self::index_exists): a pure lookup that does
    /// NOT mint a new interned id when the name is absent.
    pub async fn index2_exists(&self, name: &str) -> bool {
        if let Ok(interner) = self.interner.get().await {
            if let Some(key) = interner.get_ind(name) {
                return self.index2_registry.get_by_name(key.id()).await.is_some();
            }
        }
        false
    }

    /// Look up records by index value.
    pub async fn lookup_by_index(
        &self,
        name: &str,
        values: &[InnerValue],
    ) -> DbResult<BTreeSet<RecordId>> {
        let name_id = self.intern_string(name).await?;
        // Audit 1.5/3.2: the internal `IndexManager::lookup_by_index` now
        // returns `Arc<[RecordId]>` (sorted slice, O(1) cache-hit). This
        // public wrapper keeps the legacy `BTreeSet<RecordId>` signature
        // for API stability; the one-time collect here is at the boundary,
        // NOT on the internal hot path (engine-internal callers go through
        // `index_manager_ref().lookup_by_index` directly and consume the
        // `Arc<[RecordId]>` slice without collecting).
        //
        // ACCEPTED TRADE-OFF (task #488 review): this boundary clone
        // reintroduces the audit's O(|postings|) cost for whoever reaches
        // this specific public method — but as of this fix, NOTHING does:
        // `DbInstance::lookup_by_index`/`RepoInstance::lookup_by_index`
        // (the only callers of THIS function) have zero callers anywhere
        // in `shamir-server`/`shamir-client`/`shamir-sdk`/`shamir-db`/
        // `shamir-connect` (verified by grep across those crates). The
        // audit's actual dominant cost centers — SELECT execution
        // (`read_exec.rs`/`read_index_scan.rs`), write-path uniqueness/FK
        // checks (`write_helpers.rs`, `fk_restrict.rs`, `fk_on_update.rs`),
        // and validator dedup (`validator_db.rs`) — all bypass this
        // wrapper and call `index_manager_ref().lookup_by_index(...)`
        // directly, getting the true O(1) Arc-clone benefit. If a future
        // caller (SDK, server) starts using this public method on a hot
        // path, propagate `Arc<BTreeSet<RecordId>>` out through this
        // wrapper (and `RepoInstance`/`DbInstance`) instead of adding a
        // second clone-avoidance layer here.
        // P0-3a (#1011): the internal `IndexManager::lookup_by_index` now
        // returns `Option<Arc<[RecordId]>>`: `None` means a DROP of this index
        // is currently in its drain→sweep window, so the result is
        // deliberately withheld. This public introspection-by-name API is NOT a
        // query-planning path — a silent empty result here would be
        // indistinguishable from "the index legitimately has no matches", which
        // is strictly worse than an honest error. Surface it as `NotFound`
        // naming the index so the caller can re-plan / retry explicitly.
        let ids = self.index_manager.lookup_by_index(name_id, values).await?;
        let ids = ids.ok_or_else(|| {
            shamir_storage::error::DbError::NotFound(format!(
                "index '{name}' is currently being dropped (reader-drain window); \
                 the lookup result is unavailable — retry or re-plan without this index"
            ))
        })?;
        Ok(ids.iter().copied().collect())
    }

    /// Check if a regular index exists.
    ///
    /// Note: This method is async because it may need to load the interner.
    pub async fn index_exists(&self, name: &str) -> bool {
        // Try to get interned ID; if not interned, index doesn't exist
        if let Ok(interner) = self.interner.get().await {
            if let Some(key) = interner.get_ind(name) {
                return self.index_manager.index_exists(key.id());
            }
        }
        false
    }

    /// Check if a unique index exists.
    ///
    /// Note: This method is async because it may need to load the interner.
    pub async fn unique_index_exists(&self, name: &str) -> bool {
        if let Ok(interner) = self.interner.get().await {
            if let Some(key) = interner.get_ind(name) {
                return self.index_manager.unique_index_exists(key.id());
            }
        }
        false
    }

    /// R0-C (#1010): does `name` exist in ANY of the four index families
    /// (regular, unique, sorted, index2) on this table?
    ///
    /// Single shared helper combining `index_exists` / `unique_index_exists`
    /// / `sorted_index_exists` / `index2_exists` so every CREATE path uses
    /// the SAME cross-family check instead of drifting independent call
    /// sites. Callers that need admission-time atomicity (i.e. every real
    /// CREATE entry point) MUST call this AFTER acquiring
    /// `begin_write_barrier` — see each `create_*` method's call site for
    /// why: the admission-guarded window is what rules out another family's
    /// CREATE interleaving between this check and the eventual registration
    /// (the same TOCTOU class `create_index_v2`'s tombstone pre-check
    /// comment documents for the drop-in-flight case).
    pub async fn any_index_exists(&self, name: &str) -> bool {
        self.index_exists(name).await
            || self.unique_index_exists(name).await
            || self.sorted_index_exists(name).await
            || self.index2_exists(name).await
    }

    // ============================================================================
    // Internal helpers
    // ============================================================================

    /// Intern a single string, returning its u64 ID.
    async fn intern_string(&self, s: &str) -> DbResult<u64> {
        let interner = self.interner.get().await?;
        match interner.touch_ind(s) {
            Ok(TouchInd::New(key)) | Ok(TouchInd::Exists(key)) => Ok(key.id()),
            Err(e) => Err(shamir_storage::error::DbError::Codec(e.to_string())),
        }
    }

    /// Intern a path string like "user.address.city" into Vec<u64>.
    async fn intern_path(&self, path: &str) -> DbResult<Vec<u64>> {
        let interner = self.interner.get().await?;
        let mut result = Vec::new();

        for component in path.split('.') {
            let id = match interner.touch_ind(component) {
                Ok(TouchInd::New(key)) | Ok(TouchInd::Exists(key)) => key.id(),
                Err(e) => return Err(shamir_storage::error::DbError::Codec(e.to_string())),
            };
            result.push(id);
        }

        Ok(result)
    }

    /// Build IndexDefinition from string name and paths.
    async fn build_index_definition(
        &self,
        name: &str,
        paths: &[&str],
    ) -> DbResult<IndexDefinition> {
        let name_id = self.intern_string(name).await?;

        let mut interned_paths = Vec::with_capacity(paths.len());
        for path in paths {
            let path_components = self.intern_path(path).await?;
            interned_paths.push(IndexInfoItem::new(path_components));
        }

        Ok(IndexDefinition::new(name_id, interned_paths))
    }

    // ============================================================================
    // Rename index (rekey in place, preserve all posting data)
    // ============================================================================

    /// Rename an index from `old_name` to `new_name` on this table.
    ///
    /// Handles all four index kinds:
    ///   - **regular** (hash, `is_unique=0`): drop+rebuild — the hash-index
    ///     physical key embeds `name_interned` into the dual hash
    ///     (`compute_leaf_hashes` / `compute_lookup_hashes` both mix
    ///     `name_interned` into h1+h2), so a raw key-rewrite would leave
    ///     orphaned entries that the lookup path (which recomputes hashes
    ///     with the NEW name_interned) cannot find. The index is derived
    ///     data, so drop+rebuild from the live record stream is safe.
    ///   - **unique** (hash, `is_unique=1`): same as regular — drop+rebuild.
    ///   - **sorted** (B-tree-by-value, `SORTED_TAG` prefix): rekeys physical
    ///     entries from old to new name_interned (big-endian 8 bytes after
    ///     the tag byte). No hash mixing — sorted keys embed the raw
    ///     value bytes, not a hash of (name, value), so the rewrite is exact.
    ///   - **index2** (FTS / functional / vector): posting entries are keyed
    ///     by the compact `u32` `index_id`, not by name_interned, so no
    ///     physical move is needed — only the `by_name` lookup table AND the
    ///     authoritative name slots in the registry's `by_id` entry are updated
    ///     (P0-5a / #961), then the persisted metadata is re-saved.
    ///
    /// RENAME INDEX (hash regular + unique, sorted, index2) by name.
    ///
    /// `old_name` must exist; `new_name` must be free. Returns `Err` when the
    /// source does not exist or the destination name is already occupied by any
    /// index on this table.
    ///
    /// `op_id` is the operation ID minted at dispatch time (#1015); threaded
    /// through to tombstones so recovery can write `SucceededViaCrashRecovery`
    /// status under the same ID.
    pub async fn rename_index(
        &self,
        old_name: &str,
        new_name: &str,
        op_id: Option<shamir_types::types::record_id::RecordId>,
    ) -> DbResult<()> {
        let old_id = self.intern_string(old_name).await?;
        let new_id = self.intern_string(new_name).await?;

        if old_id == new_id {
            // Nothing to do — same interned id means same name.
            return Ok(());
        }

        // ── Classify what kind(s) of index exist under old_name ──────────────
        let is_regular = self.index_manager.index_exists(old_id);
        let is_unique = self.index_manager.unique_index_exists(old_id);
        let is_sorted = self.sorted_indexes.find_by_name_interned(old_id).is_some();
        let is_index2 = self.index2_registry.get_by_name(old_id).await.is_some();

        if !is_regular && !is_unique && !is_sorted && !is_index2 {
            return Err(shamir_storage::error::DbError::Internal(format!(
                "index '{}' not found on this table",
                old_name
            )));
        }

        // R0-C (#1010): refuse instead of silently resolving when `old_name`
        // is a PRE-EXISTING cross-family collision (a name present in more
        // than one of the four families — only reachable on a table that
        // acquired the collision before the #1010 CREATE-time preflight
        // landed, since CREATE now refuses to introduce a new one). Without
        // this guard, the family-specific blocks below (`is_regular`,
        // `is_unique`, `is_sorted`, `is_index2`) each act independently on
        // EVERY family that matches `old_name` — not just one — silently
        // renaming every colliding sibling instead of the single index the
        // caller meant. A full redesign (explicit per-family disambiguator)
        // is out of scope here (tracked as #1025); this is the low-risk
        // "detect and refuse" the R0-C brief permits.
        let matching_families = [is_regular, is_unique, is_sorted, is_index2]
            .iter()
            .filter(|&&m| m)
            .count();
        if matching_families > 1 {
            return Err(shamir_storage::error::DbError::Internal(format!(
                "index '{old_name}' exists in {matching_families} different index families \
                 on this table (a pre-existing cross-family name collision) — RENAME INDEX \
                 cannot safely resolve which one to rename. Run TableManager::verify() to \
                 see the affected families, then drop or rename the colliding sibling(s) \
                 individually before renaming '{old_name}'."
            )));
        }

        // ── Guard: destination name must not be occupied ──────────────────────
        let dst_regular = self.index_manager.index_exists(new_id);
        let dst_unique = self.index_manager.unique_index_exists(new_id);
        let dst_sorted = self.sorted_indexes.find_by_name_interned(new_id).is_some();
        let dst_index2 = self.index2_registry.get_by_name(new_id).await.is_some();

        if dst_regular || dst_unique || dst_sorted || dst_index2 {
            return Err(shamir_storage::error::DbError::Internal(format!(
                "index '{}' already exists on this table; cannot rename '{}' to it",
                new_name, old_name
            )));
        }

        // ── Regular (hash): create-new-first, drop-old-second ───────────────
        // Hash-index keys embed name_interned into hash1/hash2; a raw key
        // rewrite breaks lookup. Rebuild from the live record stream instead.
        //
        // Order matters for concurrency (audit A9): `create_index` now
        // registers the new definition FIRST (Option A), so the live
        // write-hook immediately starts maintaining postings for the NEW
        // index. By creating the new index BEFORE dropping the old one,
        // there is NEVER a window where a concurrent write is invisible to
        // BOTH the old and new indexes — during the brief overlap, a write
        // goes to both (postings are idempotent: same (index_key, record_id)
        // → same physical key, empty value). Dropping the old afterward
        // only removes old-id-prefixed postings, leaving the new index intact.
        if is_regular {
            let old_def = self
                .index_manager
                .get_index_definition(old_id)
                .ok_or_else(|| {
                    shamir_storage::error::DbError::Internal(
                        "index definition disappeared mid-rename".to_string(),
                    )
                })?;
            let interner = self.interner.get().await?;
            let paths = resolve_index_paths(interner, &old_def.paths);
            let path_refs: Vec<&str> = paths.iter().map(|s| s.as_str()).collect();

            // #997: write a durable rename tombstone BEFORE the first mutating
            // step (create_index). The tombstone carries the resolved string
            // names + paths so recovery can rebuild from nothing if the crash
            // strands it (unlike sorted's id-pair tombstone, a hash rename is
            // drop+rebuild — the old def is gone by the time the tombstone
            // matters). Cleared AFTER the last step succeeds. Mirrors #962's
            // durable "Renaming" tombstone (sorted) and #959's `idx_drop`
            // (same IndexManager owner).
            self.index_manager
                .add_to_renaming(
                    false,
                    old_id,
                    crate::index::index_manager::HashRenameTombstone {
                        old_name: old_name.to_string(),
                        new_name: new_name.to_string(),
                        paths: paths.clone(),
                        op_id: op_id.as_ref().map(|id| id.to_string()),
                    },
                )
                .await?;

            // Create the new index FIRST (registers + backfills under new_id).
            self.create_index(new_name, &path_refs).await?;

            // #997 test seam — park here (tombstone written, new index created,
            // old index NOT yet dropped) if a test installed the pause hook.
            // This is the EXACT crash window recovery tests simulate.
            self.index_manager.maybe_pause_rename_mid().await;

            // Then drop the old index (removes old-id postings only).
            // P1-2 (#967): if this fails after create_index succeeded, both
            // old and new indexes exist — enrich the error with context AND
            // write a structured `Failed { detail }` to the op-status log.
            match self.index_manager.drop_index(old_id, None).await {
                Ok(_) => {}
                Err(e) => {
                    let detail = format!(
                        "RENAME INDEX '{old_name}' → '{new_name}': the new index \
                         '{new_name}' was created successfully, but dropping the old \
                         index '{old_name}' failed: {e}. Both indexes now exist — \
                         call TableManager::verify() to confirm state."
                    );
                    // Write structured failure status if op_id is available.
                    if let Some(ref id) = op_id {
                        let status = shamir_query_types::read::DdlOpStatus {
                            op_id: *id,
                            kind: shamir_query_types::read::DdlOpKind::RenameHashIndex {
                                old_name: old_name.to_string(),
                                new_name: new_name.to_string(),
                            },
                            state: shamir_query_types::read::DdlOpState::Failed {
                                detail: detail.clone(),
                            },
                        };
                        if let Err(write_err) =
                            crate::table::ddl_op_log::write_op_status(&self.info_store, &status)
                                .await
                        {
                            log::error!(
                                "#967: failed to write Failed status for RENAME '{old_name}' → '{new_name}': {write_err}",
                            );
                        }
                    }
                    return Err(shamir_storage::error::DbError::Internal(detail));
                }
            }

            // #997: clear the rename tombstone now that the rename is durable.
            // If this fails, the tombstone remains — recovery will reconcile.
            self.index_manager
                .clear_from_renaming(false, old_id)
                .await
                .map_err(|e| {
                    shamir_storage::error::DbError::Internal(format!(
                        "RENAME INDEX '{old_name}' → '{new_name}': the rename is \
                         complete (new index created, old dropped), but clearing \
                         the rename tombstone failed: {e}. On restart, recovery \
                         will reconcile. Call TableManager::verify() to confirm state."
                    ))
                })?;
        }

        // ── Unique (hash): write-barrier across drop→backfill→register ──────
        // Unique-index uniqueness validation during backfill is NOT safely
        // idempotent — a duplicate slipping through the gap is a correctness
        // violation of the uniqueness guarantee, not a harmless double-write.
        // So we hold the table-wide `unique_write_lock` across the ENTIRE
        // drop→backfill→register sequence (Option B), preventing ANY writer
        // (non-tx insert, tx commit Phase 2.5) from inserting a duplicate
        // while the unique index is between its old and new registered states.
        // No gap = no duplicates possible. Acceptable cost for a low-frequency
        // DDL operation.
        if is_unique {
            let old_def = self
                .index_manager
                .get_unique_index_definition(old_id)
                .ok_or_else(|| {
                    shamir_storage::error::DbError::Internal(
                        "unique index definition disappeared mid-rename".to_string(),
                    )
                })?;
            let interner = self.interner.get().await?;
            let paths = resolve_index_paths(interner, &old_def.paths);
            let path_refs: Vec<&str> = paths.iter().map(|s| s.as_str()).collect();

            // #997: write the durable rename tombstone BEFORE the barrier (the
            // tombstone write is an I/O op; holding the barrier during it is a
            // real cost). A crash after the tombstone write but before the
            // barrier/drop is safe: nothing was mutated yet, and recovery sees
            // the tombstone + old still present → re-runs the rename.
            self.index_manager
                .add_to_renaming(
                    true,
                    old_id,
                    crate::index::index_manager::HashRenameTombstone {
                        old_name: old_name.to_string(),
                        new_name: new_name.to_string(),
                        paths: paths.clone(),
                        op_id: op_id.as_ref().map(|id| id.to_string()),
                    },
                )
                .await?;

            // Hold the barrier bit + unique_write_lock across drop + create.
            // This is the SAME lock non-tx writers acquire
            // (table_manager_crud.rs) and the tx commit pipeline acquires
            // (Phase 2.5), so it blocks all writer classes for the rename's
            // duration.
            //
            // F-70 (#897, P0): acquired via the canonical drain-then-lock
            // path (`begin_write_barrier`) — raise `UNIQUE_INDEX_CREATE`,
            // drain in-flight fast-path writers, THEN take
            // `unique_write_lock` — instead of lock-then-drain (F-57, #883's
            // original order, which deadlocks against `pre_commit_prelock`'s
            // drain-guard-then-lock shape on a second table). Holding the
            // guards across BOTH `drop_unique_index` AND the create body is a
            // SEPARATE, still-valid requirement (uniqueness-gap atomicity,
            // audit A9) — unrelated to drain ordering: the drain always
            // completes strictly before this span begins, regardless of how
            // long the span itself then holds the lock for.
            let (_barrier, _uwl_guard) = self
                .begin_write_barrier(crate::index::write_barrier_flags::UNIQUE_INDEX_CREATE)
                .await;

            self.index_manager.drop_unique_index(old_id, None).await?;

            // #997 test seam — park here (tombstone written, old dropped,
            // new NOT yet created) if a test installed the pause hook. This
            // is the EXACT SEVERE crash window: neither index exists.
            self.index_manager.maybe_pause_rename_mid().await;

            // Use the body variant: the barrier + lock are already held
            // above, and `create_unique_index` would re-acquire the lock →
            // deadlock (`tokio::sync::Mutex` is NOT reentrant).
            // P1-2 (#967): if this fails after drop_unique_index succeeded,
            // the old index is gone but the new one doesn't exist — enrich the
            // error with context AND write a structured `Failed { detail }`
            // to the op-status log.
            match self.create_unique_index_body(new_name, &path_refs).await {
                Ok(_) => {}
                Err(e) => {
                    let detail = format!(
                        "RENAME INDEX '{old_name}' → '{new_name}': the old unique \
                         index '{old_name}' was dropped, but creating the new \
                         unique index '{new_name}' failed: {e}. The old index is \
                         gone — call TableManager::verify() to confirm state, or \
                         re-create the index manually."
                    );
                    // Write structured failure status if op_id is available.
                    if let Some(ref id) = op_id {
                        let status = shamir_query_types::read::DdlOpStatus {
                            op_id: *id,
                            kind: shamir_query_types::read::DdlOpKind::RenameUniqueHashIndex {
                                old_name: old_name.to_string(),
                                new_name: new_name.to_string(),
                            },
                            state: shamir_query_types::read::DdlOpState::Failed {
                                detail: detail.clone(),
                            },
                        };
                        if let Err(write_err) =
                            crate::table::ddl_op_log::write_op_status(&self.info_store, &status)
                                .await
                        {
                            log::error!(
                                "#967: failed to write Failed status for unique RENAME '{old_name}' → '{new_name}': {write_err}",
                            );
                        }
                    }
                    return Err(shamir_storage::error::DbError::Internal(detail));
                }
            }

            // #997: clear the rename tombstone now that the rename is durable.
            // The barrier + lock are still held here (they release when the
            // guards drop at end of block), but the clear is just a Store::set
            // under a separate key — no contention concern.
            self.index_manager
                .clear_from_renaming(true, old_id)
                .await
                .map_err(|e| {
                    shamir_storage::error::DbError::Internal(format!(
                        "RENAME INDEX '{old_name}' → '{new_name}': the unique rename \
                         is complete (old dropped, new created), but clearing the \
                         rename tombstone failed: {e}. On restart, recovery will \
                         reconcile. Call TableManager::verify() to confirm state."
                    ))
                })?;
        }

        // ── Rekey sorted index posting entries ────────────────────────────────
        // P0-5b (#962): the full sorted-rename orchestration (durable "Renaming"
        // tombstone → definition swap → settle-loop rekey → tombstone clear)
        // now lives in `SortedIndexManager::rename_index_sorted`, mirroring how
        // `drop_sorted_index` delegates to `drop_index`. The durable tombstone
        // makes an interrupted rekey resumable on restart via
        // `recover_in_progress_renames` (previously a rekey `Err` after the
        // definition swap orphaned postings permanently). Concurrency (audit
        // A9): the definition is renamed FIRST (atomic RCU swap), so the live
        // write-hook starts writing under new_id immediately; the rekey's
        // settle re-scan then catches any old-id entry a concurrent writer
        // landed in the brief rename→rekey window.
        //
        // R0-A (#1012): wrapped in `begin_write_barrier(SORTED_INDEX_CREATE)` —
        // reuses the family's CREATE bit (same reasoning as `drop_sorted_index`),
        // held across the ENTIRE tombstone → definition-swap → rekey →
        // tombstone-clear sequence inside `rename_index_sorted`. Before this
        // fix, nothing serialized this rename against a concurrent CREATE/
        // DROP/RENAME on the same table's sorted family.
        if is_sorted {
            let (_barrier, _uwl_guard) = self
                .begin_write_barrier(crate::index::write_barrier_flags::SORTED_INDEX_CREATE)
                .await;
            self.sorted_indexes
                .rename_index_sorted(old_id, new_id)
                .await?;
        }

        // ── Rekey index2 (FTS / functional / vector) ──────────────────────────
        // Physical posting entries are keyed by `index_id` (u32), not by
        // name_interned — no data movement needed. Only the by_name lookup
        // table in the registry changes, plus the persisted metadata.
        //
        // R0-A (#1012): wrapped in `begin_write_barrier(INDEX2_CREATE)` —
        // reuses the family's CREATE bit (same reasoning as `drop_index2`),
        // held across the `rename_entry` registry mutation AND the metadata
        // persist below. Before this fix, nothing serialized this rename
        // against a concurrent CREATE/DROP/RENAME on the same table's index2
        // family — two registry-mutating ops could race `IndexRegistry`'s
        // ticket/generation bookkeeping.
        if is_index2 {
            let (_barrier, _uwl_guard) = self
                .begin_write_barrier(crate::index::write_barrier_flags::INDEX2_CREATE)
                .await;
            // rename_entry moves the by_name mapping old_id → new_id AND
            // updates the authoritative name slots in the by_id entry so the
            // rename survives `save_index2_metadata` (P0-5a / #961: without
            // the by_id update, the persisted descriptor carried the stale
            // original name and the rename was silently reverted on reopen).
            let ok = self
                .index2_registry
                .rename_entry(old_id, new_name.to_string(), new_id)
                .await;
            if !ok {
                return Err(shamir_storage::error::DbError::Internal(
                    "index2 rename_entry failed (concurrent conflict?)".to_string(),
                ));
            }
            crate::index2::persistence::save_index2_metadata(
                &self.index2_registry,
                &self.info_store,
            )
            .await?;
        }

        Ok(())
    }

    /// #1087: Phase B (micro-barrier) + Phase A (barrier-free backfill) for online CREATE INDEX.
    ///
    /// Performs the first two phases of online CREATE INDEX orchestration (RFC v2).
    /// Returns `Ok(true)` on success, `Ok(false)` if online build is unavailable
    /// (changefeed not wired), or `Err` on failure.
    ///
    /// Phase B: acquire barrier, open snapshot, register at Building, mark in-flight.
    /// Phase A: barrier-free scan via `mvcc_store().snapshot_stream()`, backfill postings.
    ///
    /// The index remains in `Building` state after this method returns (Phase C+D
    /// flip to Ready happens in #1088). The in-flight registry and dirty-set remain
    /// active, so concurrent writes during Phase A are captured.
    ///
    /// # Parameters
    ///
    /// - `index_def`: Index definition with `state` already set to `Building` by the caller.
    /// - `batch_size`: Batch size for the `snapshot_stream` scan (same role as
    ///   `list_stream`'s parameter).
    ///
    /// #1088: Phase C (catch-up loop) + Phase D (publish barrier) for online CREATE INDEX.
    ///
    /// Phase B+A backfill (returns SnapshotGuard for Phase C/D use).
    ///
    /// Phase B holds the write barrier (`REGULAR_INDEX_CREATE`) across the snapshot
    /// pin and registration. Phase A runs barrier-free — the snapshot is versioned,
    /// so concurrent writes are visible to the dirty-set capture mechanism (activated
    /// by `mark_build_in_flight` in Phase B) but do not interfere with the scan.
    ///
    /// Returns `Ok(None)` when changefeed is not wired (online build unavailable).
    /// Returns `Ok(Some(PhaseBAResult { guard, pin }))` on success, where `guard`
    /// must be kept alive through Phase C/D (it pins the version for pin-time reads).
    #[allow(dead_code)] // Will be wired in #1089
    pub(crate) async fn phase_b_a_backfill(
        &self,
        index_def: crate::index::index_definition::IndexDefinition,
        batch_size: usize,
    ) -> DbResult<Option<PhaseBAResult>> {
        use futures::StreamExt;

        let name_interned = index_def.name_interned;

        // ── Phase B: micro-barrier (raise → drain → lock) ──────────────────────
        // F-70 (#897): canonical drain-then-lock acquisition — raise
        // `REGULAR_INDEX_CREATE`, drain in-flight fast-path writers, THEN take
        // `unique_write_lock`. This order is load-bearing (see F-70 deadlock
        // proof in `writer_drain_barrier.rs` and `f70_lock_order_inversion_tests.rs`).
        let (_barrier, _uwl_guard) = self
            .begin_write_barrier(crate::index::write_barrier_flags::REGULAR_INDEX_CREATE)
            .await;

        // Under the barrier, open the snapshot. If changefeed is not wired,
        // online build is unavailable for this table — return None signal
        // for the caller (#1089) to fall back to the old path.
        let Some(guard) = self.open_index_build_snapshot().await else {
            // Barrier guard drops here (RAII), releasing bit+lock.
            return Ok(None);
        };

        let pin = guard.version();

        // Capture paths before moving index_def.
        let paths = index_def.paths.clone();

        // ── Register at Building (same sequence as create_index_from_stream Phase 1) ──
        // F-72 (#899, P0): register first at Building, durably persist, then
        // backfill. This makes the index planner-invisible until Phase D flips
        // it to Ready, avoiding half-populated index queries.
        self.index_manager
            .register_index_at_building(index_def)
            .await?;

        // #1058: mark in-flight so live write-hooks capture to dirty-set.
        self.index_manager.mark_build_in_flight(name_interned);

        // ── Drop barrier guards (RAII) ─────────────────────────────────────────
        // Phase A is barrier-free — writers must be able to proceed while we scan.
        // The SnapshotGuard (`guard`) stays alive through Phase A to pin the
        // version floor, but the barrier bit and lock are released now.
        drop(_barrier);
        drop(_uwl_guard);

        // ── Phase A: barrier-free backfill scan ─────────────────────────────────
        // `guard` (SnapshotGuard) remains alive through the scan — it pins the
        // version floor for the snapshot_stream. Writers can now proceed
        // concurrently; any write that touches the indexed paths is captured
        // in the dirty-set (activated by mark_build_in_flight above). Phase C
        // (#1088) drains and applies those deltas.

        let mvcc = self.mvcc_store().ok_or_else(|| {
            shamir_storage::error::DbError::Internal(
                "phase_b_a_backfill: mvcc_store unavailable but open_index_build_snapshot succeeded"
                    .to_string(),
            )
        })?;

        // Stream from the pinned snapshot (same at_version for all batches).
        let stream = mvcc.snapshot_stream(batch_size, pin);

        // Adapt each batch: decode (Bytes, Bytes) to (RecordId, InnerValue).
        let posting_stream = stream.map(move |batch_result| {
            batch_result.and_then(|batch| {
                batch
                    .into_iter()
                    .map(|(key_bytes, value_bytes)| {
                        use shamir_types::types::record_id::RecordId;
                        use shamir_types::types::value::InnerValue;

                        let record_id = RecordId::try_from_bytes(&key_bytes).ok_or_else(|| {
                            shamir_storage::error::DbError::Internal(
                                "Failed to parse RecordId from key".to_string(),
                            )
                        })?;

                        let inner_value = InnerValue::from_bytes(&value_bytes).map_err(|e| {
                            shamir_storage::error::DbError::Internal(format!(
                                "Failed to decode InnerValue: {e}"
                            ))
                        })?;

                        Ok((record_id, inner_value))
                    })
                    .collect::<DbResult<Vec<_>>>()
            })
        });

        // ── Backfill: batch-write postings (same pattern as create_index_from_stream Phase 2) ──
        let mut count = 0usize;

        // Use helpers from index_manager for building posting keys.
        use shamir_index::base_index::index_keys::{
            build_index_key_from_record, build_posting_key,
        };

        let backfill_start = std::time::Instant::now();
        let mut last_progress_log = std::time::Instant::now();
        let mut batch_no = 0u64;
        const BACKFILL_PROGRESS_LOG_INTERVAL: std::time::Duration =
            std::time::Duration::from_secs(5);

        let mut posting_stream = Box::pin(posting_stream);

        while let Some(batch_result) = posting_stream.next().await {
            // Test seam: pause mid-scan after processing at least one batch.
            #[cfg(test)]
            {
                if batch_no == 1 {
                    if let Some(hook) = self.online_index_backfill_hook.load_full() {
                        hook.wait_at_window().await;
                    }
                }
            }

            let batch = batch_result.map_err(|e| {
                shamir_storage::error::DbError::Internal(format!(
                    "CREATE INDEX '{name_interned}': online build backfill (Phase A) failed: {e}. \
                         The index is Building (planner-invisible) but under-populated."
                ))
            })?;

            // Declare per-batch accumulators FRESH inside the loop (no shadowing).
            let mut posting_writes: Vec<(bytes::Bytes, bytes::Bytes)> = Vec::with_capacity(131_072);
            let mut cache_index_keys: Vec<bytes::Bytes> = Vec::with_capacity(131_072);

            for (record_id, value) in &batch {
                if let Some(irk) = build_index_key_from_record(false, name_interned, value, &paths)
                {
                    let index_key = irk.to_bytes();
                    let posting_key = build_posting_key(&index_key, record_id);
                    posting_writes.push((posting_key, bytes::Bytes::new()));
                    cache_index_keys.push(index_key);
                    count += 1;
                }
            }

            if !posting_writes.is_empty() {
                // Use the new IndexManager helper to write the batch and clear cache.
                self.index_manager
                    .write_postings_batch(posting_writes, cache_index_keys)
                    .await
                    .map_err(|e| {
                        shamir_storage::error::DbError::Internal(format!(
                            "CREATE INDEX '{name_interned}': online build backfill (Phase A) \
                             failed: {e}. The index is Building (planner-invisible) but \
                             under-populated."
                        ))
                    })?;
            }

            batch_no += 1;
            if last_progress_log.elapsed() >= BACKFILL_PROGRESS_LOG_INTERVAL {
                log::info!(
                    "CREATE INDEX '{}': online backfill (Phase A) in progress — {} rows \
                     indexed across {} batches ({:.1}s elapsed)",
                    name_interned,
                    count,
                    batch_no,
                    backfill_start.elapsed().as_secs_f64()
                );
                last_progress_log = std::time::Instant::now();
            }
        }

        // ── Return SnapshotGuard and pin to caller (Phase C/D) ─────────────────────
        // The guard must stay alive through Phase C/D to support pin-time reads.
        // Caller is responsible for dropping it after Phase D completes.
        log::info!(
            "CREATE INDEX '{}': Phase B+A completed — {} rows indexed in {:.1}s, index \
             left in Building state (dirty-set capture active)",
            name_interned,
            count,
            backfill_start.elapsed().as_secs_f64()
        );

        Ok(Some(PhaseBAResult { guard, pin }))
    }

    #[allow(dead_code)]
    pub(crate) async fn phase_c_d_catchup_and_publish(
        &self,
        name_interned: u64,
        phase_ba: PhaseBAResult,
    ) -> DbResult<()> {
        let PhaseBAResult { guard, pin } = phase_ba;

        // ── Phase C: barrier-free catch-up loop ─────────────────────────────
        for _ in 0..Self::CATCHUP_ITERATION_CAP {
            let dirty = self.index_manager.drain_dirty_set(name_interned);
            if dirty.is_empty() {
                break;
            }
            self.apply_catchup_for_ids(name_interned, &dirty, pin)
                .await?;
        }

        // ── Phase D: short publish barrier ──────────────────────────────────
        let (_barrier, _uwl_guard) = self
            .begin_write_barrier(crate::index::write_barrier_flags::REGULAR_INDEX_CREATE)
            .await;

        // Final residual — whatever accumulated since the loop above's last
        // drain. Bounded by construction (the loop only exits on empty or cap).
        let final_dirty = self.index_manager.drain_dirty_set(name_interned);
        if !final_dirty.is_empty() {
            self.apply_catchup_for_ids(name_interned, &final_dirty, pin)
                .await?;
        }

        // Flip Building -> Ready + persist — mirror index_manager.rs:1645-1673
        // (create_index_from_stream's Phase 3) EXACTLY: flip in-memory first,
        // then save_index_info(), matching the existing publish-then-persist
        // ordering invariant (F-72/#899) documented there.
        self.index_manager
            .flip_to_ready(name_interned)
            .await
            .map_err(|e| {
                shamir_storage::error::DbError::Internal(format!(
                    "CREATE INDEX '{name_interned}': catch-up completed and the \
                     index was flipped to Ready in memory, but the final durable \
                     persist of the Ready state (Phase D) failed: {e}. The index is \
                     queryable in THIS process but durably Building on disk — a \
                     restart will reload it as Building (planner-invisible). Call \
                     TableManager::verify() to confirm state, or \
                     TableManager::repair() to rebuild it."
                ))
            })?;

        self.index_manager.clear_build_in_flight(name_interned);

        drop(guard); // release the pin — Phase C/D's last use of get_at(pin) was above.
                     // _barrier / _uwl_guard drop via RAII at function end.

        Ok(())
    }

    /// Shared by Phase C's loop and Phase D's final residual: batched
    /// pin-vs-current read for `ids`, then one `apply_catchup_batch` call.
    #[allow(dead_code)]
    async fn apply_catchup_for_ids(
        &self,
        name_interned: u64,
        ids: &[shamir_types::types::record_id::RecordId],
        pin: u64,
    ) -> DbResult<()> {
        let mvcc = self.mvcc_store().ok_or_else(|| {
            shamir_storage::error::DbError::Internal(
                "apply_catchup_for_ids: mvcc_store unavailable mid-catchup".to_string(),
            )
        })?;

        let keys: Vec<bytes::Bytes> = ids.iter().map(|id| id.to_bytes()).collect();
        let at_pin = mvcc.get_at_many(&keys, pin).await?;
        let at_now = self.get_many(ids).await?; // TableManager::get_many, already
                                                // decodes to InnerValue (table_manager_crud.rs:607)

        let mut deltas = Vec::with_capacity(ids.len());
        for i in 0..ids.len() {
            let old_value = at_pin[i]
                .as_ref()
                .map(InnerValue::from_bytes)
                .transpose()
                .map_err(|e| {
                    shamir_storage::error::DbError::Internal(format!(
                        "Phase C: failed to decode pin-time value: {e}"
                    ))
                })?;
            deltas.push((ids[i], old_value, at_now[i].clone()));
        }

        self.index_manager
            .apply_catchup_batch(name_interned, deltas)
            .await
    }
}

/// Resolve interned path ids back to dot-separated string paths.
///
/// Used by `rename_index` to capture the field paths of a hash index
/// before drop+rebuild: the `IndexDefinition.paths` are `Vec<IndexInfoItem>`
/// whose segments are interned u64 ids. We resolve each segment through the
/// interner to recover the original string path (e.g. `"user.email"`).
fn resolve_index_paths(
    interner: &shamir_types::core::interner::Interner,
    paths: &[IndexInfoItem],
) -> Vec<String> {
    use shamir_types::core::interner::InternerKey;
    paths
        .iter()
        .map(|item| {
            item.path
                .iter()
                .map(|&id| {
                    interner
                        .get_str(&InternerKey::new(id))
                        .map(|s| (*s).to_string())
                        .unwrap_or_default()
                })
                .collect::<Vec<_>>()
                .join(".")
        })
        .collect()
}
