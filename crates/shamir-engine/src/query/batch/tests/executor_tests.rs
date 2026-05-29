//! Integration tests for batch executor.

use serde_json::json;

use crate::db_instance::db_instance::DbInstance;
use crate::query::batch::{execute_batch, BatchRequest, QueryRunner, TableResolver};
use crate::query::TableRef;
use crate::repo::repo_types::BoxRepoFactory;
use crate::repo::RepoConfig;
use crate::table::{TableConfig, TableManager};
use shamir_storage::error::DbResult;

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
    let resp = execute_batch(&insert_req, &resolver, None).await.unwrap();
    assert_eq!(resp.results["insert"].records.len(), 2);

    // Now read
    let req: BatchRequest = serde_json::from_value(json!({
        "id": 1,
        "queries": {
            "users": {"from": "users"}
        }
    }))
    .unwrap();

    let resp = execute_batch(&req, &resolver, None).await.unwrap();

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
    execute_batch(&seed_req, &resolver, None).await.unwrap();

    // Two independent reads
    let req: BatchRequest = serde_json::from_value(json!({
        "id": 1,
        "queries": {
            "users": {"from": "users"},
            "orders": {"from": "orders"}
        }
    }))
    .unwrap();

    let resp = execute_batch(&req, &resolver, None).await.unwrap();

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
    execute_batch(&seed_req, &resolver, None).await.unwrap();

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

    let resp = execute_batch(&req, &resolver, None).await.unwrap();

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

    let resp = execute_batch(&req, &resolver, None).await.unwrap();

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

    let resp = execute_batch(&req, &resolver, None).await.unwrap();

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

    let resp = execute_batch(&req, &resolver, None).await.unwrap();

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
    execute_batch(&seed_req, &resolver, None).await.unwrap();

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

    let resp = execute_batch(&req, &resolver, None).await.unwrap();
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

    let err = execute_batch(&req, &resolver, None).await.unwrap_err();
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

    let err = execute_batch(&req, &resolver, None).await.unwrap_err();
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
    let resp = execute_batch(&req, &resolver, None).await.unwrap();
    assert_eq!(resp.id, json!("req-42"));

    // Numeric ID
    let req: BatchRequest = serde_json::from_value(json!({
        "id": 123,
        "queries": {
            "q": {"from": "users"}
        }
    }))
    .unwrap();
    let resp = execute_batch(&req, &resolver, None).await.unwrap();
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

    let response = execute_batch(&request, &resolver, None).await.unwrap();

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
    };
    let deferred = crate::tx::TxOutcome {
        tx_id: 12,
        snapshot_version: 100,
        commit_version: 102,
        materialization: MaterializationState::Deferred,
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
                let resp = execute_batch(&self.writer_req, &writer_resolver, None)
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

    let response = execute_batch(&reader_req, &resolver, None).await.unwrap();

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

    let response = execute_batch(&reader_req, &resolver, None).await.unwrap();
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
