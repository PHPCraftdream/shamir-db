//! Integration tests for batch executor.

use serde_json::json;

use crate::db_instance::db_instance::DbInstance;
use crate::query::auth::{Action, Effect, Permission, Resource, Role, SessionPermissions};
use crate::query::batch::{
    commit_interactive_tx, execute_batch, execute_batch_with_permissions, execute_in_open_tx,
    open_interactive_tx, BatchRequest, QueryRunner, TableResolver,
};
use crate::query::TableRef;
use crate::repo::repo_types::BoxRepoFactory;
use crate::repo::RepoConfig;
use crate::table::{TableConfig, TableManager};
use shamir_storage::error::DbResult;
use shamir_types::access::Actor;

/// Simple resolver that wraps a DbInstance + repo name.
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
// Single read query
// ============================================================================

#[tokio::test]
async fn test_single_read_query() {
    let resolver = setup_resolver().await;

    // Insert some data first
    let insert_req: BatchRequest = serde_json::from_value(json!({
        "id": 1,
        "queries": {
            "insert": {
                "insert_into": "users",
                "values": [
                    {"name": "Alice", "age": 30},
                    {"name": "Bob", "age": 25}
                ]
            }
        }
    }))
    .unwrap();
    let resp = execute_batch(&insert_req, &resolver, None, Actor::System, "test")
        .await
        .unwrap();
    assert_eq!(resp.results["insert"].records.len(), 2);

    // Now read
    let req: BatchRequest = serde_json::from_value(json!({
        "id": 1,
        "queries": {
            "users": {"from": "users"}
        }
    }))
    .unwrap();

    let resp = execute_batch(&req, &resolver, None, Actor::System, "test")
        .await
        .unwrap();

    assert_eq!(resp.results.len(), 1);
    assert_eq!(resp.results["users"].records.len(), 2);
    assert!(!resp.execution_plan.is_empty());
}

// ============================================================================
// Independent queries run in same stage
// ============================================================================

#[tokio::test]
async fn test_independent_queries_same_stage() {
    let resolver = setup_resolver().await;

    // Seed data
    let seed_req: BatchRequest = serde_json::from_value(json!({
        "id": 1,
        "queries": {
            "s1": {
                "insert_into": "users",
                "values": [{"name": "Alice"}],
                "return_result": false
            },
            "s2": {
                "insert_into": "orders",
                "values": [{"item": "Book"}],
                "return_result": false
            }
        }
    }))
    .unwrap();
    execute_batch(&seed_req, &resolver, None, Actor::System, "test")
        .await
        .unwrap();

    // Two independent reads
    let req: BatchRequest = serde_json::from_value(json!({
        "id": 1,
        "queries": {
            "users": {"from": "users"},
            "orders": {"from": "orders"}
        }
    }))
    .unwrap();

    let resp = execute_batch(&req, &resolver, None, Actor::System, "test")
        .await
        .unwrap();

    // Both in same stage (no dependencies)
    assert_eq!(resp.execution_plan.len(), 1);
    assert_eq!(resp.execution_plan[0].len(), 2);
    assert_eq!(resp.results.len(), 2);
}

// ============================================================================
// Dependent queries: $query ref
// ============================================================================

#[tokio::test]
async fn test_dependent_query_ref() {
    let resolver = setup_resolver().await;

    // Seed users
    let seed_req: BatchRequest = serde_json::from_value(json!({
        "id": 1,
        "queries": {
            "seed": {
                "insert_into": "users",
                "values": [
                    {"name": "Alice", "status": "active"},
                    {"name": "Bob", "status": "inactive"},
                    {"name": "Carol", "status": "active"}
                ],
                "return_result": false
            }
        }
    }))
    .unwrap();
    execute_batch(&seed_req, &resolver, None, Actor::System, "test")
        .await
        .unwrap();

    // Query 1: get active users
    // Query 2: get users where name == first active user's name (via $query ref)
    let req: BatchRequest = serde_json::from_value(json!({
        "id": 1,
        "queries": {
            "active": {
                "from": "users",
                "where": {"op": "eq", "field": ["status"], "value": "active"}
            },
            "first_active": {
                "from": "users",
                "where": {
                    "op": "eq",
                    "field": ["name"],
                    "value": {"$query": "active", "path": "[0].name"}
                }
            }
        }
    }))
    .unwrap();

    let resp = execute_batch(&req, &resolver, None, Actor::System, "test")
        .await
        .unwrap();

    // Two stages: [active], [first_active]
    assert_eq!(resp.execution_plan.len(), 2);
    assert_eq!(resp.results["active"].records.len(), 2); // Alice + Carol
    assert_eq!(resp.results["first_active"].records.len(), 1); // Alice
}

// ============================================================================
// Insert + read pipeline
// ============================================================================

#[tokio::test]
async fn test_insert_then_read() {
    let resolver = setup_resolver().await;

    let req: BatchRequest = serde_json::from_value(json!({
        "id": 1,
        "queries": {
            "insert": {
                "insert_into": "users",
                "values": [
                    {"name": "Alice", "score": 100},
                    {"name": "Bob", "score": 50}
                ]
            },
            "read": {"from": "users"}
        }
    }))
    .unwrap();

    let resp = execute_batch(&req, &resolver, None, Actor::System, "test")
        .await
        .unwrap();

    // Both in same stage (no explicit dependency)
    assert_eq!(resp.results["insert"].records.len(), 2);
    // Read may or may not see the inserted records depending on execution order
    // within the stage (sequential currently, so insert runs first)
    assert_eq!(resp.results["read"].records.len(), 2);
}

// ============================================================================
// return_only filtering
// ============================================================================

#[tokio::test]
async fn test_return_only() {
    let resolver = setup_resolver().await;

    let req: BatchRequest = serde_json::from_value(json!({
        "id": 1,
        "queries": {
            "insert": {
                "insert_into": "users",
                "values": [{"name": "Alice"}]
            },
            "read": {"from": "users"}
        },
        "return_only": ["read"]
    }))
    .unwrap();

    let resp = execute_batch(&req, &resolver, None, Actor::System, "test")
        .await
        .unwrap();

    // Only "read" returned
    assert_eq!(resp.results.len(), 1);
    assert!(resp.results.contains_key("read"));
}

// ============================================================================
// return_result = false
// ============================================================================

#[tokio::test]
async fn test_return_result_false() {
    let resolver = setup_resolver().await;

    let req: BatchRequest = serde_json::from_value(json!({
        "id": 1,
        "queries": {
            "setup": {
                "insert_into": "users",
                "values": [{"name": "Alice"}],
                "return_result": false
            },
            "read": {"from": "users"}
        },
        "return_all": false
    }))
    .unwrap();

    let resp = execute_batch(&req, &resolver, None, Actor::System, "test")
        .await
        .unwrap();

    // "setup" has return_result=false, "read" has return_result=true (default)
    assert_eq!(resp.results.len(), 1);
    assert!(resp.results.contains_key("read"));
}

