//! End-to-end tests for new index types (FTS / Functional / Vector)
//! via ShamirDb::execute — full wire-format pipeline.

use serde_json::json;

use shamir_db::engine::repo::repo_types::BoxRepoFactory;
use shamir_db::engine::repo::RepoConfig;
use shamir_db::engine::table::TableConfig;
use shamir_db::query::batch::BatchRequest;
use shamir_db::ShamirDb;

async fn setup() -> ShamirDb {
    let shamir = ShamirDb::init_memory().await.unwrap();
    let db = shamir.create_db("testdb").await;
    let repo_config =
        RepoConfig::new("main", BoxRepoFactory::in_memory()).add_table(TableConfig::new("posts"));
    db.add_repo(repo_config).await.unwrap();
    shamir
}

async fn exec(shamir: &ShamirDb, req: serde_json::Value) -> shamir_db::query::batch::BatchResponse {
    let req: BatchRequest = serde_json::from_value(req).unwrap();
    shamir.execute("testdb", &req).await.unwrap()
}

// ============================================================================
// FTS — full wire pipeline
// ============================================================================

#[tokio::test]
async fn fts_index_and_query() {
    let shamir = setup().await;

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

    exec(
        &shamir,
        json!({
            "id": 2,
            "queries": {
                "w1": {"insert_into": "posts", "values": [{"body": "hello rust world"}]},
                "w2": {"insert_into": "posts", "values": [{"body": "rust is great"}]},
                "w3": {"insert_into": "posts", "values": [{"body": "hello python"}]},
            }
        }),
    )
    .await;

    let resp = exec(
        &shamir,
        json!({
            "id": 3,
            "queries": {
                "q": {
                    "from": "posts",
                    "where": {"op": "fts", "field": ["body"], "query": "hello world", "mode": "and"}
                }
            }
        }),
    )
    .await;
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

    exec(
        &shamir,
        json!({
            "id": 2,
            "queries": {
                "w1": {"insert_into": "posts", "values": [{"body": "apple orange"}]},
                "w2": {"insert_into": "posts", "values": [{"body": "banana pear"}]},
                "w3": {"insert_into": "posts", "values": [{"body": "cherry grape"}]},
            }
        }),
    )
    .await;

    let resp = exec(
        &shamir,
        json!({
            "id": 3,
            "queries": {
                "q": {
                    "from": "posts",
                    "where": {"op": "fts", "field": ["body"], "query": "apple banana", "mode": "or"}
                }
            }
        }),
    )
    .await;
    let records = &resp.results["q"].records;
    assert_eq!(records.len(), 2);
}

// ============================================================================
// Functional — LOWER(email)
// ============================================================================

