//! Tests for subscription feature-gate rejections (Etap 2, Live Subscriptions v1.1).

use shamir_query_builder::batch::subscribe::{SourceBuilder, Subscribe};
use shamir_query_builder::batch::Batch;
use shamir_query_types::filter::{Filter, FilterValue};
use shamir_query_types::TableRef;

use crate::db_instance::db_instance::DbInstance;
use crate::query::batch::{execute_batch, BatchError, TableResolver};
use crate::repo::repo_types::BoxRepoFactory;
use crate::repo::RepoConfig;
use crate::table::{TableConfig, TableManager};
use shamir_storage::error::DbResult;
use shamir_types::access::Actor;

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

#[tokio::test]
async fn subscribe_multi_repo_rejected() {
    let resolver = setup_resolver().await;

    let src1 = SourceBuilder::table(TableRef::with_repo("default", "users")).build();
    let src2 = SourceBuilder::table(TableRef::with_repo("other_repo", "sessions")).build();
    let sub_op = Subscribe::sources(vec![src1, src2]).build();

    let mut b = Batch::new();
    b.id(1);
    b.subscribe("sub1", sub_op);
    let req = b.build();

    let err = execute_batch(&req, &resolver, None, None, Actor::System, "test")
        .await
        .unwrap_err();
    match &err {
        BatchError::QueryError { code, .. } => {
            assert_eq!(
                code.as_deref(),
                Some("multi_repo_subscriptions_not_supported")
            );
        }
        other => panic!("expected QueryError with multi_repo code, got {:?}", other),
    }
}

#[tokio::test]
async fn subscribe_with_filter_accepted() {
    let resolver = setup_resolver().await;

    let src = SourceBuilder::table(TableRef::with_repo("default", "users"))
        .filter(Filter::Eq {
            field: vec!["name".to_string()],
            value: FilterValue::String("alice".to_string()),
        })
        .build();
    let sub_op = Subscribe::source(src).build();

    let mut b = Batch::new();
    b.id(1);
    b.subscribe("sub1", sub_op);
    let req = b.build();

    let result = execute_batch(&req, &resolver, None, None, Actor::System, "test").await;
    assert!(
        result.is_ok(),
        "subscribe with filter should succeed: {:?}",
        result.err()
    );
}

#[tokio::test]
async fn subscribe_with_initial_accepted() {
    let resolver = setup_resolver().await;

    let sub_op = Subscribe::table(TableRef::with_repo("default", "users"))
        .with_initial()
        .build();

    let mut b = Batch::new();
    b.id(1);
    b.subscribe("sub1", sub_op);
    let req = b.build();

    let result = execute_batch(&req, &resolver, None, None, Actor::System, "test").await;
    assert!(
        result.is_ok(),
        "subscribe with initial should succeed: {:?}",
        result.err()
    );
}

#[tokio::test]
async fn subscribe_single_repo_no_filter_no_initial_succeeds() {
    let resolver = setup_resolver().await;

    let sub_op = Subscribe::table(TableRef::with_repo("default", "users")).build();

    let mut b = Batch::new();
    b.id(1);
    b.subscribe("sub1", sub_op);
    let req = b.build();

    let result = execute_batch(&req, &resolver, None, None, Actor::System, "test").await;
    assert!(
        result.is_ok(),
        "simple subscribe should succeed: {:?}",
        result.err()
    );
}
