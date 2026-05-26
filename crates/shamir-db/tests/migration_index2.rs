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

use serde_json::json;

use shamir_db::engine::repo::repo_types::BoxRepoFactory;
use shamir_db::engine::repo::RepoConfig;
use shamir_db::engine::table::TableConfig;
use shamir_db::query::batch::BatchRequest;
use shamir_db::ShamirDb;

async fn setup() -> ShamirDb {
    let shamir = ShamirDb::init_memory().await.unwrap();
    let db = shamir.create_db("testdb").await;
    let repo_config = RepoConfig::new("src_repo", BoxRepoFactory::in_memory())
        .add_table(TableConfig::new("docs"));
    db.add_repo(repo_config).await.unwrap();
    shamir
}

async fn exec(shamir: &ShamirDb, req: serde_json::Value) -> shamir_db::query::batch::BatchResponse {
    let req: BatchRequest = serde_json::from_value(req).unwrap();
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

    exec(
        &shamir,
        json!({
            "id": 2,
            "queries": {
                "w1": {"insert_into": ["src_repo", "docs"], "values": [{"body": "hello rust world"}]},
                "w2": {"insert_into": ["src_repo", "docs"], "values": [{"body": "rust is great"}]},
                "w3": {"insert_into": ["src_repo", "docs"], "values": [{"body": "hello python"}]},
                "w4": {"insert_into": ["src_repo", "docs"], "values": [{"body": "goodbye world"}]},
                "w5": {"insert_into": ["src_repo", "docs"], "values": [{"body": "hello world rust"}]},
            }
        }),
    )
    .await;

    let src_resp = exec(
        &shamir,
        json!({
            "id": 3,
            "queries": {
                "q": {
                    "from": ["src_repo", "docs"],
                    "where": {"op": "fts", "field": ["body"], "query": "hello world", "mode": "and"}
                }
            }
        }),
    )
    .await;
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

    let dst_resp = exec(
        &shamir,
        json!({
            "id": 6,
            "queries": {
                "q": {
                    "from": ["dst_repo", "docs"],
                    "where": {"op": "fts", "field": ["body"], "query": "hello world", "mode": "and"}
                }
            }
        }),
    )
    .await;
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

    exec(
        &shamir,
        json!({
            "id": 2,
            "queries": {
                "w1": {"insert_into": ["src_repo", "docs"], "values": [{"email": "Alice@FOO.com", "name": "alice"}]},
                "w2": {"insert_into": ["src_repo", "docs"], "values": [{"email": "BOB@bar.org", "name": "bob"}]},
                "w3": {"insert_into": ["src_repo", "docs"], "values": [{"email": "Charlie@BAZ.net", "name": "charlie"}]},
                "w4": {"insert_into": ["src_repo", "docs"], "values": [{"email": "alice@foo.com", "name": "alice2"}]},
            }
        }),
    )
    .await;

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

    exec(
        &shamir,
        json!({
            "id": 2,
            "queries": {
                "w1": {"insert_into": ["src_repo", "docs"], "values": [{"embedding": [1.0, 0.0, 0.0], "label": "x"}]},
                "w2": {"insert_into": ["src_repo", "docs"], "values": [{"embedding": [0.0, 1.0, 0.0], "label": "y"}]},
                "w3": {"insert_into": ["src_repo", "docs"], "values": [{"embedding": [0.95, 0.1, 0.0], "label": "x_near"}]},
                "w4": {"insert_into": ["src_repo", "docs"], "values": [{"embedding": [0.0, 0.0, 1.0], "label": "z"}]},
                "w5": {"insert_into": ["src_repo", "docs"], "values": [{"embedding": [0.9, 0.05, 0.05], "label": "x_near2"}]},
            }
        }),
    )
    .await;

    let src_resp = exec(
        &shamir,
        json!({
            "id": 3,
            "queries": {
                "q": {
                    "from": ["src_repo", "docs"],
                    "where": {
                        "op": "vector_similarity",
                        "field": ["embedding"],
                        "query": [1.0, 0.0, 0.0],
                        "k": 3
                    }
                }
            }
        }),
    )
    .await;
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

    let dst_resp = exec(
        &shamir,
        json!({
            "id": 6,
            "queries": {
                "q": {
                    "from": ["dst_repo", "docs"],
                    "where": {
                        "op": "vector_similarity",
                        "field": ["embedding"],
                        "query": [1.0, 0.0, 0.0],
                        "k": 3
                    }
                }
            }
        }),
    )
    .await;
    let dst_records = &dst_resp.results["q"].records;
    assert_eq!(dst_records.len(), 3);
    let dst_labels: Vec<&str> = dst_records
        .iter()
        .map(|r| r["label"].as_str().unwrap())
        .collect();
    let dst_stats = dst_resp.results["q"].stats.as_ref().expect("dst stats");
    assert_eq!(dst_stats.index_used.as_deref(), Some("index2_ranked"));
    assert_eq!(src_labels, dst_labels, "vector top-k should match");
}