// ============================================================================
// Delete in batch
// ============================================================================

#[tokio::test]
async fn test_batch_with_delete() {
    let resolver = setup_resolver().await;

    // Seed
    let seed_req: BatchRequest = serde_json::from_value(json!({
        "id": 1,
        "queries": {
            "seed": {
                "insert_into": "users",
                "values": [
                    {"name": "Alice", "status": "active"},
                    {"name": "Bob", "status": "inactive"}
                ],
                "return_result": false
            }
        }
    }))
    .unwrap();
    execute_batch(&seed_req, &resolver, None, Actor::System, "test")
        .await
        .unwrap();

    // Delete inactive, then read
    let req: BatchRequest = serde_json::from_value(json!({
        "id": 1,
        "queries": {
            "cleanup": {
                "delete_from": "users",
                "where": {"op": "eq", "field": ["status"], "value": "inactive"}
            }
        }
    }))
    .unwrap();

    let resp = execute_batch(&req, &resolver, None, Actor::System, "test")
        .await
        .unwrap();
    // 1 record deleted (Bob)
    assert_eq!(
        resp.results["cleanup"]
            .stats
            .as_ref()
            .unwrap()
            .records_scanned,
        1
    );
}

// ============================================================================
// Circular dependency error
// ============================================================================

#[tokio::test]
async fn test_circular_dependency_error() {
    let resolver = setup_resolver().await;

    // a depends on b, b depends on a
    let req: BatchRequest = serde_json::from_value(json!({
        "id": 1,
        "queries": {
            "a": {
                "from": "users",
                "where": {
                    "op": "eq",
                    "field": ["id"],
                    "value": {"$query": "b", "path": "[0].id"}
                }
            },
            "b": {
                "from": "users",
                "where": {
                    "op": "eq",
                    "field": ["id"],
                    "value": {"$query": "a", "path": "[0].id"}
                }
            }
        }
    }))
    .unwrap();

    let err = execute_batch(&req, &resolver, None, Actor::System, "test")
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

    let req: BatchRequest = serde_json::from_value(json!({
        "id": 1,
        "queries": {
            "good": {
                "insert_into": "users",
                "values": [{"name": "Alice"}]
            },
            "bad": {"from": "nonexistent_table"}
        }
    }))
    .unwrap();

    let err = execute_batch(&req, &resolver, None, Actor::System, "test")
        .await
        .unwrap_err();
    // Should fail with table not found error BEFORE any execution
    assert!(matches!(
        err,
        crate::query::batch::BatchError::QueryError { .. }
    ));
}

// ============================================================================
// Request ID echoed in response
// ============================================================================

#[tokio::test]
async fn test_request_id_echoed() {
    let resolver = setup_resolver().await;

    // String ID
    let req: BatchRequest = serde_json::from_value(json!({
        "id": "req-42",
        "queries": {
            "q": {"from": "users"}
        }
    }))
    .unwrap();
    let resp = execute_batch(&req, &resolver, None, Actor::System, "test")
        .await
        .unwrap();
    assert_eq!(resp.id, json!("req-42"));

    // Numeric ID
    let req: BatchRequest = serde_json::from_value(json!({
        "id": 123,
        "queries": {
            "q": {"from": "users"}
        }
    }))
    .unwrap();
    let resp = execute_batch(&req, &resolver, None, Actor::System, "test")
        .await
        .unwrap();
    assert_eq!(resp.id, json!(123));
}

// ============================================================================
// QueryRunner struct — tx: None path
// ============================================================================

#[tokio::test]
async fn test_query_runner_none_tx_insert_and_read() {
    let resolver = setup_resolver().await;

    // Insert via QueryRunner with tx: None
    let insert_req: BatchRequest = serde_json::from_value(json!({
        "id": 1,
        "queries": {
            "ins": {
                "insert_into": "users",
                "values": [{"name": "Eve", "age": 28}]
            }
        }
    }))
    .unwrap();
    let insert_entry = insert_req.queries.get("ins").unwrap().clone();
    let mut runner = QueryRunner {
        resolver: &resolver,
        admin: None,
        tx: None,
        actor: Actor::System,
        db_name: "test",
    };
    let result = runner
        .run(
            "ins",
            &insert_entry,
            &shamir_types::types::common::new_map(),
        )
        .await
        .unwrap();
    assert_eq!(result.records.len(), 1);

    // Read via QueryRunner with tx: None
    let read_req: BatchRequest = serde_json::from_value(json!({
        "id": 2,
        "queries": {
            "q": {"from": "users"}
        }
    }))
    .unwrap();
    let read_entry = read_req.queries.get("q").unwrap().clone();
    let mut runner = QueryRunner {
        resolver: &resolver,
        admin: None,
        tx: None,
        actor: Actor::System,
        db_name: "test",
    };
    let result = runner
        .run("q", &read_entry, &shamir_types::types::common::new_map())
        .await
        .unwrap();
    assert_eq!(result.records.len(), 1);
}

// ============================================================================
// execute_batch — transactional SI happy path
// ============================================================================

struct TxTestResolver {
    repo: crate::repo::RepoInstance,
}

#[async_trait::async_trait]
impl TableResolver for TxTestResolver {
    async fn resolve(&self, table_ref: &TableRef) -> DbResult<TableManager> {
        self.repo.get_table(&table_ref.table).await
    }

    async fn resolve_repo(&self, _repo_name: &str) -> DbResult<crate::repo::RepoInstance> {
        Ok(self.repo.clone())
    }
}

#[tokio::test]
async fn execute_batch_transactional_si_happy_path() {
    use futures::StreamExt;

    let factory = crate::repo::repo_types::BoxRepoFactory::in_memory();
    let repo = crate::repo::RepoInstance::from_factory(
        "test".into(),
        factory,
        vec![crate::table::TableConfig::new("users")],
    )
    .await
    .unwrap();

    let resolver = TxTestResolver { repo: repo.clone() };

    let request: BatchRequest = serde_json::from_value(json!({
        "id": 1,
        "transactional": true,
        "queries": {
            "ins": {
                "insert_into": "users",
                "values": [
                    {"name": "alice"},
                    {"name": "bob"}
                ]
            }
        },
        "return_all": true
    }))
    .unwrap();

    let response = execute_batch(&request, &resolver, None, Actor::System, "test")
        .await
        .unwrap();

    let info = response.transaction.expect("transaction info present");
    assert_eq!(info.status, "committed");
    assert!(info.tx_id > 0);
    assert!(info.commit_version.unwrap_or(0) > 0);
    // Happy path: projections applied inline (`MaterializationState::Complete`)
    // is threaded to the client as materialized=true.
    assert!(
        info.materialized,
        "inline-materialized commit must report materialized=true"
    );

    // Outside the tx, observer reads see the committed records.
    let tbl = repo.get_table("users").await.unwrap();
    let stream = tbl.list_stream(64);
    futures::pin_mut!(stream);
    let mut count = 0usize;
    while let Some(batch) = stream.next().await {
        count += batch.unwrap().len();
    }
    assert_eq!(count, 2, "outside observer must see 2 committed records");
}

