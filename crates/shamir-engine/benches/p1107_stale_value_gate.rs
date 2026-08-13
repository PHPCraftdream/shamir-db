//! Bench for #1107: Zero-cost gate for `rederive_stale_value_ops_post_stage`.
//!
//! Before fix (#1107): The function had a non-firing gate (`tx.base_index_stage_gens.is_empty()`)
//! that never skipped the per-row MVCC read + re-plan work on the common path (quiet repo).
//!
//! After fix: Added a repo-global staleness gate that skips the per-row work when
//! `gate.version_allocation_high_water_mark() <= tx.snapshot_version` (nothing has even
//! started committing since tx opened).
//!
//! This bench measures commit latency on a table with indexes when NO concurrent
//! tx is modifying it. The gate should short-circuit, avoiding per-row MVCC reads.
//!
//! Expected before fix: High latency (O(N) per-row MVCC reads + O(N²) dedup scans).
//! Expected after fix: Low latency (single atomic load gate, per-row work skipped entirely).
//!
//! IMPORTANT: The bench MUST update an INDEXED field to actually exercise the code path
//! being measured. The original version updated "score" (not indexed), so it never triggered
//! the O(N²) dedup loops this gate exists to bound — the benefit was unobservable by construction.
//! This fixed version updates "email" (indexed by idx_email) to actually exercise the path.
//!
//! Note: The gate is repo-global (conservative), so a concurrent commit on a DIFFERENT
//! table defeats the fast path. This bench measures the common case: a quiet repo.

use std::sync::Arc;

use bench_scale_tool::Harness;
use shamir_engine::query::batch::{
    execute_batch, BatchOp, BatchRequest, QueryEntry, ResultEncoding, TableResolver,
};
use shamir_engine::repo::{BoxRepo, RepoInstance};
use shamir_engine::table::{TableConfig, TableManager};
use shamir_query_types::write::InsertOp;
use shamir_query_types::TableRef;
use shamir_storage::error::DbResult;
use shamir_storage::storage_in_memory::InMemoryRepo;
use shamir_types::access::Actor;
use shamir_types::mpack;
use shamir_types::types::common::new_map;
use shamir_types::types::value::QueryValue;

struct Resolver {
    repo: RepoInstance,
}

#[async_trait::async_trait]
impl TableResolver for Resolver {
    async fn resolve(&self, table_ref: &TableRef) -> DbResult<TableManager> {
        self.repo.get_table(&table_ref.table).await
    }
    async fn resolve_repo(&self, _repo_name: &str) -> DbResult<RepoInstance> {
        Ok(self.repo.clone())
    }
}

fn main() {
    let mut h = Harness::new("p1107_stale_value_gate", env!("CARGO_MANIFEST_DIR"));

    // Benchmark: Transactional UPDATE N rows on a table WITH indexes.
    // This exercises `rederive_stale_value_ops_post_stage` because UPDATE
    // requires re-planning the removal ops for indexed fields.
    //
    // The common path: NO concurrent tx committed since this tx opened.
    // The gate should short-circuit, avoiding per-row MVCC reads.
    //
    // IMPORTANT: We UPDATE the "email" field (indexed by idx_email), NOT "score" (unindexed).
    // Updating an indexed field triggers the O(N²) dedup loops this gate exists to bound.
    // The original version updated "score", which never exercised this path by construction.
    //
    // Before fix (#1107): Per-row MVCC reads + O(N²) dedup scans on the indexed field.
    // After fix: Single atomic load gate, per-row work skipped entirely.

    for &n in &[400usize, 800usize, 1600usize] {
        h.bench_batched_async(
            &format!("tx_update_quiet_repo/n_{n}"),
            move || async move {
                // FRESH instance per iteration (no shared state across bench iterations)
                let repo = Arc::new(InMemoryRepo::new());
                let instance =
                    RepoInstance::new("bench".into(), BoxRepo::InMemory(repo), Vec::new());
                instance.add_table(TableConfig::new("users".to_string()));

                // Create table with regular and unique indexes
                let tbl = instance.get_table("users").await.unwrap();
                tbl.create_index("idx_email", &["email"]).await.unwrap();
                tbl.create_unique_index("uniq_id", &["id"]).await.unwrap();
                drop(tbl);

                let resolver = Resolver {
                    repo: instance.clone(),
                };
                let insert_values: Vec<QueryValue> = (0..n)
                    .map(|i| {
                        mpack!({
                            "id": @(QueryValue::from(i as i64)),
                            "email": @(QueryValue::from(format!("user_{i}@example.com"))),
                            "name": @(QueryValue::from(format!("name_{i}"))),
                            "score": @(QueryValue::from(i as i64)),
                        })
                    })
                    .collect();

                // First, insert N rows (setup, not timed)
                let mut insert_queries = new_map();
                insert_queries.insert(
                    "ins".to_string(),
                    QueryEntry {
                        op: BatchOp::Insert(InsertOp {
                            insert_into: TableRef::new("users"),
                            values: insert_values,
                            records_idmsgpack: Vec::new(),
                            select: None,
                        }),
                        return_result: false,
                        after: Vec::new(),
                        when: None,
                    },
                );
                let insert_request = BatchRequest {
                    id: QueryValue::Int(1),
                    name: None,
                    transactional: false, // non-tx insert for fast setup
                    isolation: None,
                    durability: None,
                    queries: insert_queries,
                    return_all: false,
                    return_only: None,
                    limits: Default::default(),
                    interner_epochs: Default::default(),
                    result_encoding: ResultEncoding::default(),
                };

                // Build the update request (timed)
                // CRITICAL: Update "email" (INDEXED by idx_email), NOT "score" (unindexed)
                // This actually exercises the O(N²) dedup loops this gate exists to bound
                let update_values = mpack!({
                    "email": @(QueryValue::from(format!("updated_{}@example.com", 999))),
                });

                let mut update_queries = new_map();
                update_queries.insert(
                    "upd".to_string(),
                    QueryEntry {
                        op: BatchOp::Update(shamir_query_types::write::UpdateOp {
                            update: TableRef::new("users"),
                            where_clause: Some(shamir_engine::query::filter::Filter::Gte {
                                field: vec!["id".to_string()],
                                value: shamir_engine::query::filter::FilterValue::Int(0),
                            }),
                            set: update_values,
                            select: None,
                            expected_version: None,
                        }),
                        return_result: false,
                        after: Vec::new(),
                        when: None,
                    },
                );
                let update_request = BatchRequest {
                    id: QueryValue::Int(2),
                    name: None,
                    transactional: true, // TRANSACTIONAL UPDATE (the measured path)
                    isolation: None,
                    durability: None,
                    queries: update_queries,
                    return_all: false,
                    return_only: None,
                    limits: Default::default(),
                    interner_epochs: Default::default(),
                    result_encoding: ResultEncoding::default(),
                };

                // Insert rows (setup, not timed)
                execute_batch(
                    &insert_request,
                    &resolver,
                    None,
                    None,
                    Actor::System,
                    "bench",
                )
                .await
                .unwrap();
                (resolver, update_request)
            },
            |(resolver, update_request)| async move {
                // Transactional UPDATE (timed)
                // This runs in a quiet repo: nothing committed between setup and here,
                // so `gate.last_committed() == tx.snapshot_version` should hold.
                execute_batch(
                    &update_request,
                    &resolver,
                    None,
                    None,
                    Actor::System,
                    "bench",
                )
                .await
                .unwrap();
            },
        );
    }

    h.run();
}
