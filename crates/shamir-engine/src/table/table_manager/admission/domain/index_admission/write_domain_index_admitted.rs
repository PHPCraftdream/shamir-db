use std::sync::Arc;

use super::TableManager;
use crate::index2::backend::IndexBackend;
use crate::index2::descriptor::IndexDescriptor;
use crate::index2::kind::{
    FunctionalConfig, IndexKind, VectorBackendRef, VectorConfig, VectorMetric, VectorQuantization,
};
use crate::repo::RepoWritePermit;
use shamir_storage::error::DbResult;
use shamir_types::core::interner::TouchInd;
use smallvec::SmallVec;

impl TableManager {
    /// S5c-X admitted twin — caller already holds repo admission.
    pub async fn create_index_v2_admitted(
        &self,
        op: &shamir_query_types::admin::CreateIndexOp,
        permit: &RepoWritePermit,
    ) -> DbResult<()> {
        self.write_domain.debug_check_permit(permit);
        let index_type = op.index_type.as_deref().unwrap_or("btree");
        if index_type == "btree" {
            let paths: Vec<String> = op.fields.iter().map(|segs| segs.join(".")).collect();
            let path_refs: Vec<&str> = paths.iter().map(|s| s.as_str()).collect();
            return if op.unique {
                self.create_unique_index_admitted(&op.create_index, &path_refs, permit)
                    .await
            } else {
                self.create_index_admitted(&op.create_index, &path_refs, permit)
                    .await
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
        // Same P0-3b fast-reject as the pre-admission pre-check in
        // `create_index_v2` (shared helper): admitted callers (e.g. the
        // shamir-db DDL path) are rejected here without waiting for a DROP
        // parked at its pre-sweep hook — the parked drop holds the repo
        // permit, so the admitted twin itself must not block behind it.
        self.reject_create_while_drop_in_flight(&op.create_index)
            .await?;

        // #534 finding 1 (write-barrier, Option B — mirrors
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
            .begin_write_barrier_admitted(crate::index::write_barrier_flags::INDEX2_CREATE, permit)
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
                let backend: Arc<dyn IndexBackend> =
                    Arc::new(crate::index2::vector::VectorBackend::new(
                        desc,
                        first_path,
                        adapter,
                        Some(Arc::clone(&self.info_store)),
                    ));
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
                let e = shamir_storage::error::DbError::from(e);
                let detail = format!(
                    "CREATE INDEX '{}': the index was durably persisted as \
                     Building, but the backfill failed: {e}. The index is NOT \
                     queryable — it remains permanently Building until rebuilt. \
                     On restart, the table-open self-heal will detect the \
                     Building state and re-backfill. Call TableManager::verify() \
                     to confirm state, or TableManager::repair() to rebuild it.",
                    op.create_index
                );
                log::error!("{}", detail);
                shamir_index::backend::contextualize_db_error(e, detail)
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
            let detail = format!(
                "CREATE INDEX '{}': the index was durably persisted as \
                     Building and the backfill completed, but registering the \
                     backend in the live registry failed: {e}. The index is \
                     NOT queryable in THIS process — on restart, the table-open \
                     self-heal will detect the Building state and re-backfill. \
                     Call TableManager::verify() to confirm state, or \
                     TableManager::repair() to rebuild it.",
                op.create_index
            );
            log::error!("{}", detail);
            shamir_index::backend::contextualize_db_error(
                shamir_storage::error::DbError::from(e),
                detail,
            )
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
                let detail = format!(
                    "CREATE INDEX '{}': the backfill completed and the index was \
                     flipped to Ready in memory, but the final durable persist of \
                     the Ready state failed: {e}. The index is queryable in THIS \
                     process but durably Building on disk — on restart, the \
                     table-open self-heal will detect the Building state and \
                     re-backfill. Call TableManager::verify() to confirm state, \
                     or TableManager::repair() to rebuild it.",
                    op.create_index
                );
                log::error!("{}", detail);
                shamir_index::backend::contextualize_db_error(e, detail)
            })?;

        Ok(())
    }

    // S5c-X admitted twin — remaining index methods live in write_domain_index_core.rs.
}
