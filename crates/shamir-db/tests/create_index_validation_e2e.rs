//! P1-6 (#970): server-side e2e validation parity for CREATE INDEX.
//!
//! These tests exercise the FULL wire pipeline (`Batch → msgpack →
//! ShamirDb::execute → handle_create_index`) to prove the new cross-type
//! validation checks in `admin_table_index.rs` actually fire — not just the
//! client-side `try_build()` layer.
//!
//! The headline case (`unique` + `index_type("vector")`) was a live
//! silently-accepted bug before P1-6: the non-btree dispatch at line ~374 of
//! `admin_table_index.rs` returned BEFORE any `unique`/`sorted`/cross-family
//! check, and `create_index_v2` never reads `op.unique`, so a `.unique()`
//! vector index silently became a NON-unique vector index. This test file
//! proves that path is now rejected at the server level.

use shamir_db::engine::repo::repo_types::BoxRepoFactory;
use shamir_db::engine::repo::RepoConfig;
use shamir_db::engine::table::TableConfig;
use shamir_db::query::batch::BatchRequest;
use shamir_db::ShamirDb;
use shamir_query_builder::batch::Batch;
use shamir_query_builder::ddl;

/// Create an in-memory DB + repo with a single `docs` table.
async fn setup() -> ShamirDb {
    let shamir = ShamirDb::init_memory().await.unwrap();
    let db = shamir.create_db("testdb").await;
    let repo_config =
        RepoConfig::new("main", BoxRepoFactory::in_memory()).add_table(TableConfig::new("docs"));
    db.add_repo(repo_config).await.unwrap();
    shamir
}

/// Build a one-op batch that creates an index, serialize through msgpack,
/// and execute it — returning the raw `Result` so callers can assert Ok/Err.
async fn exec_create(shamir: &ShamirDb, builder: ddl::CreateIndex) -> Result<(), String> {
    let mut b = Batch::new();
    b.id(1);
    b.create_index("mk", builder);
    let req: BatchRequest = b.to_request_via_msgpack();
    match shamir.execute("testdb", &req).await {
        Ok(_) => Ok(()),
        Err(e) => Err(e.to_string()),
    }
}

// ============================================================================
// The live bug: `unique` + `index_type("vector")` was silently accepted.
// ============================================================================

/// `.unique().index_type("vector")` MUST be rejected by the server.
///
/// Before P1-6 this silently created a NON-unique vector index (the non-btree
/// dispatch returned before any unique check, and `create_index_v2` never
/// reads `op.unique`).
#[tokio::test]
async fn server_rejects_unique_vector_index() {
    let shamir = setup().await;
    let result = exec_create(
        &shamir,
        ddl::create_index("vec_unique", "docs")
            .field("embedding")
            .unique()
            .index_type("vector")
            .vector_dim(128),
    )
    .await;
    assert!(
        result.is_err(),
        "unique + vector must be rejected by the server, got Ok"
    );
    let msg = result.unwrap_err();
    assert!(
        msg.contains("unique") && msg.contains("vector"),
        "error should mention 'unique' and 'vector': {msg}"
    );
}

// ============================================================================
// Other cross-type rejection cases (e2e parity).
// ============================================================================

#[tokio::test]
async fn server_rejects_sorted_fts_index() {
    let shamir = setup().await;
    let result = exec_create(
        &shamir,
        ddl::create_index("fts_sorted", "docs")
            .field("body")
            .sorted()
            .index_type("fts"),
    )
    .await;
    assert!(result.is_err(), "sorted + fts must be rejected");
    let msg = result.unwrap_err();
    assert!(
        msg.contains("sorted") && msg.contains("fts"),
        "error should mention 'sorted' and 'fts': {msg}"
    );
}

