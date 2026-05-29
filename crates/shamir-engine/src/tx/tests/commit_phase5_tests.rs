//! HIGH-6 Phase 5c/5d: committed-tx index + HNSW application.
//!
//! Proves the happy-path commit pipeline now:
//!  * Part A — applies `tx.index_write_set` postings to the table's
//!    `info_store` so index2 (FTS) queries surface the record after a
//!    committed `insert_tx` (Phase 5c, `apply_index_ops_at_commit`).
//!  * Part B — promotes the tx's staged vectors into the live graph
//!    under the commit lock so a non-tx vector search finds them
//!    (Phase 5d, `apply_staged_vectors`).
//!  * Part C — RAII: an aborted (dropped) tx leaves no staged vectors
//!    behind, because they live inside the `TxContext::staged_vectors`
//!    and vanish with the tx — the live graph was never touched.

use std::sync::Arc;

use shamir_query_types::admin::CreateIndexOp;
use shamir_storage::storage_in_memory::InMemoryRepo;
use shamir_tx::IsolationLevel;
use shamir_types::core::interner::{InternerKey, TouchInd};
use shamir_types::types::common::new_map_wc;
use shamir_types::types::value::InnerValue;

use crate::index2::backend::{FtsMode, IndexQuery, IndexResult};
use crate::index2::tokenizer::token_hash;
use crate::repo::repo_instance::RepoInstance;
use crate::repo::repo_types::BoxRepo;
use crate::table::table_manager::table_token_for;
use crate::table::TableConfig;

fn make_repo() -> RepoInstance {
    let repo = Arc::new(InMemoryRepo::new());
    RepoInstance::new("test".into(), BoxRepo::InMemory(repo), Vec::new())
}

async fn field_id(tbl: &crate::table::TableManager, name: &str) -> u64 {
    let interner = tbl.interner().get().await.unwrap();
    match interner.touch_ind(name).unwrap() {
        TouchInd::Exists(k) | TouchInd::New(k) => k.id(),
    }
}

fn fts_index_op() -> CreateIndexOp {
    CreateIndexOp {
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
    }
}

fn vector_index_op() -> CreateIndexOp {
    CreateIndexOp {
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
    }
}

fn text_record(body_key_id: u64, text: &str) -> InnerValue {
    let mut m = new_map_wc(1);
    m.insert(InternerKey::new(body_key_id), InnerValue::Str(text.into()));
    InnerValue::Map(m)
}

fn vec_record(emb_key_id: u64, vec: &[f64]) -> InnerValue {
    let mut m = new_map_wc(1);
    m.insert(
        InternerKey::new(emb_key_id),
        InnerValue::List(vec.iter().map(|f| InnerValue::F64(*f)).collect()),
    );
    InnerValue::Map(m)
}

/// Part A: a record inserted via `insert_tx` and committed must be
/// findable through the FTS index2 backend — proving Phase 5c applied
/// the staged `SetPosting` ops to the table's `info_store` (and the
/// `BumpFtsStats` op to the backend's in-memory BM25 stats).
#[tokio::test]
async fn committed_tx_index_postings_visible() {
    let repo = make_repo();
    repo.add_table(TableConfig::new("docs"));
    let tbl = repo.get_table("docs").await.unwrap();
    tbl.create_index_v2(&fts_index_op()).await.unwrap();

    let body_id = field_id(&tbl, "body").await;

    // Resolve the FTS backend up front.
    let name_id = field_id(&tbl, "body_fts").await;
    let backend = tbl
        .index2_registry()
        .get_by_name(name_id)
        .await
        .expect("body_fts must be registered");

    // Insert via tx then COMMIT.
    let (mut tx, guard) = repo.begin_tx(IsolationLevel::Snapshot).await.unwrap();
    let rid = tbl
        .insert_tx(
            &text_record(body_id, "shamir transaction commit"),
            Some(&mut tx),
        )
        .await
        .unwrap();
    repo.commit_tx(tx).await.unwrap();
    drop(guard);

    let result = backend
        .lookup(IndexQuery::Fts {
            tokens: vec![token_hash("transaction")],
            mode: FtsMode::AndAll,
        })
        .await
        .unwrap();

    match result {
        IndexResult::Ranked(hits) => {
            let rids: Vec<_> = hits.iter().map(|(r, _)| *r).collect();
            assert!(
                rids.contains(&rid),
                "committed insert_tx must make FTS postings visible (HIGH-6 Phase 5c); \
                 got {:?}, expected {:?}",
                rids,
                rid
            );
        }
        _ => panic!("FTS lookup must return Ranked"),
    }
}

