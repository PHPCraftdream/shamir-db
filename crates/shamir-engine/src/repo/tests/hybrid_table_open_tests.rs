//! F-33 Step 3 (#837): measures whether `TableManager::create`'s existing
//! open sequence already produces a coherent result when handed stores
//! from a [`BoxRepo::Hybrid`] repo across a simulated process restart.
//!
//! Builds a hybrid repo directly (`BoxRepoFactory::hybrid(path)`), opens a
//! table by hand — mirroring `RepoInstance::create_table_context`'s exact
//! `store_get` call shape (`__data__<table>`, `__info__<table>`,
//! `__history__<table>`, `__interner__`) — exercises it, then simulates a
//! restart by dropping every handle and building a FRESH
//! `BoxRepoFactory::hybrid` over the SAME path. See the companion memo at
//! `docs/dev-artifacts/research/f33-step3-warm-info-cold-history-measurement.md`
//! for the full write-up of what each scenario proved.
//!
//! This step deliberately does NOT go through `RepoInstance`/the DDL
//! surface — that wiring is Step 4 (#838).

use std::sync::Arc;

use shamir_query_types::admin::CreateIndexOp;
use shamir_storage::types::{Repo, Store};
use shamir_types::core::interner::TouchInd;
use shamir_types::types::common::{new_map, new_map_wc};
use shamir_types::types::value::{InnerValue, QueryValue};

use crate::repo::repo_types::{BoxRepo, BoxRepoFactory, RepoFactory};
use crate::table::interner_manager::InternerManager;
use crate::table::table_manager::TableManager;
use crate::table::tests::test_helpers::query_value_to_inner_tracked;

/// Mirrors `RepoInstance::create_table_context`'s exact 4 `store_get`
/// calls (`repo_instance.rs` ~line 381-418), minus the MVCC/changefeed
/// wiring that is out of scope for this measurement — this step is
/// scoped to `TableManager::create` + `.with_interner(...)` only.
async fn open_table(repo: &BoxRepo, table_name: &str) -> TableManager {
    let data_store: Arc<dyn Store> = repo
        .store_get(format!("__data__{table_name}"))
        .await
        .unwrap();
    let info_store: Arc<dyn Store> = repo
        .store_get(format!("__info__{table_name}"))
        .await
        .unwrap();
    // `__history__<table>` — read exactly like `create_table_context`
    // does, even though this measurement doesn't attach an `MvccStore`
    // (out of scope: that's `RepoInstance`-level wiring). Fetching it
    // still exercises the hybrid repo's routing for the name.
    let _history_store: Arc<dyn Store> = repo
        .store_get(format!("__history__{table_name}"))
        .await
        .unwrap();
    let interner_store: Arc<dyn Store> = repo.store_get("__interner__").await.unwrap();
    let repo_interner = InternerManager::new(interner_store);

    let tbl = TableManager::create(table_name.to_string(), data_store, info_store)
        .await
        .unwrap();
    tbl.with_interner(repo_interner)
}

/// Build a `QueryValue::Map` with a single `field -> Int(value)` entry,
/// intern it against `tbl`'s interner, and persist any newly-touched
/// interner keys — mirroring `s9b_rebuild_on_open_tests.rs`'s pattern.
async fn interned_row(tbl: &TableManager, field: &str, value: i64) -> InnerValue {
    let interner = tbl.interner().get().await.unwrap();
    let mut m = new_map();
    m.insert(field.to_string(), QueryValue::Int(value));
    let qv = QueryValue::Map(m);
    let (iv, new_keys) = query_value_to_inner_tracked(&qv, interner).unwrap();
    if !new_keys.is_empty() {
        tbl.interner().save_new_keys(&new_keys).await.unwrap();
    }
    iv
}

// ============================================================================
// 1. Index definitions survive, postings don't.
// ============================================================================

