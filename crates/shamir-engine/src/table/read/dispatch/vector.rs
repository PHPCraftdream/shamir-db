use std::sync::Arc;
use std::time::Instant;

use crate::index2::vector::hnsw_adapter::{
    CO_FILTER_MAX_SELECTIVITY, MAX_TOPK, PRE_FILTER_MAX_CANDIDATES,
};
use futures::StreamExt;

use crate::index2::backend::{IndexQuery, IndexResult};
use crate::index2::vector::vector_backend::VectorBackend;
use crate::index2::vector::{SearchOpts, VectorAdapter as _};
use crate::query::filter::eval::{compile_filter, intern_field_path, FilterNode};
use crate::query::filter::eval_context::FilterContext;
use crate::query::read::{exec, CorruptRecordRef, QueryResult, ReadQuery};
use crate::table::support::storage::record_cow::RecordCow;
use crate::table::table_manager::TableManager;
use bytes::Bytes;

use shamir_query_types::batch::ResultEncoding;
use shamir_storage::error::{DbError, DbResult};
use shamir_types::core::interner::Interner;
use shamir_types::types::record_id::RecordId;

use crate::table::read::execution::candidate_set::index2_shortcut_eligible;
use crate::table::read::execution::dispatch::DynBatchStream;
use crate::table::read::vector::filtered_vector::{resolve_oversample, FilteredVectorQuery};

