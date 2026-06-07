//! Integration test: vector HNSW index survives TableManager reopen.
//!
//! The HNSW graph lives in RAM (inside `HnswAdapter`). On reopen the
//! descriptor is restored from `__meta__/indexes` and `rebuild(data_store)`
//! repopulates the graph from persisted records. This test asserts that
//! the same similarity query returns the same top-k after a full drop +
//! reopen cycle.

use crate::table::table_manager::TableManager;
use shamir_query_types::admin::types::CreateIndexOp;
use shamir_storage::storage_in_memory::InMemoryStore;
use shamir_storage::types::Store;
use shamir_types::core::interner::{InternerKey, TouchInd};
use shamir_types::types::common::new_map_wc;
use shamir_types::types::value::InnerValue;
use std::sync::Arc;

/// Shared stores so that dropping the TableManager does NOT lose data.
fn make_stores() -> (Arc<dyn Store>, Arc<dyn Store>) {
    let data: Arc<dyn Store> = Arc::new(InMemoryStore::new());
    let info: Arc<dyn Store> = Arc::new(InMemoryStore::new());
    (data, info)
}

#[tokio::test]
async fn vector_index_survives_reopen() {
    let (data_store, info_store) = make_stores();

    // --- Phase 1: create index, insert records, query ---
    let mgr1 = TableManager::create(
        "vecs".into(),
        Arc::clone(&data_store),
        Arc::clone(&info_store),
    )
    .await
    .unwrap();

    // Create vector index via create_index_v2.
    let op = CreateIndexOp {
        create_index: "vec_idx".into(),
        table: "vecs".into(),
        fields: vec![vec!["embedding".into()]],
        unique: false,
        sorted: false,
        repo: "main".into(),
        index_type: Some("vector".into()),
        fts_tokenizer: None,
        fts_language: None,
        functional_op: None,
        functional_args: None,
        vector_dim: Some(3),
        vector_metric: Some("cosine".into()),
        include: Vec::new(),
        if_not_exists: false,
    };
    mgr1.create_index_v2(&op).await.unwrap();

    // Intern field keys so we can build InnerValue records.
    let (emb_key_id, lbl_key_id) = {
        let interner = mgr1.interner().get().await.unwrap();
        let emb = match interner.touch_ind("embedding").unwrap() {
            TouchInd::Exists(k) | TouchInd::New(k) => k.id(),
        };
        let lbl = match interner.touch_ind("label").unwrap() {
            TouchInd::Exists(k) | TouchInd::New(k) => k.id(),
        };
        (emb, lbl)
    };

    // Insert 5 records with known vectors.
    let vectors: [(&[f64], &str); 5] = [
        (&[1.0, 0.0, 0.0], "x"),
        (&[0.0, 1.0, 0.0], "y"),
        (&[0.95, 0.05, 0.0], "x_near"),
        (&[0.0, 0.0, 1.0], "z"),
        (&[0.5, 0.5, 0.0], "diag"),
    ];

    for (vec, label) in &vectors {
        let mut m = new_map_wc(2);
        m.insert(
            InternerKey::new(emb_key_id),
            InnerValue::List(vec.iter().map(|f| InnerValue::F64(*f)).collect()),
        );
        m.insert(
            InternerKey::new(lbl_key_id),
            InnerValue::Str((*label).into()),
        );
        let rec = InnerValue::Map(m);
        mgr1.insert(&rec).await.unwrap();
    }

    // Persist interner so keys survive reopen.
    mgr1.interner().persist().await.unwrap();

    // Query vector similarity via index2 registry.
    use crate::index2::backend::{IndexQuery, IndexResult};
    let registry = mgr1.index2_registry();
    let backends = registry.all_backends().await;
    assert_eq!(backends.len(), 1, "expected exactly 1 index2 backend");
    let backend = &backends[0];

    let result = backend
        .lookup(IndexQuery::Vector {
            vec: vec![1.0, 0.0, 0.0],
            k: 2,
        })
        .await
        .unwrap();
    let phase1_labels = match &result {
        IndexResult::Ranked(ranked) => {
            assert_eq!(ranked.len(), 2, "phase1: expected top-2, got {ranked:?}");
            let mut labels = Vec::new();
            for (rid, _score) in ranked {
                let rec_bytes = data_store.get(rid.to_bytes()).await.unwrap();
                let rec = InnerValue::from_bytes(&rec_bytes).unwrap();
                let lbl = match &rec {
                    InnerValue::Map(m) => match m.get(&InternerKey::new(lbl_key_id)) {
                        Some(InnerValue::Str(s)) => s.clone(),
                        _ => "?".into(),
                    },
                    _ => "?".into(),
                };
                labels.push(lbl);
            }
            labels
        }
        _ => panic!("phase1: expected Ranked result"),
    };

    // --- Drop TableManager 1 ---
    drop(mgr1);

    // --- Phase 2: reopen on same stores ---
    let mgr2 = TableManager::create(
        "vecs".into(),
        Arc::clone(&data_store),
        Arc::clone(&info_store),
    )
    .await
    .unwrap();

    // Verify index was restored.
    let registry2 = mgr2.index2_registry();
    let backends2 = registry2.all_backends().await;
    assert_eq!(
        backends2.len(),
        1,
        "phase2: expected 1 restored backend, got {}",
        backends2.len()
    );
    let backend2 = &backends2[0];
    assert!(
        matches!(
            backend2.descriptor().kind,
            crate::index2::kind::IndexKind::Vector(_)
        ),
        "expected Vector kind"
    );

    // Same query — must return the same labels.
    let result2 = backend2
        .lookup(IndexQuery::Vector {
            vec: vec![1.0, 0.0, 0.0],
            k: 2,
        })
        .await
        .unwrap();
    match &result2 {
        IndexResult::Ranked(ranked) => {
            assert_eq!(
                ranked.len(),
                2,
                "phase2: expected top-2, got {} results: {ranked:?}",
                ranked.len()
            );
            let mut phase2_labels = Vec::new();
            for (rid, _score) in ranked {
                let rec_bytes = data_store.get(rid.to_bytes()).await.unwrap();
                let rec = InnerValue::from_bytes(&rec_bytes).unwrap();
                let lbl = match &rec {
                    InnerValue::Map(m) => match m.get(&InternerKey::new(lbl_key_id)) {
                        Some(InnerValue::Str(s)) => s.clone(),
                        _ => "?".into(),
                    },
                    _ => "?".into(),
                };
                phase2_labels.push(lbl);
            }
            assert_eq!(
                phase1_labels, phase2_labels,
                "labels after reopen must match phase1 labels"
            );
        }
        _ => panic!("phase2: expected Ranked result"),
    }
}