#[tokio::test]
async fn index_definition_survives_but_postings_do_not() {
    let temp_dir = tempfile::tempdir().unwrap();

    // --- Phase 1: create table, index, insert a matching row ---
    let repo1 = BoxRepoFactory::hybrid(temp_dir.path())
        .create()
        .await
        .unwrap();
    let tbl1 = open_table(&repo1, "widgets").await;

    tbl1.create_index("by_x", &["x"]).await.unwrap();
    let iv = interned_row(&tbl1, "x", 42).await;
    tbl1.insert(&iv).await.unwrap();
    tbl1.interner().persist().await.unwrap();
    tbl1.flush_metadata().await.unwrap();

    // Confirm the index actually returns a hit pre-restart.
    let def = tbl1
        .index_manager_ref()
        .iter_indexes()
        .next()
        .expect("by_x index must exist pre-restart");
    let name_interned = def.name_interned;
    let hits_before = tbl1
        .index_manager_ref()
        .lookup_by_index(name_interned, &[InnerValue::Int(42)])
        .await
        .unwrap();
    assert_eq!(
        hits_before.len(),
        1,
        "pre-restart lookup must find the inserted row"
    );

    // --- Simulate a restart: drop every handle, then build a FRESH
    //     hybrid repo over the SAME path. ---
    drop(tbl1);
    drop(repo1);

    let repo2 = BoxRepoFactory::hybrid(temp_dir.path())
        .create()
        .await
        .unwrap();
    let tbl2 = open_table(&repo2, "widgets").await;

    // The index DEFINITION must still be visible (schema is config).
    let def2 = tbl2
        .index_manager_ref()
        .iter_indexes()
        .next()
        .expect("index definition must survive restart (config, not data)");
    assert_eq!(
        def2.name_interned, name_interned,
        "same index name must resolve to the same interned id post-restart"
    );

    // But a lookup for the same value must return NO hit — data +
    // postings are ephemeral and must be gone, not stale/wrong.
    let hits_after = tbl2
        .index_manager_ref()
        .lookup_by_index(def2.name_interned, &[InnerValue::Int(42)])
        .await
        .unwrap();
    assert_eq!(
        hits_after.len(),
        0,
        "postings must NOT survive restart on a hybrid repo"
    );
}

// ============================================================================
// 2. Validator bindings survive.
// ============================================================================

#[tokio::test]
async fn validator_bindings_survive_restart() {
    let temp_dir = tempfile::tempdir().unwrap();

    let repo1 = BoxRepoFactory::hybrid(temp_dir.path())
        .create()
        .await
        .unwrap();
    let tbl1 = open_table(&repo1, "accounts").await;

    let validator_id = shamir_types::types::record_id::RecordId::system("v_test");
    tbl1.add_validator_binding(crate::validator::ValidatorBinding {
        validator_id,
        ops: smallvec::smallvec![crate::validator::WriteOp::Insert],
        priority: 1000,
    })
    .await
    .unwrap();
    assert_eq!(tbl1.validator_bindings().len(), 1);

    drop(tbl1);
    drop(repo1);

    let repo2 = BoxRepoFactory::hybrid(temp_dir.path())
        .create()
        .await
        .unwrap();
    let tbl2 = open_table(&repo2, "accounts").await;

    assert_eq!(
        tbl2.validator_bindings().len(),
        1,
        "validator binding must survive restart (config, not data)"
    );
    assert_eq!(tbl2.validator_bindings()[0].validator_id, validator_id);
}

// ============================================================================
// 3. Interner coherence — the single most safety-critical assertion.
// ============================================================================

