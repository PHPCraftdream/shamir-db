//! Integration test: index2 migration cutover at the TableManager level.
//!
//! Verifies `replicate_index2_descriptors_from` + `bulk_populate_index2`.
//! Simulates a migration manually:
//!   1. open src TableManager, create index2 indexes, insert records
//!   2. open dst TableManager on separate stores; intern the same field
//!      names in the same order so InternerKey ids align (the real
//!      MigrationCoordinator does NOT replicate interner state — that's
//!      task #102; for this test we control the interner manually so we
//!      can exercise the index2 cutover logic in isolation)
//!   3. copy src data_store bytes verbatim into dst data_store
//!   4. `dst.replicate_index2_descriptors_from(&src)` — creates empty
//!      backends on dst with paths re-interned through dst's interner
//!   5. `dst.bulk_populate_index2()` — streams dst data_store records
//!      and creates postings + in-memory state on dst's backends
//!   6. query dst's backends directly — assert results match src

use crate::index2::backend::{FtsMode, IndexQuery, IndexResult};
use crate::index2::functional_backend::FunctionalBackend;
use crate::index2::tokenizer::token_hash;
use crate::table::table_manager::TableManager;
use bytes::Bytes;
use futures::StreamExt;
use shamir_query_types::admin::types::CreateIndexOp;
use shamir_storage::storage_in_memory::InMemoryStore;
use shamir_storage::types::Store;
use shamir_types::core::interner::{InternerKey, TouchInd};
use shamir_types::types::common::new_map_wc;
use shamir_types::types::value::InnerValue;
use std::sync::Arc;

async fn copy_data_store(src: &Arc<dyn Store>, dst: &Arc<dyn Store>) {
    let mut stream = src.iter_stream(256);
    while let Some(batch_res) = stream.next().await {
        let batch = batch_res.unwrap();
        for (key, val) in batch {
            dst.set(key, Bytes::copy_from_slice(&val)).await.unwrap();
        }
    }
}

/// Intern the same list of names in both interners in the same order so
/// `InternerKey` ids align. The real migration code (#102) needs to do
/// this via info_store replication; here we pre-warm both interners.
async fn warm_interners(tms: &[&TableManager], names: &[&str]) {
    for n in names {
        for tm in tms {
            let i = tm.interner().get().await.unwrap();
            let _ = i.touch_ind(n).unwrap();
        }
    }
}

#[tokio::test]
async fn migrate_index2_fts() {
    let src_data: Arc<dyn Store> = Arc::new(InMemoryStore::new());
    let src_info: Arc<dyn Store> = Arc::new(InMemoryStore::new());
    let dst_data: Arc<dyn Store> = Arc::new(InMemoryStore::new());
    let dst_info: Arc<dyn Store> = Arc::new(InMemoryStore::new());

    let src = TableManager::create("docs".into(), Arc::clone(&src_data), Arc::clone(&src_info))
        .await
        .unwrap();
    let dst = TableManager::create("docs".into(), Arc::clone(&dst_data), Arc::clone(&dst_info))
        .await
        .unwrap();

    warm_interners(&[&src, &dst], &["body"]).await;

    let op = CreateIndexOp {
        create_index: "body_fts".into(),
        table: "docs".into(),
        fields: vec![vec!["body".into()]],
        unique: false,
        sorted: false,
        repo: "main".into(),
        index_type: Some("fts".into()),
        fts_tokenizer: Some("whitespace".into()),
        fts_language: None,
        functional_op: None,
        functional_args: None,
        vector_dim: None,
        vector_metric: None,
    };
    src.create_index_v2(&op).await.unwrap();

    let body_k = {
        let i = src.interner().get().await.unwrap();
        match i.touch_ind("body").unwrap() {
            TouchInd::Exists(k) | TouchInd::New(k) => k.id(),
        }
    };

    let records = [
        "hello rust world",
        "rust is great",
        "hello python",
        "goodbye world",
        "hello world rust",
    ];
    for body in &records {
        let mut m = new_map_wc(1);
        m.insert(InternerKey::new(body_k), InnerValue::Str((*body).into()));
        src.insert(&InnerValue::Map(m)).await.unwrap();
    }

    // --- simulate migration: copy data_store + replicate + populate ---
    copy_data_store(&src_data, &dst_data).await;
    dst.replicate_index2_descriptors_from(&src).await.unwrap();
    dst.bulk_populate_index2().await.unwrap();

    // dst should now have 1 backend
    let backends = dst.index2_registry().all_backends().await;
    assert_eq!(backends.len(), 1, "expected 1 FTS backend on dst");
    let fts = &backends[0];

    let result = fts
        .lookup(IndexQuery::Fts {
            tokens: vec![token_hash("hello"), token_hash("world")],
            mode: FtsMode::AndAll,
        })
        .await
        .unwrap();
    let hit_count = match &result {
        IndexResult::Ranked(r) => r.len(),
        IndexResult::Set(s) => s.len(),
    };
    assert_eq!(
        hit_count, 2,
        "FTS 'hello AND world' should match 2: {result:?}"
    );
}

