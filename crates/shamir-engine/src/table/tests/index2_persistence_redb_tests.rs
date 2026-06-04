//! Integration test: index2 full round-trip through redb (disk-backed store).
//!
//! Unlike `index2_persistence_tests` which uses `InMemoryStore`, this test
//! exercises the **real serialisation path** (`Store::set` / `Store::get`
//! → redb tables on disk). The cycle is:
//!
//! 1. Open RedbRepo on a tempdir → get `data` + `info` stores.
//! 2. Create TableManager, create 3 indexes (FTS, Functional, Vector).
//! 3. Insert records, query all 3 indexes, remember results.
//! 4. Drop TableManager **and** the RedbRepo (closes the database file).
//! 5. Reopen RedbRepo on the **same** tempdir → fresh stores.
//! 6. Reopen TableManager → indexes are restored + rebuilt from disk.
//! 7. Same 3 queries → identical results.

use crate::index2::backend::{FtsMode, IndexQuery, IndexResult};
use crate::index2::functional_backend::FunctionalBackend;
use crate::index2::tokenizer::token_hash;
use crate::table::table_manager::TableManager;
use shamir_query_types::admin::types::CreateIndexOp;
use shamir_storage::storage_redb::RedbRepo;
use shamir_storage::types::Repo;
use shamir_types::core::interner::{InternerKey, TouchInd};
use shamir_types::types::common::new_map_wc;
use shamir_types::types::value::InnerValue;
use std::sync::Arc;

/// Helper: open two redb-backed stores (`data` / `info`) from one database.
async fn open_redb_stores(
    dir: &std::path::Path,
) -> (
    Arc<RedbRepo>,
    Arc<dyn shamir_storage::types::Store>,
    Arc<dyn shamir_storage::types::Store>,
) {
    let repo = Arc::new(RedbRepo::new(dir.join("db.redb")).unwrap());
    let data = repo.store_get("data").await.unwrap();
    let info = repo.store_get("info").await.unwrap();
    (repo, data, info)
}

/// Build a record: `{ body: <s>, email: <s>, emb: <vec>, label: <s> }`.
#[allow(clippy::too_many_arguments)] // test helper: each arg maps to a record field
fn make_record(
    body_key: u64,
    email_key: u64,
    emb_key: u64,
    lbl_key: u64,
    body: &str,
    email: &str,
    emb: &[f64],
    label: &str,
) -> InnerValue {
    let mut m = new_map_wc(4);
    m.insert(InternerKey::new(body_key), InnerValue::Str(body.into()));
    m.insert(InternerKey::new(email_key), InnerValue::Str(email.into()));
    m.insert(
        InternerKey::new(emb_key),
        InnerValue::List(emb.iter().map(|f| InnerValue::F64(*f)).collect()),
    );
    m.insert(InternerKey::new(lbl_key), InnerValue::Str(label.into()));
    InnerValue::Map(m)
}

/// Extract the `label` from a stored record (by reading raw bytes from data_store).
async fn label_of(
    data_store: &Arc<dyn shamir_storage::types::Store>,
    rid: &shamir_types::types::record_id::RecordId,
    lbl_key: u64,
) -> String {
    let bytes = data_store.get(rid.to_bytes()).await.unwrap();
    let rec = InnerValue::from_bytes(&bytes).unwrap();
    match &rec {
        InnerValue::Map(m) => match m.get(&InternerKey::new(lbl_key)) {
            Some(InnerValue::Str(s)) => s.clone(),
            _ => "?".into(),
        },
        _ => "?".into(),
    }
}

