//! Wire-level migration test for index2 indexes (FTS / Functional / Vector).
//!
//! Exercises the full path through `ShamirDb::execute`:
//!   1. create src repo + table with index2 indexes
//!   2. insert records, query src, capture baseline + stats.index_used
//!   3. start_migration (with interner replication, index2 descriptor
//!      replication, snapshot, drain)
//!   4. commit_migration (final drain + bulk_populate_index2)
//!   5. query dst -- identical results + same index_used (no fall-through
//!      to brute-force scan, no missing fields from broken interner)
//!
//! # Migration note
//!
//! All batches (read/write AND DDL/migration ops) are constructed with
//! `shamir_query_builder` and round-tripped through MessagePack.

use futures::StreamExt;
use std::sync::Arc;

use shamir_db::engine::migration::{MigrationCoordinator, MigrationShadowLog};
use shamir_types::mpack;

use shamir_db::engine::repo::repo_types::BoxRepoFactory;
use shamir_db::engine::repo::RepoConfig;
use shamir_db::engine::table::TableConfig;
use shamir_db::query::batch::{BatchRequest, BatchResponse};
use shamir_db::ShamirDb;
use shamir_query_builder::batch::Batch;
use shamir_query_builder::ddl;
use shamir_query_builder::doc;
use shamir_query_builder::filter;
use shamir_query_builder::q;
use shamir_query_builder::write::Insert;
use shamir_query_builder::Query;

async fn setup() -> ShamirDb {
    let shamir = ShamirDb::init_memory().await.unwrap();
    // F-5 (#795): the migration API is gated as experimental and disabled
    // by default. These index-preservation tests exercise the REAL
    // mechanism end-to-end and have nothing to do with the unfixed
    // write-interception gap, so opt in here (the exact "opt-in for
    // internal testing, disabled for real clients" posture the gate is
    // going for).
    shamir.enable_experimental_migration_api();
    let db = shamir.create_db("testdb").await;
    let repo_config = RepoConfig::new("src_repo", BoxRepoFactory::in_memory())
        .add_table(TableConfig::new("docs"));
    db.add_repo(repo_config).await.unwrap();
    shamir
}

async fn exec_built(shamir: &ShamirDb, req: BatchRequest) -> BatchResponse {
    shamir.execute("testdb", &req).await.unwrap()
}

#[tokio::test]
async fn migration_preserves_fts_index() {
    let shamir = setup().await;

    let mut b = Batch::new();
    b.id(1);
    b.create_index(
        "mk",
        ddl::create_index("body_fts", "docs")
            .repo("src_repo")
            .field("body")
            .index_type("fts")
            .fts_tokenizer("whitespace"),
    );
    exec_built(&shamir, b.to_request_via_msgpack()).await;

    let mut b = Batch::new();
    b.id(2);
    b.insert(
        "w1",
        Insert::with_repo("src_repo", "docs").row(doc! { "body" => "hello rust world" }),
    );
    b.insert(
        "w2",
        Insert::with_repo("src_repo", "docs").row(doc! { "body" => "rust is great" }),
    );
    b.insert(
        "w3",
        Insert::with_repo("src_repo", "docs").row(doc! { "body" => "hello python" }),
    );
    b.insert(
        "w4",
        Insert::with_repo("src_repo", "docs").row(doc! { "body" => "goodbye world" }),
    );
    b.insert(
        "w5",
        Insert::with_repo("src_repo", "docs").row(doc! { "body" => "hello world rust" }),
    );
    exec_built(&shamir, b.to_request_via_msgpack()).await;

    let mut b = Batch::new();
    b.id(3);
    b.query(
        "q",
        Query::with_repo("src_repo", "docs").fts("body", "hello world", "and"),
    );
    let src_resp = exec_built(&shamir, b.to_request_via_msgpack()).await;
    let src_records = &src_resp.results["q"].records;
    assert_eq!(
        src_records.len(),
        2,
        "src FTS AND should match 2: {src_records:?}"
    );
    let src_stats = src_resp.results["q"].stats.as_ref().expect("src stats");
    assert_eq!(src_stats.index_used.as_deref(), Some("index2_ranked"));

    let mut b = Batch::new();
    b.id(4);
    b.start_migration(
        "m",
        ddl::start_migration("docs", "dst_repo", "in_memory").repo("src_repo"),
    );
    let mig = exec_built(&shamir, b.to_request_via_msgpack()).await;
    let migration_id = mig.results["m"].records[0]
        .get_value_str("migration_id")
        .expect("migration_id must be a string")
        .to_string();

    let mut b = Batch::new();
    b.id(5);
    b.commit_migration("c", ddl::commit_migration(&migration_id));
    exec_built(&shamir, b.to_request_via_msgpack()).await;

    let mut b = Batch::new();
    b.id(6);
    b.query(
        "q",
        Query::with_repo("dst_repo", "docs").fts("body", "hello world", "and"),
    );
    let dst_resp = exec_built(&shamir, b.to_request_via_msgpack()).await;
    let dst_records = &dst_resp.results["q"].records;
    assert_eq!(
        dst_records.len(),
        src_records.len(),
        "dst FTS AND: expected {}, got {dst_records:?}",
        src_records.len()
    );
    let dst_stats = dst_resp.results["q"].stats.as_ref().expect("dst stats");
    assert_eq!(
        dst_stats.index_used.as_deref(),
        Some("index2_ranked"),
        "dst should use FTS index after migration"
    );
}

