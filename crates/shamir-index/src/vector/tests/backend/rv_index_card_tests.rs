//! Consolidated vector-backend regression tests (RV-INDEX cards).
//!
//! `rv_index_0b756318` — `drop_all` must sweep the per-index snapshot
//! namespace using the trailing-dot delimiter (`__vec_snap__9.`) so a
//! sibling index (90/91) sharing the id as a raw prefix survives, and must
//! work on a freshly built backend that was never reopened.
//!
//! `rv_index_ab9403d5` — a staged vector whose declared dimension mismatches
//! the index descriptor must be rejected (staged_vector returns None).
//!
//! `rv_index_fab4e4e5` — `build_index2_backend` must propagate the
//! descriptor's HNSW tuning (`m`, `ef_construct`) into the built adapter
//! instead of hardcoded defaults.

use crate::backend::IndexBackend;
use crate::backends::build_backend::build_index2_backend;
use crate::descriptor::IndexDescriptor;
use crate::kind::{IndexKind, VectorBackendRef, VectorConfig, VectorMetric};
use crate::vector::brute_force::BruteForceAdapter;
use crate::vector::vector_backend::VectorBackend;
use bytes::Bytes;
use shamir_storage::storage_in_memory::InMemoryStore;
use shamir_storage::types::Store;
use shamir_types::core::interner::{Interner, TouchInd};
use shamir_types::types::record_id::RecordId;
use shamir_types::types::value::InnerValue;
use smallvec::SmallVec;
use std::sync::Arc;

fn key(suffix: &str) -> Bytes {
    Bytes::from(format!("__vec_snap__{suffix}").into_bytes())
}

fn vector_desc(id: u32, name: &str) -> IndexDescriptor {
    vector_desc_tuned(id, name, 2, 16, 200)
}

fn vector_desc_tuned(id: u32, name: &str, dim: u32, m: u32, ef_construct: u32) -> IndexDescriptor {
    let interner = Interner::new();
    let name_id = match interner.touch_ind(name).unwrap() {
        TouchInd::Exists(k) | TouchInd::New(k) => k.id(),
    };
    IndexDescriptor::new(
        id,
        name,
        name_id,
        SmallVec::new(),
        IndexKind::Vector(Box::new(VectorConfig {
            dim,
            metric: VectorMetric::L2,
            backend: VectorBackendRef::InProcessHnsw { m, ef_construct },
            quantization: None,
        })),
    )
}

#[tokio::test]
async fn rv_index_0b756318_vector_drop_all_sweeps_snapshot_namespace() {
    let desc = vector_desc(916, "drop_idx");
    let store: Arc<dyn Store> = Arc::new(InMemoryStore::new());
    for k in ["916.manifest", "916.g1.qdata.000000"] {
        store
            .set(key(k).into(), Bytes::from_static(b"data"))
            .await
            .unwrap();
    }
    let backend = VectorBackend::new(
        desc,
        vec![],
        Arc::new(BruteForceAdapter::new(2, VectorMetric::L2)),
        Some(Arc::clone(&store)),
    );
    backend
        .restore_on_open(Arc::clone(&store), Arc::clone(&store))
        .await
        .unwrap();
    backend.drop_all().await.unwrap();
    assert!(store.get(key("916.manifest").into()).await.is_err());
    assert!(store.get(key("916.g1.qdata.000000").into()).await.is_err());
}

#[tokio::test]
async fn rv_index_ab9403d5_staged_vector_rejects_wrong_dimension() {
    let interner = Interner::new();
    let path = match interner.touch_ind("embedding").unwrap() {
        TouchInd::Exists(k) | TouchInd::New(k) => k.id(),
    };
    let desc = vector_desc(918, "v");
    let backend = VectorBackend::new(
        desc,
        vec![path],
        Arc::new(BruteForceAdapter::new(2, VectorMetric::L2)),
        Some(Arc::new(InMemoryStore::new())),
    );
    let mut m = shamir_types::types::common::new_map_wc(1);
    m.insert(
        shamir_types::core::interner::InternerKey::new(path),
        InnerValue::List(vec![InnerValue::F64(1.0)]),
    );
    let rec = InnerValue::Map(m);
    assert!(backend
        .staged_vector(RecordId([0; 16]), &rec)
        .await
        .is_none());
}

// RV-INDEX-FAB4E4E5: build_index2_backend must propagate the descriptor's
// HNSW tuning (m, ef_construct) into the built adapter — not hardcoded
// m:16 / ef_construction:200 defaults.
#[test]
fn rv_index_fab4e4e5_builder_preserves_backend_tuning() {
    let desc = vector_desc_tuned(917, "ix", 2, 8, 77);
    let store: Arc<dyn Store> = Arc::new(InMemoryStore::new());
    let backend = build_index2_backend(desc, &store);
    let vb = backend
        .as_any()
        .downcast_ref::<crate::vector::VectorBackend>()
        .expect("vector descriptor must build a VectorBackend");
    let adapter = vb.adapter_for_test();
    let hnsw = adapter
        .as_hnsw_adapter()
        .expect("descriptor must build an HnswAdapter");
    let cfg = hnsw.build_config();
    assert_eq!(cfg.m, 8, "descriptor m must reach the built adapter");
    assert_eq!(
        cfg.ef_construction, 77,
        "descriptor ef_construct must reach the built adapter"
    );
}

// Trailing-dot delimiter guard: index 9's sweep must not match ids 90/91,
// which share `9` as a raw prefix.
#[tokio::test]
async fn rv_index_0b756318_drop_index_9_spares_indexes_90_and_91() {
    let desc = vector_desc(9, "n9");
    let store: Arc<dyn Store> = Arc::new(InMemoryStore::new());
    for k in [
        "9.manifest",
        "9.g0.data.000000",
        "90.manifest",
        "91.g0.qgraph.000000",
    ] {
        store
            .set(key(k).into(), Bytes::from_static(b"data"))
            .await
            .unwrap();
    }
    let backend = VectorBackend::new(
        desc,
        vec![],
        Arc::new(BruteForceAdapter::new(2, VectorMetric::L2)),
        Some(Arc::clone(&store)),
    );
    backend.drop_all().await.unwrap();
    assert!(store.get(key("9.manifest").into()).await.is_err());
    assert!(store.get(key("9.g0.data.000000").into()).await.is_err());
    assert!(store.get(key("90.manifest").into()).await.is_ok());
    assert!(store.get(key("91.g0.qgraph.000000").into()).await.is_ok());
}

// drop_all on a freshly built backend (no restore_on_open) must sweep too:
// VectorBackend::new must initialize the info store from its `info_store` arg.
#[tokio::test]
async fn rv_index_0b756318_create_then_drop_without_reopen() {
    let desc = vector_desc(920, "n920");
    let store: Arc<dyn Store> = Arc::new(InMemoryStore::new());
    for k in ["920.manifest", "920.g0.sidecar"] {
        store
            .set(key(k).into(), Bytes::from_static(b"data"))
            .await
            .unwrap();
    }
    let backend = build_index2_backend(desc, &store);
    let vb = backend
        .as_any()
        .downcast_ref::<crate::vector::VectorBackend>()
        .expect("vector descriptor must build a VectorBackend");
    IndexBackend::drop_all(vb).await.unwrap();
    assert!(store.get(key("920.manifest").into()).await.is_err());
    assert!(store.get(key("920.g0.sidecar").into()).await.is_err());
}