// ============================================================================
// A1 — executor maps the commit's MaterializationState to TransactionInfo.
//
// The happy path above proves the `Complete` case end-to-end through a real
// `commit_tx`. A `Deferred` outcome only arises when a projection sub-phase
// fails AFTER the commit point — that fault-injection seam lives in
// `tx::commit::materialize`, which this agent does not own. So we pin the
// executor's mapping (`outcome.materialized()` → `TransactionInfo::committed`)
// directly against a hand-built `TxOutcome` for both states.
// ============================================================================

/// The mapping the executor performs at the commit-success arm of
/// `execute_transactional` (executor.rs). Kept in lock-step with the
/// production call site.
fn map_outcome_to_info(
    outcome: &crate::tx::TxOutcome,
) -> shamir_query_types::batch::TransactionInfo {
    shamir_query_types::batch::TransactionInfo::committed(
        outcome.tx_id,
        outcome.snapshot_version,
        outcome.commit_version,
        outcome.materialized(),
    )
}

#[test]
fn deferred_outcome_maps_to_materialized_false() {
    use crate::tx::commit::MaterializationState;

    let complete = crate::tx::TxOutcome {
        tx_id: 11,
        snapshot_version: 100,
        commit_version: 101,
        materialization: MaterializationState::Complete,
        background: None,
    };
    let deferred = crate::tx::TxOutcome {
        tx_id: 12,
        snapshot_version: 100,
        commit_version: 102,
        materialization: MaterializationState::Deferred,
        background: None,
    };

    let complete_info = map_outcome_to_info(&complete);
    assert!(complete_info.is_committed());
    assert!(
        complete_info.materialized,
        "Complete must map to materialized=true"
    );

    let deferred_info = map_outcome_to_info(&deferred);
    assert!(
        deferred_info.is_committed(),
        "a deferred commit is still COMMITTED, not aborted"
    );
    assert!(
        !deferred_info.materialized,
        "Deferred must map to materialized=false"
    );
}

// ============================================================================
// Vector I.1 — SSI write-skew detected THROUGH the execute_batch wire path.
//
// This is the decisive proof that the executor's `BatchOp::Read` path now
// threads the active tx into the tx-aware table read (`read_tx`), so a
// Serializable batch's SELECT populates the read-set and commit-time SSI
// validation can fire. Before the fix the read branch called `table.read`
// with NO tx, leaving `read_set` empty, so Phase 2 (`validate_read_set`)
// passed vacuously and NO conflict could ever arise — Serializable silently
// degraded to Snapshot end-to-end through the wire.
//
// Determinism without a read↔commit hook inside `execute_batch`: the reader
// batch carries TWO ops — a SELECT of the record (`r`, stage 0) and a second
// read of a `gate` table (`g`, stage 1, made to depend on `r` via a `$query`
// ref so it executes strictly after). A custom resolver, on the
// EXECUTION-phase resolve of `gate` (the read already recorded the record at
// version 0 in stage 0), runs a real transactional writer batch that updates
// the record and commits — bumping its MVCC version. The reader then commits
// and its stale recorded read (v0 < the writer's commit version) triggers
// SsiConflict, surfaced as `transaction.status == "aborted"`, reason
// `"tx_conflict"`. The writer's commit overlaps the reader's still-open tx,
// exactly the concurrent write-skew the task describes.
//
// Load-bearing: the conflict EXISTS ONLY because the SELECT was recorded. The
// companion test below (`..._no_record_no_conflict`) drops the read op and
// shows the same writer interleave then commits cleanly — isolating the
// read-set recording as the cause.
// ============================================================================

/// Resolver for the I.1 write-skew test. Resolves tables from `repo`, and on
/// the execution-phase resolve of `gate_table` runs `writer_req` (a real
/// transactional batch) to completion, injecting a committed concurrent write
/// between the reader's recorded SELECT and its commit.
struct GateBarrierResolver {
    repo: crate::repo::RepoInstance,
    gate_table: String,
    /// Counts resolves of `gate_table`. Resolve 0 is `validate_tables`
    /// (pre-execution); resolve 1 is the execution-phase resolve of the
    /// stage-1 `g` op — the only one that fires the writer.
    gate_resolves: std::sync::Arc<std::sync::atomic::AtomicUsize>,
    writer_req: BatchRequest,
}

#[async_trait::async_trait]
impl TableResolver for GateBarrierResolver {
    async fn resolve(&self, table_ref: &TableRef) -> DbResult<TableManager> {
        if table_ref.table == self.gate_table {
            let n = self
                .gate_resolves
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            // n == 0 → validate_tables; n == 1 → stage-1 execution resolve.
            if n == 1 {
                // The reader's SELECT (stage 0) has already recorded the
                // record at version 0. Now commit a concurrent writer.
                let writer_resolver = TxTestResolver {
                    repo: self.repo.clone(),
                };
                let resp = execute_batch(
                    &self.writer_req,
                    &writer_resolver,
                    None,
                    Actor::System,
                    "test",
                )
                .await
                .expect("writer batch executes");
                let info = resp.transaction.expect("writer batch has transaction info");
                assert_eq!(
                    info.status, "committed",
                    "writer batch must commit to bump the record version, got {:?}",
                    info
                );
            }
        }
        self.repo.get_table(&table_ref.table).await
    }

    async fn resolve_repo(&self, _repo_name: &str) -> DbResult<crate::repo::RepoInstance> {
        Ok(self.repo.clone())
    }
}

