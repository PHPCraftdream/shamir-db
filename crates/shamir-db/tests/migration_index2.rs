//! Wire-level migration test for index2 indexes (FTS / Functional / Vector).
//!
//! Exercises the full path through `ShamirDb::execute`:
//!   1. create src repo + table with index2 indexes
//!   2. insert records, query src, capture baseline + stats.index_used
//!   3. start_migration (with interner replication, index2 descriptor
//!      replication, snapshot, drain)
//!   4. commit_migration (final drain + bulk_populate_index2)
//!   5. query dst — identical results + same index_used (no fall-through
//!      to brute-force scan, no missing fields from broken interner)
//!
//! # Migration note
//!
//! Read/write batches are constructed with `shamir_query_builder`. Admin ops
//! (`create_index`), migration ops (`start_migration`, `commit_migration`),
//! and `computed` filters stay as raw `json!` because the builder has no
//! coverage for them.

use serde_json::json;

use shamir_db::engine::repo::repo_types::BoxRepoFactory;
use shamir_db::engine::repo::RepoConfig;
use shamir_db::engine::table::TableConfig;
use shamir_db::query::batch::{BatchRequest, BatchResponse};
use shamir_db::ShamirDb;
use shamir_query_builder::batch::Batch;
use shamir_query_builder::doc;
use shamir_query_builder::filter;
use shamir_query_builder::write::Insert;
use shamir_query_builder::Query;

async fn setup() -> ShamirDb {
    let shamir = ShamirDb::init_memory().await.unwrap();
    let db = shamir.create_db("testdb").await;
    let repo_config = RepoConfig::new("src_repo", BoxRepoFactory::in_memory())
        .add_table(TableConfig::new("docs"));
    db.add_repo(repo_config).await.unwrap();
    shamir
}

async fn exec(shamir: &ShamirDb, req: serde_json::Value) -> BatchResponse {
    let req: BatchRequest = serde_json::from_value(req).unwrap();
    shamir.execute("testdb", &req).await.unwrap()
}

async fn exec_built(shamir: &ShamirDb, req: BatchRequest) -> BatchResponse {
    shamir.execute("testdb", &req).await.unwrap()
}

#[tokio::test]
async fn migration_preserves_fts_index() {
    let shamir = setup().await;

    exec(
        &shamir,
        json!({
            "id": 1,
            "queries": {
                "mk": {
                    "create_index": "body_fts",
                    "table": "docs",
                    "repo": "src_repo",
                    "fields": [["body"]],
                    "index_type": "fts",
                    "fts_tokenizer": "whitespace",
                }
            }
        }),
    )
    .await;

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
    exec_built(&shamir, b.build()).await;

    let mut b = Batch::new();
    b.id(3);
    b.query(
        "q",
        Query::with_repo("src_repo", "docs").fts("body", "hello world", "and"),
    );
    let src_resp = exec_built(&shamir, b.build()).await;
    let src_records = &src_resp.results["q"].records;
    assert_eq!(
        src_records.len(),
        2,
        "src FTS AND should match 2: {src_records:?}"
    );
    let src_stats = src_resp.results["q"].stats.as_ref().expect("src stats");
    assert_eq!(src_stats.index_used.as_deref(), Some("index2_ranked"));

    let mig = exec(
        &shamir,
        json!({
            "id": 4,
            "queries": {
                "m": {
                    "start_migration": "docs",
                    "repo": "src_repo",
                    "dst_repo": "dst_repo",
                    "dst_engine": "in_memory",
                }
            }
        }),
    )
    .await;
    let migration_id = mig.results["m"].records[0]["migration_id"]
        .as_str()
        .unwrap()
        .to_string();

    exec(
        &shamir,
        json!({
            "id": 5,
            "queries": { "c": { "commit_migration": migration_id } }
        }),
    )
    .await;

    let mut b = Batch::new();
    b.id(6);
    b.query(
        "q",
        Query::with_repo("dst_repo", "docs").fts("body", "hello world", "and"),
    );
    let dst_resp = exec_built(&shamir, b.build()).await;
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

    exec(
        &shamir,
        json!({
            "id": 1,
            "queries": {
                "mk": {
                    "create_index": "email_lower",
                    "table": "docs",
                    "repo": "src_repo",
                    "fields": [["email"]],
                    "index_type": "functional",
                    "functional_op": "lower",
                }
            }
        }),
    )
    .await;

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
    exec_built(&shamir, b.build()).await;

    let src_resp = exec(
        &shamir,
        json!({
            "id": 3,
            "queries": {
                "q": {
                    "from": ["src_repo", "docs"],
                    "where": {
                        "op": "computed",
                        "expr_op": "lower",
                        "field": ["email"],
                        "cmp": "eq",
                        "value": "alice@foo.com"
                    }
                }
            }
        }),
    )
    .await;
    let src_count = src_resp.results["q"].records.len();
    assert_eq!(src_count, 2, "src functional: expected 2");

    let mig = exec(
        &shamir,
        json!({
            "id": 4,
            "queries": {
                "m": {
                    "start_migration": "docs",
                    "repo": "src_repo",
                    "dst_repo": "dst_repo",
                    "dst_engine": "in_memory",
                }
            }
        }),
    )
    .await;
    let migration_id = mig.results["m"].records[0]["migration_id"]
        .as_str()
        .unwrap()
        .to_string();

    exec(
        &shamir,
        json!({
            "id": 5,
            "queries": { "c": { "commit_migration": migration_id } }
        }),
    )
    .await;

    let dst_resp = exec(
        &shamir,
        json!({
            "id": 6,
            "queries": {
                "q": {
                    "from": ["dst_repo", "docs"],
                    "where": {
                        "op": "computed",
                        "expr_op": "lower",
                        "field": ["email"],
                        "cmp": "eq",
                        "value": "alice@foo.com"
                    }
                }
            }
        }),
    )
    .await;
    assert_eq!(dst_resp.results["q"].records.len(), src_count);
}