#[tokio::test]
async fn migration_preserves_functional_index() {
    let shamir = setup().await;

    let mut b = Batch::new();
    b.id(1);
    b.create_index(
        "mk",
        ddl::create_index("email_lower", "docs")
            .repo("src_repo")
            .field("email")
            .index_type("functional")
            .functional_op("lower"),
    );
    exec_built(&shamir, b.to_request_via_msgpack()).await;

    let mut b = Batch::new();
    b.id(2);
    b.insert(
        "w1",
        Insert::with_repo("src_repo", "docs")
            .row(doc! { "email" => "Alice@FOO.com", "name" => "alice" }),
    );
    b.insert(
        "w2",
        Insert::with_repo("src_repo", "docs")
            .row(doc! { "email" => "BOB@bar.org", "name" => "bob" }),
    );
    b.insert(
        "w3",
        Insert::with_repo("src_repo", "docs")
            .row(doc! { "email" => "Charlie@BAZ.net", "name" => "charlie" }),
    );
    b.insert(
        "w4",
        Insert::with_repo("src_repo", "docs")
            .row(doc! { "email" => "alice@foo.com", "name" => "alice2" }),
    );
    exec_built(&shamir, b.to_request_via_msgpack()).await;

    let mut b = Batch::new();
    b.id(3);
    b.query(
        "q",
        q!(from src_repo.docs where computed("lower", email, "eq", "alice@foo.com")),
    );
    let src_resp = exec_built(&shamir, b.to_request_via_msgpack()).await;
    let src_count = src_resp.results["q"].records.len();
    assert_eq!(src_count, 2, "src functional: expected 2");

    let mut b = Batch::new();
    b.id(4);
    b.start_migration(
        "m",
        ddl::start_migration("docs", "dst_repo", "in_memory").repo("src_repo"),
    );
    let mig = exec_built(&shamir, b.to_request_via_msgpack()).await;
    let migration_id = mig.results["m"].records[0]
        .get_value_str("migration_id")
        .expect("migration_id must be a string")
        .to_string();

    let mut b = Batch::new();
    b.id(5);
    b.commit_migration("c", ddl::commit_migration(&migration_id));
    exec_built(&shamir, b.to_request_via_msgpack()).await;

    let mut b = Batch::new();
    b.id(6);
    b.query(
        "q",
        q!(from dst_repo.docs where computed("lower", email, "eq", "alice@foo.com")),
    );
    let dst_resp = exec_built(&shamir, b.to_request_via_msgpack()).await;
    assert_eq!(dst_resp.results["q"].records.len(), src_count);
}