#[tokio::test]
async fn index2_round_trip_through_redb() {
    let temp_dir = tempfile::tempdir().unwrap();

    // ---------- helpers for index creation ----------
    let mk_fts_op = || CreateIndexOp {
        create_index: "body_fts".into(),
        table: "main".into(),
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
        if_not_exists: false,
    };

    let mk_func_op = || CreateIndexOp {
        create_index: "email_lower".into(),
        table: "main".into(),
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
        if_not_exists: false,
    };

    let mk_vec_op = || CreateIndexOp {
        create_index: "emb_vec".into(),
        table: "main".into(),
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
        if_not_exists: false,
    };

    // ==================================================================
    // Phase 1 — open redb, create indexes, insert, query
    // ==================================================================
    let (_repo1, data1, info1) = open_redb_stores(temp_dir.path()).await;

    let mgr1 = TableManager::create("main".into(), Arc::clone(&data1), Arc::clone(&info1))
        .await
        .unwrap();

    mgr1.create_index_v2(&mk_fts_op()).await.unwrap();
    mgr1.create_index_v2(&mk_func_op()).await.unwrap();
    mgr1.create_index_v2(&mk_vec_op()).await.unwrap();

    // Intern field keys.
    let (body_k, email_k, emb_k, lbl_k) = {
        let ig = mgr1.interner().get().await.unwrap();
        let resolve = |s: &str| match ig.touch_ind(s).unwrap() {
            TouchInd::Exists(k) | TouchInd::New(k) => k.id(),
        };
        (
            resolve("body"),
            resolve("email"),
            resolve("embedding"),
            resolve("label"),
        )
    };

    // Insert 6 records.
    let records = [
        (
            "hello rust world",
            "Alice@FOO.com",
            &[1.0, 0.0, 0.0][..],
            "a",
        ),
        ("rust is fast", "Bob@Bar.com", &[0.0, 1.0, 0.0], "b"),
        ("hello world", "alice@foo.com", &[0.95, 0.05, 0.0], "c"),
        ("the rust language", "CAROL@BAZ.com", &[0.0, 0.0, 1.0], "d"),
        ("goodbye world", "Dave@QUX.com", &[0.5, 0.5, 0.0], "e"),
        (
            "rust rust rust great",
            "carol@baz.com",
            &[0.1, 0.9, 0.0],
            "f",
        ),
    ];

    for (body, email, emb, label) in &records {
        let rec = make_record(body_k, email_k, emb_k, lbl_k, body, email, emb, label);
        mgr1.insert(&rec).await.unwrap();
    }

    // Persist interner.
    mgr1.interner().persist().await.unwrap();

    // --- Query phase 1 ---

    // FTS: search "rust"
    let reg = Arc::clone(mgr1.index2_registry());
    let backends = reg.all_backends().await;
    assert_eq!(backends.len(), 3, "expected 3 index2 backends");

    let fts_be = backends
        .iter()
        .find(|b| {
            matches!(
                b.descriptor().kind,
                crate::index2::kind::IndexKind::Fts { .. }
            )
        })
        .expect("FTS backend not found");

    let fts_res1 = fts_be
        .lookup(IndexQuery::Fts {
            tokens: vec![token_hash("rust")],
            mode: FtsMode::AndAll,
        })
        .await
        .unwrap();

    let mut fts_labels1: Vec<String> = match &fts_res1 {
        IndexResult::Ranked(ranked) => {
            let mut labels = Vec::new();
            for (rid, _score) in ranked {
                labels.push(label_of(&data1, rid, lbl_k).await);
            }
            labels
        }
        _ => panic!("FTS: expected Ranked result"),
    };
    fts_labels1.sort();
    assert!(
        fts_labels1.len() >= 4,
        "FTS phase1: expected >= 4 results, got {}: {fts_labels1:?}",
        fts_labels1.len()
    );

    // Functional: lower(email) == "alice@foo.com"
    let func_be = backends
        .iter()
        .find(|b| {
            matches!(
                b.descriptor().kind,
                crate::index2::kind::IndexKind::Functional(_)
            )
        })
        .expect("Functional backend not found");

    let alice_hash = FunctionalBackend::hash_value(&InnerValue::Str("alice@foo.com".into()));
    let func_res1 = func_be
        .lookup(IndexQuery::Point {
            keys: smallvec::smallvec![alice_hash.to_vec()],
        })
        .await
        .unwrap();

    let func_labels1: Vec<String> = {
        let mut v = Vec::new();
        if let IndexResult::Set(ids) = &func_res1 {
            for rid in ids {
                let bytes = data1.get(rid.to_bytes()).await.unwrap();
                let rec = InnerValue::from_bytes(&bytes).unwrap();
                let lbl = match &rec {
                    InnerValue::Map(m) => match m.get(&InternerKey::new(lbl_k)) {
                        Some(InnerValue::Str(s)) => s.clone(),
                        _ => "?".into(),
                    },
                    _ => "?".into(),
                };
                v.push(lbl);
            }
        }
        v.sort();
        v
    };
    // "Alice@FOO.com".to_lowercase() and "alice@foo.com".to_lowercase() both equal "alice@foo.com"
    assert_eq!(
        func_labels1,
        vec!["a".to_string(), "c".to_string()],
        "Functional phase1: expected [a, c], got {func_labels1:?}"
    );

    // Vector: top-2 nearest to [1.0, 0.0, 0.0]
    let vec_be = backends
        .iter()
        .find(|b| {
            matches!(
                b.descriptor().kind,
                crate::index2::kind::IndexKind::Vector(_)
            )
        })
        .expect("Vector backend not found");

    let vec_res1 = vec_be
        .lookup(IndexQuery::Vector {
            vec: vec![1.0, 0.0, 0.0],
            k: 2,
        })
        .await
        .unwrap();

    let vec_labels1: Vec<String> = match &vec_res1 {
        IndexResult::Ranked(ranked) => {
            assert_eq!(ranked.len(), 2, "Vector phase1: expected top-2");
            let mut labels = Vec::new();
            for (rid, _score) in ranked {
                labels.push(label_of(&data1, rid, lbl_k).await);
            }
            labels
        }
        _ => panic!("Vector: expected Ranked result"),
    };

    // --- Drop everything (closes the database file) ---
    // Drop in reverse order: backends → registry → mgr → stores → repo.
    drop(backends);
    drop(reg);
    drop(mgr1);
    drop(data1);
    drop(info1);
    drop(_repo1);

    // ==================================================================
    // Phase 2 — reopen redb on the same directory, rebuild, query again
    // ==================================================================
    let (_repo2, data2, info2) = open_redb_stores(temp_dir.path()).await;

    let mgr2 = TableManager::create("main".into(), Arc::clone(&data2), Arc::clone(&info2))
        .await
        .unwrap();

    // All 3 backends must be restored.
    let reg2 = Arc::clone(mgr2.index2_registry());
    let backends2 = reg2.all_backends().await;
    assert_eq!(
        backends2.len(),
        3,
        "phase2: expected 3 backends, got {}",
        backends2.len()
    );

    let fts_be2 = backends2
        .iter()
        .find(|b| {
            matches!(
                b.descriptor().kind,
                crate::index2::kind::IndexKind::Fts { .. }
            )
        })
        .expect("FTS backend not found after reopen");

    let func_be2 = backends2
        .iter()
        .find(|b| {
            matches!(
                b.descriptor().kind,
                crate::index2::kind::IndexKind::Functional(_)
            )
        })
        .expect("Functional backend not found after reopen");

    let vec_be2 = backends2
        .iter()
        .find(|b| {
            matches!(
                b.descriptor().kind,
                crate::index2::kind::IndexKind::Vector(_)
            )
        })
        .expect("Vector backend not found after reopen");

    // FTS query — same results.
    let fts_res2 = fts_be2
        .lookup(IndexQuery::Fts {
            tokens: vec![token_hash("rust")],
            mode: FtsMode::AndAll,
        })
        .await
        .unwrap();

    let mut fts_labels2: Vec<String> = match &fts_res2 {
        IndexResult::Ranked(ranked) => {
            let mut labels = Vec::new();
            for (rid, _score) in ranked {
                labels.push(label_of(&data2, rid, lbl_k).await);
            }
            labels
        }
        _ => panic!("FTS phase2: expected Ranked result"),
    };
    fts_labels2.sort();
    assert_eq!(
        fts_labels1, fts_labels2,
        "FTS: labels after reopen must match phase1"
    );

    // Functional query — same results.
    let func_res2 = func_be2
        .lookup(IndexQuery::Point {
            keys: smallvec::smallvec![alice_hash.to_vec()],
        })
        .await
        .unwrap();

    let func_labels2: Vec<String> = {
        let mut v = Vec::new();
        if let IndexResult::Set(ids) = &func_res2 {
            for rid in ids {
                let bytes = data2.get(rid.to_bytes()).await.unwrap();
                let rec = InnerValue::from_bytes(&bytes).unwrap();
                let lbl = match &rec {
                    InnerValue::Map(m) => match m.get(&InternerKey::new(lbl_k)) {
                        Some(InnerValue::Str(s)) => s.clone(),
                        _ => "?".into(),
                    },
                    _ => "?".into(),
                };
                v.push(lbl);
            }
        }
        v.sort();
        v
    };
    assert_eq!(
        func_labels1, func_labels2,
        "Functional: labels after reopen must match phase1"
    );

    // Vector query — same results.
    let vec_res2 = vec_be2
        .lookup(IndexQuery::Vector {
            vec: vec![1.0, 0.0, 0.0],
            k: 2,
        })
        .await
        .unwrap();

    let vec_labels2: Vec<String> = match &vec_res2 {
        IndexResult::Ranked(ranked) => {
            assert_eq!(ranked.len(), 2, "Vector phase2: expected top-2");
            let mut labels = Vec::new();
            for (rid, _score) in ranked {
                labels.push(label_of(&data2, rid, lbl_k).await);
            }
            labels
        }
        _ => panic!("Vector phase2: expected Ranked result"),
    };
    assert_eq!(
        vec_labels1, vec_labels2,
        "Vector: labels after reopen must match phase1"
    );
}
