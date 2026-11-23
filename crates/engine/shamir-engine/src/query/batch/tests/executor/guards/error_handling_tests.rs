//! Error handling and validation tests for the batch executor.

use std::time::Duration;

use shamir_query_builder::batch::Batch;
use shamir_query_builder::query::Query;
use shamir_query_builder::write;
use shamir_query_builder::write::doc;
use shamir_query_types::batch::BatchRequest;
use shamir_types::access::{AccessError, Action, Actor, ResourcePath};
use shamir_types::types::value::QueryValue;

use crate::db_instance::db_instance::DbInstance;
use crate::query::batch::{
    execute_batch_unchecked as execute_batch, AccessGate, Authorized, BatchError, BatchResolver,
    TableResolver,
};
use crate::query::TableRef;
use crate::repo::repo_types::BoxRepoFactory;
use crate::repo::RepoConfig;
use crate::table::{TableConfig, TableManager};
use shamir_storage::error::{DbError, DbResult};

use super::super::common::setup_resolver;

/// Every awaited call in this module is bounded, so a regression that parks
/// a call can never hang the suite.
const CALL_TIMEOUT: Duration = Duration::from_secs(120);

// ============================================================================
// Circular dependency error
// ============================================================================

#[tokio::test]
async fn test_circular_dependency_error() {
    let resolver = setup_resolver().await;

    // a depends on b, b depends on a
    let mut b = Batch::new();
    b.id(1);
    // We need to build $query refs manually because Handle is returned per-insert
    // and we'd need both aliases before either is registered.
    // Use raw qref since it's a circular dep that can't be expressed via Handle.
    b.query(
        "a",
        Query::from("users").where_eq("id", shamir_query_builder::val::qref("b", "[0].id")),
    );
    b.query(
        "b",
        Query::from("users").where_eq("id", shamir_query_builder::val::qref("a", "[0].id")),
    );
    let req = b.build();

    let err = execute_batch(&req, &resolver, None, None, Actor::System, "test")
        .await
        .unwrap_err();
    assert!(matches!(
        err,
        crate::query::batch::BatchError::CircularDependency { .. }
    ));
}

// ============================================================================
// Pre-validation: unknown table fails before execution
// ============================================================================

#[tokio::test]
async fn test_unknown_table_fails_early() {
    let resolver = setup_resolver().await;

    let mut b = Batch::new();
    b.id(1);
    b.insert(
        "good",
        write::insert("users").row(doc().set("name", "Alice").unwrap()),
    );
    b.query("bad", Query::from("nonexistent_table"));
    let req = b.build();

    let err = execute_batch(&req, &resolver, None, None, Actor::System, "test")
        .await
        .unwrap_err();
    // Should fail with table not found error BEFORE any execution
    assert!(matches!(
        err,
        crate::query::batch::BatchError::QueryError { .. }
    ));
}

// ============================================================================
// Parity: an ABSENT table and an EXISTING-but-CLOSED table answer the SAME
// refusal (no existence oracle)
// ============================================================================

/// Repo `default` holding exactly one table, `items` — the "exists but is
/// closed" side of the pair.
struct ItemsResolver {
    db: DbInstance,
}

#[async_trait::async_trait]
impl TableResolver for ItemsResolver {
    async fn resolve(&self, table_ref: &TableRef) -> DbResult<TableManager> {
        self.db.get_table(&table_ref.repo, &table_ref.table).await
    }
}

#[async_trait::async_trait]
impl BatchResolver for ItemsResolver {
    async fn resolve_repo(&self, repo_name: &str) -> DbResult<crate::tx::RepoCore> {
        self.db
            .get_repo(repo_name)
            .map(|repo| repo.core.clone())
            .ok_or_else(|| DbError::NotFound(format!("Repository '{repo_name}' not found")))
    }
}

async fn setup_items_resolver() -> ItemsResolver {
    let repo_config = RepoConfig {
        name: "default".to_string(),
        factory: BoxRepoFactory::in_memory(),
        tables: vec![TableConfig::new("items")],
    };
    let db = DbInstance::with_repos(vec![repo_config]).await.unwrap();
    ItemsResolver { db }
}

/// Allows everything except a TABLE `Read` — the closed table of the pair.
struct ClosedTableGate;