#[tokio::test]
async fn interner_resolves_same_field_to_same_id_after_restart() {
    let temp_dir = tempfile::tempdir().unwrap();

    let repo1 = BoxRepoFactory::hybrid(temp_dir.path())
        .create()
        .await
        .unwrap();
    let tbl1 = open_table(&repo1, "people").await;

    let id_before = {
        let interner = tbl1.interner().get().await.unwrap();
        let ti = interner.touch_ind("email").unwrap();
        if let TouchInd::New(k) = &ti {
            tbl1.interner()
                .save_new_keys(&[(
                    k.clone(),
                    shamir_types::core::interner::UserKey::from_str("email"),
                )])
                .await
                .unwrap();
        }
        ti.into_key().id()
    };

    drop(tbl1);
    drop(repo1);

    let repo2 = BoxRepoFactory::hybrid(temp_dir.path())
        .create()
        .await
        .unwrap();
    let tbl2 = open_table(&repo2, "people").await;

    let id_after = {
        let interner = tbl2.interner().get().await.unwrap();
        let ti = interner.touch_ind("email").unwrap();
        assert!(
            matches!(ti, TouchInd::Exists(_)),
            "the SAME field name must already exist in the restored interner, \
             not be freshly (re-)minted — a fresh id here would silently \
             corrupt every persisted index definition that references the \
             old id"
        );
        ti.into_key().id()
    };

    assert_eq!(
        id_before, id_after,
        "same field name must resolve to the SAME id post-restart"
    );
}

// ============================================================================
// 4. Record counter reads 0 post-restart, not the pre-restart count.
// ============================================================================

#[tokio::test]
async fn record_counter_reads_zero_after_restart() {
    let temp_dir = tempfile::tempdir().unwrap();

    let repo1 = BoxRepoFactory::hybrid(temp_dir.path())
        .create()
        .await
        .unwrap();
    let tbl1 = open_table(&repo1, "orders").await;

    for i in 0..3i64 {
        let iv = interned_row(&tbl1, "x", i).await;
        tbl1.insert(&iv).await.unwrap();
    }
    tbl1.interner().persist().await.unwrap();
    tbl1.flush_metadata().await.unwrap();
    assert_eq!(
        tbl1.counter().get().await.unwrap(),
        3,
        "pre-restart counter must reflect the 3 inserted rows"
    );

    drop(tbl1);
    drop(repo1);

    let repo2 = BoxRepoFactory::hybrid(temp_dir.path())
        .create()
        .await
        .unwrap();
    let tbl2 = open_table(&repo2, "orders").await;

    assert_eq!(
        tbl2.counter().get().await.unwrap(),
        0,
        "counter is derived-from-data (Class B) and must read 0 against a \
         hybrid repo's fresh, empty __data__/__history__, even though \
         __info__ (config) survived"
    );
}

// ============================================================================
// 5. `create()` does not error or panic on any of the above sequences —
//    folded into every test above via `.unwrap()` on `TableManager::create`
//    (a panic on `.unwrap()` would fail the test outright). This dedicated
//    test additionally exercises the "no legacy indexes at all" + "legacy
//    index format marker present" combination in one place.
// ============================================================================

#[tokio::test]
async fn create_does_not_error_or_panic_across_restart() {
    let temp_dir = tempfile::tempdir().unwrap();

    let repo1 = BoxRepoFactory::hybrid(temp_dir.path())
        .create()
        .await
        .unwrap();
    let tbl1 = open_table(&repo1, "plain").await;
    tbl1.create_index("by_y", &["y"]).await.unwrap();
    let iv = interned_row(&tbl1, "y", 7).await;
    tbl1.insert(&iv).await.unwrap();
    tbl1.interner().persist().await.unwrap();
    tbl1.flush_metadata().await.unwrap();

    // legacy_indexes_need_rebuild's marker (`_m.idx.lfv`) must have been
    // stamped by the first open — confirm it round-trips as durable
    // config on its own (independent of the index-def check above).
    let stamped_before = crate::index2::persistence::load_legacy_index_version(tbl1.info_store())
        .await
        .unwrap();
    assert_eq!(stamped_before, 2, "first open must stamp version 2");

    drop(tbl1);
    drop(repo1);

    let repo2 = BoxRepoFactory::hybrid(temp_dir.path())
        .create()
        .await
        .unwrap();
    // The interesting assertion here IS that this doesn't error/panic —
    // `create()` must tolerate warm __info__ (with a legacy-index-def AND
    // a current-version marker) paired with a cold, empty __data__/
    // __history__ without misfiring `repair()` into an inconsistent state.
    let tbl2 = open_table(&repo2, "plain").await;

    let stamped_after = crate::index2::persistence::load_legacy_index_version(tbl2.info_store())
        .await
        .unwrap();
    assert_eq!(
        stamped_after, 2,
        "marker survives restart and stays current — no wasted repair() re-stamp"
    );

    let report = tbl2.verify().await.unwrap();
    assert!(
        report.is_healthy(),
        "reopened table must be healthy: 0 real postings is already \
         consistent with 0 rows: {report:?}"
    );
}

