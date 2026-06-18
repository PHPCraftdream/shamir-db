//! Batch planner micro-bench.
//!
//! Run: `cargo bench -p shamir-query-types --bench batch_planner`
//!
//! Measures `BatchPlanner::plan()` — dependency graph analysis +
//! topological sort. Pure CPU, no I/O.

use criterion::{black_box, criterion_group, criterion_main, BenchmarkId, Criterion};
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
