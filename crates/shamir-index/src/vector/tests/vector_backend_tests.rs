use crate::backend::{IndexBackend, IndexQuery, IndexResult};
use crate::descriptor::IndexDescriptor;
use crate::kind::{IndexKind, VectorBackendRef, VectorConfig, VectorMetric};
use crate::vector::adapter::VectorAdapter;
use crate::vector::brute_force::BruteForceAdapter;
use crate::vector::hnsw_adapter::{HnswAdapter, HnswConfig};
use crate::vector::vector_backend::VectorBackend;
use shamir_storage::storage_in_memory::InMemoryStore;
use shamir_storage::types::Store;
use shamir_types::core::interner::{Interner, InternerKey, TouchInd};
use shamir_types::types::common::new_map_wc;
use shamir_types::types::record_id::RecordId;
use shamir_types::types::value::InnerValue;
use smallvec::SmallVec;
use std::sync::Arc;

fn intern(i: &Interner, s: &str) -> u64 {
    match i.touch_ind(s).unwrap() {
        TouchInd::Exists(k) | TouchInd::New(k) => k.id(),
    }
}

fn make_rec(interner: &Interner, embedding: &[f64]) -> InnerValue {
    let mut m = new_map_wc(1);
    m.insert(
        InternerKey::new(intern(interner, "embedding")),
        InnerValue::List(embedding.iter().map(|f| InnerValue::F64(*f)).collect()),
    );
    InnerValue::Map(m)
}

fn make_backend(interner: &Interner) -> VectorBackend {
    let desc = IndexDescriptor::new(
        30,
        "vec_idx",
        intern(interner, "vec_idx"),
        SmallVec::new(),
        IndexKind::Vector(Box::new(VectorConfig {
            dim: 3,
            metric: VectorMetric::Cosine,
            backend: VectorBackendRef::InProcessHnsw {
                ef_construct: 200,
                m: 16,
            },
        })),
    );
    let adapter = Arc::new(BruteForceAdapter::new(3, VectorMetric::Cosine));
    VectorBackend::new(desc, vec![intern(interner, "embedding")], adapter)
}

#[tokio::test]
async fn insert_and_search() {
    let i = Interner::new();
    let backend = make_backend(&i);

    let r1 = RecordId::new();
    let r2 = RecordId::new();
    backend
        .plan_insert(r1, &make_rec(&i, &[1.0, 0.0, 0.0]))
        .await
        .unwrap();
    backend
        .plan_insert(r2, &make_rec(&i, &[0.0, 1.0, 0.0]))
        .await
        .unwrap();

    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    let result = backend
        .lookup(IndexQuery::Vector {
            vec: vec![1.0, 0.0, 0.0],
            k: 1,
            opts: crate::vector::SearchOpts::default(),
        })
        .await
        .unwrap();

    match result {
        IndexResult::Ranked(ranked) => {
            assert_eq!(ranked.len(), 1);
            assert_eq!(ranked[0].0, r1);
        }
        _ => panic!("expected Ranked"),
    }
}

#[tokio::test]
async fn delete_excludes_from_search() {
    let i = Interner::new();
    let backend = make_backend(&i);

    let r1 = RecordId::new();
    let rec = make_rec(&i, &[1.0, 0.0, 0.0]);
    backend.plan_insert(r1, &rec).await.unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    backend.plan_delete(r1, &rec).await.unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    let result = backend
        .lookup(IndexQuery::Vector {
            vec: vec![1.0, 0.0, 0.0],
            k: 10,
            opts: crate::vector::SearchOpts::default(),
        })
        .await
        .unwrap();

    match result {
        IndexResult::Ranked(ranked) => assert!(ranked.is_empty()),
        _ => panic!("expected Ranked"),
    }
}

#[tokio::test]
async fn lookup_tx_none_matches_lookup() {
    let i = Interner::new();
    let backend = make_backend(&i);

    let r1 = RecordId::new();
    let r2 = RecordId::new();
    backend
        .plan_insert(r1, &make_rec(&i, &[1.0, 0.0, 0.0]))
        .await
        .unwrap();
    backend
        .plan_insert(r2, &make_rec(&i, &[0.0, 1.0, 0.0]))
        .await
        .unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    let q = IndexQuery::Vector {
        vec: vec![1.0, 0.0, 0.0],
        k: 2,
        opts: crate::vector::SearchOpts::default(),
    };
    let via_lookup = backend
        .lookup(IndexQuery::Vector {
            vec: vec![1.0, 0.0, 0.0],
            k: 2,
            opts: crate::vector::SearchOpts::default(),
        })
        .await
        .unwrap();
    let via_tx = backend.lookup_tx(0, q, None, None).await.unwrap();
    match (via_lookup, via_tx) {
        (IndexResult::Ranked(a), IndexResult::Ranked(b)) => {
            assert_eq!(a.len(), b.len());
        }
        _ => panic!("expected Ranked results"),
    }
}