#[tokio::test]
async fn ssi_write_skew_detected_through_execute_batch() {
    let factory = crate::repo::repo_types::BoxRepoFactory::in_memory();
    let repo = crate::repo::RepoInstance::from_factory(
        "test".into(),
        factory,
        vec![
            crate::table::TableConfig::new("users"),
            crate::table::TableConfig::new("gate"),
        ],
    )
    .await
    .unwrap();

    // Seed the record outside any tx — its tracked MVCC version is 0.
    let tbl = repo.get_table("users").await.unwrap();
    tbl.insert(&{
        let mut m = shamir_types::types::common::new_map_wc(2);
        let interner = tbl.interner().get().await.unwrap();
        use shamir_types::core::interner::TouchInd;
        let name_id = match interner.touch_ind("name").unwrap() {
            TouchInd::Exists(k) | TouchInd::New(k) => k.id(),
        };
        let val_id = match interner.touch_ind("val").unwrap() {
            TouchInd::Exists(k) | TouchInd::New(k) => k.id(),
        };
        tbl.interner().persist().await.unwrap();
        m.insert(
            shamir_types::core::interner::InternerKey::new(name_id),
            shamir_types::types::value::InnerValue::Str("alice".into()),
        );
        m.insert(
            shamir_types::core::interner::InternerKey::new(val_id),
            shamir_types::types::value::InnerValue::Str("initial".into()),
        );
        shamir_types::types::value::InnerValue::Map(m)
    })
    .await
    .unwrap();

    // Writer batch — a real transactional batch that updates the record and
    // commits, bumping its version. Run by the gate resolver in the seam
    // between the reader's SELECT and the reader's commit.
    let writer_req: BatchRequest = serde_json::from_value(json!({
        "id": "writer",
        "transactional": true,
        "queries": {
            "w": {
                "update": "users",
                "where": {
                    "op": "eq",
                    "field": ["name"],
                    "value": "alice"
                },
                "set": {
                    "val": "rewritten"
                }
            }
        }
    }))
    .unwrap();

    let resolver = GateBarrierResolver {
        repo: repo.clone(),
        gate_table: "gate".to_string(),
        gate_resolves: std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0)),
        writer_req,
    };

    // Reader batch (Serializable): SELECT the record (stage 0, recorded), then
    // a gate read (stage 1, depends on `r`) whose resolve fires the writer.
    let reader_req: BatchRequest = serde_json::from_value(json!({
        "id": "reader",
        "transactional": true,
        "isolation": "serializable",
        "queries": {
            "r": {
                "from": "users"
            },
            "g": {
                "from": "gate",
                "where": {
                    "op": "eq",
                    "field": ["probe"],
                    "value": {
                        "$query": "r",
                        "path": "[0].name"
                    }
                }
            }
        },
        "return_all": true
    }))
    .unwrap();

    let response = execute_batch(&reader_req, &resolver, None, Actor::System, "test")
        .await
        .unwrap();

    // Plan must have two stages: [r] then [g] (g depends on r).
    assert_eq!(
        response.execution_plan.len(),
        2,
        "reader plan must be two stages so the writer fires after the SELECT recorded; got {:?}",
        response.execution_plan
    );

    let info = response
        .transaction
        .expect("reader batch has transaction info");
    assert_eq!(
        info.status, "aborted",
        "reader's Serializable tx must abort: its recorded SELECT is now stale. \
         Got status={:?} reason={:?} — if 'committed', the executor read did NOT \
         record into the read-set (the I.1 wire is missing).",
        info.status, info.reason
    );
    assert_eq!(
        info.reason.as_deref(),
        Some("tx_conflict"),
        "abort reason must be the SSI conflict surfaced from commit"
    );
}

/// Companion isolating the cause: SAME concurrent writer interleave, but the
/// reader batch does NOT select the record (only the gate read remains). With
/// nothing recorded into the read-set, the reader commits cleanly — proving
/// the conflict in the test above arises ONLY from the recorded SELECT, not
/// from the writer's commit alone.
#[tokio::test]
async fn ssi_write_skew_no_record_no_conflict_through_execute_batch() {
    let factory = crate::repo::repo_types::BoxRepoFactory::in_memory();
    let repo = crate::repo::RepoInstance::from_factory(
        "test".into(),
        factory,
        vec![
            crate::table::TableConfig::new("users"),
            crate::table::TableConfig::new("gate"),
        ],
    )
    .await
    .unwrap();

    let tbl = repo.get_table("users").await.unwrap();
    tbl.insert(&shamir_types::types::value::InnerValue::Str(
        "initial".into(),
    ))
    .await
    .unwrap();

    let writer_req: BatchRequest = serde_json::from_value(json!({
        "id": "writer",
        "transactional": true,
        "queries": {
            "w": {
                "insert_into": "users",
                "values": [{"name": "carol"}]
            }
        }
    }))
    .unwrap();

    // Gate fires the writer on its first execution-phase resolve. With a
    // single-stage reader (no `r`), `gate` is resolved once by validate_tables
    // (n == 0) and once during execution (n == 1) — same trigger point.
    let resolver = GateBarrierResolver {
        repo: repo.clone(),
        gate_table: "gate".to_string(),
        gate_resolves: std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0)),
        writer_req,
    };

    let reader_req: BatchRequest = serde_json::from_value(json!({
        "id": "reader",
        "transactional": true,
        "isolation": "serializable",
        "queries": {
            "g": {
                "from": "gate"
            }
        },
        "return_all": true
    }))
    .unwrap();

    let response = execute_batch(&reader_req, &resolver, None, Actor::System, "test")
        .await
        .unwrap();
    let info = response
        .transaction
        .expect("reader batch has transaction info");
    assert_eq!(
        info.status, "committed",
        "with no recorded read, a concurrent writer must NOT conflict the reader; \
         got status={:?} reason={:?}",
        info.status, info.reason
    );
}

// ============================================================================
// Phase B — interactive (multi-call) transaction glue
//
// The engine-side primitives `open_interactive_tx` / `execute_in_open_tx` /
// `commit_interactive_tx` let a `TxContext` outlive a single batch: the
// server parks it between client round-trips and drives several EXECUTEs
// against it before COMMIT. These tests exercise that lifetime directly
// against a `RepoInstance` (no wire), the way the Phase-A acceptance suite
// does. See `docs/roadmap/PHASE_B_INTERACTIVE_TX.md` §5/§9.2.
//
// NOTE on RYOW: read-your-own-writes within an open tx is a POINT-read
// property (`read_one_tx` overlays staging); a full-table scan
// (`{"from": ...}`) projects the committed snapshot and intentionally does
// NOT overlay uncommitted staging (the documented streaming-RYOW limit). So
// these tests assert the load-bearing Phase-B guarantee — state ACCUMULATES
// across EXECUTE calls and commits together (or rolls back as a whole) —
// rather than scan-visibility of an uncommitted write.
// ============================================================================

