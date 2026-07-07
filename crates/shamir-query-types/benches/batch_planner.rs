//! Batch planner micro-bench.
//!
//! Run: `cargo bench -p shamir-query-types --bench batch_planner`
//!
//! Measures `BatchPlanner::plan()` — dependency graph analysis +
//! topological sort. Pure CPU, no I/O.
//!
//! Migrated to the fixed-iteration harness (`bench_scale_tool`): each batch
//! is built ONCE outside the timed closure — plan 1 (shared setup).

use std::hint::black_box;

use bench_scale_tool::Harness;
use shamir_collections::TMap;
use shamir_types::types::value::QueryValue;

use shamir_query_types::batch::{BatchLimits, BatchOp, BatchPlanner, BatchRequest, QueryEntry};
use shamir_query_types::filter::{Filter, FilterValue};
use shamir_query_types::read::ReadQuery;

fn make_batch(n: usize, chain: bool) -> BatchRequest {
    let mut queries: TMap<String, QueryEntry> = TMap::default();
    for i in 0..n {
        let alias = format!("q{i}");
        let op = if chain && i > 0 {
            let prev = format!("q{}", i - 1);
            let mut rq = ReadQuery::new("t");
            rq.r#where = Some(Filter::Eq {
                field: vec!["id".to_string()],
                value: FilterValue::QueryRef {
                    alias: prev,
                    path: Some("[0].id".to_string()),
                },
            });
            BatchOp::Read(rq)
        } else {
            BatchOp::Read(ReadQuery::new("t"))
        };
        queries.insert(
            alias,
            QueryEntry {
                op,
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
        interner_epochs: TMap::default(),
        result_encoding: Default::default(),
    }
}

fn main() {
    let mut h = Harness::new("batch_planner", env!("CARGO_MANIFEST_DIR"));

    // `chain/N` and `independent/N` each plan a batch of N items (independent
    // reads, or a linear dependency chain of depth N-1) in ONE planner call.
    // This is a genuine bulk-operation-size comparison — the calibrated
    // per-call cost stays well under ~10ms even at N=50 (~0.10ms at the
    // 0.05s calibration budget) — so no tier is too expensive to keep. We
    // still gate the larger tiers behind an opt-in env var so the default run
    // is a fast single-tier sweep, while `BENCH_BATCH_PLANNER_SCALING=1`
    // restores the full ladder for scaling analysis.
    let wide = std::env::var("BENCH_BATCH_PLANNER_SCALING")
        .map(|v| matches!(v.as_str(), "1" | "true" | "yes" | "on"))
        .unwrap_or(false);
    let sizes: &[usize] = if wide { &[5, 10, 20, 50] } else { &[5] };
    // Chain depth is capped at 20 (depth 19), which exceeds the default
    // `max_dependency_depth` DoS guard (10 — see batch_limits.rs). This bench
    // measures deep-chain planning cost specifically, so raise the ceiling for
    // planning instead of shrinking the tested chain length.
    let chain_limits = BatchLimits {
        max_dependency_depth: 25,
        ..BatchLimits::default()
    };

    for &n in sizes {
        let batch = make_batch(n, false);
        let limits = BatchLimits::default();
        let id = format!("batch_planner/independent/{n}");
        h.bench(&id, move || {
            black_box(BatchPlanner::plan(&batch.queries, &limits).unwrap());
        });

        if n <= 20 {
            let batch = make_batch(n, true);
            let chain_limits = chain_limits.clone();
            let id = format!("batch_planner/chain/{n}");
            h.bench(&id, move || {
                black_box(BatchPlanner::plan(&batch.queries, &chain_limits).unwrap());
            });
        }
    }

    h.run();
}
