//! Integration tests for write operation execution.
//!
//! Request-building uses the typed query builder (`shamir_query_builder`).

use crate::db_instance::db_instance::DbInstance;
use crate::query::batch::BatchError;
use crate::query::filter::eval_context::FilterContext;
use crate::query::write::{InsertOp, WriteResult};
use crate::repo::repo_types::BoxRepoFactory;
use crate::repo::{RepoConfig, RepoInstance};
use crate::table::tests::test_helpers::query_value_to_inner_tracked;
use crate::table::TableConfig;
use shamir_types::access::Actor;
use shamir_types::types::common::new_map;
use shamir_types::types::value::QueryValue;

// ---------------------------------------------------------------------------
// Test-only helpers: drive a single INSERT / UPDATE / DELETE through the
// PRODUCTION implicit-tx path (`run_implicit_batch_tx` → the `_tx` variant
// → commit). The legacy non-tx `execute_insert` / `execute_update` /
// `execute_delete` were removed (W3a); these helpers preserve the
// "single-op autocommit" ergonomics the tests rely on while exercising the
// real staging + commit pipeline (so index / counter / doctor side-effects
// land exactly as in production).
// ---------------------------------------------------------------------------

/// Test-only INSERT via the production implicit-tx + commit path.
/// Returns the `Result` so tests can assert on `.is_err()` (e.g. for
/// fail-closed computed-field cases) or `.unwrap()`.
pub(crate) async fn insert_via_tx(
    repo: &RepoInstance,
    table: &crate::table::TableManager,
    op: &InsertOp,
    return_result: bool,
) -> Result<WriteResult, BatchError> {
    let owned_op = op.clone();
    let owned_table = table.clone();
    repo.run_implicit_batch_tx(Actor::System, "test_insert", move |tx| {
        Box::pin(async move {
            owned_table
                .execute_insert_tx(
                    &owned_op,
                    tx,
                    return_result,
                    None,
                    &shamir_types::access::Actor::System,
                )
                .await
        })
    })
    .await
}

/// Test-only UPDATE via the production implicit-tx + commit path.
/// `ctx` is rebuilt inside the staging closure (FilterContext borrows the
/// interner Arc, which must be obtained AFTER `begin_tx`).
pub(crate) async fn update_via_tx(
    repo: &RepoInstance,
    table: &crate::table::TableManager,
    op: &shamir_query_types::write::UpdateOp,
    refs: &shamir_types::types::common::TMap<String, crate::query::read::QueryResult>,
) -> Result<WriteResult, BatchError> {
    let owned_op = op.clone();
    let owned_table = table.clone();
    let owned_refs = refs.clone();
    repo.run_implicit_batch_tx(Actor::System, "test_update", move |tx| {
        Box::pin(async move {
            let interner = owned_table.interner().get().await?;
            let ctx = FilterContext::new(interner, &owned_refs);
            owned_table
                .execute_update_tx(
                    &owned_op,
                    &ctx,
                    tx,
                    None,
                    &shamir_types::access::Actor::System,
                )
                .await
        })
    })
    .await
}

/// Test-only DELETE via the production implicit-tx + commit path.
pub(crate) async fn delete_via_tx(
    repo: &RepoInstance,
    table: &crate::table::TableManager,
    op: &shamir_query_types::write::DeleteOp,
    refs: &shamir_types::types::common::TMap<String, crate::query::read::QueryResult>,
) -> Result<WriteResult, BatchError> {
    let owned_op = op.clone();
    let owned_table = table.clone();
    let owned_refs = refs.clone();
    repo.run_implicit_batch_tx(Actor::System, "test_delete", move |tx| {
        Box::pin(async move {
            let interner = owned_table.interner().get().await?;
            let ctx = FilterContext::new(interner, &owned_refs);
            owned_table
                .execute_delete_tx(
                    &owned_op,
                    &ctx,
                    tx,
                    None,
                    &shamir_types::access::Actor::System,
                )
                .await
        })
    })
    .await
}

/// Test-only SET (upsert) via the production implicit-tx + commit path.
pub(crate) async fn set_via_tx(
    repo: &RepoInstance,
    table: &crate::table::TableManager,
    op: &shamir_query_types::write::SetOp,
) -> Result<WriteResult, BatchError> {
    let owned_op = op.clone();
    let owned_table = table.clone();
    repo.run_implicit_batch_tx(Actor::System, "test_set", move |tx| {
        Box::pin(async move {
            owned_table
                .execute_set_tx(&owned_op, tx, None, &shamir_types::access::Actor::System)
                .await
        })
    })
    .await
}

/// Create a DbInstance with one "users" table, return the table manager + repo.
pub(crate) async fn setup_empty_table() -> (crate::table::TableManager, RepoInstance) {
    let repo_config = RepoConfig {
        name: "default".to_string(),
        factory: BoxRepoFactory::in_memory(),
        tables: vec![TableConfig::new("users")],
    };
    let db = DbInstance::with_repos(vec![repo_config]).await.unwrap();
    let table = db.get_table("default", "users").await.unwrap();
    let repo = db.get_repo("default").unwrap();
    (table, repo)
}

/// Setup table with pre-inserted users.
pub(crate) async fn setup_table_with_users() -> (crate::table::TableManager, RepoInstance) {
    let (table, repo) = setup_empty_table().await;

    let users = vec![
        vec![
            ("name", QueryValue::Str("Alice".into())),
            ("age", QueryValue::Int(30)),
            ("status", QueryValue::Str("active".into())),
        ],
        vec![
            ("name", QueryValue::Str("Bob".into())),
            ("age", QueryValue::Int(25)),
            ("status", QueryValue::Str("active".into())),
        ],
        vec![
            ("name", QueryValue::Str("Carol".into())),
            ("age", QueryValue::Int(35)),
            ("status", QueryValue::Str("inactive".into())),
        ],
    ];

    let interner = table.interner().get().await.unwrap();
    for fields in &users {
        let mut map = new_map();
        for (k, v) in fields {
            map.insert(k.to_string(), v.clone());
        }
        let user_val = QueryValue::Map(map);
        let (inner_val, new_keys) = query_value_to_inner_tracked(&user_val, interner).unwrap();
        if !new_keys.is_empty() {
            table.interner().save_new_keys(&new_keys).await.unwrap();
        }
        table.insert(&inner_val).await.unwrap();
    }

    (table, repo)
}
