//! Build an `IndexBackend` from an `IndexDescriptor` + `info_store`.
//!
//! Shared by `TableManager::create` (reopen path) and
//! `replicate_index2_descriptors_from` (migration cutover).

use crate::index2::backend::IndexBackend;
use crate::index2::descriptor::IndexDescriptor;
use std::sync::Arc;
use shamir_storage::types::Store;

pub fn build_index2_backend(
    desc: IndexDescriptor,
    info_store: &Arc<dyn Store>,
) -> Arc<dyn IndexBackend> {
    let first_path = desc.paths.first().cloned().unwrap_or_default();
    match desc.kind.clone() {
        crate::index2::kind::IndexKind::Fts { .. } => {
            Arc::new(crate::index2::fts_ranked_backend::FtsRankedBackend::new(
                desc,
                first_path,
                Arc::clone(info_store),
            ))
        }
        crate::index2::kind::IndexKind::Functional(cfg) => {
            Arc::new(crate::index2::functional_backend::FunctionalBackend::new(
                desc,
                cfg.expr.clone(),
                Arc::clone(info_store),
            ))
        }
        crate::index2::kind::IndexKind::Vector(cfg) => {
            let adapter = Arc::new(
                crate::index2::vector::hnsw_adapter::HnswAdapter::new(
                    cfg.dim,
                    cfg.metric,
                    crate::index2::vector::hnsw_adapter::HnswConfig {
                        max_elements: 100_000,
                        m: 16,
                        ef_construction: 200,
                        ef_search: 50,
                        ..Default::default()
                    },
                ),
            );
            Arc::new(crate::index2::vector::VectorBackend::new(
                desc,
                first_path,
                adapter,
            ))
        }
        crate::index2::kind::IndexKind::Btree { .. } => {
            unreachable!("Btree indexes are handled by the legacy index manager")
        }
    }
}