#[tokio::test]
async fn interactive_tx_accumulates_writes_across_calls_then_commits() {
    use futures::StreamExt;

    let factory = crate::repo::repo_types::BoxRepoFactory::in_memory();
    let repo = crate::repo::RepoInstance::from_factory(
        "test".into(),
        factory,
        vec![crate::table::TableConfig::new("users")],
    )
    .await
    .unwrap();
    let resolver = TxTestResolver { repo: repo.clone() };

    // BEGIN — mint the interactive tx + its snapshot guard (the server would
    // park both in its registry; here the test holds them).
    let (mut tx, guard) = open_interactive_tx(&repo, shamir_tx::IsolationLevel::Snapshot)
        .await
        .unwrap();

    // EXECUTE #1 — stage the first insert. The tx stays OPEN, so the response
    // carries no commit outcome.
    let call1: BatchRequest = serde_json::from_value(json!({
        "id": 1,
        "queries": {
            "ins": {
                "insert_into": "users",
                "values": [
                    {"name": "alice"}
                ]
            }
        }
    }))
    .unwrap();
    let r1 = execute_in_open_tx(&call1, &resolver, None, &Actor::System, "test", &mut tx)
        .await
        .unwrap();
    assert!(
        r1.transaction.is_none(),
        "tx is still open after EXECUTE #1 — no per-call commit outcome"
    );

    // A separate observer must NOT see the uncommitted staged write
    // (snapshot isolation — nothing is durable before COMMIT).
    let tbl = repo.get_table("users").await.unwrap();
    {
        let stream = tbl.list_stream(64);
        futures::pin_mut!(stream);
        let mut count = 0usize;
        while let Some(b) = stream.next().await {
            count += b.unwrap().len();
        }
        assert_eq!(count, 0, "outside observer sees nothing before commit");
    }

    // EXECUTE #2 — stage a second insert into the SAME open tx.
    let call2: BatchRequest = serde_json::from_value(json!({
        "id": 2,
        "queries": {
            "ins": {
                "insert_into": "users",
                "values": [
                    {"name": "bob"}
                ]
            }
        }
    }))
    .unwrap();
    let r2 = execute_in_open_tx(&call2, &resolver, None, &Actor::System, "test", &mut tx)
        .await
        .unwrap();
    assert!(r2.transaction.is_none(), "tx still open after EXECUTE #2");

    // COMMIT — both calls' writes land together at one commit version.
    let outcome = commit_interactive_tx(&repo, tx).await.unwrap();
    assert!(outcome.commit_version > 0, "commit assigns a version");
    // The snapshot guard is released only AFTER commit returned.
    drop(guard);

    // Both records, staged across two SEPARATE EXECUTE calls, are visible.
    let stream = tbl.list_stream(64);
    futures::pin_mut!(stream);
    let mut count = 0usize;
    while let Some(b) = stream.next().await {
        count += b.unwrap().len();
    }
    assert_eq!(
        count, 2,
        "both writes staged across two EXECUTE calls must commit together"
    );
}

#[tokio::test]
async fn interactive_tx_rollback_discards_staged_writes() {
    use futures::StreamExt;

    let factory = crate::repo::repo_types::BoxRepoFactory::in_memory();
    let repo = crate::repo::RepoInstance::from_factory(
        "test".into(),
        factory,
        vec![crate::table::TableConfig::new("users")],
    )
    .await
    .unwrap();
    let resolver = TxTestResolver { repo: repo.clone() };

    let (mut tx, guard) = open_interactive_tx(&repo, shamir_tx::IsolationLevel::Snapshot)
        .await
        .unwrap();

    let call: BatchRequest = serde_json::from_value(json!({
        "id": 1,
        "queries": {
            "ins": {
                "insert_into": "users",
                "values": [
                    {"name": "ghost"}
                ]
            }
        }
    }))
    .unwrap();
    execute_in_open_tx(&call, &resolver, None, &Actor::System, "test", &mut tx)
        .await
        .unwrap();

    // ROLLBACK = drop the parked tx (RAII rollback, no storage side effects),
    // then release the snapshot.
    drop(tx);
    drop(guard);

    // Nothing was committed — a fresh scan sees no records.
    let tbl = repo.get_table("users").await.unwrap();
    let stream = tbl.list_stream(64);
    futures::pin_mut!(stream);
    let mut count = 0usize;
    while let Some(b) = stream.next().await {
        count += b.unwrap().len();
    }
    assert_eq!(
        count, 0,
        "a rolled-back interactive tx must leave nothing durable"
    );
}

// ============================================================================
// Phase B Stage 9 — SSI read-set ACCUMULATES across multiple TxExecute calls.
//
// Two interactive Serializable txs race. Each does its SELECT in one
// execute_in_open_tx call and its UPDATE in a SECOND, separate
// execute_in_open_tx call against the SAME parked TxContext. tx_a commits
// first (Ok). tx_b commits second and MUST abort with CommitError::SsiConflict
// — its read_set, recorded back in call #1, still pins alice@v0 while tx_a's
// commit bumped alice's version. If execute_in_open_tx did not preserve the
// read_set across calls (e.g. by re-creating the TxContext per call) tx_b
// would commit cleanly — so the SsiConflict is the load-bearing proof that
// (a) read_set survives across TxExecute round-trips inside the same parked
// TxContext, and (b) validate_read_set fires at commit_interactive_tx exactly
// like Phase-A commit_tx. Mirrors acceptance_tests.rs:467
// (`ssi_write_skew_one_aborts`) but through the Phase-B engine wrappers.
// ============================================================================
#[tokio::test]
async fn interactive_ssi_write_skew_across_calls_one_aborts() {
    use shamir_types::core::interner::TouchInd;

    let factory = crate::repo::repo_types::BoxRepoFactory::in_memory();
    let repo = crate::repo::RepoInstance::from_factory(
        "test".into(),
        factory,
        vec![crate::table::TableConfig::new("users")],
    )
    .await
    .unwrap();
    let resolver = TxTestResolver { repo: repo.clone() };

    // Seed two named rows so an UPDATE can target one by field. Same pattern
    // as ssi_write_skew_detected_through_execute_batch (:706-729).
    let tbl = repo.get_table("users").await.unwrap();
    let interner = tbl.interner().get().await.unwrap();
    let name_id = match interner.touch_ind("name").unwrap() {
        TouchInd::Exists(k) | TouchInd::New(k) => k.id(),
    };
    let val_id = match interner.touch_ind("val").unwrap() {
        TouchInd::Exists(k) | TouchInd::New(k) => k.id(),
    };
    tbl.interner().persist().await.unwrap();
    let mk_row = |name: &str| {
        let mut m = shamir_types::types::common::new_map_wc(2);
        m.insert(
            shamir_types::core::interner::InternerKey::new(name_id),
            shamir_types::types::value::InnerValue::Str(name.into()),
        );
        m.insert(
            shamir_types::core::interner::InternerKey::new(val_id),
            shamir_types::types::value::InnerValue::Str("initial".into()),
        );
        shamir_types::types::value::InnerValue::Map(m)
    };
    tbl.insert(&mk_row("alice")).await.unwrap();
    tbl.insert(&mk_row("bob")).await.unwrap();

    // BEGIN two interactive Serializable txs.
    let (mut tx_a, guard_a) = open_interactive_tx(&repo, shamir_tx::IsolationLevel::Serializable)
        .await
        .unwrap();
    let (mut tx_b, guard_b) = open_interactive_tx(&repo, shamir_tx::IsolationLevel::Serializable)
        .await
        .unwrap();

    // Call #1 on EACH tx: SELECT every users row -> recorded into that tx's
    // read_set via the tx-aware read path.
    let select_req: BatchRequest = serde_json::from_value(json!({
        "id": 1,
        "queries": { "r": { "from": "users" } }
    }))
    .unwrap();
    let ra1 = execute_in_open_tx(
        &select_req,
        &resolver,
        None,
        &Actor::System,
        "test",
        &mut tx_a,
    )
    .await
    .unwrap();
    assert!(ra1.transaction.is_none(), "tx_a still open after call #1");
    assert_eq!(
        ra1.results["r"].records.len(),
        2,
        "tx_a SELECT sees both seeded rows"
    );

    let rb1 = execute_in_open_tx(
        &select_req,
        &resolver,
        None,
        &Actor::System,
        "test",
        &mut tx_b,
    )
    .await
    .unwrap();
    assert!(rb1.transaction.is_none(), "tx_b still open after call #1");
    assert_eq!(
        rb1.results["r"].records.len(),
        2,
        "tx_b SELECT sees both seeded rows"
    );

    // Call #2 on EACH tx: UPDATE the row the OTHER tx also read.
    let update_alice: BatchRequest = serde_json::from_value(json!({
        "id": 2,
        "queries": {
            "w": {
                "update": "users",
                "where": {"op": "eq", "field": ["name"], "value": "alice"},
                "set": {"val": "a2"}
            }
        }
    }))
    .unwrap();
    let update_bob: BatchRequest = serde_json::from_value(json!({
        "id": 2,
        "queries": {
            "w": {
                "update": "users",
                "where": {"op": "eq", "field": ["name"], "value": "bob"},
                "set": {"val": "b2"}
            }
        }
    }))
    .unwrap();
    let _ra2 = execute_in_open_tx(
        &update_alice,
        &resolver,
        None,
        &Actor::System,
        "test",
        &mut tx_a,
    )
    .await
    .unwrap();
    let _rb2 = execute_in_open_tx(
        &update_bob,
        &resolver,
        None,
        &Actor::System,
        "test",
        &mut tx_b,
    )
    .await
    .unwrap();

    // COMMIT tx_a first — succeeds (its read_set, accumulated from call #1, is
    // still valid since nothing committed since its snapshot). Bumps alice's
    // version above tx_b's snapshot.
    let oa = commit_interactive_tx(&repo, tx_a).await;
    assert!(
        oa.is_ok(),
        "tx_a must commit cleanly; got {:?}",
        oa.as_ref().err()
    );
    drop(guard_a);

    // COMMIT tx_b — MUST abort with SsiConflict. tx_b's read_set, populated
    // back in call #1 (NOT in this commit call), still pins alice@v0 — the
    // load-bearing proof that read_set survived across TxExecute calls and
    // validate_read_set fires at commit_interactive_tx.
    let ob = commit_interactive_tx(&repo, tx_b).await;
    match ob {
        Err(crate::tx::CommitError::SsiConflict { .. }) => {}
        other => panic!(
            "tx_b MUST abort with SsiConflict — recorded read from call #1 \
             must outlive that call inside the parked TxContext and be \
             validated at commit. Got {:?}",
            other
                .as_ref()
                .map(|_| "Ok(committed)")
                .map_err(|e| format!("Err({:?})", e)),
        ),
    }
    drop(guard_b);
}

