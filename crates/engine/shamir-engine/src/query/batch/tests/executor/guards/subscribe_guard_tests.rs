//! Tests for subscription feature-gate rejections (Etap 2, Live Subscriptions v1.1).

use std::time::Duration;

use shamir_query_builder::batch::subscribe::{SourceBuilder, Subscribe};
use shamir_query_builder::batch::Batch;
use shamir_query_types::filter::{Filter, FilterValue};
use shamir_query_types::TableRef;

use crate::db_instance::db_instance::DbInstance;
use crate::query::batch::{
    execute_batch_unchecked as execute_batch, AccessGate, Authorized, BatchError, BatchResolver,
    TableResolver,
};
use crate::repo::repo_types::BoxRepoFactory;
use crate::repo::RepoConfig;
use crate::table::{TableConfig, TableManager};
use shamir_storage::error::DbResult;
use shamir_types::access::{AccessError, Action, Actor, ResourcePath};

/// Every awaited call in this module is bounded, so a regression that parks
/// a call can never hang the suite.
const CALL_TIMEOUT: Duration = Duration::from_secs(120);

struct TestResolver {
    db: DbInstance,
    repo: String,
}

#[async_trait::async_trait]
impl TableResolver for TestResolver {
    async fn resolve(&self, table_ref: &TableRef) -> DbResult<TableManager> {
        self.db.get_table(&self.repo, &table_ref.table).await
    }
}

#[async_trait::async_trait]
impl BatchResolver for TestResolver {
    async fn resolve_repo(&self, _repo_name: &str) -> DbResult<crate::tx::RepoCore> {
        // Back repo lookups with the real in-memory repo: the source
        // classification consults the catalogue (`resolve_repo`) to tell an
        // absent TABLE from an absent PARENT.
        self.db
            .get_repo(&self.repo)
            .map(|repo| repo.core.clone())
            .ok_or_else(|| {
                shamir_storage::error::DbError::NotFound(format!(
                    "Repository '{}' not found",
                    self.repo
                ))
            })
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

/// Allows everything except a TABLE `Read` — the closed source of the pair.
struct ClosedSourceGate;

#[async_trait::async_trait]
impl AccessGate for ClosedSourceGate {
    async fn check(
        &self,
        actor: &Actor,
        path: &ResourcePath,
        action: Action,
    ) -> Result<(), AccessError> {
        if action == Action::Read && matches!(path, ResourcePath::Table { .. }) {
            return Err(AccessError {
                actor: actor.clone(),
                path: path.to_string(),
                action,
            });
        }
        Ok(())
    }
}

/// One single-source subscription — the SAME request shape for both halves
/// of the pair; only the source table NAME differs.
fn single_source_subscribe(alias: &str, table: &str) -> shamir_query_types::batch::BatchRequest {
    let sub_op = Subscribe::table(TableRef::with_repo("default", table)).build();
    let mut b = Batch::new();
    b.id(1);
    b.subscribe(alias, sub_op);
    b.build()
}

/// `(code, alias, message)` of a refusal — the whole observable refusal.
fn refusal(error: &BatchError) -> (Option<String>, String, String) {
    match error {
        BatchError::QueryError {
            alias,
            message,
            code,
        } => (code.clone(), alias.clone(), message.clone()),
        other => panic!("expected a QueryError refusal, got {other:?}"),
    }
}

/// Parity pair: a subscription whose source EXISTS but is closed (its Read
/// is denied) and one whose source is ABSENT must answer the SAME fixed
/// `unknown_table` refusal, and hand out NO grant.
///
/// mutant: before parity `Authorized::authorize` never classified
/// subscription sources (they carry no `required_access` entry), so the
/// CLOSED source was GRANTED — a subscription id for a table the actor may
/// not read — while the absent source answered `table_not_found` with the
/// table name echoed. Both halves of this assertion fail on the parent.
#[tokio::test]
async fn absent_and_closed_subscription_sources_refuse_identically() {
    let resolver = tokio::time::timeout(CALL_TIMEOUT, setup_resolver())
        .await
        .expect("setup must be bounded by CALL_TIMEOUT");

    let closed_req = single_source_subscribe("sub1", "users");
    let absent_req = single_source_subscribe("sub1", "absent_items");

    let closed = tokio::time::timeout(
        CALL_TIMEOUT,
        Authorized::authorize(&closed_req, Actor::User(7), "test", &ClosedSourceGate),
    )
    .await
    .expect("authorization must be bounded by CALL_TIMEOUT")
    .unwrap_err();
    let absent = tokio::time::timeout(
        CALL_TIMEOUT,
        execute_batch(&absent_req, &resolver, None, None, Actor::System, "test"),
    )
    .await
    .expect("execution must be bounded by CALL_TIMEOUT")
    .unwrap_err();

    assert_eq!(
        refusal(&closed),
        refusal(&absent),
        "an absent source and a closed source must answer the identical refusal"
    );
    assert_eq!(
        refusal(&closed),
        (
            Some("unknown_table".to_string()),
            String::new(),
            "table is unavailable".to_string()
        )
    );

    let (_, alias, message) = refusal(&absent);
    assert!(
        !message.contains("users") && !message.contains("absent_items"),
        "the refusal must not echo the source table: {message}"
    );
    assert!(
        !alias.contains("sub1"),
        "the refusal must not echo the alias: {alias}"
    );
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