/// Part B: a vector staged via `insert_tx` and committed must be
/// findable via a NON-tx vector search — proving Phase 5d promoted the
/// tx's staged vector (`TxContext::staged_vectors`) into the live HNSW
/// graph via `apply_staged_vectors`.
#[tokio::test]
async fn committed_tx_hnsw_vector_searchable() {
    let repo = make_repo();
    repo.add_table(TableConfig::new("vecs"));
    let tbl = repo.get_table("vecs").await.unwrap();
    tbl.create_index_v2(&vector_index_op()).await.unwrap();

    let emb_id = field_id(&tbl, "embedding").await;
    let name_id = field_id(&tbl, "vec_idx").await;
    let backend = tbl
        .index2_registry()
        .get_by_name(name_id)
        .await
        .expect("vec_idx must be registered");

    // Stage + commit a vector through a tx.
    let (mut tx, guard) = repo.begin_tx(IsolationLevel::Snapshot).await.unwrap();
    let rid = tbl
        .insert_tx(&vec_record(emb_id, &[1.0, 0.0, 0.0]), Some(&mut tx))
        .await
        .unwrap();

    // Before commit: a NON-tx search must NOT see the staged vector.
    let pre = backend
        .lookup(IndexQuery::Vector {
            vec: vec![1.0, 0.0, 0.0],
            k: 10,
        })
        .await
        .unwrap();
    if let IndexResult::Ranked(hits) = pre {
        assert!(
            !hits.iter().any(|(r, _)| *r == rid),
            "pre-commit non-tx search must not see staged vector"
        );
    }

    repo.commit_tx(tx).await.unwrap();
    drop(guard);

    // Let the spawn_blocking HNSW insert (in apply_staged_vectors) settle.
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    let post = backend
        .lookup(IndexQuery::Vector {
            vec: vec![1.0, 0.0, 0.0],
            k: 10,
        })
        .await
        .unwrap();
    match post {
        IndexResult::Ranked(hits) => {
            let rids: Vec<_> = hits.iter().map(|(r, _)| *r).collect();
            assert!(
                rids.contains(&rid),
                "committed insert_tx must promote staged vector into live HNSW \
                 graph (HIGH-6 Phase 5d); got {:?}, expected {:?}",
                rids,
                rid
            );
        }
        _ => panic!("Vector lookup must return Ranked"),
    }
}

/// Part C: aborting (dropping) a tx that staged an HNSW vector leaves no
/// trace — the staged vector lived inside `TxContext::staged_vectors` and
/// vanished with the tx (RAII). The live graph was never touched, so a
/// non-tx search finds nothing.
#[tokio::test]
async fn aborted_tx_hnsw_staged_cleaned() {
    let repo = make_repo();
    repo.add_table(TableConfig::new("vecs"));
    let tbl = repo.get_table("vecs").await.unwrap();
    tbl.create_index_v2(&vector_index_op()).await.unwrap();

    let emb_id = field_id(&tbl, "embedding").await;
    let name_id = field_id(&tbl, "vec_idx").await;
    let token = table_token_for("vecs");
    let backend = tbl
        .index2_registry()
        .get_by_name(name_id)
        .await
        .expect("vec_idx must be registered");

    let rid = {
        let (mut tx, guard) = repo.begin_tx(IsolationLevel::Snapshot).await.unwrap();
        let rid = tbl
            .insert_tx(&vec_record(emb_id, &[1.0, 0.0, 0.0]), Some(&mut tx))
            .await
            .unwrap();

        // The staged vector IS visible to an in-tx search before abort:
        // resolve the tx's own staged slice and merge it in the lookup.
        let res = backend
            .lookup_tx(
                IndexQuery::Vector {
                    vec: vec![1.0, 0.0, 0.0],
                    k: 10,
                },
                tx.staged_vectors_for(token),
            )
            .await
            .unwrap();
        let in_tx_before = match res {
            IndexResult::Ranked(hits) => hits.iter().any(|(r, _)| *r == rid),
            _ => panic!("expected Ranked"),
        };
        assert!(
            in_tx_before,
            "staged vector must be visible to an in-tx search before abort"
        );

        // Abort: just drop the tx. No explicit rollback call — RAII.
        drop(tx);
        drop(guard);
        rid
    };

    // The live graph was never touched: a non-tx search finds nothing.
    let post = backend
        .lookup(IndexQuery::Vector {
            vec: vec![1.0, 0.0, 0.0],
            k: 10,
        })
        .await
        .unwrap();
    match post {
        IndexResult::Ranked(hits) => {
            assert!(
                !hits.iter().any(|(r, _)| *r == rid),
                "aborted tx must leave no vector in the live graph (HIGH-6 Part C); got {:?}",
                hits.iter().map(|(r, _)| *r).collect::<Vec<_>>()
            );
        }
        _ => panic!("expected Ranked"),
    }
}

