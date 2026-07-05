use std::collections::BTreeSet;
use std::sync::Arc;

use bytes::Bytes;
use futures::StreamExt;
use shamir_storage::error::DbResult;
use shamir_storage::types::Store;
use shamir_tunables::store_defaults::FULL_SCAN_BATCH;
use shamir_types::core::interner::TouchInd;
use shamir_types::types::record_id::RecordId;
use shamir_types::types::value::InnerValue;

use super::table_manager::TableManager;
use crate::index::index_definition::IndexDefinition;
use crate::index::index_info_item::IndexInfoItem;

impl TableManager {
    // ============================================================================
    // Index Management API (string paths → interned internally)
    // ============================================================================

    /// Create a regular or specialized (fts/vector/functional) index.
    ///
    /// Routes `btree` + `unique` variants through the legacy `IndexManager`
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
                let desc = IndexDescriptor::new(
                    id,
                    &op.create_index,
                    name_key,
                    interned_paths.clone(),
                    kind.clone(),
                );
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
                let desc = IndexDescriptor::new(
                    id,
                    &op.create_index,
                    name_key,
                    interned_paths.clone(),
                    kind.clone(),
                );
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
                let desc = IndexDescriptor::new(
                    id,
                    &op.create_index,
                    name_key,
                    interned_paths.clone(),
                    kind.clone(),
                );
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

        self.index2_registry
            .insert(backend)
            .await
            .map_err(|e| shamir_storage::error::DbError::Internal(e.to_string()))?;

        crate::index2::persistence::save_index2_metadata(&self.index2_registry, &self.info_store)
            .await?;

        Ok(())
    }

    /// Create a regular index on specified paths.
    pub async fn create_index(&self, name: &str, paths: &[&str]) -> DbResult<()> {
        let index_def = self.build_index_definition(name, paths).await?;
        // Always use the seam: collect_all_current_records routes
        // attached→log / unattached→data_store, so it is correct for
        // both cases.
        let records = self.collect_all_current_records().await?;
        self.index_manager
            .create_index_from_records(index_def, records)
            .await
    }

    /// Create a unique index on specified paths.
    ///
    /// # Errors
    /// Returns `DbError::UniqueIndexCreationFailed` if duplicate values exist.
    pub async fn create_unique_index(&self, name: &str, paths: &[&str]) -> DbResult<()> {
        let index_def = self.build_index_definition(name, paths).await?;
        // Always use the seam: collect_all_current_records routes
        // attached→log / unattached→data_store, so it is correct for
        // both cases.
        let records = self.collect_all_current_records().await?;
        self.index_manager
            .create_unique_index_from_records(index_def, records)
            .await
    }

    /// Drop a regular index by name.
    ///
    /// # Returns
    /// `true` if index existed and was removed, `false` if not found.
    pub async fn drop_index(&self, name: &str) -> DbResult<bool> {
        let name_id = self.intern_string(name).await?;
        self.index_manager.drop_index(name_id).await
    }

    /// Drop a unique index by name.
    ///
    /// # Returns
    /// `true` if index existed and was removed, `false` if not found.
    pub async fn drop_unique_index(&self, name: &str) -> DbResult<bool> {
        let name_id = self.intern_string(name).await?;
        self.index_manager.drop_unique_index(name_id).await
    }

    /// Look up records by index value.
    pub async fn lookup_by_index(
        &self,
        name: &str,
        values: &[InnerValue],
    ) -> DbResult<BTreeSet<RecordId>> {
        let name_id = self.intern_string(name).await?;
        self.index_manager.lookup_by_index(name_id, values).await
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
    ///     physical move is needed — only the `by_name` lookup table in the
    ///     registry is updated, and the persisted metadata is re-saved.
    ///
    /// Returns `Err` when the source does not exist or the destination name is
    /// already occupied by any index on this table.
    pub async fn rename_index(&self, old_name: &str, new_name: &str) -> DbResult<()> {
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

        // ── Regular (hash): drop + rebuild under new name ────────────────────
        // Hash-index keys embed name_interned into hash1/hash2; a raw key
        // rewrite breaks lookup. Rebuild from the live record stream instead.
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

            self.index_manager.drop_index(old_id).await?;
            self.create_index(new_name, &path_refs).await?;
        }

        // ── Unique (hash): drop + rebuild under new name ─────────────────────
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

            self.index_manager.drop_unique_index(old_id).await?;
            self.create_unique_index(new_name, &path_refs).await?;
        }

        // ── Rekey sorted index posting entries ────────────────────────────────
        if is_sorted {
            rekey_sorted_prefix(&*self.info_store, old_id, new_id).await?;

            // Drop old sorted-index definition and register new one.
            // `drop_index` would delete the physical entries we just moved, so
            // we use `rename_definition` which only swaps the in-memory entry.
            self.sorted_indexes
                .rename_definition(old_id, new_id)
                .await?;
        }

        // ── Rekey index2 (FTS / functional / vector) ──────────────────────────
        // Physical posting entries are keyed by `index_id` (u32), not by
        // name_interned — no data movement needed. Only the by_name lookup
        // table in the registry changes, plus the persisted metadata.
        if is_index2 {
            // rename_entry moves the by_name mapping from old_id → new_id.
            let ok = self.index2_registry.rename_entry(old_id, new_id).await;
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
}

// ============================================================================
// Prefix-scan rekey helpers (module-private)
// ============================================================================

/// Rekey all posting entries of a sorted index.
///
/// Sorted-index physical key layout:
///   `[SORTED_TAG: 1][name_interned BE: 8][encoded_value: var][record_id: 16]`
///
/// For each key found under the old prefix, bytes [1..9] are replaced
/// with `new_id.to_be_bytes()` (note: **big-endian**, unlike hash indexes).
async fn rekey_sorted_prefix(info_store: &dyn Store, old_id: u64, new_id: u64) -> DbResult<()> {
    // SORTED_TAG = 0x80 — the tag byte that distinguishes sorted-index physical
    // keys from hash-index keys and system RecordId keys in the same info_store.
    // Mirrors `shamir_index::legacy::sorted_index_definition::SORTED_TAG` (pub(crate)
    // there, so we inline the constant here with a comment instead of importing).
    const SORTED_TAG: u8 = 0x80;

    let mut old_prefix_buf = Vec::with_capacity(9);
    old_prefix_buf.push(SORTED_TAG);
    old_prefix_buf.extend_from_slice(&old_id.to_be_bytes());
    let old_prefix = Bytes::from(old_prefix_buf);

    let new_id_bytes = new_id.to_be_bytes();

    let stream = info_store.scan_prefix_stream(old_prefix, FULL_SCAN_BATCH);
    futures::pin_mut!(stream);

    let mut to_write: Vec<(Bytes, Bytes)> = Vec::new();
    let mut to_remove: Vec<Bytes> = Vec::new();

    while let Some(batch) = stream.next().await {
        for (key, value) in batch? {
            if key.len() < 9 {
                continue; // malformed; skip
            }
            let mut new_key = key.to_vec();
            new_key[1..9].copy_from_slice(&new_id_bytes);
            to_write.push((Bytes::from(new_key), value));
            to_remove.push(key);
        }
    }

    if !to_write.is_empty() {
        let _ = info_store.set_many(to_write).await?;
    }
    if !to_remove.is_empty() {
        let _ = info_store.remove_many(to_remove).await?;
    }

    Ok(())
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
