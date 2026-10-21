//! Regression tests for the type-level authorization seam (#1199) —
//! [`Authorized`]/[`AccessGate`] in `authorized.rs`.
//!
//! These tests prove the enforcement mechanism actually blocks an
//! unauthorized call (not just that the types compile): a denying
//! [`AccessGate`] stops [`Authorized::authorize`] from ever minting a
//! token, and — because [`execute_batch`] takes `Authorized<'_>` by value,
//! not a raw `&BatchRequest` + `Actor` pair — there is no `Authorized`
//! value left to hand it. A permissive gate, by contrast, mints a token
//! that `execute_batch` genuinely executes against.

use std::sync::Mutex;

use shamir_query_builder::batch::Batch;
use shamir_query_builder::query::Query;
use shamir_query_types::batch::{BatchOp, QueryEntry, SubBatchOp};

use crate::db_instance::db_instance::DbInstance;
use crate::query::batch::{execute_batch, AccessGate, Authorized, BatchError, TableResolver};
use crate::query::TableRef;
use crate::repo::repo_types::BoxRepoFactory;
use crate::repo::{RepoConfig, RepoInstance};
use crate::table::{TableConfig, TableManager};
use shamir_storage::error::DbResult;
use shamir_types::access::{AccessError, Action, Actor, ResourcePath};
use shamir_types::types::common::new_map;

// ============================================================================
// Test gates
// ============================================================================

/// Denies every `(actor, path, action)` triple.
struct DenyGate;

#[async_trait::async_trait]
impl AccessGate for DenyGate {
    async fn check(
        &self,
        actor: &Actor,
        path: &ResourcePath,
        action: Action,
    ) -> Result<(), AccessError> {
        Err(AccessError {
            actor: actor.clone(),
            path: path.to_string(),
            action,
        })
    }
}

/// Allows every `(actor, path, action)` triple.
struct AllowGate;

#[async_trait::async_trait]
impl AccessGate for AllowGate {
    async fn check(
        &self,
        _actor: &Actor,
        _path: &ResourcePath,
        _action: Action,
    ) -> Result<(), AccessError> {
        Ok(())
    }
}

/// Allows every triple, but records what it saw — used to prove
/// `Authorized::authorize`'s recursive walk (via `collect_required_access`)
/// still reaches nested `Batch`/`ForEach` bodies after the per-op
/// authorization loop moved out of `shamir-db` and into this seam.
#[derive(Default)]
struct RecordingGate {
    seen: Mutex<Vec<(Actor, ResourcePath, Action)>>,
}

#[async_trait::async_trait]
impl AccessGate for RecordingGate {
    async fn check(
        &self,
        actor: &Actor,
        path: &ResourcePath,
        action: Action,
    ) -> Result<(), AccessError> {
        self.seen
            .lock()
            .unwrap()
            .push((actor.clone(), path.clone(), action));
        Ok(())
    }
}

// ============================================================================
// Minimal resolver (only needed for the one test that actually executes)
// ============================================================================

struct TestResolver {
    db: DbInstance,
}

#[async_trait::async_trait]
impl TableResolver for TestResolver {
    async fn resolve(&self, table_ref: &TableRef) -> DbResult<TableManager> {
        self.db.get_table("default", &table_ref.table).await
    }

    async fn resolve_repo(&self, _repo_name: &str) -> DbResult<RepoInstance> {
        self.db.get_repo("default").ok_or_else(|| {
            shamir_storage::error::DbError::NotFound("repo 'default' not found".into())
        })
    }
}

async fn setup_resolver() -> TestResolver {
    let repo_config = RepoConfig {
        name: "default".to_string(),
        factory: BoxRepoFactory::in_memory(),
        tables: vec![TableConfig::new("users")],
    };
    let db = DbInstance::with_repos(vec![repo_config]).await.unwrap();
    TestResolver { db }
}

// ============================================================================
// Tests
// ============================================================================

/// A denying gate stops `Authorized::authorize` from minting a token —
/// there is nothing left to pass to `execute_batch`.
#[tokio::test]
async fn deny_gate_blocks_authorization() {
    let mut b = Batch::new();
    b.id(1);
    b.query("q1", Query::from("users"));
    let req = b.build();

    let err = Authorized::authorize(&req, Actor::User(7), "test", &DenyGate)
        .await
        .unwrap_err();
    match err {
        BatchError::QueryError { code, .. } => {
            assert_eq!(code.as_deref(), Some("access_denied"));
        }
        other => panic!("expected QueryError{{code: access_denied}}, got {other:?}"),
    }
}

/// The DB-visibility check runs even for a batch with ZERO data ops (no
/// `(Action, ResourcePath)` pairs for `collect_required_access` to find) —
/// proving authorization isn't skipped just because the per-op loop has
/// nothing to iterate.
#[tokio::test]
async fn deny_gate_blocks_even_an_empty_batch() {
    let mut b = Batch::new();
    b.id(1);
    let req = b.build(); // no queries at all

    let err = Authorized::authorize(&req, Actor::User(1), "test", &DenyGate)
        .await
        .unwrap_err();
    assert!(matches!(
        err,
        BatchError::QueryError { code, .. } if code.as_deref() == Some("access_denied")
    ));
}

/// A permissive gate mints a real token, and `execute_batch` genuinely
/// executes against it — contrasted with `deny_gate_blocks_authorization`
/// to prove the two gates produce genuinely different observable outcomes,
/// not just different names.
#[tokio::test]
async fn allow_gate_authorizes_and_execute_batch_runs() {
    let resolver = setup_resolver().await;

    let mut b = Batch::new();
    b.id(1);
    b.query("q1", Query::from("users"));
    let req = b.build();

    let auth = Authorized::authorize(&req, Actor::System, "test", &AllowGate)
        .await
        .unwrap();
    let resp = execute_batch(auth, &resolver, None, None)
        .await
        .expect("execute_batch must run once a token is minted");
    assert!(resp.results.contains_key("q1"));
}

/// `collect_required_access`'s recursive walk into nested `Batch` bodies
/// (the #660-class fix) still runs from INSIDE `Authorized::authorize`
/// after the per-op loop moved here from `shamir-db`'s `execute_as` (#1199)
/// — a table only reachable through a nested `BatchOp::Batch` must still
/// reach the gate.
#[tokio::test]
async fn authorize_recurses_into_nested_batch_ops() {
    let mut inner_b = Batch::new();
    inner_b.id(2);
    inner_b.query("inner_read", Query::from("orders"));
    let inner_req = inner_b.build();

    let sub_entry = QueryEntry {
        op: BatchOp::Batch(SubBatchOp {
            batch: inner_req,
            bind: new_map(),
        }),
        return_result: true,
        after: Vec::new(),
        when: None,
    };

    let mut outer_b = Batch::new();
    outer_b.id(1);
    outer_b.query("outer_read", Query::from("users"));
    let mut outer_req = outer_b.build();
    outer_req.queries.insert("sub".to_string(), sub_entry);

    let gate = RecordingGate::default();
    Authorized::authorize(&outer_req, Actor::User(3), "test", &gate)
        .await
        .unwrap();

    let seen = gate.seen.lock().unwrap();
    assert!(
        seen.iter().any(|(_, path, _)| matches!(
            path,
            ResourcePath::Table { table, .. } if table == "orders"
        )),
        "nested Batch body's inner op must reach the gate; saw: {seen:?}"
    );
    assert!(
        seen.iter().any(|(_, path, _)| matches!(
            path,
            ResourcePath::Table { table, .. } if table == "users"
        )),
        "outer op must also reach the gate; saw: {seen:?}"
    );
}
