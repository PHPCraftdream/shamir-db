//! Build an `IndexBackend` from an `IndexDescriptor` + `info_store`.
//!
//! Shared by `TableManager::create` (reopen path) and
//! `replicate_index2_descriptors_from` (migration cutover).

use crate::backend::IndexBackend;
use crate::descriptor::IndexDescriptor;
use crate::kind::IndexKind;
use crate::vector::hnsw_adapter::{HnswAdapter, HnswConfig};
use shamir_storage::types::Store;
use std::sync::Arc;

pub fn build_index2_backend(
    desc: IndexDescriptor,
    info_store: &Arc<dyn Store>,
) -> Arc<dyn IndexBackend> {
    let first_path = desc.paths.first().cloned().unwrap_or_default();
    match desc.kind.clone() {
        crate::kind::IndexKind::Fts { .. } => {
            Arc::new(crate::fts_ranked_backend::FtsRankedBackend::new(
                desc,
                first_path,
                Arc::clone(info_store),
            ))
        }
        crate::kind::IndexKind::Functional(cfg) => {
            Arc::new(crate::functional_backend::FunctionalBackend::new(
                desc,
                cfg.expr.clone(),
                Arc::clone(info_store),
            ))
        }
        IndexKind::Vector(cfg) => {
            let adapter = Arc::new(HnswAdapter::new(
                cfg.dim,
                cfg.metric,
                HnswConfig {
                    max_elements: 100_000,
                    m: 16,
                    ef_construction: 200,
                    ef_search: 50,
                    ..Default::default()
                },
            ));
            Arc::new(crate::vector::VectorBackend::new(desc, first_path, adapter))
        }
        crate::kind::IndexKind::Btree { .. } => {
            unreachable!("Btree indexes are handled by the legacy index manager")
        }
    }
}