#[async_trait::async_trait]
impl AccessGate for ClosedTableGate {
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

/// The pair sends the SAME request shape (same id, same alias, one read);
/// only the table NAME differs, and that name must not survive anywhere in
/// the refusal.
fn single_read_batch(alias: &str, table: &str) -> BatchRequest {
    let mut b = Batch::new();
    b.id(1);
    b.query(alias, Query::with_repo("default", table));
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

/// Parity: `items` EXISTS but closed (its Read is denied) and
/// `absent_table` does not exist at all. Both must answer the ONE fixed
/// refusal — identical code, alias and message — and neither may echo the
/// table name or the alias.
///
/// mutant: keeping the two current refusals (`access_denied` + the gate
/// diagnostic for the closed table, `unknown_table` + `"Table '…' in repo
/// '…' not found: …"` for the absent one) makes the messages (and codes)
/// differ, so this test fails on the parent.
#[tokio::test]
async fn absent_and_closed_table_read_refusals_are_identical() {
    let resolver = tokio::time::timeout(CALL_TIMEOUT, setup_items_resolver())
        .await
        .expect("setup must be bounded by CALL_TIMEOUT");

    let closed_req = single_read_batch("items_alias", "items");
    let absent_req = single_read_batch("items_alias", "absent_table");

    let closed = tokio::time::timeout(
        CALL_TIMEOUT,
        Authorized::authorize(&closed_req, Actor::User(7), "default", &ClosedTableGate),
    )
    .await
    .expect("authorization must be bounded by CALL_TIMEOUT")
    .unwrap_err();
    let absent = tokio::time::timeout(
        CALL_TIMEOUT,
        execute_batch(&absent_req, &resolver, None, None, Actor::System, "default"),
    )
    .await
    .expect("execution must be bounded by CALL_TIMEOUT")
    .unwrap_err();

    assert_eq!(
        refusal(&closed),
        refusal(&absent),
        "an absent table and a closed table must answer the identical refusal"
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
        !message.contains("items") && !message.contains("absent_table"),
        "the refusal must not echo the table name: {message}"
    );
    assert!(
        !alias.contains("items_alias"),
        "the refusal must not echo the alias: {alias}"
    );
}

/// A missing PARENT (repository) is NOT an absent table: it keeps the
/// pre-parity `code: None` shape so an absent repository never
/// impersonates `unknown_table` (and never becomes an existence oracle for
/// the tables inside it).
#[tokio::test]
async fn absent_repository_is_not_reported_as_unknown_table() {
    let resolver = tokio::time::timeout(CALL_TIMEOUT, setup_items_resolver())
        .await
        .expect("setup must be bounded by CALL_TIMEOUT");

    let mut b = Batch::new();
    b.id(1);
    b.query("items_alias", Query::with_repo("other_repo", "items"));
    let req = b.build();

    let err = tokio::time::timeout(
        CALL_TIMEOUT,
        execute_batch(&req, &resolver, None, None, Actor::System, "default"),
    )
    .await
    .expect("execution must be bounded by CALL_TIMEOUT")
    .unwrap_err();

    let (code, alias, message) = refusal(&err);
    assert_eq!(
        code, None,
        "a missing repository must keep the existing shape, not unknown_table"
    );
    assert_eq!(alias, "items_alias");
    assert_eq!(
        message,
        "Table 'items' in repo 'other_repo' not found: Item not found: Repository 'other_repo' not found"
    );
}

// ============================================================================
// Request ID echoed in response
// ============================================================================

#[tokio::test]
async fn test_request_id_echoed() {
    let resolver = setup_resolver().await;

    // String ID
    let mut b = Batch::new();
    b.id("req-42");
    b.query("q", Query::from("users"));
    let req = b.build();
    let resp = execute_batch(&req, &resolver, None, None, Actor::System, "test")
        .await
        .unwrap();
    assert_eq!(resp.id, QueryValue::Str("req-42".into()));

    // Numeric ID
    let mut b = Batch::new();
    b.id(123);
    b.query("q", Query::from("users"));
    let req = b.build();
    let resp = execute_batch(&req, &resolver, None, None, Actor::System, "test")
        .await
        .unwrap();
    assert_eq!(resp.id, QueryValue::Int(123));
}