#[tokio::test]
async fn migration_preserves_vector_index() {
    let shamir = setup().await;

    let mut b = Batch::new();
    b.id(1);
    b.create_index(
        "mk",
        ddl::create_index("vec_idx", "docs")
            .repo("src_repo")
            .field("embedding")
            .index_type("vector")
            .vector_dim(3)
            .vector_metric("cosine"),
    );
    exec_built(&shamir, b.to_request_via_msgpack()).await;

    let mut b = Batch::new();
    b.id(2);
    b.insert(
        "w1",
        Insert::with_repo("src_repo", "docs")
            .row(doc! { "label" => "x" }.set_value("embedding", mpack!([1.0, 0.0, 0.0]))),
    );
    b.insert(
        "w2",
        Insert::with_repo("src_repo", "docs")
            .row(doc! { "label" => "y" }.set_value("embedding", mpack!([0.0, 1.0, 0.0]))),
    );
    b.insert(
        "w3",
        Insert::with_repo("src_repo", "docs")
            .row(doc! { "label" => "x_near" }.set_value("embedding", mpack!([0.95, 0.1, 0.0]))),
    );
    b.insert(
        "w4",
        Insert::with_repo("src_repo", "docs")
            .row(doc! { "label" => "z" }.set_value("embedding", mpack!([0.0, 0.0, 1.0]))),
    );
    b.insert(
        "w5",
        Insert::with_repo("src_repo", "docs")
            .row(doc! { "label" => "x_near2" }.set_value("embedding", mpack!([0.9, 0.05, 0.05]))),
    );
    exec_built(&shamir, b.to_request_via_msgpack()).await;

    let mut b = Batch::new();
    b.id(3);
    b.query(
        "q",
        Query::with_repo("src_repo", "docs").where_(filter::vector_similarity(
            "embedding",
            vec![1.0, 0.0, 0.0],
            3,
        )),
    );
    let src_resp = exec_built(&shamir, b.to_request_via_msgpack()).await;
    let src_records = &src_resp.results["q"].records;
    assert_eq!(src_records.len(), 3);
    let src_label_strs: Vec<String> = src_records
        .iter()
        .map(|r| r.get_value_str("label").unwrap().to_string())
        .collect();
    let src_labels: Vec<&str> = src_label_strs.iter().map(String::as_str).collect();
    let src_stats = src_resp.results["q"].stats.as_ref().expect("src stats");
    assert_eq!(src_stats.index_used.as_deref(), Some("index2_ranked"));

    let mut b = Batch::new();
    b.id(4);
    b.start_migration(
        "m",
        ddl::start_migration("docs", "dst_repo", "in_memory").repo("src_repo"),
    );
    let mig = exec_built(&shamir, b.to_request_via_msgpack()).await;
    let migration_id = mig.results["m"].records[0]
        .get_value_str("migration_id")
        .expect("migration_id must be a string")
        .to_string();

    let mut b = Batch::new();
    b.id(5);
    b.commit_migration("c", ddl::commit_migration(&migration_id));
    exec_built(&shamir, b.to_request_via_msgpack()).await;

    let mut b = Batch::new();
    b.id(6);
    b.query(
        "q",
        Query::with_repo("dst_repo", "docs").where_(filter::vector_similarity(
            "embedding",
            vec![1.0, 0.0, 0.0],
            3,
        )),
    );
    let dst_resp = exec_built(&shamir, b.to_request_via_msgpack()).await;
    let dst_records = &dst_resp.results["q"].records;
    assert_eq!(dst_records.len(), 3);
    let dst_label_strs: Vec<String> = dst_records
        .iter()
        .map(|r| r.get_value_str("label").unwrap().to_string())
        .collect();
    let dst_labels: Vec<&str> = dst_label_strs.iter().map(String::as_str).collect();
    let dst_stats = dst_resp.results["q"].stats.as_ref().expect("dst stats");
    assert_eq!(dst_stats.index_used.as_deref(), Some("index2_ranked"));

    // HNSW is an APPROXIMATE index whose graph is built with randomised layer
    // assignment; the source and the migration-rebuilt destination are two
    // INDEPENDENT graphs, so their approximate top-k can legitimately differ
    // in the borderline tail -- `x_near` (cosine ~0.994) and `x_near2` (~0.997)
    // are nearly tied, and a tiny 5-vector graph can even surface an
    // orthogonal vector on a recall miss. Asserting exact `src_labels ==
    // dst_labels` tested a graph-level determinism HNSW does not provide
    // (flaky). Assert the robust, deterministic preservation invariants
    // instead: after migration the destination is still vector-index-backed
    // (above), returns k=3, and ranks the EXACT match (`x` -- the query vector
    // itself, cosine 1.0, uniquely maximal) first. Exact-match recall is
    // reliable even for approximate search, so this never flakes.
    assert_eq!(src_labels.len(), 3, "source returns k=3");
    assert_eq!(
        dst_labels.len(),
        3,
        "destination returns k=3 after migration"
    );
    assert_eq!(src_labels[0], "x", "exact match ranks first on the source");
    assert_eq!(
        dst_labels[0], "x",
        "exact match ranks first after migration -- the vector index was preserved"
    );
}

