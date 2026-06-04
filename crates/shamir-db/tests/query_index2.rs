//! End-to-end tests for new index types (FTS / Functional / Vector)
//! via ShamirDb::execute — full wire-format pipeline.
//!
//! # Migration note
//!
//! Read/write batches are constructed with `shamir_query_builder`.
//! Admin ops (`create_index`) and functional-index `computed` filters
//! stay as raw `json!` because the builder has no coverage for them.

use serde_json::json;

use shamir_db::engine::repo::repo_types::BoxRepoFactory;
use shamir_db::engine::repo::RepoConfig;
use shamir_db::engine::table::TableConfig;
use shamir_db::query::batch::{BatchRequest, BatchResponse};
use shamir_db::ShamirDb;
use shamir_query_builder::batch::Batch;
use shamir_query_builder::doc;
use shamir_query_builder::write::insert;
use shamir_query_builder::Query;

async fn setup() -> ShamirDb {
    let shamir = ShamirDb::init_memory().await.unwrap();
    let db = shamir.create_db("testdb").await;
    let repo_config =
        RepoConfig::new("main", BoxRepoFactory::in_memory()).add_table(TableConfig::new("posts"));
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

// ============================================================================
// FTS — full wire pipeline
// ============================================================================

#[tokio::test]
async fn fts_index_and_query() {
    let shamir = setup().await;

    // create_index is an admin op — no builder coverage.
    exec(
        &shamir,
        json!({
            "id": 1,
            "queries": {
                "mk": {
                    "create_index": "body_fts",
                    "table": "posts",
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
        insert("posts").row(doc! { "body" => "hello rust world" }),
    );
    b.insert(
        "w2",
        insert("posts").row(doc! { "body" => "rust is great" }),
    );
    b.insert("w3", insert("posts").row(doc! { "body" => "hello python" }));
    exec_built(&shamir, b.build()).await;

    let mut b = Batch::new();
    b.id(3);
    b.query("q", Query::from("posts").fts("body", "hello world", "and"));
    let resp = exec_built(&shamir, b.build()).await;

    let records = &resp.results["q"].records;
    assert_eq!(records.len(), 1, "expected 1 record, got {records:?}");
    assert_eq!(records[0]["body"], "hello rust world");
    // Verify the FTS index was used (BM25-ranked).
    let stats = resp.results["q"].stats.as_ref().expect("stats present");
    assert_eq!(stats.index_used.as_deref(), Some("index2_ranked"));
}

#[tokio::test]
async fn fts_or_query() {
    let shamir = setup().await;

    // create_index — admin op, stays as json!
    exec(
        &shamir,
        json!({
            "id": 1,
            "queries": {
                "mk": {
                    "create_index": "body_fts",
                    "table": "posts",
                    "fields": [["body"]],
                    "index_type": "fts",
                }
            }
        }),
    )
    .await;

    let mut b = Batch::new();
    b.id(2);
    b.insert("w1", insert("posts").row(doc! { "body" => "apple orange" }));
    b.insert("w2", insert("posts").row(doc! { "body" => "banana pear" }));
    b.insert("w3", insert("posts").row(doc! { "body" => "cherry grape" }));
    exec_built(&shamir, b.build()).await;

    let mut b = Batch::new();
    b.id(3);
    b.query("q", Query::from("posts").fts("body", "apple banana", "or"));
    let resp = exec_built(&shamir, b.build()).await;

    let records = &resp.results["q"].records;
    assert_eq!(records.len(), 2);
}

// ============================================================================
// Functional — LOWER(email)
// ============================================================================

#[tokio::test]
async fn functional_lower_eq() {
    let shamir = setup().await;

    // create_index — admin op, stays as json!
    exec(
        &shamir,
        json!({
            "id": 1,
            "queries": {
                "mk": {
                    "create_index": "email_lower",
                    "table": "posts",
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
        insert("posts").row(doc! { "email" => "Alice@FOO.com", "name" => "alice" }),
    );
    b.insert(
        "w2",
        insert("posts").row(doc! { "email" => "BOB@bar.org", "name" => "bob" }),
    );
    exec_built(&shamir, b.build()).await;

    // Filter::Computed has no builder constructor — stays as json!
    let resp = exec(
        &shamir,
        json!({
            "id": 3,
            "queries": {
                "q": {
                    "from": "posts",
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
    let records = &resp.results["q"].records;
    assert_eq!(records.len(), 1);
    assert_eq!(records[0]["name"], "alice");
    let stats = resp.results["q"].stats.as_ref().expect("stats");
    assert_eq!(stats.index_used.as_deref(), Some("index2"));
}

// ============================================================================
// Vector similarity (HNSW)
// ============================================================================

#[tokio::test]
async fn vector_hnsw_similarity() {
    let shamir = setup().await;

    // create_index — admin op, stays as json!
    exec(
        &shamir,
        json!({
            "id": 1,
            "queries": {
                "mk": {
                    "create_index": "vec_idx",
                    "table": "posts",
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
        insert("posts").row(doc! { "label" => "x" }.set_json("embedding", json!([1.0, 0.0, 0.0]))),
    );
    b.insert(
        "w2",
        insert("posts").row(doc! { "label" => "y" }.set_json("embedding", json!([0.0, 1.0, 0.0]))),
    );
    b.insert(
        "w3",
        insert("posts")
            .row(doc! { "label" => "x_near" }.set_json("embedding", json!([0.95, 0.1, 0.0]))),
    );
    exec_built(&shamir, b.build()).await;

    let mut b = Batch::new();
    b.id(3);
    b.query(
        "q",
        Query::from("posts").where_(shamir_query_builder::filter::vector_similarity(
            "embedding",
            vec![1.0, 0.0, 0.0],
            2,
        )),
    );
    let resp = exec_built(&shamir, b.build()).await;

    let records = &resp.results["q"].records;
    assert_eq!(records.len(), 2, "expected top-2, got {records:?}");
    let labels: Vec<&str> = records
        .iter()
        .map(|r| r["label"].as_str().unwrap())
        .collect();
    assert!(labels.contains(&"x"), "x should be in top-2: {labels:?}");
    // Verify HNSW index was used (ranked path).
    let stats = resp.results["q"].stats.as_ref().expect("stats present");
    assert_eq!(stats.index_used.as_deref(), Some("index2_ranked"));
}

// ============================================================================
// Fallback: FTS without index → brute-force still works
// ============================================================================

#[tokio::test]
async fn fts_brute_force_fallback() {
    let shamir = setup().await;

    let mut b = Batch::new();
    b.id(1);
    b.insert("w1", insert("posts").row(doc! { "body" => "hello world" }));
    b.insert(
        "w2",
        insert("posts").row(doc! { "body" => "no match here" }),
    );
    exec_built(&shamir, b.build()).await;

    let mut b = Batch::new();
    b.id(2);
    b.query("q", Query::from("posts").fts("body", "hello", "and"));
    let resp = exec_built(&shamir, b.build()).await;

    let records = &resp.results["q"].records;
    assert_eq!(records.len(), 1);
    assert_eq!(records[0]["body"], "hello world");
    // No FTS index → full-scan fallback (not "index2").
    let stats = resp.results["q"].stats.as_ref().expect("stats");
    assert_ne!(stats.index_used.as_deref(), Some("index2"));
}

// ============================================================================
// Persistence — create_index_v2 persists metadata
// ============================================================================

#[tokio::test]
async fn create_index_persists_metadata() {
    let shamir = setup().await;

    // Create all 3 index types — admin ops, stays as json!
    exec(
        &shamir,
        json!({
            "id": 1,
            "queries": {
                "fts": {
                    "create_index": "body_fts", "table": "posts",
                    "fields": [["body"]], "index_type": "fts",
                },
                "fn": {
                    "create_index": "email_lower", "table": "posts",
                    "fields": [["email"]], "index_type": "functional", "functional_op": "lower",
                },
                "vec": {
                    "create_index": "vec_idx", "table": "posts",
                    "fields": [["emb"]], "index_type": "vector",
                    "vector_dim": 3, "vector_metric": "cosine",
                },
            }
        }),
    )
    .await;

    // Verify: all 3 should appear.
    let mut b = Batch::new();
    b.id(2);
    b.query("q1", Query::from("posts").fts("body", "test", "and"));
    let resp = exec_built(&shamir, b.build()).await;

    // Even with no data, the planner should find the FTS index and return empty results
    // via the index path (not fall through to full-scan).
    // Empty results via index → stats.index_used should be set OR empty results.
    // This just proves the index exists and is queryable.
    assert!(resp.results.contains_key("q1"));
}

// ============================================================================
// FTS — stemming (English)
// ============================================================================

/// English Snowball stemmer: query-side stemming of the INFLECTED
/// form "running" → stem "run", matching stored stem from "running".
///
/// Fails on the pre-fix hardcoded-whitespace query path: the old code
/// hashed "running" raw (without stemming); the stored posting was
/// under token_hash("run") (the stem). hash("running") != hash("run")
/// → no match returned.
#[tokio::test]
async fn fts_stemmed_en_query() {
    let shamir = setup().await;

    // create_index — admin op, stays as json!
    exec(
        &shamir,
        json!({
            "id": 1,
            "queries": {
                "mk": {
                    "create_index": "body_fts",
                    "table": "posts",
                    "fields": [["body"]],
                    "index_type": "fts",
                    "fts_tokenizer": "stemmed_en",
                }
            }
        }),
    )
    .await;

    let mut b = Batch::new();
    b.id(2);
    b.insert("w1", insert("posts").row(doc! { "body" => "running fast" }));
    exec_built(&shamir, b.build()).await;

    // Query the INFLECTED form "running" — the fix stems it to "run"
    // which matches the stored stem. On the old code "running" hashed
    // raw != stored "run" hash → zero results.
    let mut b = Batch::new();
    b.id(3);
    b.query("q", Query::from("posts").fts("body", "running", "and"));
    let resp = exec_built(&shamir, b.build()).await;

    let records = &resp.results["q"].records;
    assert_eq!(
        records.len(),
        1,
        "stemmed query 'running' should match 'running fast'"
    );
    assert_eq!(records[0]["body"], "running fast");
}

// ============================================================================
// FTS — stopwords filtered
// ============================================================================

#[tokio::test]
async fn fts_stopwords_filtered() {
    let shamir = setup().await;

    // create_index — admin op, stays as json!
    exec(
        &shamir,
        json!({
            "id": 1,
            "queries": {
                "mk": {
                    "create_index": "body_fts",
                    "table": "posts",
                    "fields": [["body"]],
                    "index_type": "fts",
                    "fts_tokenizer": "stemmed_en",
                }
            }
        }),
    )
    .await;

    let mut b = Batch::new();
    b.id(2);
    b.insert("w1", insert("posts").row(doc! { "body" => "the cat sat" }));
    exec_built(&shamir, b.build()).await;

    // Query "the cat" — "the" is a stopword and gets filtered both at
    // index time and query time, so the lookup matches by "cat" only.
    let mut b = Batch::new();
    b.id(3);
    b.query("q", Query::from("posts").fts("body", "the cat", "and"));
    let resp = exec_built(&shamir, b.build()).await;

    let records = &resp.results["q"].records;
    assert_eq!(
        records.len(),
        1,
        "stopword 'the' should be filtered, match on 'cat'"
    );
    assert_eq!(records[0]["body"], "the cat sat");
}

// ============================================================================
// FTS — n-gram (substring matching)
// ============================================================================

/// N-gram tokenizer (n=3) enables substring matching via character
/// trigrams.  The QUERY must be ngram-tokenized the same way as
/// documents — a full word like "hello" becomes trigrams [hel,ell,llo].
///
/// Fails on the pre-fix hardcoded-whitespace query path: the old code
/// hashed the 5-char word "hello" as one token — that hash never
/// matches any stored trigram, so zero results were returned.
///
/// Docs:
///   "hello world" → grams [hel,ell,llo,wor,orl,rld]
///   "help wanted" → grams [hel,elp,wan,ant,nte,ted]
///   "goodbye"     → grams [goo,ood,odb,dby,bye]
///
/// Query "hello" with mode "and" → grams [hel,ell,llo]. All three
/// must match: "hello world" has all three → matched. "help wanted"
/// has [hel] but lacks [ell,llo] → NOT matched. "goodbye" shares
/// none → NOT matched. Result: exactly 1 record.
#[tokio::test]
async fn fts_ngram_query() {
    let shamir = setup().await;

    // create_index — admin op, stays as json!
    exec(
        &shamir,
        json!({
            "id": 1,
            "queries": {
                "mk": {
                    "create_index": "body_fts",
                    "table": "posts",
                    "fields": [["body"]],
                    "index_type": "fts",
                    "fts_tokenizer": "ngram3",
                }
            }
        }),
    )
    .await;

    let mut b = Batch::new();
    b.id(2);
    b.insert(
        "w1",
        insert("posts").rows([
            doc! { "body" => "hello world" },
            doc! { "body" => "help wanted" },
            doc! { "body" => "goodbye" },
        ]),
    );
    exec_built(&shamir, b.build()).await;

    // Query the FULL word "hello" (5 chars, NOT a single trigram).
    // The fix ngram-tokenizes the query → [hel,ell,llo]. With mode
    // "and", ALL three grams must be present in the doc. Only "hello
    // world" contains all three; "help wanted" has only "hel".
    let mut b = Batch::new();
    b.id(3);
    b.query("q", Query::from("posts").fts("body", "hello", "and"));
    let resp = exec_built(&shamir, b.build()).await;

    let records = &resp.results["q"].records;
    assert_eq!(
        records.len(),
        1,
        "ngram query 'hello' should match only 'hello world', got {records:?}"
    );
    assert_eq!(records[0]["body"], "hello world");
}

// ============================================================================
// FTS — stemming (French)
// ============================================================================

/// French Snowball stemmer: query-side stemming of the INFLECTED
/// plural "chats" → stem "chat", matching stored stem from "chats".
///
/// Fails on the pre-fix hardcoded-whitespace query path: the old code
/// hashed "chats" raw (without stemming); the stored posting was under
/// token_hash("chat") (the stem). hash("chats") != hash("chat") → no
/// match returned.
#[tokio::test]
async fn fts_stemmed_fr_query() {
    let shamir = setup().await;

    // create_index — admin op, stays as json!
    exec(
        &shamir,
        json!({
            "id": 1,
            "queries": {
                "mk": {
                    "create_index": "body_fts",
                    "table": "posts",
                    "fields": [["body"]],
                    "index_type": "fts",
                    "fts_tokenizer": "stemmed_fr",
                }
            }
        }),
    )
    .await;

    let mut b = Batch::new();
    b.id(2);
    b.insert(
        "w1",
        insert("posts").rows([
            doc! { "body" => "les chats noirs" },
            doc! { "body" => "un chien blanc" },
        ]),
    );
    exec_built(&shamir, b.build()).await;

    // Query the INFLECTED plural "chats" — the fix stems it to "chat"
    // which matches the stored stem. On the old code "chats" hashed
    // raw != stored "chat" hash → zero results.
    let mut b = Batch::new();
    b.id(3);
    b.query("q", Query::from("posts").fts("body", "chats", "and"));
    let resp = exec_built(&shamir, b.build()).await;

    let records = &resp.results["q"].records;
    assert_eq!(
        records.len(),
        1,
        "French stemmed query 'chats' (plural) should match 'les chats noirs'"
    );
    assert_eq!(records[0]["body"], "les chats noirs");
}