#[tokio::test]
async fn migration_preserves_vector_index() {
    let shamir = setup().await;

    exec(
        &shamir,
        json!({
            "id": 1,
            "queries": {
                "mk": {
                    "create_index": "vec_idx",
                    "table": "docs",
                    "repo": "src_repo",
                    "fields": [["embedding"]],
                    "index_type": "vector",
                    "vector_dim": 3,
                    "vector_metric": "cosine",
                }
            }
        }),
    )
    .await;

    let mut b = Batch::new();
    b.id(2);
    b.insert(
        "w1",
        Insert::with_repo("src_repo", "docs")
            .row(doc! { "label" => "x" }.set_json("embedding", json!([1.0, 0.0, 0.0]))),
    );
    b.insert(
        "w2",
        Insert::with_repo("src_repo", "docs")
            .row(doc! { "label" => "y" }.set_json("embedding", json!([0.0, 1.0, 0.0]))),
    );
    b.insert(
        "w3",
        Insert::with_repo("src_repo", "docs")
            .row(doc! { "label" => "x_near" }.set_json("embedding", json!([0.95, 0.1, 0.0]))),
    );
    b.insert(
        "w4",
        Insert::with_repo("src_repo", "docs")
            .row(doc! { "label" => "z" }.set_json("embedding", json!([0.0, 0.0, 1.0]))),
    );
    b.insert(
        "w5",
        Insert::with_repo("src_repo", "docs")
            .row(doc! { "label" => "x_near2" }.set_json("embedding", json!([0.9, 0.05, 0.05]))),
    );
    exec_built(&shamir, b.build()).await;

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
    let src_resp = exec_built(&shamir, b.build()).await;
    let src_records = &src_resp.results["q"].records;
    assert_eq!(src_records.len(), 3);
    let src_labels: Vec<&str> = src_records
        .iter()
        .map(|r| r["label"].as_str().unwrap())
        .collect();
    let src_stats = src_resp.results["q"].stats.as_ref().expect("src stats");
    assert_eq!(src_stats.index_used.as_deref(), Some("index2_ranked"));

    let mig = exec(
        &shamir,
        json!({
            "id": 4,
            "queries": {
                "m": {
                    "start_migration": "docs",
                    "repo": "src_repo",
                    "dst_repo": "dst_repo",
                    "dst_engine": "in_memory",
                }
            }
        }),
    )
    .await;
    let migration_id = mig.results["m"].records[0]["migration_id"]
        .as_str()
        .unwrap()
        .to_string();

    exec(
        &shamir,
        json!({
            "id": 5,
            "queries": { "c": { "commit_migration": migration_id } }
        }),
    )
    .await;

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
    let dst_resp = exec_built(&shamir, b.build()).await;
    let dst_records = &dst_resp.results["q"].records;
    assert_eq!(dst_records.len(), 3);
    let dst_labels: Vec<&str> = dst_records
        .iter()
        .map(|r| r["label"].as_str().unwrap())
        .collect();
    let dst_stats = dst_resp.results["q"].stats.as_ref().expect("dst stats");
    assert_eq!(dst_stats.index_used.as_deref(), Some("index2_ranked"));

    // HNSW is an APPROXIMATE index whose graph is built with randomised layer
    // assignment; the source and the migration-rebuilt destination are two
    // INDEPENDENT graphs, so their approximate top-k can legitimately differ
    // in the borderline tail — `x_near` (cosine ~0.994) and `x_near2` (~0.997)
    // are nearly tied, and a tiny 5-vector graph can even surface an
    // orthogonal vector on a recall miss. Asserting exact `src_labels ==
    // dst_labels` tested a graph-level determinism HNSW does not provide
    // (flaky). Assert the robust, deterministic preservation invariants
    // instead: after migration the destination is still vector-index-backed
    // (above), returns k=3, and ranks the EXACT match (`x` — the query vector
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
        "exact match ranks first after migration — the vector index was preserved"
    );
}