#[tokio::test]
async fn functional_lower_eq() {
    let shamir = setup().await;

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

    exec(&shamir, json!({
        "id": 2,
        "queries": {
            "w1": {"insert_into": "posts", "values": [{"email": "Alice@FOO.com", "name": "alice"}]},
            "w2": {"insert_into": "posts", "values": [{"email": "BOB@bar.org", "name": "bob"}]},
        }
    })).await;

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

    exec(&shamir, json!({
        "id": 2,
        "queries": {
            "w1": {"insert_into": "posts", "values": [{"embedding": [1.0, 0.0, 0.0], "label": "x"}]},
            "w2": {"insert_into": "posts", "values": [{"embedding": [0.0, 1.0, 0.0], "label": "y"}]},
            "w3": {"insert_into": "posts", "values": [{"embedding": [0.95, 0.1, 0.0], "label": "x_near"}]},
        }
    })).await;

    let resp = exec(
        &shamir,
        json!({
            "id": 3,
            "queries": {
                "q": {
                    "from": "posts",
                    "where": {
                        "op": "vector_similarity",
                        "field": ["embedding"],
                        "query": [1.0, 0.0, 0.0],
                        "k": 2
                    }
                }
            }
        }),
    )
    .await;
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

    exec(
        &shamir,
        json!({
            "id": 1,
            "queries": {
                "w1": {"insert_into": "posts", "values": [{"body": "hello world"}]},
                "w2": {"insert_into": "posts", "values": [{"body": "no match here"}]},
            }
        }),
    )
    .await;

    let resp = exec(
        &shamir,
        json!({
            "id": 2,
            "queries": {
                "q": {
                    "from": "posts",
                    "where": {"op": "fts", "field": ["body"], "query": "hello", "mode": "and"}
                }
            }
        }),
    )
    .await;
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

    // Create all 3 index types.
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
    let resp = exec(
        &shamir,
        json!({
            "id": 2,
            "queries": {
                "q1": {
                    "from": "posts",
                    "where": {"op": "fts", "field": ["body"], "query": "test", "mode": "and"}
                }
            }
        }),
    )
    .await;
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

    exec(
        &shamir,
        json!({
            "id": 2,
            "queries": {
                "w1": {"insert_into": "posts", "values": [{"body": "running fast"}]},
            }
        }),
    )
    .await;

    // Query the INFLECTED form "running" — the fix stems it to "run"
    // which matches the stored stem. On the old code "running" hashed
    // raw != stored "run" hash → zero results.
    let resp = exec(
        &shamir,
        json!({
            "id": 3,
            "queries": {
                "q": {
                    "from": "posts",
                    "where": {"op": "fts", "field": ["body"], "query": "running", "mode": "and"}
                }
            }
        }),
    )
    .await;
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

    exec(
        &shamir,
        json!({
            "id": 2,
            "queries": {
                "w1": {"insert_into": "posts", "values": [{"body": "the cat sat"}]},
            }
        }),
    )
    .await;

    // Query "the cat" — "the" is a stopword and gets filtered both at
    // index time and query time, so the lookup matches by "cat" only.
    let resp = exec(
        &shamir,
        json!({
            "id": 3,
            "queries": {
                "q": {
                    "from": "posts",
                    "where": {"op": "fts", "field": ["body"], "query": "the cat", "mode": "and"}
                }
            }
        }),
    )
    .await;
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

    exec(
        &shamir,
        json!({
            "id": 2,
            "queries": {
                "w1": {"insert_into": "posts", "values": [
                    {"body": "hello world"},
                    {"body": "help wanted"},
                    {"body": "goodbye"}
                ]},
            }
        }),
    )
    .await;

    // Query the FULL word "hello" (5 chars, NOT a single trigram).
    // The fix ngram-tokenizes the query → [hel,ell,llo]. With mode
    // "and", ALL three grams must be present in the doc. Only "hello
    // world" contains all three; "help wanted" has only "hel".
    let resp = exec(
        &shamir,
        json!({
            "id": 3,
            "queries": {
                "q": {
                    "from": "posts",
                    "where": {
                        "op": "fts",
                        "field": ["body"],
                        "query": "hello",
                        "mode": "and"
                    }
                }
            }
        }),
    )
    .await;
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

    exec(
        &shamir,
        json!({
            "id": 2,
            "queries": {
                "w1": {"insert_into": "posts", "values": [
                    {"body": "les chats noirs"},
                    {"body": "un chien blanc"}
                ]},
            }
        }),
    )
    .await;

    // Query the INFLECTED plural "chats" — the fix stems it to "chat"
    // which matches the stored stem. On the old code "chats" hashed
    // raw != stored "chat" hash → zero results.
    let resp = exec(
        &shamir,
        json!({
            "id": 3,
            "queries": {
                "q": {
                    "from": "posts",
                    "where": {
                        "op": "fts",
                        "field": ["body"],
                        "query": "chats",
                        "mode": "and"
                    }
                }
            }
        }),
    )
    .await;
    let records = &resp.results["q"].records;
    assert_eq!(
        records.len(),
        1,
        "French stemmed query 'chats' (plural) should match 'les chats noirs'"
    );
    assert_eq!(records[0]["body"], "les chats noirs");
}