#[tokio::test]
async fn server_rejects_vector_missing_dim() {
    let shamir = setup().await;
    let result = exec_create(
        &shamir,
        ddl::create_index("vec_nodim", "docs")
            .field("embedding")
            .index_type("vector"),
    )
    .await;
    assert!(result.is_err(), "vector without dim must be rejected");
    let msg = result.unwrap_err();
    assert!(
        msg.contains("vector_dim"),
        "error should mention 'vector_dim': {msg}"
    );
}

#[tokio::test]
async fn server_rejects_vector_zero_dim() {
    let shamir = setup().await;
    let result = exec_create(
        &shamir,
        ddl::create_index("vec_zerodim", "docs")
            .field("embedding")
            .index_type("vector")
            .vector_dim(0),
    )
    .await;
    assert!(result.is_err(), "vector with dim=0 must be rejected");
}

#[tokio::test]
async fn server_rejects_unknown_vector_metric() {
    let shamir = setup().await;
    let result = exec_create(
        &shamir,
        ddl::create_index("vec_badmetric", "docs")
            .field("embedding")
            .index_type("vector")
            .vector_dim(128)
            .vector_metric("consine"),
    )
    .await;
    assert!(result.is_err(), "unknown vector_metric must be rejected");
    let msg = result.unwrap_err();
    assert!(
        msg.contains("consine"),
        "error should mention the bad metric 'consine': {msg}"
    );
}

#[tokio::test]
async fn server_rejects_vector_options_on_non_vector() {
    let shamir = setup().await;
    let result = exec_create(
        &shamir,
        ddl::create_index("bt_with_vec", "docs")
            .field("embedding")
            .vector_dim(128),
    )
    .await;
    assert!(
        result.is_err(),
        "vector_dim on a non-vector index must be rejected"
    );
}

#[tokio::test]
async fn server_rejects_fts_options_on_non_fts() {
    let shamir = setup().await;
    let result = exec_create(
        &shamir,
        ddl::create_index("bt_with_fts", "docs")
            .field("body")
            .fts_tokenizer("whitespace"),
    )
    .await;
    assert!(
        result.is_err(),
        "fts_tokenizer on a non-fts index must be rejected"
    );
}

#[tokio::test]
async fn server_rejects_functional_options_on_non_functional() {
    let shamir = setup().await;
    let result = exec_create(
        &shamir,
        ddl::create_index("bt_with_fn", "docs")
            .field("body")
            .functional_op("lower"),
    )
    .await;
    assert!(
        result.is_err(),
        "functional_op on a non-functional index must be rejected"
    );
}

#[tokio::test]
async fn server_rejects_empty_fields() {
    let shamir = setup().await;
    let result = exec_create(&shamir, ddl::create_index("no_fields", "docs")).await;
    assert!(result.is_err(), "empty fields must be rejected");
    let msg = result.unwrap_err();
    assert!(
        msg.contains("at least one field"),
        "error should mention 'at least one field': {msg}"
    );
}

#[tokio::test]
async fn server_rejects_include_on_non_btree() {
    let shamir = setup().await;
    let result = exec_create(
        &shamir,
        ddl::create_index("fts_include", "docs")
            .field("body")
            .index_type("fts")
            .include([vec!["title".to_string()]]),
    )
    .await;
    assert!(
        result.is_err(),
        "include on a non-btree index must be rejected"
    );
    let msg = result.unwrap_err();
    assert!(
        msg.contains("include") && msg.contains("fts"),
        "error should mention 'include' and 'fts': {msg}"
    );
}

// ============================================================================
// Valid cases still succeed (regression guard).
// ============================================================================

#[tokio::test]
async fn server_accepts_valid_vector_index() {
    let shamir = setup().await;
    let result = exec_create(
        &shamir,
        ddl::create_index("vec_ok", "docs")
            .field("embedding")
            .index_type("vector")
            .vector_dim(128)
            .vector_metric("cosine"),
    )
    .await;
    assert!(
        result.is_ok(),
        "valid vector index must succeed: {:?}",
        result.err()
    );
}