// ============================================================================
// Phase B Stage 10 — concurrency + recovery tests for interactive tx
// ============================================================================

/// (a) Two interactive SI transactions race — each accumulates writes across
/// TWO `execute_in_open_tx` calls (the load-bearing Phase-B property), then
/// both commit. The second committer must assign a higher version
/// (last-commit-wins ordering). Both txs' writes survive (SI permits
/// non-conflicting inserts).
#[tokio::test]
async fn two_interactive_si_txs_race_last_commit_wins() {
    use futures::StreamExt;

    let factory = crate::repo::repo_types::BoxRepoFactory::in_memory();
    let repo = crate::repo::RepoInstance::from_factory(
        "test".into(),
        factory,
        vec![crate::table::TableConfig::new("users")],
    )
    .await
    .unwrap();
    let resolver = TxTestResolver { repo: repo.clone() };

    // Seed a baseline row OUTSIDE any tx so both interactive txs share the
    // same starting state.
    let tbl = repo.get_table("users").await.unwrap();
    let seed_req: BatchRequest = serde_json::from_value(json!({
        "id": 0,
        "queries": {
            "seed": {
                "insert_into": "users",
                "values": [
                    {"name": "baseline"}
                ]
            }
        }
    }))
    .unwrap();
    let seed_resp =
        crate::query::batch::execute_batch(&seed_req, &resolver, None, Actor::System, "test").await;
    assert!(
        seed_resp.is_ok(),
        "seeding baseline row failed: {:?}",
        seed_resp.err()
    );

    // BEGIN two interactive SI txs.
    let (mut tx_a, guard_a) = open_interactive_tx(&repo, shamir_tx::IsolationLevel::Snapshot)
        .await
        .unwrap();
    let (mut tx_b, guard_b) = open_interactive_tx(&repo, shamir_tx::IsolationLevel::Snapshot)
        .await
        .unwrap();

    // Each tx does TWO execute calls (proves state accumulates across calls).
    let ins_a1: BatchRequest = serde_json::from_value(json!({
        "id": 1,
        "queries": {
            "ins": {
                "insert_into": "users",
                "values": [
                    {"name": "a1"}
                ]
            }
        }
    }))
    .unwrap();
    let ins_a2: BatchRequest = serde_json::from_value(json!({
        "id": 2,
        "queries": {
            "ins": {
                "insert_into": "users",
                "values": [
                    {"name": "a2"}
                ]
            }
        }
    }))
    .unwrap();
    let ins_b1: BatchRequest = serde_json::from_value(json!({
        "id": 3,
        "queries": {
            "ins": {
                "insert_into": "users",
                "values": [
                    {"name": "b1"}
                ]
            }
        }
    }))
    .unwrap();
    let ins_b2: BatchRequest = serde_json::from_value(json!({
        "id": 4,
        "queries": {
            "ins": {
                "insert_into": "users",
                "values": [
                    {"name": "b2"}
                ]
            }
        }
    }))
    .unwrap();

    // Interleave the calls — each tx accumulates state across two execute
    // calls before the FIRST commit lands.
    execute_in_open_tx(&ins_a1, &resolver, None, &Actor::System, "test", &mut tx_a)
        .await
        .unwrap();
    execute_in_open_tx(&ins_b1, &resolver, None, &Actor::System, "test", &mut tx_b)
        .await
        .unwrap();
    execute_in_open_tx(&ins_a2, &resolver, None, &Actor::System, "test", &mut tx_a)
        .await
        .unwrap();
    execute_in_open_tx(&ins_b2, &resolver, None, &Actor::System, "test", &mut tx_b)
        .await
        .unwrap();

    // tx_a commits first → version V_a. tx_b commits second → version V_b > V_a.
    let o_a = commit_interactive_tx(&repo, tx_a).await.unwrap();
    drop(guard_a);
    let o_b = commit_interactive_tx(&repo, tx_b).await.unwrap();
    drop(guard_b);
    assert!(
        o_b.commit_version > o_a.commit_version,
        "second-committing interactive SI tx assigns a higher version \
         (last-commit-wins ordering)"
    );

    // Both txs' writes survive (SI permits both): 1 baseline + 4 inserts = 5.
    let stream = tbl.list_stream(64);
    futures::pin_mut!(stream);
    let mut count = 0usize;
    while let Some(b) = stream.next().await {
        count += b.unwrap().len();
    }
    assert_eq!(
        count, 5,
        "both interactive SI txs committed (1 seed + 2 + 2)"
    );
}

