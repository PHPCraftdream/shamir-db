//! VR-10 (#432): one vector index per table — DDL validation.
//!
//! Background: `staged_vectors` in `TxContext` is keyed by the TABLE token
//! (not per-index), and `promote_vectors` / `apply_vector_batch` drive the
//! SAME batch through every vector backend on the table
//! (`commit_phases.rs`). A second vector index with a different `dim` →
//! `DimMismatch` and a failed promote. Until the staging/promote pipeline is
//! reworked to key vectors per-index, the DDL must refuse a second vector
//! index on a table that already has one.
//!
//! These tests pin the guard: the second `create_index_v2(index_type=vector)`
//! on the SAME table is rejected with a clear message; a first index still
//! succeeds; a non-vector (fts) index coexisting with a vector index is still
//! allowed (the cross-broadcast bug is vector↔vector only).

use crate::table::table_manager::TableManager;
use shamir_query_types::admin::types::CreateIndexOp;
use shamir_storage::storage_in_memory::InMemoryStore;
use shamir_storage::types::Store;
use std::sync::Arc;

fn make_stores() -> (Arc<dyn Store>, Arc<dyn Store>) {
    let data: Arc<dyn Store> = Arc::new(InMemoryStore::new());
    let info: Arc<dyn Store> = Arc::new(InMemoryStore::new());
    (data, info)
}

fn vector_op(name: &str, field: &str, dim: u32) -> CreateIndexOp {
    CreateIndexOp {
        create_index: name.into(),
        table: "vecs".into(),
        fields: vec![vec![field.into()]],
        unique: false,
        sorted: false,
        repo: "main".into(),
        index_type: Some("vector".into()),
        fts_tokenizer: None,
        fts_language: None,
        functional_op: None,
        functional_args: None,
        vector_dim: Some(dim),
        vector_metric: Some("cosine".into()),
        vector_quantization: None,
        include: Vec::new(),
        if_not_exists: false,
    }
}

fn fts_op(name: &str, field: &str) -> CreateIndexOp {
    CreateIndexOp {
        create_index: name.into(),
        table: "vecs".into(),
        fields: vec![vec![field.into()]],
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
        vector_quantization: None,
        include: Vec::new(),
        if_not_exists: false,
    }
}

#[tokio::test]
async fn first_vector_index_succeeds() {
    let (data_store, info_store) = make_stores();
    let mgr = TableManager::create("vecs".into(), data_store, info_store)
        .await
        .unwrap();
    // Sanity: the FIRST vector index is accepted.
    mgr.create_index_v2(&vector_op("vec_idx", "embedding", 3))
        .await
        .expect("first vector index must succeed");
}

#[tokio::test]
async fn second_vector_index_same_field_is_rejected() {
    let (data_store, info_store) = make_stores();
    let mgr = TableManager::create("vecs".into(), data_store, info_store)
        .await
        .unwrap();
    mgr.create_index_v2(&vector_op("vec_idx", "embedding", 3))
        .await
        .expect("first vector index must succeed");

    // Same field, same dim — still rejected (one vector index per table,
    // regardless of field/dim overlap).
    let err = mgr
        .create_index_v2(&vector_op("vec_idx_2", "embedding", 3))
        .await
        .expect_err("second vector index on same table must be rejected");
    let msg = err.to_string();
    assert!(
        msg.contains("vector index") && msg.contains("already"),
        "error must explain the one-vector-index-per-table limit; got: {msg}"
    );
}

#[tokio::test]
async fn second_vector_index_different_field_and_dim_is_rejected() {
    // The motivating bug: two vector indexes with different fields AND
    // different dims. Before the guard this was silently accepted, and a
    // subsequent vector insert hit DimMismatch in promote_vectors.
    let (data_store, info_store) = make_stores();
    let mgr = TableManager::create("vecs".into(), data_store, info_store)
        .await
        .unwrap();
    mgr.create_index_v2(&vector_op("vec_a", "embedding_a", 128))
        .await
        .expect("first vector index must succeed");

    let err = mgr
        .create_index_v2(&vector_op("vec_b", "embedding_b", 256))
        .await
        .expect_err("second vector index (different field + dim) must be rejected");
    let msg = err.to_string();
    assert!(
        msg.contains("vector index") && msg.contains("already"),
        "error must explain the one-vector-index-per-table limit; got: {msg}"
    );
}

#[tokio::test]
async fn fts_index_coexists_with_vector_index() {
    // The cross-broadcast bug is vector↔vector only: a non-vector index2
    // backend does NOT consume `staged_vectors`, so it must still coexist.
    let (data_store, info_store) = make_stores();
    let mgr = TableManager::create("vecs".into(), data_store, info_store)
        .await
        .unwrap();
    mgr.create_index_v2(&vector_op("vec_idx", "embedding", 3))
        .await
        .expect("vector index must succeed");
    mgr.create_index_v2(&fts_op("title_fts", "title"))
        .await
        .expect("fts index must coexist with a vector index");
}