#[tokio::test]
async fn fts_ranked_index_survives_reopen() {
    let (data_store, info_store) = make_stores();

    // --- Phase 1: create FTS index, insert records, query ---
    let mgr1 = TableManager::create(
        "docs".into(),
        Arc::clone(&data_store),
        Arc::clone(&info_store),
    )
    .await
    .unwrap();

    // Create FTS index on "body" field.
    let op = CreateIndexOp {
        create_index: "body_fts".into(),
        table: "docs".into(),
        fields: vec![vec!["body".into()]],
        unique: false,
        sorted: false,
        repo: "main".into(),
        index_type: Some("fts".into()),
        fts_tokenizer: None,
        fts_language: None,
        functional_op: None,
        functional_args: None,
        vector_dim: None,
        vector_metric: None,
        include: Vec::new(),
        if_not_exists: false,
    };
    mgr1.create_index_v2(&op).await.unwrap();

    // Intern field keys so we can build InnerValue records.
    let (body_key_id, lbl_key_id) = {
        let interner = mgr1.interner().get().await.unwrap();
        let body = match interner.touch_ind("body").unwrap() {
            TouchInd::Exists(k) | TouchInd::New(k) => k.id(),
        };
        let lbl = match interner.touch_ind("label").unwrap() {
            TouchInd::Exists(k) | TouchInd::New(k) => k.id(),
        };
        (body, lbl)
    };

    // Insert 5 records with different body lengths.
    let docs: [(&str, &str); 5] = [
        ("rust rust rust is great", "r1"),
        ("rust is ok", "r2"),
        ("rust rocks", "r3"),
        ("the rust programming language is fast and safe", "r4"),
        ("hello world", "r5"),
    ];

    for (body, label) in &docs {
        let mut m = new_map_wc(2);
        m.insert(
            InternerKey::new(body_key_id),
            InnerValue::Str((*body).into()),
        );
        m.insert(
            InternerKey::new(lbl_key_id),
            InnerValue::Str((*label).into()),
        );
        let rec = InnerValue::Map(m);
        mgr1.insert(&rec).await.unwrap();
    }

    // Persist interner so keys survive reopen.
    mgr1.interner().persist().await.unwrap();

    // Query FTS for "rust" via index2 registry.
    use crate::index2::backend::{FtsMode, IndexQuery, IndexResult};
    use crate::index2::tokenizer::token_hash;
    let registry = mgr1.index2_registry();
    let backends = registry.all_backends().await;
    assert_eq!(backends.len(), 1, "expected exactly 1 index2 backend");
    let backend = &backends[0];

    let result = backend
        .lookup(IndexQuery::Fts {
            tokens: vec![token_hash("rust")],
            mode: FtsMode::AndAll,
        })
        .await
        .unwrap();

    let phase1_labels: Vec<String> = match &result {
        IndexResult::Ranked(ranked) => {
            assert!(
                ranked.len() >= 4,
                "phase1: expected at least 4 results, got {ranked:?}"
            );
            let mut labels = Vec::new();
            for (rid, _score) in ranked {
                let rec_bytes = data_store.get(rid.to_bytes()).await.unwrap();
                let rec = InnerValue::from_bytes(&rec_bytes).unwrap();
                let lbl = match &rec {
                    InnerValue::Map(m) => match m.get(&InternerKey::new(lbl_key_id)) {
                        Some(InnerValue::Str(s)) => s.clone(),
                        _ => "?".into(),
                    },
                    _ => "?".into(),
                };
                labels.push(lbl);
            }
            labels
        }
        _ => panic!("phase1: expected Ranked result"),
    };

    // --- Drop TableManager 1 ---
    drop(mgr1);

    // --- Phase 2: reopen on same stores ---
    let mgr2 = TableManager::create(
        "docs".into(),
        Arc::clone(&data_store),
        Arc::clone(&info_store),
    )
    .await
    .unwrap();

    // Verify index was restored.
    let registry2 = mgr2.index2_registry();
    let backends2 = registry2.all_backends().await;
    assert_eq!(
        backends2.len(),
        1,
        "phase2: expected 1 restored backend, got {}",
        backends2.len()
    );
    let backend2 = &backends2[0];
    assert!(
        matches!(
            backend2.descriptor().kind,
            crate::index2::kind::IndexKind::Fts { .. }
        ),
        "expected Fts kind"
    );

    // Same query — must return the same labels in the same order.
    let result2 = backend2
        .lookup(IndexQuery::Fts {
            tokens: vec![token_hash("rust")],
            mode: FtsMode::AndAll,
        })
        .await
        .unwrap();
    match &result2 {
        IndexResult::Ranked(ranked) => {
            assert!(
                ranked.len() >= 4,
                "phase2: expected at least 4 results, got {} results: {ranked:?}",
                ranked.len()
            );
            let mut phase2_labels = Vec::new();
            for (rid, _score) in ranked {
                let rec_bytes = data_store.get(rid.to_bytes()).await.unwrap();
                let rec = InnerValue::from_bytes(&rec_bytes).unwrap();
                let lbl = match &rec {
                    InnerValue::Map(m) => match m.get(&InternerKey::new(lbl_key_id)) {
                        Some(InnerValue::Str(s)) => s.clone(),
                        _ => "?".into(),
                    },
                    _ => "?".into(),
                };
                phase2_labels.push(lbl);
            }
            assert_eq!(
                phase1_labels, phase2_labels,
                "labels after reopen must match phase1 labels"
            );
        }
        _ => panic!("phase2: expected Ranked result"),
    }
}
