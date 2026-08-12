//! Bench for #1099: O(N²) scaling in transactional UPDATEs with unique indexes.
//!
//! Measures the cost of `update_tx_bytes` when called PER ROW on tables WITH
//! a unique index. Before the fix, the O(N²) shape came from
//! `touched_records_in_tx` walking `tx.write_set[table]` from scratch on
//! each row; that function has since been deleted and replaced with an
//! on-demand `is_record_touched` probe (an O(1) check called only when a
//! durable conflict is actually found).
//!
//! Scaling is near-linear (4× for 4× rows, not ~8-12×) for this bench's
//! specific workload (the UPDATE leaves the unique column unchanged, so
//! `released_unique_keys_in_tx` — deliberately left O(N)-per-call, out of
//! this task's scope — stays O(1) throughout). A workload that DOES
//! release-and-reclaim a unique key on every row would still show O(N²)
//! from that half.

use std::sync::Arc;

use bench_scale_tool::Harness;
use shamir_engine::query::batch::{
    execute_batch, BatchOp, BatchRequest, QueryEntry, ResultEncoding, TableResolver,
};
use shamir_engine::query::filter::{Filter, FilterValue};
use shamir_engine::repo::{BoxRepo, RepoInstance};
use shamir_engine::table::{TableConfig, TableManager};
use shamir_query_types::write::{InsertOp, UpdateOp};
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
    let mut h = Harness::new("p1099_touched_probe", env!("CARGO_MANIFEST_DIR"));

    // Benchmark: Transactional UPDATE N rows on a table WITH a unique index.
    //
    // Before fix: O(N²) scaling because `update_tx_bytes` calls
    // `touched_records_in_tx` (O(N) set walk) on every row.
    // After fix: Near-linear scaling because we use O(1) on-demand probe.
    //
    // Expected before fix: N=400→1600, ~12× (68ms→827ms measured previously).
    // Expected after fix: N=400→1600, ~4× (near-linear).

    for &n in &[400usize, 800usize, 1600usize] {
        h.bench_batched_async(
            &format!("tx_update_unique_index/n_{n}"),
            move || async move {
                // FRESH instance per iteration (no shared state across bench iterations)
                let repo = Arc::new(InMemoryRepo::new());
                let instance =
                    RepoInstance::new("bench".into(), BoxRepo::InMemory(repo), Vec::new());
                instance.add_table(TableConfig::new("users".to_string()));

                // Create table with unique index on 'email' field
                let tbl = instance.get_table("users").await.unwrap();
                tbl.create_unique_index("uniq_email", &["email"])
                    .await
                    .unwrap();
                drop(tbl);

                let resolver = Resolver {
                    repo: instance.clone(),
                };
                let insert_values: Vec<QueryValue> = (0..n)
                    .map(|i| {
                        mpack!({
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
                let update_values = mpack!({
                    "score": @(QueryValue::from(999)),
                });

                let mut update_queries = new_map();
                update_queries.insert(
                    "upd".to_string(),
                    QueryEntry {
                        op: BatchOp::Update(UpdateOp {
                            update: TableRef::new("users"),
                            where_clause: Some(Filter::Gt {
                                field: vec!["score".to_string()],
                                value: FilterValue::Int(0),
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