// ============================================================================
// 6. Vector index2 backend: postings/graph must NOT resurrect non-empty
//    post-restart. Wired directly via `create_index_v2` (a TableManager
//    method, not the DDL surface), so this is in-scope per the brief.
// ============================================================================

#[tokio::test]
async fn vector_index_does_not_resurrect_after_restart() {
    use crate::index2::backend::{IndexQuery, IndexResult};

    let temp_dir = tempfile::tempdir().unwrap();

    let repo1 = BoxRepoFactory::hybrid(temp_dir.path())
        .create()
        .await
        .unwrap();
    let tbl1 = open_table(&repo1, "vecs").await;

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
        vector_quantization: None,
        include: Vec::new(),
        if_not_exists: false,
    };
    tbl1.create_index_v2(&op).await.unwrap();

    let emb_key_id = {
        let interner = tbl1.interner().get().await.unwrap();
        let ti = interner.touch_ind("embedding").unwrap();
        if let TouchInd::New(k) = &ti {
            tbl1.interner()
                .save_new_keys(&[(
                    k.clone(),
                    shamir_types::core::interner::UserKey::from_str("embedding"),
                )])
                .await
                .unwrap();
        }
        ti.into_key().id()
    };

    for vec in [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.95, 0.05, 0.0]] {
        let mut m = new_map_wc(1);
        m.insert(
            shamir_types::core::interner::InternerKey::new(emb_key_id),
            InnerValue::List(vec.iter().map(|f| InnerValue::F64(*f)).collect()),
        );
        tbl1.insert(&InnerValue::Map(m)).await.unwrap();
    }
    tbl1.interner().persist().await.unwrap();
    tbl1.flush_metadata().await.unwrap();

    // Confirm the search finds hits pre-restart.
    let backends1 = tbl1.index2_registry().all_backends().await;
    assert_eq!(backends1.len(), 1, "expected exactly 1 index2 backend");
    let result1 = backends1[0]
        .lookup(IndexQuery::Vector {
            vec: vec![1.0, 0.0, 0.0],
            k: 2,
            opts: crate::index2::vector::SearchOpts::default(),
        })
        .await
        .unwrap();
    match &result1 {
        IndexResult::Ranked(ranked) => {
            assert_eq!(ranked.len(), 2, "pre-restart search must find hits");
        }
        _ => panic!("expected Ranked result pre-restart"),
    }

    drop(tbl1);
    drop(repo1);

    let repo2 = BoxRepoFactory::hybrid(temp_dir.path())
        .create()
        .await
        .unwrap();
    let tbl2 = open_table(&repo2, "vecs").await;

    let backends2 = tbl2.index2_registry().all_backends().await;
    assert_eq!(
        backends2.len(),
        1,
        "the vector index DEFINITION/descriptor must survive restart"
    );
    let result2 = backends2[0]
        .lookup(IndexQuery::Vector {
            vec: vec![1.0, 0.0, 0.0],
            k: 2,
            opts: crate::index2::vector::SearchOpts::default(),
        })
        .await
        .unwrap();
    match &result2 {
        IndexResult::Ranked(ranked) => {
            assert_eq!(
                ranked.len(),
                0,
                "restore_on_open must NOT resurrect a non-empty vector index \
                 from a persisted HNSW snapshot when the underlying data is \
                 gone — a hybrid repo's __info__ must never carry a durable \
                 vector snapshot chunk"
            );
        }
        other => panic!("expected Ranked([]) post-restart, got {other:?}"),
    }
}
