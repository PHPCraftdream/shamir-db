//! Epic01/E (task #632) — batch stage sequencing benchmarks.
//!
//! Measures the per-op overhead of executing a single batch **stage** of N
//! mutually independent `Read` ops through the real `execute_batch` /
//! `execute_plan_impl` path (`crates/shamir-engine/src/query/batch/batch_execute.rs`).
//! The ops share no `after`/`$query` edges by construction — the planner puts
//! all N reads into ONE stage — so this isolates the sequential-loop
//! overhead `execute_plan_impl` pays per op within a stage, without any
//! cross-stage dependency-resolution cost.
//!
//! Context: Phase A (`docs/dev-artifacts/design/oql-01-stage-parallelism-adr.md`)
//! deferred a `tokio::spawn`-per-query concurrent-stage executor pending
//! evidence from this benchmark phase. The ADR's working hypothesis is that
//! for an in-memory, CPU-bound `Read` workload there are no `.await`
//! suspension points inside a single query's execution path, so a
//! same-task `try_join_all` (previously tried) is a no-op, and real
//! parallelism would need `tokio::spawn`-per-query — a real design cost.
//! This bench's job is to put numbers behind that hypothesis: if
//! per-stage cost scales ~linearly with N and stays in the
//! microseconds-per-op range, sequential execution is not the bottleneck
//! and spawning tasks per query would only add scheduling overhead.
//!
//! `batch_stage/reads_10` and `batch_stage/reads_50` — one stage, N
//! independent `Read` ops against a small pre-populated in-memory table.
//!
//! `batch_plan/plan_50_reads` — planning-only cost (topological sort +
//! edge-provenance construction) for a 50-op batch with no `after` edges.
//! Recorded as an absolute baseline (no prior bench existed for planning
//! cost) so a future edge-provenance/DAG change can be compared against
//! this number.

use std::sync::Arc;

use bench_scale_tool::Harness;
use shamir_engine::query::batch::{
    execute_batch, BatchLimits, BatchOp, BatchPlanner, BatchRequest, QueryEntry, ResultEncoding,
    TableResolver,
};
use shamir_engine::query::read::ReadQuery;
use shamir_engine::repo::{BoxRepo, RepoInstance};
use shamir_engine::table::{TableConfig, TableManager};
use shamir_query_builder::query::Query;
use shamir_query_types::TableRef;
use shamir_storage::error::DbResult;
use shamir_storage::storage_in_memory::InMemoryRepo;
use shamir_types::access::Actor;
use shamir_types::types::common::{new_map, new_map_wc};
use shamir_types::types::value::{InnerValue, QueryValue};

/// A single-table resolver, same shape as the one in `tx_pipeline.rs`.
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

/// Build an in-memory repo/table with a handful of rows so each `Read` op
/// has something to scan (empty-table reads would hide realistic
/// per-record projection cost).
async fn make_repo_with_rows(n_rows: usize) -> RepoInstance {
    let repo = Arc::new(InMemoryRepo::new());
    let instance = RepoInstance::new(
        "bench".into(),
        BoxRepo::InMemory(repo),
        vec![TableConfig::new("rows".to_string())],
    );
    let table = instance.get_table("rows").await.unwrap();
    let interner = table.interner().get().await.unwrap();
    let touch = |s: &str| match interner.touch_ind(s).unwrap() {
        shamir_types::core::interner::TouchInd::Exists(k)
        | shamir_types::core::interner::TouchInd::New(k) => k,
    };
    let k_id = touch("id");
    let k_city = touch("city");
    for i in 0..n_rows {
        let mut m = new_map_wc(2);
        m.insert(k_id.clone(), InnerValue::Int(i as i64));
        m.insert(
            k_city.clone(),
            InnerValue::Str(if i % 2 == 0 { "Jerusalem" } else { "Tzfat" }.to_string()),
        );
        table.insert(&InnerValue::Map(m)).await.unwrap();
    }
    instance
}

/// Build a batch of `n` mutually independent `Read` ops (no `after` edges,
/// no `$query` references) — the planner places all of them into a single
/// stage.
fn build_independent_read_batch(n: usize) -> BatchRequest {
    let mut queries = new_map();
    for i in 0..n {
        let read_q: ReadQuery = Query::from("rows").where_eq("city", "Jerusalem").into();
        queries.insert(
            format!("r{i}"),
            QueryEntry {
                op: BatchOp::Read(read_q),
                return_result: true,
                after: Vec::new(),
            },
        );
    }
    BatchRequest {
        id: QueryValue::Int(1),
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

fn main() {
    let mut h = Harness::new("batch_stage_parallelism", env!("CARGO_MANIFEST_DIR"));

    // ── batch_stage — one stage, N independent Read ops ─────────────────
    for &n in &[10usize, 50usize] {
        let repo = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(make_repo_with_rows(200));

        h.bench_batched_async(
            &format!("batch_stage/reads_{n}"),
            move || {
                let resolver = Resolver { repo: repo.clone() };
                let request = build_independent_read_batch(n);
                async move { (resolver, request) }
            },
            move |(resolver, request)| async move {
                let resp = execute_batch(&request, &resolver, None, None, Actor::System, "bench")
                    .await
                    .unwrap();
                std::hint::black_box(resp);
            },
        );
    }

    // ── batch_plan — planning-only cost for a 50-op independent batch ───
    // No prior bench of `BatchPlanner::plan` existed; this is an absolute
    // baseline for future edge-provenance/DAG-construction changes.
    {
        let request = build_independent_read_batch(50);
        h.bench("batch_plan/plan_50_reads", move || {
            let plan = BatchPlanner::plan(&request.queries, &request.limits).unwrap();
            std::hint::black_box(plan);
        });
    }

    h.run();
}
