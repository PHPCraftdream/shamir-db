//! `Call`-in-tx rejection tests.
//!
//! A `Call` op delegates to `FunctionInvoker` with autocommit semantics — the
//! function's own writes commit independently of any outer transaction. Inside
//! a transactional batch (`self.tx.is_some()` because
//! `execute_transactional_impl` opened a `TxContext`) or an interactive tx
//! (`self.tx.is_some()` because `execute_in_open_tx` threaded the caller's
//! `TxContext`), that breaks atomicity silently: the outer abort would not
//! roll back the `Call`'s writes.
//!
//! These tests verify that the new `call_in_tx_not_supported` guard converts
//! that silent atomicity violation into an explicit error, mirroring the
//! `nested_tx_not_supported` guard for transactional sub-batches. A `Call`
//! inside a NON-transactional batch (the working case) continues to execute
//! exactly as before.

use shamir_query_types::batch::{BatchLimits, BatchOp, BatchRequest, QueryEntry, ResultEncoding};
use shamir_query_types::call::CallOp;
use shamir_types::access::Actor;
use shamir_types::mpack;
use shamir_types::types::common::new_map;

use crate::query::batch::{
    execute_in_open_tx, open_interactive_tx, ExecutionDeadline, QueryRunner,
};
use crate::query::TableRef;
use crate::repo::repo_types::BoxRepoFactory;
use crate::repo::RepoInstance;
use crate::table::TableConfig;

// ============================================================================
// Shared infrastructure
// ============================================================================

struct TxTestResolver {
    repo: RepoInstance,
}

#[async_trait::async_trait]
impl crate::query::batch::TableResolver for TxTestResolver {
    async fn resolve(
        &self,
        table_ref: &TableRef,
    ) -> shamir_storage::error::DbResult<crate::table::TableManager> {
        self.repo.get_table(&table_ref.table).await
    }

    async fn resolve_repo(
        &self,
        _repo_name: &str,
    ) -> shamir_storage::error::DbResult<RepoInstance> {
        Ok(self.repo.clone())
    }
}

/// Build a `Call` query entry.
fn call_entry() -> QueryEntry {
    QueryEntry {
        op: BatchOp::Call(CallOp {
            call: "some_function".to_string(),
            params: Vec::new(),
            repo: "main".to_string(),
        }),
        return_result: true,
        after: Vec::new(),
        when: None,
    }
}

/// Build a batch request containing a single Call op.
fn call_batch() -> BatchRequest {
    let mut queries = new_map();
    queries.insert("c".to_string(), call_entry());
    BatchRequest {
        id: shamir_types::types::value::QueryValue::Int(1),
        name: None,
        transactional: false,
        isolation: None,
        durability: None,
        queries,
        return_all: true,
        return_only: None,
        limits: BatchLimits::default(),
        interner_epochs: Default::default(),
        result_encoding: ResultEncoding::default(),
    }
}

async fn setup_repo() -> (RepoInstance, TxTestResolver) {
    let factory = BoxRepoFactory::in_memory();
    let repo = RepoInstance::from_factory("test".into(), factory, vec![TableConfig::new("users")])
        .await
        .unwrap();
    let resolver = TxTestResolver { repo: repo.clone() };
    (repo, resolver)
}

// ============================================================================
// Test 1: Call inside an open interactive tx → call_in_tx_not_supported
// ============================================================================

#[tokio::test]
async fn call_in_open_tx_rejected() {
    let (repo, resolver) = setup_repo().await;

    // Open an interactive tx (simulates an open transactional context).
    let (mut tx, guard) = open_interactive_tx(&repo, shamir_tx::IsolationLevel::Snapshot)
        .await
        .unwrap();

    // Drive a batch containing a Call op through the open tx. The runner
    // gets tx: Some(...) so the new guard fires before the invoker is
    // consulted.
    let req = call_batch();
    let result = execute_in_open_tx(
        &req,
        &resolver,
        None, // no admin executor
        None, // no invoker — but the guard fires BEFORE the invoker is needed
        &Actor::System,
        "test",
        &mut tx,
    )
    .await;

    drop(tx);
    drop(guard);

    let err = result.expect_err("Call inside open tx must error");
    assert_eq!(
        err.code(),
        Some("call_in_tx_not_supported"),
        "expected call_in_tx_not_supported, got {:?}",
        err
    );
}

// ============================================================================
// Test 2: Call inside a transactional batch → call_in_tx_not_supported
//
// A transactional batch opens its own TxContext (via
// execute_transactional_impl). The Call op runs inside that tx, so
// self.tx.is_some() and the guard fires. The batch also contains an Insert
// (the realistic shape: a user's transactional batch has data ops alongside
// a Call), which satisfies distinct_repos so the batch reaches execution.
// ============================================================================