impl TableManager {
    /// Exact snapshot + overlay ranking when the live graph cannot be trusted.
    #[allow(clippy::too_many_arguments)]
    pub(in crate::table::read) async fn read_snapshot_vector(
        &self,
        query: &ReadQuery,
        ctx: &FilterContext<'_>,
        interner: &Interner,
        fvq: &FilteredVectorQuery,
        tx: Option<&shamir_tx::TxContext>,
        start: Instant,
    ) -> DbResult<QueryResult> {
        let path = intern_field_path(&fvq.field, interner);
        let lease = match path {
            Some(path) => {
                self.index2_registry()
                    .lease_by_field_and_kind(&path, "vector")
                    .await?
            }
            None => None,
        };
        let Some(lease) = lease else {
            return self
                .read_fallback_no_vector_index(query, ctx, interner, tx, start)
                .await;
        };
        let vb = lease
            .backend
            .as_any()
            .downcast_ref::<VectorBackend>()
            .ok_or_else(|| DbError::Internal("vector backend type mismatch".into()))?;
        let adapter = vb.adapter_arc();
        let expected_dim = adapter.dim();
        if fvq.query.len() != expected_dim as usize {
            return Err(DbError::Internal(format!(
                "dimension mismatch: expected {expected_dim}, got {}",
                fvq.query.len(),
            )));
        }
        let hnsw = adapter.as_hnsw_adapter().ok_or_else(|| {
            DbError::Internal("snapshot vector scoring requires HNSW adapter".into())
        })?;
        let residual = fvq.residual.as_ref().map(|f| compile_filter(f, interner));
        let k = fvq.k.min(MAX_TOPK) as usize;
        let mut ranked: Vec<(RecordId, f32, Bytes)> = Vec::new();
        let mut scanned = 0;
        let mut corrupt = Vec::new();
        // O(N*d) distance work plus batch/top-k sorting; keep only a batch
        // plus k winners and reuse the index's extraction and ShamirDist kernel.
        let stream = self
            .list_stream_tx_after_predicate(tx, shamir_tunables::store_defaults::FULL_SCAN_BATCH);
        futures::pin_mut!(stream);
        while let Some(batch) = stream.next().await {
            let mut vectors = Vec::new();
            let mut bytes_by_id = shamir_collections::TMap::default();
            for (id, cow) in batch? {
                scanned += 1;
                let record = match cow.into_inner() {
                    Ok(record) => record,
                    Err(_) => {
                        corrupt.push(CorruptRecordRef {
                            table: self.name().to_string(),
                            id,
                        });
                        continue;
                    }
                };
                if residual.as_ref().is_some_and(|f| !f.matches(&record, ctx)) {
                    continue;
                }
                if let Some(vector) = lease.backend.staged_vector(id, &record).await {
                    let bytes = record
                        .to_bytes()
                        .map_err(|e| DbError::Codec(format!("Failed to encode record: {e}")))?;
                    vectors.push((id, vector));
                    bytes_by_id.insert(id, bytes);
                }
            }
            // The adapter sorts with partial_cmp before truncation. Request
            // every candidate in bounded chunks so no nonfinite distance or
            // equal-distance id tie is lost before our total-order ranking.
            for chunk in vectors.chunks(MAX_TOPK as usize) {
                #[expect(
                    clippy::expect_used,
                    reason = "guard: each score chunk is bounded by MAX_TOPK, which fits u32"
                )]
                let chunk_len =
                    u32::try_from(chunk.len()).expect("scoring chunks are bounded by MAX_TOPK");
                let scores = hnsw
                    .score_staged_candidates(&fvq.query, chunk_len, chunk)
                    .await
                    .map_err(|e| DbError::Internal(e.to_string()))?;
                for (id, distance) in scores {
                    if let Some(bytes) = bytes_by_id.swap_remove(&id) {
                        ranked.push((id, distance, bytes));
                    }
                }
                ranked.sort_by(|a, b| a.1.total_cmp(&b.1).then_with(|| a.0.cmp(&b.0)));
                ranked.truncate(k);
            }
        }
        let rows = ranked
            .into_iter()
            .map(|(id, _, bytes)| (id, bytes))
            .collect();
        let mut result = Box::pin(self.postprocess_candidates(
            query,
            ctx,
            interner,
            rows,
            "snapshot_vector_exact",
            start,
            corrupt,
        ))
        .await?;
        if let Some(stats) = result.stats.as_mut() {
            stats.index_used = None;
            stats.records_scanned = scanned;
        }
        Ok(result)
    }

    /// V3.1 / P3 leaf 3.1 — filtered ANN execution path.
    ///
    /// Recognised shape: `And([VectorSimilarity{field,query,k,ef,oversample},
    /// ...residual-predicates])`. The planner compiled it into a
    /// `FilteredVectorQuery` (vector half + residual filter).
    ///
    /// Algorithm (post-filter with adaptive oversample-retry):
    /// 1. Resolve the oversample multiplier (`None` → 2× default, clamped ≥1×).
    /// 2. Compute `k′ = min(k × oversample, MAX_TOPK)`.
    /// 3. Loop:
    /// - a. ANN search for k′ candidates (tx-aware via `lookup_tx` so
    ///   in-tx staged vectors are visible).
    /// - b. Materialise candidate records and apply the residual predicate.
    /// - c. If ≥ k survivors → truncate to k, done.
    /// - d. If < k survivors AND k′ < MAX_TOPK → double k′ (clamped to
    ///   MAX_TOPK), retry.
    /// - e. If < k survivors AND k′ == MAX_TOPK → return what we have
    ///   (even if < k; the filter is too selective to fill k from the
    ///   available candidates).
    ///
    /// The result preserves ANN ranking order (nearest-first) among the
    /// survivors, which matches the bare-VectorSimilarity path's contract.
    ///
    /// Survivors feed the shared candidate postprocessor for projection,
    /// GROUP BY, aggregates, DISTINCT, ORDER BY, and pagination.
    #[allow(clippy::too_many_arguments)]
    pub(in crate::table::read) async fn read_filtered_vector_scan(
        &self,
        query: &ReadQuery,
        ctx: &FilterContext<'_>,
        interner: &Interner,
        fvq: &FilteredVectorQuery,
        tx: Option<&shamir_tx::TxContext>,
        start: Instant,
    ) -> DbResult<QueryResult> {
        // Resolve the vector backend by field path + "vector" kind.
        let field_path = crate::query::filter::eval::intern_field_path(&fvq.field, interner);
        let lease_result = match &field_path {
            Some(fp) => {
                self.index2_registry()
                    .lease_by_field_and_kind(fp, "vector")
                    .await
            }
            None => Ok(None),
        };
        let backend = match lease_result {
            Err(e) => return Err(e),
            Ok(Some(lease)) => lease.backend,
            Ok(None) => {
                // No vector index on this field — fall through to the legacy
                // full-scan path (VectorSimilarity compiles to FilterNode::True,
                // so the scan returns residual-matched rows unranked). This is
                // the pre-V3.1 behaviour for a filtered-vector query without
                // an index.
                return self
                    .read_fallback_no_vector_index(query, ctx, interner, tx, start)
                    .await;
            }
        };

        // Compile the residual predicate once (reused across retry iterations).
        let residual_cb: Option<FilterNode> =
            fvq.residual.as_ref().map(|f| compile_filter(f, interner));

        let k = fvq.k;
        let oversample = resolve_oversample(fvq.oversample);
        let opts = SearchOpts {
            ef_search: fvq.ef_search,
            oversample: Some(oversample),
        };

        // Resolve staged vectors for tx-aware search.
        let table_token = self.table_token();
        let staged = tx.and_then(|t| t.staged_vectors_for(table_token));

        // ── V3.2 cost-based path selection ──────────────────────────────
        // Try to resolve the residual predicate against a secondary index to
        // get a candidate RID set. If successful, pick pre-filter or co-filter
        // based on cardinality; otherwise fall through to post-filter (V3.1).
        if let Some(ref residual) = fvq.residual {
            // Attempt to resolve candidate RIDs from secondary index (btree/functional).
            // IMPORTANT (C1): only enter fast-path when the index FULLY covers the
            // residual predicate. A partial cover (superset of matching RIDs) would
            // skip uncovered conjuncts and return wrong results.
            let candidate_rids: Option<Vec<RecordId>> = {
                // Try index2 path first (functional/fts/btree).
                // `try_plan_index2` only matches single-predicate shapes (Eq on
                // functional, FTS, bare Vector) — never multi-conjunct And. So a
                // successful Set result implies full coverage of the residual.
                let idx2_result = self.try_plan_index2(residual, interner).await?;
                match idx2_result {
                    Some(set) if !set.ranked => Some(set.rids),
                    _ => {
                        // Try base_index btree index scan. Only use when the index
                        // FULLY covers the residual (no leftover predicate).
                        if let Some((idx_name, lookup_sets, leftover)) =
                            self.try_plan_index_scan(residual, interner)
                        {
                            if leftover.is_some() {
                                // Partial coverage — uncovered conjuncts remain.
                                // Cannot use fast-path; fall through to post-filter.
                                None
                            } else {
                                let mut rids = Vec::new();
                                for values in &lookup_sets {
                                    // P0-3a (#1011): `Ok(None)` = DROP in its
                                    // drain→sweep window — treat as "no
                                    // base-index candidates" and let the vector
                                    // scan fall back to its full (unfiltered)
                                    // walk, exactly like the prior `Err`/empty
                                    // handling already does. NEVER silently count
                                    // it as an empty candidate intersection.
                                    if let Ok(Some(ids)) = self
                                        .index_manager_ref()
                                        .lookup_by_index(idx_name, values)
                                        .await
                                    {
                                        // Audit 1.5/3.2: `ids` is now
                                        // `Arc<[RecordId]>` (sorted slice) —
                                        // iterate the contiguous buffer.
                                        rids.extend(ids.iter().copied());
                                    }
                                }
                                if rids.is_empty() {
                                    None
                                } else {
                                    Some(rids)
                                }
                            }
                        } else {
                            None
                        }
                    }
                }
            };

            if let Some(candidates) = candidate_rids {
                let n_candidates = candidates.len();

                // Downcast the vector backend to access HnswAdapter.
                let vb_opt = backend.as_any().downcast_ref::<VectorBackend>();
                if let Some(vb) = vb_opt {
                    let adapter_arc = vb.adapter_arc();
                    if let Some(hnsw) = adapter_arc.as_hnsw_adapter() {
                        let total_live = hnsw.len();
                        let selectivity = if total_live > 0 {
                            n_candidates as f64 / total_live as f64
                        } else {
                            1.0
                        };

                        let (pre_filter_max_candidates, co_filter_max_selectivity) =
                            (PRE_FILTER_MAX_CANDIDATES, CO_FILTER_MAX_SELECTIVITY);
                        #[cfg(test)]
                        let (pre_filter_max_candidates, co_filter_max_selectivity) = self
                            .filtered_ann_thresholds
                            .unwrap_or((pre_filter_max_candidates, co_filter_max_selectivity));

                        if n_candidates <= pre_filter_max_candidates {
                            // PRE-FILTER: exact SIMD scoring over small candidate set.
                            let ranked = hnsw
                                .search_prefilter(&fvq.query, k, &candidates)
                                .await
                                .map_err(|e| DbError::Internal(e.to_string()))?;
                            // VR-5 (Б-5) — merge tx-staged vectors that pass the
                            // residual predicate so the pre-filter path sees the
                            // caller's own in-tx writes (read-your-own-writes),
                            // identical to the post-filter path.
                            // F-10 (#800): staged rows that fail to decode
                            // against the residual predicate are reported here.
                            let mut corrupt: Vec<CorruptRecordRef> = Vec::new();
                            let ranked = self
                                .merge_staged_filtered(
                                    hnsw,
                                    &fvq.query,
                                    k,
                                    ranked,
                                    staged,
                                    residual_cb.as_ref(),
                                    ctx,
                                    tx,
                                    &mut corrupt,
                                )
                                .await;
                            return Box::pin(self.build_filtered_vector_result(
                                query,
                                interner,
                                ctx,
                                tx,
                                start,
                                &ranked,
                                "pre_filter",
                                corrupt,
                            ))
                            .await;
                        } else if selectivity <= co_filter_max_selectivity {
                            // CO-FILTER: HNSW search_filter with allow-set.
                            let ranked = hnsw
                                .search_cofilter(&fvq.query, k, fvq.ef_search, &candidates)
                                .await
                                .map_err(|e| DbError::Internal(e.to_string()))?;
                            // VR-5 (Б-5) — same staged merge as pre-filter above.
                            // F-10 (#800): same corrupt-record reporting as
                            // the pre-filter branch above.
                            let mut corrupt: Vec<CorruptRecordRef> = Vec::new();
                            let ranked = self
                                .merge_staged_filtered(
                                    hnsw,
                                    &fvq.query,
                                    k,
                                    ranked,
                                    staged,
                                    residual_cb.as_ref(),
                                    ctx,
                                    tx,
                                    &mut corrupt,
                                )
                                .await;
                            return Box::pin(self.build_filtered_vector_result(
                                query,
                                interner,
                                ctx,
                                tx,
                                start,
                                &ranked,
                                "co_filter",
                                corrupt,
                            ))
                            .await;
                        }
                        // else: selectivity too high → fall through to post-filter
                    }
                }
            }
        }

        // Adaptive oversample-retry loop (POST-FILTER, V3.1).
        let mut k_prime = (((k as f32) * oversample).ceil() as u32)
            .max(k) // never below k
            .min(MAX_TOPK);
        // Survivors carry their already-resolved record bytes (plus the
        // originating RecordId, for F-10 corrupt-record reporting below) so
        // the projection loop reuses them instead of re-fetching — the
        // residual pass has already read every candidate through
        // `get_many_bytes_tx` (order is ANN rank order, preserved by the byte
        // fetch).
        let mut last_survivors: Vec<(RecordId, bytes::Bytes)>;
        // F-10 (#800): rows that fail to decode are still skipped from the
        // result set (unchanged behaviour) but reported here instead of
        // silently vanishing. Reset each retry iteration — a wider k′ rescans
        // an expanded candidate set from scratch, so a corrupt row from a
        // narrower attempt would otherwise be double-reported.
        let mut corrupt: Vec<CorruptRecordRef> = Vec::new();

        loop {
            corrupt.clear();
            let result = backend
                .lookup_tx(
                    table_token,
                    IndexQuery::Vector {
                        vec: fvq.query.clone(),
                        k: k_prime,
                        opts,
                    },
                    tx,
                    staged,
                )
                .await
                .map_err(|e| DbError::Internal(e.to_string()))?;

            let last_ranked = match result {
                IndexResult::Ranked(r) => r,
                IndexResult::Set(_) => {
                    // Vector backend returns Ranked; a Set is a contract
                    // violation. Fall back rather than panic.
                    return self
                        .read_fallback_no_vector_index(query, ctx, interner, tx, start)
                        .await;
                }
            };

            // Materialise candidate records and apply residual filter.
            // For tx-aware reads, staged records live in the tx's staging
            // store (write_set), NOT in the committed data store — so we
            // must read-through the staging store to resolve them.
            let candidates = last_ranked.len();
            let rids: Vec<RecordId> = last_ranked.iter().map(|(r, _)| *r).collect();
            let raw_records = self.get_many_bytes_tx(&rids, tx).await?;
            last_survivors = Vec::with_capacity(candidates);

            for (maybe_bytes, rid) in raw_records.iter().zip(rids.iter()) {
                let bytes = match maybe_bytes {
                    Some(b) => b,
                    None => continue, // deleted/tombstoned — skip
                };
                let passes = match &residual_cb {
                    Some(cb) => {
                        // Evaluate the residual predicate on the record.
                        match shamir_types::record_view::RecordView::new(bytes) {
                            Ok(view) => cb.matches(&view, ctx),
                            Err(_) => {
                                // malformed → skip, but report it. `corrupt`
                                // is reset each retry iteration (see above),
                                // so the final value reflects only the
                                // winning pass.
                                corrupt.push(CorruptRecordRef {
                                    table: self.name().to_string(),
                                    id: *rid,
                                });
                                continue;
                            }
                        }
                    }
                    None => true,
                };
                if passes {
                    last_survivors.push((*rid, bytes.clone()));
                }
            }

            // Got enough? Truncate to k and done.
            if last_survivors.len() >= k as usize {
                last_survivors.truncate(k as usize);
                break;
            }

            // Backend returned fewer candidates than we asked for → the HNSW
            // graph is exhausted; widening k′ cannot surface more. Stop with
            // what we have rather than spin identical lookups up to the cap.
            if candidates < k_prime as usize {
                break;
            }

            // Not enough. Can we widen k′?
            if k_prime >= MAX_TOPK {
                // Exhausted the cap — return what we have (< k).
                break;
            }

            // Double k′, clamp to MAX_TOPK, retry.
            k_prime = k_prime.saturating_mul(2).min(MAX_TOPK);
        }

        Box::pin(self.postprocess_candidates(
            query,
            ctx,
            interner,
            last_survivors,
            "filtered_vector_scan",
            start,
            corrupt,
        ))
        .await
    }

    /// Fallback for a filtered-vector query that has NO vector index on the
    /// field. Routes to `read_collecting` / `read_streaming` so the residual
    /// predicates are still applied (VectorSimilarity compiles to
    /// `FilterNode::True` → all rows pass the vector conjunct, only the
    /// residual matters). This preserves pre-V3.1 behaviour.
    async fn read_fallback_no_vector_index(
        &self,
        query: &ReadQuery,
        ctx: &FilterContext<'_>,
        interner: &Interner,
        tx: Option<&shamir_tx::TxContext>,
        start: Instant,
    ) -> DbResult<QueryResult> {
        let batch_size = shamir_tunables::store_defaults::FULL_SCAN_BATCH;
        let filter_cb: Option<Arc<FilterNode>> = query
            .r#where
            .as_ref()
            .map(|f| Arc::new(compile_filter(f, interner)));
        let has_group_by = query.group_by.is_some();
        let has_agg = exec::has_aggregates(&query.select);
        let has_order = query.order_by.is_some();
        let has_distinct = query.select.distinct;
        let needs_full_collect = has_group_by || has_agg || has_order || has_distinct;
        if needs_full_collect {
            self.read_collecting(
                query,
                ctx,
                interner,
                filter_cb.as_deref(),
                filter_cb.as_ref().map(Arc::clone),
                batch_size,
                start,
                tx,
            )
            .await
        } else if query.count_total {
            self.read_counting(
                query,
                interner,
                filter_cb.as_deref(),
                filter_cb.as_ref().map(Arc::clone),
                ctx,
                batch_size,
                start,
                tx,
            )
            .await
        } else {
            self.read_streaming(
                query,
                interner,
                filter_cb.as_deref(),
                filter_cb.as_ref().map(Arc::clone),
                ctx,
                batch_size,
                start,
                tx,
                ResultEncoding::Name,
            )
            .await
        }
    }

    /// VR-5 (Б-5) — Merge tx-staged vectors into the ranked output of the
    /// pre-filter / co-filter paths so they honour read-your-own-writes.
    ///
    /// Staged vectors live in `TxContext::staged_vectors` and are NOT in the
    /// committed HNSW graph, so `search_prefilter` / `search_cofilter` cannot
    /// see them. The post-filter path already sees staged rows (it resolves
    /// candidates via `backend.lookup_tx(..., staged)` which brute-force-merges
    /// them). To give the pre/co-filter paths identical visibility semantics,
    /// this helper:
    ///
    /// 1. Resolves the full record bytes for each staged vector (so the
    ///    residual predicate has the fields it needs, not just the embedding).
    /// 2. Applies the residual predicate via `FilterNode::matches` — staged
    ///    rows that do NOT match the residual are excluded (NOT "always
    ///    include"). This mirrors the post-filter residual pass.
    /// 3. Scores the residual-matching staged vectors brute-force via
    ///    `HnswAdapter::score_staged_candidates` (same `ShamirDist` kernel the
    ///    bare `search` path uses for its staged merge).
    /// 4. Merges into `ranked`, sorts by distance ascending, and truncates to
    ///    `k` — so the final result is the global top-k across committed +
    ///    staged, identical to what the post-filter path would return.
    ///
    /// Staged deletes are NOT consulted here: a staged delete removes the row
    /// from the tx's view, and `get_many_bytes_tx` (called by
    /// `build_filtered_vector_result` downstream) already returns `None` for a
    /// staged-removed RID, so even if a stale committed RID leaked into
    /// `ranked` it would be dropped at projection. Staged vector deletes
    /// target the graph side, which the pre/co-filter paths already respect
    /// (candidates come from the secondary index over committed rows).
    ///
    /// `staged` is `Option<&[(RecordId, Vec<f32>)]>` from
    /// `TxContext::staged_vectors_for(table_token)`. `residual_cb` is the
    /// compiled residual predicate (None when the filtered-vector query has
    /// no residual — then every staged vector passes).
    ///
    /// F-10 (#800): `corrupt` accumulates `(table, id)` for staged rows whose
    /// bytes fail to decode against the residual predicate — the row is still
    /// excluded from the merge (unchanged behaviour), but the caller's
    /// `QueryResult::corrupt_records` now surfaces it instead of silently
    /// dropping it.
    #[allow(clippy::too_many_arguments)]
    async fn merge_staged_filtered<'a>(
        &self,
        hnsw: &'a crate::index2::vector::hnsw_adapter::HnswAdapter,
        query: &[f32],
        k: u32,
        ranked: Vec<(RecordId, f32)>,
        staged: Option<&'a [(RecordId, Vec<f32>)]>,
        residual_cb: Option<&FilterNode>,
        ctx: &FilterContext<'_>,
        tx: Option<&shamir_tx::TxContext>,
        corrupt: &mut Vec<CorruptRecordRef>,
    ) -> Vec<(RecordId, f32)> {
        // No staged vectors or no tx → nothing to merge (committed-only path).
        let staged = match (staged, tx) {
            (Some(s), _) if !s.is_empty() => s,
            _ => return ranked,
        };

        // #427 (VR-5, @sh adversarial-review finding) — an UPDATE-in-tx that
        // changes an ALREADY-committed row's embedding (without removing the
        // field) does NOT stage a vector delete (`stage_vector_deletes_on_update`
        // only stages a delete when the NEW record has no embedding at all —
        // `table_manager_tx_ops.rs`), so the row's OLD version is still in the
        // committed HNSW graph and can appear in `ranked` via
        // `search_prefilter`/`search_cofilter`. The SAME rid then also appears
        // in `staged` (the tx's new embedding). Without dedup, both versions
        // would be scored and merged — the same RecordId twice in `ranked`,
        // wasting a top-k slot and violating the "no duplicate rids" contract.
        // The staged (newer, tx-visible) version must win: drop any `ranked`
        // entry whose rid is ALSO staged, before merging the staged score in.
        let staged_rid_set: shamir_collections::TFxSet<RecordId> =
            staged.iter().map(|(r, _)| *r).collect();
        let mut ranked: Vec<(RecordId, f32)> = ranked
            .into_iter()
            .filter(|(rid, _)| !staged_rid_set.contains(rid))
            .collect();

        // Resolve the full record bytes for each staged vector so the residual
        // predicate has access to every field, not just the embedding.
        let staged_rids: Vec<RecordId> = staged.iter().map(|(r, _)| *r).collect();
        let staged_bytes = match self.get_many_bytes_tx(&staged_rids, tx).await {
            Ok(b) => b,
            // On read failure we MUST NOT silently drop staged rows (that would
            // be a correctness regression); nor can we safely include them
            // unscored. Return the (already de-duplicated) committed-only
            // ranked set — the read path is best-effort here.
            Err(e) => {
                log::warn!(
                    "merge_staged_filtered: get_many_bytes_tx failed for {} staged \
                     rid(s), falling back to committed-only filtered-ANN results \
                     (read-your-own-writes degraded for this query): {e}",
                    staged_rids.len()
                );
                return ranked;
            }
        };

        // Apply the residual predicate to each staged record; collect the
        // vectors that pass into a residual-matching staged set.
        let mut passing: Vec<(RecordId, Vec<f32>)> = Vec::with_capacity(staged.len());
        for (entry, maybe_bytes) in staged.iter().zip(staged_bytes.iter()) {
            let bytes = match maybe_bytes {
                Some(b) => b,
                None => continue, // staged remove / unreadable — skip
            };
            let passes = match residual_cb {
                None => true,
                Some(cb) => match shamir_types::record_view::RecordView::new(bytes) {
                    Ok(view) => cb.matches(&view, ctx),
                    Err(_) => {
                        // malformed → skip, but report it.
                        corrupt.push(CorruptRecordRef {
                            table: self.name().to_string(),
                            id: entry.0,
                        });
                        continue;
                    }
                },
            };
            if passes {
                passing.push(entry.clone());
            }
        }

        if passing.is_empty() {
            return ranked;
        }

        // Score the residual-matching staged vectors brute-force and merge.
        let scored = match hnsw.score_staged_candidates(query, k, &passing).await {
            Ok(s) => s,
            Err(_) => return ranked,
        };
        ranked.extend(scored);
        ranked.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal));
        ranked.truncate(k as usize);
        ranked
    }

    /// Build a `QueryResult` from pre-filter / co-filter ranked output.
    ///
    /// V3.2: shared helper for pre-filter and co-filter paths. Takes the
    /// ranked `(RecordId, f32)` pairs, resolves them to record bytes,
    /// projects, and returns the QueryResult with the given `index_tag`.
    ///
    /// F-10 (#800): `corrupt` is the corrupt-record list already collected by
    /// `merge_staged_filtered` (staged rows dropped from the residual pass);
    /// this function extends it with any committed rows that fail to decode
    /// during projection below, and the union becomes
    /// `QueryResult::corrupt_records`.
    #[allow(clippy::too_many_arguments)]
    async fn build_filtered_vector_result(
        &self,
        query: &ReadQuery,
        interner: &Interner,
        ctx: &FilterContext<'_>,
        tx: Option<&shamir_tx::TxContext>,
        start: Instant,
        ranked: &[(RecordId, f32)],
        index_tag: &str,
        corrupt: Vec<CorruptRecordRef>,
    ) -> DbResult<QueryResult> {
        let rids: Vec<RecordId> = ranked.iter().map(|(r, _)| *r).collect();
        let raw = self.get_many_bytes_tx(&rids, tx).await?;
        let rows = rids
            .into_iter()
            .zip(raw)
            .filter_map(|(id, bytes)| bytes.map(|b| (id, b)))
            .collect();
        Box::pin(self.postprocess_candidates(query, ctx, interner, rows, index_tag, start, corrupt))
            .await
    }

    #[allow(clippy::too_many_arguments)]
    pub(in crate::table::read) async fn postprocess_candidates(
        &self,
        query: &ReadQuery,
        ctx: &FilterContext<'_>,
        interner: &Interner,
        rows: Vec<(RecordId, Bytes)>,
        index_tag: &str,
        start: Instant,
        mut corrupt: Vec<CorruptRecordRef>,
    ) -> DbResult<QueryResult> {
        let total = rows.len() as u64;
        let eligible = index2_shortcut_eligible(query);
        let (rows, pagination) = if eligible {
            // Validate ALL candidate bytes before slicing, using the same gate
            // as read_collecting_from. Corrupt rows neither consume the page
            // nor contribute to total_count, even outside the requested page.
            let rows: Vec<_> = rows
                .into_iter()
                .filter(|(id, bytes)| {
                    if shamir_types::record_view::RecordView::new(bytes).is_ok() {
                        true
                    } else {
                        corrupt.push(CorruptRecordRef {
                            table: self.name().to_string(),
                            id: *id,
                        });
                        false
                    }
                })
                .collect();
            let valid_total = rows.len() as u64;
            let (skip, take) = query.pagination.resolve();
            let has_next = take.is_some_and(|take| skip.saturating_add(take) < valid_total);
            let (rows, pagination) =
                exec::apply_pagination(rows, &query.pagination, query.count_total);
            let pagination = pagination.map(|info| info.with_has_next(has_next));
            (rows, pagination)
        } else {
            (rows, None)
        };
        let batch = rows
            .into_iter()
            .map(|(id, b)| (id, RecordCow::Borrowed(b)))
            .collect();
        let stream: DynBatchStream<'_> = Box::pin(futures::stream::once(async move { Ok(batch) }));
        // Eligible candidates were paged before projection. Use the SAME
        // collecting projection/corruption/version pipeline without paging twice.
        let mut unpaged;
        let effective = if eligible {
            unpaged = query.clone();
            unpaged.pagination = shamir_query_types::read::Pagination::None;
            &unpaged
        } else {
            query
        };
        let mut result =
            Box::pin(self.read_collecting_from(effective, ctx, interner, None, stream, start))
                .await?;
        if eligible {
            result.pagination = pagination;
        }
        if let Some(stats) = result.stats.as_mut() {
            stats.index_used = Some(index_tag.into());
            stats.records_scanned = total;
        }
        corrupt.append(&mut result.corrupt_records);
        result.corrupt_records = corrupt;
        Ok(result)
    }
}