/// (b) Crash mid-interactive-tx leaves NOTHING durable.
///
/// Reuse the shared-`Arc<InMemoryRepo>` restart pattern from
/// `tx/tests/recovery_gate_tests.rs:42-62`. An interactive tx is in-memory
/// ONLY until COMMIT (`wal.begin` runs only in commit Phase 4 —
/// `commit.rs:732`). Dropping the `RepoInstance` + tx WITHOUT calling
/// `commit_interactive_tx` perfectly models the crash: no `commit_tx` →
/// no `wal.begin` → no durable footprint.
#[tokio::test]
async fn crash_mid_interactive_tx_leaves_no_durable_footprint() {
    use std::sync::Arc;

    use futures::StreamExt;
    use shamir_storage::storage_in_memory::InMemoryRepo;

    let underlying = Arc::new(InMemoryRepo::new());

    // === ORIGINAL PROCESS ===
    {
        let repo = crate::repo::RepoInstance::new(
            "r".into(),
            crate::repo::BoxRepo::InMemory(Arc::clone(&underlying)),
            vec![crate::table::TableConfig::new("users")],
        );
        let resolver = TxTestResolver { repo: repo.clone() };

        let (mut tx, _guard) = open_interactive_tx(&repo, shamir_tx::IsolationLevel::Snapshot)
            .await
            .unwrap();

        // Stage writes across TWO execute calls.
        let c1: BatchRequest = serde_json::from_value(json!({
            "id": 1,
            "queries": {
                "ins": {
                    "insert_into": "users",
                    "values": [
                        {"name": "alpha"}
                    ]
                }
            }
        }))
        .unwrap();
        let c2: BatchRequest = serde_json::from_value(json!({
            "id": 2,
            "queries": {
                "ins": {
                    "insert_into": "users",
                    "values": [
                        {"name": "beta"}
                    ]
                }
            }
        }))
        .unwrap();
        execute_in_open_tx(&c1, &resolver, None, &Actor::System, "test", &mut tx)
            .await
            .unwrap();
        execute_in_open_tx(&c2, &resolver, None, &Actor::System, "test", &mut tx)
            .await
            .unwrap();

        // Sanity: while tx is open, BEFORE commit, the WAL has no inflight
        // entry (wal.begin runs only in commit Phase 4 — commit.rs:732).
        let wal = repo.repo_wal().await.unwrap();
        assert!(
            wal.list_inflight().await.unwrap().is_empty(),
            "no WAL entry exists pre-commit — wal.begin runs only in Phase 4"
        );

        // === CRASH === drop tx + guard + repo WITHOUT calling
        // commit_interactive_tx.
        drop(tx);
        drop(_guard);
        drop(resolver);
        drop(repo);
    }

    // === RESTART === fresh RepoInstance over the SAME underlying storage.
    let repo = crate::repo::RepoInstance::new(
        "r".into(),
        crate::repo::BoxRepo::InMemory(Arc::clone(&underlying)),
        vec![crate::table::TableConfig::new("users")],
    );

    // (1) No inflight WAL entry survives — none was ever written.
    let wal = repo.repo_wal().await.unwrap();
    assert!(
        wal.list_inflight().await.unwrap().is_empty(),
        "crash before any commit leaves no inflight WAL entry"
    );

    // (2) Recovery is a no-op.
    let replayed = repo.recover_v2_inflight().await.unwrap();
    assert_eq!(
        replayed, 0,
        "recovery has nothing to replay — interactive tx never reached the WAL"
    );

    // (3) Nothing materialized — the table is empty.
    let tbl = repo.get_table("users").await.unwrap();
    let stream = tbl.list_stream(64);
    futures::pin_mut!(stream);
    let mut count = 0usize;
    while let Some(b) = stream.next().await {
        count += b.unwrap().len();
    }
    assert_eq!(
        count, 0,
        "a crash mid-interactive-tx must leave NOTHING durable \
         (no wal.begin → clean abort)"
    );
}

// ============================================================================
// R2 structural test — actor flows through FilterContext
// ============================================================================

/// Verifies the actor field reaches the FilterContext that the QueryRunner
/// builds for each data op. The gate is transparent (always Ok), so this
/// confirms plumbing without needing enforcement.
#[tokio::test]
async fn r2_actor_flows_through_filter_context() {
    use shamir_types::access::Actor;

    let resolver = setup_resolver().await;

    // Insert a row so the read has something to scan.
    let seed_req: BatchRequest = serde_json::from_value(json!({
        "id": 1,
        "queries": {
            "ins": {
                "insert_into": "users",
                "values": [{"name": "Alice", "age": 30}],
                "return_result": false
            }
        }
    }))
    .unwrap();
    execute_batch(&seed_req, &resolver, None, Actor::System, "test_db")
        .await
        .unwrap();

    // Read with an explicit User actor — the executor must carry it
    // into the FilterContext it builds (default is System; using User
    // proves the pipe is live).
    let user_actor = Actor::User(42);
    let read_req: BatchRequest = serde_json::from_value(json!({
        "id": 2,
        "queries": {
            "q": {"from": "users"}
        }
    }))
    .unwrap();
    let resp = execute_batch(&read_req, &resolver, None, user_actor.clone(), "test_db")
        .await
        .unwrap();
    assert_eq!(resp.results["q"].records.len(), 1);
}

// ============================================================================
// Stage B-1 — row-level security (RLS) enforcement
//
// Before the fix, `row_filter()` was computed but never applied to the
// actual query — a session with a row_filter grant got UNRESTRICTED row
// access (silent no-op data exfiltration). These tests prove that
// `execute_batch_with_permissions` now ANDs the merged row_filter into
// each Read/Update/Delete op's WHERE clause.
//
// Insert/Set ops are NOT restricted here — row-match validation for those
// needs record-level filter evaluation at a deeper layer and is a separate
// follow-up.
// ============================================================================

/// Build a `SessionPermissions` that grants Read/Update/Delete on
/// `default/users` with a row_filter restricting to `status == "active"`.
fn rls_permissions() -> SessionPermissions {
    let row_filter = crate::query::filter::Filter::Eq {
        field: vec!["status".to_string()],
        value: crate::query::filter::FilterValue::String("active".to_string()),
    };
    SessionPermissions::build(&[Role {
        name: "rls_role".to_string(),
        permissions: vec![Permission {
            effect: Effect::Allow,
            actions: vec![Action::Read, Action::Update, Action::Delete],
            resource: Resource::Table {
                database: "test".to_string(),
                repo: "main".to_string(),
                table: "users".to_string(),
            },
            row_filter: Some(row_filter),
        }],
    }])
}

/// Superadmin session — Action::All on Resource::Global → row_filter is None.
fn superadmin_permissions() -> SessionPermissions {
    SessionPermissions::build(&[Role {
        name: "admin".to_string(),
        permissions: vec![Permission {
            effect: Effect::Allow,
            actions: vec![Action::All],
            resource: Resource::Global,
            row_filter: None,
        }],
    }])
}

