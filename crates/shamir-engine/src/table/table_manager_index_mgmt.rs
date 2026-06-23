use std::collections::BTreeSet;
use std::sync::Arc;

use shamir_storage::error::DbResult;
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
                let kind = IndexKind::Vector(Box::new(VectorConfig {
                    dim,
                    metric,
                    backend: VectorBackendRef::InProcessHnsw {
                        ef_construct: 200,
                        m: 16,
                    },
                }));
                let desc = IndexDescriptor::new(
                    id,
                    &op.create_index,
                    name_key,
                    interned_paths.clone(),
                    kind.clone(),
                );
                let adapter = Arc::new(crate::index2::vector::hnsw_adapter::HnswAdapter::new(
                    dim,
                    metric,
                    crate::index2::vector::hnsw_adapter::HnswConfig {
                        max_elements: 100_000,
                        m: 16,
                        ef_construction: 200,
                        ef_search: 50,
                        ..Default::default()
                    },
                ));
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
}