#[tokio::test]
async fn migrate_index2_functional() {
    let src_data: Arc<dyn Store> = Arc::new(InMemoryStore::new());
    let src_info: Arc<dyn Store> = Arc::new(InMemoryStore::new());
    let dst_data: Arc<dyn Store> = Arc::new(InMemoryStore::new());
    let dst_info: Arc<dyn Store> = Arc::new(InMemoryStore::new());

    let src = TableManager::create("docs".into(), Arc::clone(&src_data), Arc::clone(&src_info))
        .await
        .unwrap();
    let dst = TableManager::create("docs".into(), Arc::clone(&dst_data), Arc::clone(&dst_info))
        .await
        .unwrap();

    warm_interners(&[&src, &dst], &["email", "name"]).await;

    let op = CreateIndexOp {
        create_index: "email_lower".into(),
        table: "docs".into(),
        fields: vec![vec!["email".into()]],
        unique: false,
        sorted: false,
        repo: "main".into(),
        index_type: Some("functional".into()),
        fts_tokenizer: None,
        fts_language: None,
        functional_op: Some("lower".into()),
        functional_args: None,
        vector_dim: None,
        vector_metric: None,
    };
    src.create_index_v2(&op).await.unwrap();

    let (email_k, name_k) = {
        let i = src.interner().get().await.unwrap();
        let resolve = |s: &str| match i.touch_ind(s).unwrap() {
            TouchInd::Exists(k) | TouchInd::New(k) => k.id(),
        };
        (resolve("email"), resolve("name"))
    };

    let rows = [
        ("Alice@FOO.com", "alice"),
        ("BOB@bar.org", "bob"),
        ("alice@foo.com", "alice2"),
        ("Charlie@BAZ.net", "charlie"),
    ];
    for (email, name) in &rows {
        let mut m = new_map_wc(2);
        m.insert(InternerKey::new(email_k), InnerValue::Str((*email).into()));
        m.insert(InternerKey::new(name_k), InnerValue::Str((*name).into()));
        src.insert(&InnerValue::Map(m)).await.unwrap();
    }

    copy_data_store(&src_data, &dst_data).await;
    dst.replicate_index2_descriptors_from(&src).await.unwrap();
    dst.bulk_populate_index2().await.unwrap();

    let backends = dst.index2_registry().all_backends().await;
    assert_eq!(backends.len(), 1);

    let hash = FunctionalBackend::hash_value(&InnerValue::Str("alice@foo.com".into()));
    let result = backends[0]
        .lookup(IndexQuery::Point {
            keys: smallvec::smallvec![hash.to_vec()],
        })
        .await
        .unwrap();
    let n = match &result {
        IndexResult::Set(s) => s.len(),
        _ => panic!("expected Set"),
    };
    assert_eq!(
        n, 2,
        "lower(email)=alice@foo.com should match 2 (Alice@FOO.com + alice@foo.com)"
    );
}

#[tokio::test]
async fn migrate_index2_vector() {
    let src_data: Arc<dyn Store> = Arc::new(InMemoryStore::new());
    let src_info: Arc<dyn Store> = Arc::new(InMemoryStore::new());
    let dst_data: Arc<dyn Store> = Arc::new(InMemoryStore::new());
    let dst_info: Arc<dyn Store> = Arc::new(InMemoryStore::new());

    let src = TableManager::create("docs".into(), Arc::clone(&src_data), Arc::clone(&src_info))
        .await
        .unwrap();
    let dst = TableManager::create("docs".into(), Arc::clone(&dst_data), Arc::clone(&dst_info))
        .await
        .unwrap();

    warm_interners(&[&src, &dst], &["embedding", "label"]).await;

    let op = CreateIndexOp {
        create_index: "vec_idx".into(),
        table: "docs".into(),
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
    };
    src.create_index_v2(&op).await.unwrap();

    let (emb_k, lbl_k) = {
        let i = src.interner().get().await.unwrap();
        let resolve = |s: &str| match i.touch_ind(s).unwrap() {
            TouchInd::Exists(k) | TouchInd::New(k) => k.id(),
        };
        (resolve("embedding"), resolve("label"))
    };

    let vecs: [(&[f64], &str); 5] = [
        (&[1.0, 0.0, 0.0], "x"),
        (&[0.0, 1.0, 0.0], "y"),
        (&[0.95, 0.05, 0.0], "x_near"),
        (&[0.0, 0.0, 1.0], "z"),
        (&[0.9, 0.05, 0.05], "x_near2"),
    ];
    for (v, lbl) in &vecs {
        let mut m = new_map_wc(2);
        m.insert(
            InternerKey::new(emb_k),
            InnerValue::List(v.iter().map(|f| InnerValue::F64(*f)).collect()),
        );
        m.insert(InternerKey::new(lbl_k), InnerValue::Str((*lbl).into()));
        src.insert(&InnerValue::Map(m)).await.unwrap();
    }

    copy_data_store(&src_data, &dst_data).await;
    dst.replicate_index2_descriptors_from(&src).await.unwrap();
    dst.bulk_populate_index2().await.unwrap();

    let backends = dst.index2_registry().all_backends().await;
    assert_eq!(backends.len(), 1);

    let result = backends[0]
        .lookup(IndexQuery::Vector {
            vec: vec![1.0, 0.0, 0.0],
            k: 3,
        })
        .await
        .unwrap();
    match &result {
        IndexResult::Ranked(ranked) => {
            assert_eq!(ranked.len(), 3, "expected top-3, got {ranked:?}");
        }
        _ => panic!("expected Ranked"),
    }
}
