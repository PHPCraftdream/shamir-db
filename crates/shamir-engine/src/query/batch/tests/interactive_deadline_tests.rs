//! Per-call interactive deadlines and caller-owned rollback compatibility.

use std::time::Duration;

use shamir_query_builder::batch::Batch;
use shamir_query_builder::write::{doc, insert};
use shamir_query_builder::Query;
use shamir_query_types::batch::BatchLimits;
use shamir_types::access::Actor;

use crate::query::batch::interactive_tx::execute_in_open_tx_with_scope;
use crate::query::batch::{
    execute_in_open_tx_unchecked as execute_in_open_tx, open_interactive_tx, Authorized,
    BatchError, ExecScope, ExecutionDeadline, TableResolver,
};
use crate::query::TableRef;
use crate::repo::repo_types::BoxRepoFactory;
use crate::repo::RepoInstance;
use crate::table::{TableConfig, TableManager};

struct TxTestResolver {
    repo: RepoInstance,
}

#[async_trait::async_trait]
impl TableResolver for TxTestResolver {
    async fn resolve(&self, table_ref: &TableRef) -> shamir_storage::error::DbResult<TableManager> {
        self.repo.get_table(&table_ref.table).await
    }

    async fn resolve_repo(
        &self,
        _repo_name: &str,
    ) -> shamir_storage::error::DbResult<RepoInstance> {
        Ok(self.repo.clone())
    }
}

#[tokio::test]
async fn fast_interactive_batch_keeps_tx_usable_and_rollback_discards_writes() {
    let repo = RepoInstance::from_factory(
        "test".into(),
        BoxRepoFactory::in_memory(),
        vec![TableConfig::new("users")],
    )
    .await
    .unwrap();
    let resolver = TxTestResolver { repo: repo.clone() };
    let (mut tx, guard) = open_interactive_tx(&repo, shamir_tx::IsolationLevel::Snapshot)
        .await
        .unwrap();

    let mut batch = Batch::new();
    batch.limits(BatchLimits::default());
    batch.insert("ins", insert("users").row(doc().set("name", "alice")));
    let request = batch.build();
    let response = execute_in_open_tx(
        &request,
        &resolver,
        None,
        None,
        &Actor::System,
        "test",
        &mut tx,
    )
    .await
    .unwrap();
    assert!(response.transaction.is_none(), "EXECUTE must not commit");
    assert_eq!(response.results["ins"].records.len(), 1);

    let mut batch = Batch::new();
    batch.query("rows", Query::from("users"));
    let request = batch.build();
    let response = execute_in_open_tx(
        &request,
        &resolver,
        None,
        None,
        &Actor::System,
        "test",
        &mut tx,
    )
    .await
    .unwrap();
    assert!(response.transaction.is_none(), "the same tx stays open");
    assert_eq!(response.results["rows"].records.len(), 1);

    let (mut observer, observer_guard) =
        open_interactive_tx(&repo, shamir_tx::IsolationLevel::Snapshot)
            .await
            .unwrap();
    let response = execute_in_open_tx(
        &request,
        &resolver,
        None,
        None,
        &Actor::System,
        "test",
        &mut observer,
    )
    .await
    .unwrap();
    assert!(response.results["rows"].records.is_empty());
    drop(observer);
    drop(observer_guard);

    drop(tx);
    drop(guard);

    let (mut observer, observer_guard) =
        open_interactive_tx(&repo, shamir_tx::IsolationLevel::Snapshot)
            .await
            .unwrap();
    let response = execute_in_open_tx(
        &request,
        &resolver,
        None,
        None,
        &Actor::System,
        "test",
        &mut observer,
    )
    .await
    .unwrap();
    assert!(
        response.results["rows"].records.is_empty(),
        "caller rollback must leave no visible writes",
    );
    drop(observer);
    drop(observer_guard);
}

/// Red with the old unbounded execute_plan_tx: the expired call succeeds and
/// stages its insert instead of returning ExecutionTimedOut. The injected
/// deadline traverses the shared REAL interactive entry, including validation,
/// planning, execution and response filtering, not a direct executor-only path.
#[tokio::test]
async fn expired_interactive_deadline_returns_timeout_and_allows_caller_rollback() {
    let repo = RepoInstance::from_factory(
        "test".into(),
        BoxRepoFactory::in_memory(),
        vec![TableConfig::new("users")],
    )
    .await
    .unwrap();
    let resolver = TxTestResolver { repo: repo.clone() };
    let (mut tx, guard) = open_interactive_tx(&repo, shamir_tx::IsolationLevel::Snapshot)
        .await
        .unwrap();

    // An earlier successful call has staged a write in the same open tx.
    let mut batch = Batch::new();
    batch.insert("prior", insert("users").row(doc().set("name", "alice")));
    let request = batch.build();
    let response = execute_in_open_tx(
        &request,
        &resolver,
        None,
        None,
        &Actor::System,
        "test",
        &mut tx,
    )
    .await
    .unwrap();
    assert!(response.transaction.is_none());
    assert_eq!(response.results["prior"].records.len(), 1);

    let mut batch = Batch::new();
    batch.insert("expired", insert("users").row(doc().set("name", "bob")));
    let request = batch.build();
    // std::Instant is not Tokio's paused clock. Zero Duration guarantees
    // expiration without sleeping, preemption or process-global clock state.
    let err = execute_in_open_tx_with_scope(
        Authorized::unchecked(&request, Actor::System, "test"),
        &resolver,
        None,
        None,
        &mut tx,
        &ExecScope::from_duration(&request.limits, Duration::ZERO),
    )
    .await
    .expect_err("an expired per-call deadline must reject interactive execution");
    assert!(
        matches!(err, BatchError::ExecutionTimedOut { budget_secs: 0 }),
        "expected ExecutionTimedOut, got {err:?}",
    );

    // The error neither consumes the handle nor silently rolls it back. A
    // fresh per-call budget can still read the earlier staged write, and the
    // rejected insert has not run. The caller must now roll back the whole tx.
    let mut batch = Batch::new();
    batch.query("rows", Query::from("users"));
    let request = batch.build();
    let response = execute_in_open_tx(
        &request,
        &resolver,
        None,
        None,
        &Actor::System,
        "test",
        &mut tx,
    )
    .await
    .unwrap();
    assert!(response.transaction.is_none());
    assert_eq!(response.results["rows"].records.len(), 1);
    drop(tx);
    drop(guard);

    let (mut observer, observer_guard) =
        open_interactive_tx(&repo, shamir_tx::IsolationLevel::Snapshot)
            .await
            .unwrap();
    let response = execute_in_open_tx(
        &request,
        &resolver,
        None,
        None,
        &Actor::System,
        "test",
        &mut observer,
    )
    .await
    .unwrap();
    assert!(response.transaction.is_none());
    assert!(
        response.results["rows"].records.is_empty(),
        "caller rollback after timeout must discard earlier staged writes too",
    );
    drop(observer);
    drop(observer_guard);
}

#[test]
fn duration_deadline_zero_expires_and_overflow_remains_unbounded() {
    let err = ExecutionDeadline::from_duration(Duration::ZERO)
        .check()
        .expect_err("zero duration must deterministically expire");
    assert!(
        matches!(err, BatchError::ExecutionTimedOut { budget_secs: 0 }),
        "expected zero-second budget in timeout, got {err:?}",
    );
    assert!(
        ExecutionDeadline::from_duration(Duration::MAX)
            .check()
            .is_ok(),
        "an unrepresentable deadline must remain effectively unbounded",
    );
}