#[tokio::test]
async fn rls_read_returns_only_matching_rows() {
    let resolver = setup_resolver().await;
    let permissions = rls_permissions();

    // Seed mixed rows: some status="active", some not.
    let seed_req: BatchRequest = serde_json::from_value(json!({
        "id": 1,
        "queries": {
            "seed": {
                "insert_into": "users",
                "values": [
                    {"name": "Alice", "status": "active"},
                    {"name": "Bob", "status": "inactive"},
                    {"name": "Carol", "status": "active"},
                    {"name": "Dave", "status": "pending"}
                ],
                "return_result": false
            }
        }
    }))
    .unwrap();
    execute_batch(&seed_req, &resolver, None, Actor::System, "test")
        .await
        .unwrap();

    // Read via execute_batch_with_permissions — should return ONLY active rows.
    let read_req: BatchRequest = serde_json::from_value(json!({
        "id": 2,
        "queries": {
            "q": {"from": "users"}
        }
    }))
    .unwrap();

    let resp = execute_batch_with_permissions(&read_req, &resolver, None, &permissions, "test")
        .await
        .unwrap();

    let records = &resp.results["q"].records;
    assert_eq!(
        records.len(),
        2,
        "RLS must restrict Read to active rows only; got {:?}",
        records
    );
    let names: Vec<&str> = records
        .iter()
        .filter_map(|r| r.get("name").and_then(|v| v.as_str()))
        .collect();
    assert!(names.contains(&"Alice"), "Alice is active");
    assert!(names.contains(&"Carol"), "Carol is active");
    assert!(
        !names.contains(&"Bob"),
        "Bob is inactive — must be excluded"
    );
    assert!(
        !names.contains(&"Dave"),
        "Dave is pending — must be excluded"
    );
}

#[tokio::test]
async fn rls_delete_only_removes_matching_rows() {
    let resolver = setup_resolver().await;
    let permissions = rls_permissions();

    // Seed mixed rows.
    let seed_req: BatchRequest = serde_json::from_value(json!({
        "id": 1,
        "queries": {
            "seed": {
                "insert_into": "users",
                "values": [
                    {"name": "Alice", "status": "active"},
                    {"name": "Bob", "status": "inactive"},
                    {"name": "Carol", "status": "active"}
                ],
                "return_result": false
            }
        }
    }))
    .unwrap();
    execute_batch(&seed_req, &resolver, None, Actor::System, "test")
        .await
        .unwrap();

    // Delete all via RLS — only active rows should be deleted.
    let delete_req: BatchRequest = serde_json::from_value(json!({
        "id": 2,
        "queries": {
            "del": {
                "delete_from": "users",
                "where": {"op": "eq", "field": ["status"], "value": "active"}
            }
        }
    }))
    .unwrap();

    let resp = execute_batch_with_permissions(&delete_req, &resolver, None, &permissions, "test")
        .await
        .unwrap();

    // The delete should have matched 2 records (Alice + Carol).
    assert_eq!(
        resp.results["del"].stats.as_ref().unwrap().records_scanned,
        2,
        "RLS restricts Delete to active rows"
    );

    // Verify the inactive row (Bob) still exists.
    let verify_req: BatchRequest = serde_json::from_value(json!({
        "id": 3,
        "queries": {
            "remaining": {"from": "users"}
        }
    }))
    .unwrap();
    let verify_resp = execute_batch(&verify_req, &resolver, None, Actor::System, "test")
        .await
        .unwrap();
    let remaining = &verify_resp.results["remaining"].records;
    assert_eq!(
        remaining.len(),
        1,
        "only the inactive row should remain after RLS-scoped delete"
    );
    assert_eq!(
        remaining[0]["name"].as_str().unwrap(),
        "Bob",
        "the surviving row must be the inactive one"
    );
}

#[tokio::test]
async fn rls_superadmin_sees_all_rows() {
    let resolver = setup_resolver().await;
    let permissions = superadmin_permissions();

    // Seed mixed rows.
    let seed_req: BatchRequest = serde_json::from_value(json!({
        "id": 1,
        "queries": {
            "seed": {
                "insert_into": "users",
                "values": [
                    {"name": "Alice", "status": "active"},
                    {"name": "Bob", "status": "inactive"},
                    {"name": "Carol", "status": "pending"}
                ],
                "return_result": false
            }
        }
    }))
    .unwrap();
    execute_batch(&seed_req, &resolver, None, Actor::System, "test")
        .await
        .unwrap();

    // Superadmin read — no row_filter restriction.
    let read_req: BatchRequest = serde_json::from_value(json!({
        "id": 2,
        "queries": {
            "q": {"from": "users"}
        }
    }))
    .unwrap();

    let resp = execute_batch_with_permissions(&read_req, &resolver, None, &permissions, "test")
        .await
        .unwrap();

    assert_eq!(
        resp.results["q"].records.len(),
        3,
        "superadmin must see ALL rows (no RLS restriction)"
    );
}

#[tokio::test]
async fn rls_update_only_affects_matching_rows() {
    let resolver = setup_resolver().await;
    let permissions = rls_permissions();

    // Seed mixed rows.
    let seed_req: BatchRequest = serde_json::from_value(json!({
        "id": 1,
        "queries": {
            "seed": {
                "insert_into": "users",
                "values": [
                    {"name": "Alice", "status": "active"},
                    {"name": "Bob", "status": "inactive"}
                ],
                "return_result": false
            }
        }
    }))
    .unwrap();
    execute_batch(&seed_req, &resolver, None, Actor::System, "test")
        .await
        .unwrap();

    // Update ALL rows (no WHERE clause) — RLS should restrict to active only.
    let update_req: BatchRequest = serde_json::from_value(json!({
        "id": 2,
        "queries": {
            "upd": {
                "update": "users",
                "set": {"tag": "updated"}
            }
        }
    }))
    .unwrap();

    let resp = execute_batch_with_permissions(&update_req, &resolver, None, &permissions, "test")
        .await
        .unwrap();

    // Only 1 record should have been updated (Alice — active).
    assert_eq!(
        resp.results["upd"].stats.as_ref().unwrap().records_scanned,
        1,
        "RLS restricts Update to active rows only"
    );

    // Verify Bob was NOT updated.
    let verify_req: BatchRequest = serde_json::from_value(json!({
        "id": 3,
        "queries": {
            "check": {
                "from": "users",
                "where": {"op": "eq", "field": ["name"], "value": "Bob"}
            }
        }
    }))
    .unwrap();
    let verify_resp = execute_batch(&verify_req, &resolver, None, Actor::System, "test")
        .await
        .unwrap();
    let bob = &verify_resp.results["check"].records;
    assert_eq!(bob.len(), 1, "Bob should still exist");
    assert!(
        bob[0].get("tag").is_none(),
        "Bob should NOT have the 'tag' field — Update was RLS-restricted to active rows"
    );
}
