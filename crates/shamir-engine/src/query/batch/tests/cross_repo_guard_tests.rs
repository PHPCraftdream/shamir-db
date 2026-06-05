//! Tests for the transactional cross-repo guard (Stage 4.C).

use shamir_query_builder::batch::Batch;
use shamir_query_builder::query::Query;

use crate::db_instance::db_instance::DbInstance;
use crate::query::batch::{execute_batch, BatchError, TableResolver};
use crate::query::TableRef;
use crate::repo::repo_types::BoxRepoFactory;
use crate::repo::RepoConfig;
use crate::table::{TableConfig, TableManager};
use shamir_query_types::batch::distinct_repos;
use shamir_query_types::batch::QueryEntry;
use shamir_storage::error::DbResult;
use shamir_types::access::Actor;
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
    let mut b = Batch::new();
    b.id(1);
    b.query("q1", Query::from("users"));
    b.query("q2", Query::from("orders"));
    let req = b.build();
    let repos = distinct_repos(&req.queries);
    assert_eq!(repos.len(), 1);
    assert!(repos.contains("main"));
}

#[test]
fn distinct_repos_multiple_repos() {
    let mut b = Batch::new();
    b.id(1);
    b.query("q1", Query::from("users"));
    b.query("q2", Query::with_repo("hot", "sessions"));
    let req = b.build();
    let repos = distinct_repos(&req.queries);
    assert_eq!(repos.len(), 2);
    assert!(repos.contains("main"));
    assert!(repos.contains("hot"));
}

#[test]
fn distinct_repos_skips_admin_ops() {
    // NOTE: The list-tables admin op cannot be expressed through Query::from.
    // Using the DDL builder via Batch::op with list_tables.
    let mut b = Batch::new();
    b.id(1);
    b.query("q1", Query::from("users"));
    b.list_tables("q2", shamir_query_builder::ddl::list_tables());
    let req = b.build();
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

    let mut b = Batch::new();
    b.id(1);
    b.transactional();
    b.query("q1", Query::from("users"));
    b.query("q2", Query::with_repo("hot", "sessions"));
    let req = b.build();

    let err = execute_batch(&req, &resolver, None, Actor::System, "test")
        .await
        .unwrap_err();
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

    let mut b = Batch::new();
    b.id(1);
    b.transactional();
    b.query("q1", Query::from("users"));
    b.query("q2", Query::from("orders"));
    let req = b.build();

    let result = execute_batch(&req, &resolver, None, Actor::System, "test").await;
    assert!(
        !matches!(result, Err(BatchError::CrossRepoNotSupported { .. })),
        "single-repo transactional batch must NOT be rejected by cross-repo guard"
    );
}

#[tokio::test]
async fn non_transactional_cross_repo_batch_unaffected() {
    let resolver = setup_resolver().await;

    // NOTE: Batch builder defaults transactional to false, which matches the
    // original test's "transactional": false.
    let mut b = Batch::new();
    b.id(1);
    b.query("q1", Query::from("users"));
    b.query("q2", Query::with_repo("hot", "sessions"));
    let req = b.build();

    // Non-transactional: the guard should not fire.
    // This will still fail because the "hot" repo doesn't exist in the
    // test resolver, but the error must NOT be CrossRepoNotSupported.
    let result = execute_batch(&req, &resolver, None, Actor::System, "test").await;
    assert!(
        !matches!(result, Err(BatchError::CrossRepoNotSupported { .. })),
        "non-transactional batch must never trigger cross-repo guard"
    );
}