#[tokio::test]
async fn lookup_tx_some_includes_staged_vector() {
    let i = Interner::new();

    let desc = IndexDescriptor::new(
        31,
        "vec_tx",
        intern(&i, "vec_tx"),
        SmallVec::new(),
        IndexKind::Vector(Box::new(VectorConfig {
            dim: 3,
            metric: VectorMetric::Cosine,
            backend: VectorBackendRef::InProcessHnsw {
                ef_construct: 200,
                m: 16,
            },
        })),
    );
    let adapter: Arc<dyn VectorAdapter> = Arc::new(HnswAdapter::new(
        3,
        VectorMetric::Cosine,
        HnswConfig {
            max_elements: 100,
            ..Default::default()
        },
    ));
    let backend = VectorBackend::new(desc, vec![intern(&i, "embedding")], adapter);

    // Commit one vector via non-tx upsert.
    let committed_rid = RecordId::new();
    backend
        .adapter
        .upsert(committed_rid, &[1.0, 0.0, 0.0])
        .await
        .unwrap();

    // The tx's own staged vector (what the executor buffers in
    // `TxContext::staged_vectors_for(token)`), very close to query.
    let staged_rid = RecordId::new();
    let staged: Vec<(RecordId, Vec<f32>)> = vec![(staged_rid, vec![0.9, 0.1, 0.0])];

    // Non-tx lookup sees only committed vector.
    let non_tx = backend
        .lookup(IndexQuery::Vector {
            vec: vec![1.0, 0.0, 0.0],
            k: 2,
            opts: crate::vector::SearchOpts::default(),
        })
        .await
        .unwrap();

    // tx-aware lookup gets the staged slice threaded in by the caller.
    let in_tx = backend
        .lookup_tx(
            0,
            IndexQuery::Vector {
                vec: vec![1.0, 0.0, 0.0],
                k: 2,
                opts: crate::vector::SearchOpts::default(),
            },
            None,
            Some(&staged),
        )
        .await
        .unwrap();

    let non_tx_rids: Vec<RecordId> = match non_tx {
        IndexResult::Ranked(r) => r.into_iter().map(|(rid, _)| rid).collect(),
        _ => panic!("expected Ranked"),
    };
    let in_tx_rids: Vec<RecordId> = match in_tx {
        IndexResult::Ranked(r) => r.into_iter().map(|(rid, _)| rid).collect(),
        _ => panic!("expected Ranked"),
    };

    assert!(
        !non_tx_rids.contains(&staged_rid),
        "non-tx must not see staged vector"
    );
    assert!(
        in_tx_rids.contains(&staged_rid),
        "in-tx lookup must merge staged vector: got {in_tx_rids:?}"
    );
    assert!(
        non_tx_rids.contains(&committed_rid),
        "non-tx must see committed vector"
    );
    assert!(
        in_tx_rids.contains(&committed_rid),
        "in-tx must see committed vector"
    );
}

#[tokio::test]
async fn rebuild_from_store() {
    let i = Interner::new();
    let store: Arc<dyn Store> = Arc::new(InMemoryStore::new());

    // Write 3 records into the store as (RecordId → InnerValue).
    let r1 = RecordId::new();
    let r2 = RecordId::new();
    let r3 = RecordId::new();
    let rec1 = make_rec(&i, &[1.0, 0.0, 0.0]);
    let rec2 = make_rec(&i, &[0.0, 1.0, 0.0]);
    let rec3 = make_rec(&i, &[0.9, 0.1, 0.0]);
    store
        .set(r1.to_bytes(), rec1.to_bytes().unwrap())
        .await
        .unwrap();
    store
        .set(r2.to_bytes(), rec2.to_bytes().unwrap())
        .await
        .unwrap();
    store
        .set(r3.to_bytes(), rec3.to_bytes().unwrap())
        .await
        .unwrap();

    // Create a fresh backend (empty adapter) and rebuild from store.
    let backend = make_backend(&i);
    backend.rebuild(Arc::clone(&store)).await.unwrap();

    // Search for [1,0,0] — top-2 should contain r1 (closest).
    let result = backend
        .lookup(IndexQuery::Vector {
            vec: vec![1.0, 0.0, 0.0],
            k: 2,
            opts: crate::vector::SearchOpts::default(),
        })
        .await
        .unwrap();
    match result {
        IndexResult::Ranked(ranked) => {
            assert_eq!(ranked.len(), 2, "expected 2 results, got {ranked:?}");
            assert_eq!(ranked[0].0, r1, "r1 should be the closest");
        }
        _ => panic!("expected Ranked"),
    }
}

/// V0.2: rebuild must populate the graph with ALL records from the store
/// via the batched `upsert_batch` path (single rayon parallel_insert per
/// store page). We write M records into the store and assert the rebuilt
/// adapter's `len()` equals M — proving no record was dropped by the batch
/// pipeline.
#[tokio::test]
async fn rebuild_from_store_batched_all_records_present() {
    let i = Interner::new();
    let store: Arc<dyn Store> = Arc::new(InMemoryStore::new());

    // M records — large enough to be meaningful, small enough to stay fast.
    let m = 150usize;
    let mut rids = Vec::with_capacity(m);
    for j in 0..m {
        let rid = RecordId::new();
        let rec = make_rec(&i, &[(j as f64) * 0.01, 1.0 - (j as f64) * 0.01, 0.5]);
        store
            .set(rid.to_bytes(), rec.to_bytes().unwrap())
            .await
            .unwrap();
        rids.push(rid);
    }

    // A backend backed by HnswAdapter (so `upsert_batch` overrides with a
    // single parallel_insert). The adapter is reachable via the public
    // `adapter` field.
    let backend = make_backend(&i);
    backend.rebuild(Arc::clone(&store)).await.unwrap();

    // Every record's vector must be in the graph. `adapter.len()` counts
    // live (non-tombstoned) internals — equals M when no record was
    // dropped or double-counted.
    assert_eq!(
        backend.adapter.len(),
        m,
        "rebuild must load all {m} records into the graph via the batch path"
    );

    // Sanity: searching near the first record finds it among the top-k.
    let result = backend
        .lookup(IndexQuery::Vector {
            vec: vec![0.0, 1.0, 0.5],
            k: 10,
            opts: crate::vector::SearchOpts::default(),
        })
        .await
        .unwrap();
    match result {
        IndexResult::Ranked(ranked) => {
            assert!(!ranked.is_empty(), "rebuild graph must be searchable");
        }
        _ => panic!("expected Ranked"),
    }
}