/// RAII guarantee, stated directly: stage a vector into a `TxContext`,
/// drop the tx without commit, and confirm (a) the live graph never saw
/// it and (b) nothing lingers — `staged_vectors` was owned by the tx, so
/// the drop freed it. Since staging is now tx-local, the "no lingering
/// buffer" half is structurally guaranteed; we assert the live-graph
/// cleanliness via a search.
#[tokio::test]
async fn dropped_tx_staged_vectors_freed_via_raii() {
    let repo = make_repo();
    repo.add_table(TableConfig::new("vecs"));
    let tbl = repo.get_table("vecs").await.unwrap();
    tbl.create_index_v2(&vector_index_op()).await.unwrap();

    let emb_id = field_id(&tbl, "embedding").await;
    let name_id = field_id(&tbl, "vec_idx").await;
    let token = table_token_for("vecs");
    let backend = tbl
        .index2_registry()
        .get_by_name(name_id)
        .await
        .expect("vec_idx must be registered");

    let rid = {
        let (mut tx, guard) = repo.begin_tx(IsolationLevel::Snapshot).await.unwrap();
        let rid = tbl
            .insert_tx(&vec_record(emb_id, &[0.0, 1.0, 0.0]), Some(&mut tx))
            .await
            .unwrap();
        // The vector lives in the tx, keyed by this table's token.
        assert_eq!(
            tx.staged_vectors_for(token).map(<[_]>::len),
            Some(1),
            "vector must be staged inside the TxContext"
        );
        drop(tx); // RAII: staged_vectors freed, no commit.
        drop(guard);
        rid
    };

    let post = backend
        .lookup(IndexQuery::Vector {
            vec: vec![0.0, 1.0, 0.0],
            k: 10,
        })
        .await
        .unwrap();
    match post {
        IndexResult::Ranked(hits) => assert!(
            !hits.iter().any(|(r, _)| *r == rid),
            "dropped tx must never reach the live graph (RAII); got {:?}",
            hits.iter().map(|(r, _)| *r).collect::<Vec<_>>()
        ),
        _ => panic!("expected Ranked"),
    }
}

/// In-tx read-your-own-writes for vectors: stage a vector, then search
/// WITHIN the same tx (threading `tx.staged_vectors_for(token)` into the
/// lookup) and find it. Proves the staged-slice wiring of `lookup_tx`.
#[tokio::test]
async fn in_tx_search_sees_own_staged_vector() {
    let repo = make_repo();
    repo.add_table(TableConfig::new("vecs"));
    let tbl = repo.get_table("vecs").await.unwrap();
    tbl.create_index_v2(&vector_index_op()).await.unwrap();

    let emb_id = field_id(&tbl, "embedding").await;
    let name_id = field_id(&tbl, "vec_idx").await;
    let token = table_token_for("vecs");
    let backend = tbl
        .index2_registry()
        .get_by_name(name_id)
        .await
        .expect("vec_idx must be registered");

    let (mut tx, _guard) = repo.begin_tx(IsolationLevel::Snapshot).await.unwrap();
    let rid = tbl
        .insert_tx(&vec_record(emb_id, &[1.0, 0.0, 0.0]), Some(&mut tx))
        .await
        .unwrap();

    let res = backend
        .lookup_tx(
            IndexQuery::Vector {
                vec: vec![1.0, 0.0, 0.0],
                k: 10,
            },
            tx.staged_vectors_for(token),
        )
        .await
        .unwrap();
    match res {
        IndexResult::Ranked(hits) => assert!(
            hits.iter().any(|(r, _)| *r == rid),
            "in-tx search must see the tx's own staged vector; got {:?}",
            hits.iter().map(|(r, _)| *r).collect::<Vec<_>>()
        ),
        _ => panic!("expected Ranked"),
    }
}