#[tokio::test]
async fn call_in_transactional_batch_rejected() {
    use shamir_query_types::write::InsertOp;

    let (_repo, resolver) = setup_repo().await;

    // Insert op (stage 1) — satisfies the distinct_repos / "no data ops" guard
    // so the transactional batch actually reaches the execution phase.
    let insert_entry = QueryEntry {
        op: BatchOp::Insert(InsertOp {
            insert_into: TableRef::new("users"),
            values: vec![mpack!({ "name": "seed" })],
            records_idmsgpack: Vec::new(),
            select: None,
        }),
        return_result: false,
        after: Vec::new(),
        when: None,
    };

    // Call op — has an `after` edge on the Insert so it runs in a LATER stage.
    // This guarantees the Call is reached and hits the guard.
    let mut call_ent = call_entry();
    call_ent.after = vec!["ins".to_string()];

    let mut queries = new_map();
    queries.insert("ins".to_string(), insert_entry);
    queries.insert("c".to_string(), call_ent);
    let req = BatchRequest {
        id: shamir_types::types::value::QueryValue::Int(2),
        name: None,
        transactional: true, // ← this opens a TxContext
        isolation: None,
        durability: None,
        queries,
        return_all: true,
        return_only: None,
        limits: BatchLimits::default(),
        interner_epochs: Default::default(),
        result_encoding: ResultEncoding::default(),
    };

    // Execute through the batch entry point (execute_batch).
    let result = crate::query::batch::execute_batch(
        &req,
        &resolver,
        None, // no admin
        None, // no invoker — guard fires first
        Actor::System,
        "test",
    )
    .await;

    // A transactional batch error is converted to a tx ABORT (Ok response
    // with an aborted TransactionInfo), not propagated as Err — matching the
    // execute_transactional_impl error-handling contract. The error code
    // appears in the tx abort reason (debug-formatted BatchError).
    let resp = result.expect("transactional batch should return Ok with tx info");
    let tx_info = resp
        .transaction
        .as_ref()
        .expect("transactional batch should carry TransactionInfo");
    assert!(
        !tx_info.is_committed(),
        "transactional batch with Call should abort, not commit"
    );
    let reason = tx_info
        .reason
        .as_ref()
        .expect("aborted tx should carry a reason");
    assert!(
        reason.contains("call_in_tx_not_supported"),
        "tx abort reason should contain call_in_tx_not_supported, got: {reason}"
    );
}

// ============================================================================
// Test 3: Regression — Call in a NON-transactional batch works as before
//
// tx: None → the guard does NOT fire. The Call op reaches the invoker.
// Since no invoker is wired, the result is the pre-existing "Function calls
// not supported in this context" error — the key assertion is that it is NOT
// call_in_tx_not_supported.
// ============================================================================

#[tokio::test]
async fn call_in_non_tx_batch_unaffected() {
    let (_repo, resolver) = setup_repo().await;

    // Non-transactional batch containing a Call op.
    let req = call_batch();

    let result = crate::query::batch::execute_batch(
        &req,
        &resolver,
        None, // no admin
        None, // no invoker
        Actor::System,
        "test",
    )
    .await;

    // The Call op reaches the invoker dispatch. Since invoker is None, it
    // gets the pre-existing "not supported" error — NOT the new guard error.
    let err = result.expect_err("Call with no invoker should error (pre-existing behavior)");
    assert_ne!(
        err.code(),
        Some("call_in_tx_not_supported"),
        "non-tx Call must NOT be rejected by the new guard, got: {:?}",
        err
    );
}

// ============================================================================
// Test 4: Direct QueryRunner test — tx: None passes the guard
//
// Constructs a QueryRunner with tx: None directly (like
// query_runner_tests.rs) and verifies a Call op passes the new guard.
// ============================================================================

#[tokio::test]
async fn query_runner_call_with_tx_none_passes_guard() {
    let (_repo, resolver) = setup_repo().await;

    let entry = call_entry();
    let empty_params = new_map();
    let mut runner = QueryRunner {
        resolver: &resolver,
        admin: None,
        invoker: None, // no invoker — pre-existing "not supported" error expected
        tx: None,      // ← non-tx path: guard must NOT fire
        actor: Actor::System,
        db_name: "test",
        depth: 0,
        params: &empty_params,
        result_encoding: ResultEncoding::Name,
        deadline: ExecutionDeadline::unbounded(),
    };
    let result = runner.run("c", &entry, &new_map()).await;

    let err = result.expect_err("Call with no invoker should error (pre-existing behavior)");
    assert_ne!(
        err.code(),
        Some("call_in_tx_not_supported"),
        "non-tx Call must NOT be rejected by the new guard, got: {:?}",
        err
    );
}