// Red before #1219: persisted Committed is not recoverable by CommitMigration.
async fn assert_committed_crash_retry(seed_marker: bool) {
    let shamir = setup().await;
    let mut batch = Batch::new();
    batch.create_index(
        "index",
        ddl::create_index("body_fts", "docs")
            .repo("src_repo")
            .field("body")
            .index_type("fts")
            .fts_tokenizer("whitespace"),
    );
    batch.insert(
        "row",
        Insert::with_repo("src_repo", "docs").row(doc! { "body" => "hello rust" }),
    );
    exec_built(&shamir, batch.to_request_via_msgpack()).await;
    let mut batch = Batch::new();
    batch.start_migration(
        "migration",
        ddl::start_migration("docs", "dst_repo", "in_memory").repo("src_repo"),
    );
    let response = exec_built(&shamir, batch.to_request_via_msgpack()).await;
    let id = response.results["migration"].records[0]
        .get_value_str("migration_id")
        .unwrap()
        .to_string();
    let db = shamir.get_db("testdb").unwrap();
    let src = db.get_table("src_repo", "docs").await.unwrap();
    let dst = db.get_table("dst_repo", "docs").await.unwrap();
    let coord = shamir.active_migrations().get(&id).unwrap().value().clone();
    coord.final_drain_and_commit().await.unwrap();
    // Crash after seed but before its stage marker: retry must derive that
    // rows already have current versions instead of allocating new versions.
    dst.seed_log_from_data_store().await.unwrap();
    let mvcc = dst.mvcc_store().unwrap();
    mvcc.flush_history().await.unwrap();
    if seed_marker {
        coord.complete_finalisation_step(1).await.unwrap();
    }
    let mut records = dst.table().data_store().iter_stream(256);
    let key = records.next().await.unwrap().unwrap()[0].0.clone();
    let version = mvcc.version_of(key.as_ref());
    // Crash after population but before the index marker: its BM25 bumps
    // must be reset rather than added again on retry.
    dst.bulk_populate_index2().await.unwrap();
    shamir.active_migrations().remove(&id);
    drop(coord);
    let state = MigrationCoordinator::load_state(src.info_store(), &id)
        .await
        .unwrap()
        .unwrap();
    let shadow = Arc::new(
        MigrationShadowLog::recover(id.clone(), Arc::clone(src.info_store()))
            .await
            .unwrap(),
    );
    let rebuilt = Arc::new(MigrationCoordinator::new(
        state,
        shadow,
        Arc::clone(src.table().data_store()),
        Arc::clone(dst.table().data_store()),
        src.mvcc_store(),
    ));
    shamir
        .active_migrations()
        .insert(id.clone(), rebuilt.clone());
    let mut batch = Batch::new();
    batch.commit_migration("commit", ddl::commit_migration(&id));
    exec_built(&shamir, batch.to_request_via_msgpack()).await;
    assert_eq!(rebuilt.finalisation_progress().await.unwrap(), 2);
    assert!(shamir.active_migrations().is_empty());
    assert_eq!(mvcc.version_of(key.as_ref()), version);
    assert!(dst.index2_registry().reader_gate().enter().is_some());
    let backend = dst.index2_registry().all_backends().await.pop().unwrap();
    let ranked = backend
        .as_any()
        .downcast_ref::<shamir_db::engine::index2::fts_ranked_backend::FtsRankedBackend>()
        .unwrap();
    assert_eq!(ranked.doc_count(), 1);
    assert_eq!(ranked.sum_doc_len(), 2);
    // Crash after both markers, before cleanup (empty active map); retry
    // must recover the receipt and skip both completed collaborators.
    exec_built(&shamir, batch.to_request_via_msgpack()).await;
    assert!(shamir.active_migrations().is_empty());
    assert_eq!(ranked.doc_count(), 1);
    assert_eq!(mvcc.version_of(key.as_ref()), version);
    let mut query = Batch::new();
    query.query(
        "rows",
        Query::with_repo("dst_repo", "docs").fts("body", "rust", "and"),
    );
    let response = exec_built(&shamir, query.to_request_via_msgpack()).await;
    assert_eq!(response.results["rows"].records.len(), 1);
    assert_eq!(
        response.results["rows"]
            .stats
            .as_ref()
            .unwrap()
            .index_used
            .as_deref(),
        Some("index2_ranked"),
    );
}

#[tokio::test]
async fn committed_crash_retry_without_seed_marker_reconciles_once() {
    assert_committed_crash_retry(false).await;
}

#[tokio::test]
async fn committed_crash_retry_with_seed_marker_finishes_remaining_steps_once() {
    assert_committed_crash_retry(true).await;
}
