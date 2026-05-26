//! Batch planner micro-bench.
//!
//! Run: `cargo bench -p shamir-query-types --bench batch_planner`
//!
//! Measures `BatchPlanner::plan()` — dependency graph analysis +
//! topological sort. Pure CPU, no I/O.

use criterion::{black_box, criterion_group, criterion_main, BenchmarkId, Criterion};
use serde_json::json;

use shamir_query_types::batch::{BatchLimits, BatchPlanner, BatchRequest};

fn make_batch(n: usize, chain: bool) -> BatchRequest {
    let mut queries = serde_json::Map::new();
    for i in 0..n {
        let alias = format!("q{i}");
        if chain && i > 0 {
            queries.insert(
                alias,
                json!({
                    "from": "t",
                    "where": {
                        "op": "eq",
                        "field": ["id"],
                        "value": {"$query": format!("q{}", i - 1), "path": "[0].id"}
                    }
                }),
            );
        } else {
            queries.insert(alias, json!({"from": "t"}));
        }
    }
    serde_json::from_value(json!({
        "id": 1,
        "queries": queries,
    }))
    .unwrap()
}

fn bench_planner(c: &mut Criterion) {
    let limits = BatchLimits::default();

    let mut group = c.benchmark_group("batch_planner");
    for n in [5, 10, 20, 50] {
        group.bench_with_input(BenchmarkId::new("independent", n), &n, |b, &n| {
            let batch = make_batch(n, false);
            b.iter(|| black_box(BatchPlanner::plan(&batch.queries, &limits).unwrap()));
        });
        if n <= 20 {
            group.bench_with_input(BenchmarkId::new("chain", n), &n, |b, &n| {
                let batch = make_batch(n, true);
                b.iter(|| black_box(BatchPlanner::plan(&batch.queries, &limits).unwrap()));
            });
        }
    }
    group.finish();
}

criterion_group!(benches, bench_planner);
criterion_main!(benches);
