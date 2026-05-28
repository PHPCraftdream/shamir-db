//! Tests for the transactional cross-repo guard (Stage 4.C).

use serde_json::json;

use crate::db_instance::db_instance::DbInstance;
use crate::query::batch::{execute_batch, BatchError, BatchRequest, TableResolver};
use crate::query::TableRef;
use crate::repo::repo_types::BoxRepoFactory;
use crate::repo::RepoConfig;
use crate::table::{TableConfig, TableManager};
use shamir_query_types::batch::distinct_repos;
use shamir_query_types::batch::QueryEntry;
use shamir_storage::error::DbResult;
use shamir_types::types::common::TMap;

struct TestResolver {
    db: DbInstance,
    repo: String,
}

#[async_trait::async_trait]
impl TableResolver for TestResolver {
    async fn resolve(&self, table_ref: &TableRef) -> DbResult<TableManager> {
        self.db.get_table(&self.repo, &table_ref.table).await
    }

    async fn resolve_repo(&self, _repo_name: &str) -> DbResult<crate::repo::RepoInstance> {
        Err(shamir_storage::error::DbError::NotFound(
            "TestResolver does not back transactional repo lookups".into(),
        ))
    }
}

async fn setup_resolver() -> TestResolver {
    let repo_config = RepoConfig {
        name: "default".to_string(),
        factory: BoxRepoFactory::in_memory(),
        tables: vec![TableConfig::new("users"), TableConfig::new("orders")],
    };
    let db = DbInstance::with_repos(vec![repo_config]).await.unwrap();
    TestResolver {
        db,
        repo: "default".to_string(),
    }
}

// ============================================================================
// distinct_repos unit tests
// ============================================================================

#[test]
fn distinct_repos_empty_queries() {
    let queries: TMap<String, QueryEntry> = TMap::default();
    let repos = distinct_repos(&queries);
    assert!(repos.is_empty(), "empty queries map -> no repos");
}

#[test]
fn distinct_repos_single_repo() {
    let req: BatchRequest = serde_json::from_value(json!({
        "id": 1,
        "queries": {
            "q1": {"from": "users"},
            "q2": {"from": "orders"}
        }
    }))
    .unwrap();
    let repos = distinct_repos(&req.queries);
    assert_eq!(repos.len(), 1);
    assert!(repos.contains("main"));
}

#[test]
fn distinct_repos_multiple_repos() {
    let req: BatchRequest = serde_json::from_value(json!({
        "id": 1,
        "queries": {
            "q1": {"from": "users"},
            "q2": {"from": ["hot", "sessions"]}
        }
    }))
    .unwrap();
    let repos = distinct_repos(&req.queries);
    assert_eq!(repos.len(), 2);
    assert!(repos.contains("main"));
    assert!(repos.contains("hot"));
}

#[test]
fn distinct_repos_skips_admin_ops() {
    let req: BatchRequest = serde_json::from_value(json!({
        "id": 1,
        "queries": {
            "q1": {"from": "users"},
            "q2": {"list": "tables"}
        }
    }))
    .unwrap();
    let repos = distinct_repos(&req.queries);
    assert_eq!(repos.len(), 1);
    assert!(repos.contains("main"));
}

// ============================================================================
// Executor integration tests
// ============================================================================

#[tokio::test]
async fn cross_repo_transactional_batch_rejected() {
    let resolver = setup_resolver().await;

    let req: BatchRequest = serde_json::from_value(json!({
        "id": 1,
        "transactional": true,
        "queries": {
            "q1": {"from": "users"},
            "q2": {"from": ["hot", "sessions"]}
        }
    }))
    .unwrap();

    let err = execute_batch(&req, &resolver, None).await.unwrap_err();
    match &err {
        BatchError::CrossRepoNotSupported { repos } => {
            assert_eq!(repos.len(), 2);
            assert_eq!(repos[0], "hot");
            assert_eq!(repos[1], "main");
        }
        other => panic!("expected CrossRepoNotSupported, got {:?}", other),
    }
}

#[tokio::test]
async fn single_repo_transactional_batch_passes_guard() {
    let resolver = setup_resolver().await;

    let req: BatchRequest = serde_json::from_value(json!({
        "id": 1,
        "transactional": true,
        "queries": {
            "q1": {"from": "users"},
            "q2": {"from": "orders"}
        }
    }))
    .unwrap();

    let result = execute_batch(&req, &resolver, None).await;
    assert!(
        !matches!(result, Err(BatchError::CrossRepoNotSupported { .. })),
        "single-repo transactional batch must NOT be rejected by cross-repo guard"
    );
}

#[tokio::test]
async fn non_transactional_cross_repo_batch_unaffected() {
    let resolver = setup_resolver().await;

    let req: BatchRequest = serde_json::from_value(json!({
        "id": 1,
        "transactional": false,
        "queries": {
            "q1": {"from": "users"},
            "q2": {"from": ["hot", "sessions"]}
        }
    }))
    .unwrap();

    // Non-transactional: the guard should not fire.
    // This will still fail because the "hot" repo doesn't exist in the
    // test resolver, but the error must NOT be CrossRepoNotSupported.
    let result = execute_batch(&req, &resolver, None).await;
    assert!(
        !matches!(result, Err(BatchError::CrossRepoNotSupported { .. })),
        "non-transactional batch must never trigger cross-repo guard"
    );
}
