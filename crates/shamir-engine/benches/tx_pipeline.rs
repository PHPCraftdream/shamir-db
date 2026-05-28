//! Stage 4.D.6 pipeline benchmarks.
//!
//! Measures D5 obligation: tx overhead vs non-tx baseline, plus
//! commit_tx phase timing.

use std::sync::Arc;

use criterion::{criterion_group, criterion_main, Criterion, Throughput};
use shamir_engine::query::batch::{
    execute_batch, BatchOp, BatchRequest, QueryEntry, TableResolver,
};
use shamir_engine::repo::{BoxRepo, RepoInstance};
use shamir_engine::table::{TableConfig, TableManager};
use shamir_query_types::write::InsertOp;
use shamir_query_types::TableRef;
use shamir_storage::error::DbResult;
use shamir_storage::storage_in_memory::InMemoryRepo;
use shamir_types::types::common::new_map;
use shamir_types::types::value::InnerValue;

fn rt() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
}

fn make_repo() -> RepoInstance {
    let repo = Arc::new(InMemoryRepo::new());
    let instance = RepoInstance::new("bench".into(), BoxRepo::InMemory(repo), Vec::new());
    instance.add_table(TableConfig::new("bench_table".to_string()));
    instance
}

fn bench_insert_tx_vs_non_tx(c: &mut Criterion) {
    let mut group = c.benchmark_group("tx_overhead/single_insert");
    let rt = rt();
    let repo = make_repo();
    let tbl = rt.block_on(repo.get_table("bench_table")).unwrap();

    group.throughput(Throughput::Elements(1));

    group.bench_function("non_tx", |b| {
        b.to_async(&rt).iter(|| {
            let tbl = tbl.clone();
            async move {
                tbl.insert(&InnerValue::Str("v".into())).await.unwrap();
            }
        });
    });

    group.bench_function("tx_staged", |b| {
        b.to_async(&rt).iter(|| {
            let repo = repo.clone();
            let tbl = tbl.clone();
            async move {
                let (mut tx, _g) = repo
                    .begin_tx(shamir_tx::IsolationLevel::Snapshot)
                    .await
                    .unwrap();
                let _ = tbl
                    .insert_tx(&InnerValue::Str("v".into()), Some(&mut tx))
                    .await
                    .unwrap();
                let _ = repo.commit_tx(tx).await.unwrap();
            }
        });
    });

    group.finish();
}

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

fn bench_batch_insert_pipeline(c: &mut Criterion) {
    let mut group = c.benchmark_group("tx_overhead/batch_pipeline");
    let rt = rt();
    let repo = make_repo();
    let resolver = Resolver { repo: repo.clone() };

    for &n in &[1usize, 10, 100] {
        group.throughput(Throughput::Elements(n as u64));

        for transactional in [false, true] {
            let label = if transactional { "tx" } else { "non_tx" };
            group.bench_function(format!("{}/{}", label, n), |b| {
                b.to_async(&rt).iter(|| {
                    let resolver = &resolver;
                    async move {
                        let mut queries = new_map();
                        queries.insert(
                            "ins".to_string(),
                            QueryEntry {
                                op: BatchOp::Insert(InsertOp {
                                    insert_into: TableRef::new("bench_table"),
                                    values: (0..n).map(|i| serde_json::json!({"i": i})).collect(),
                                }),
                                return_result: true,
                            },
                        );
                        let request = BatchRequest {
                            id: serde_json::json!(1),
                            name: None,
                            transactional,
                            isolation: None,
                            queries,
                            return_all: false,
                            return_only: None,
                            limits: Default::default(),
                        };
                        let _ = execute_batch(&request, resolver, None).await.unwrap();
                    }
                });
            });
        }
    }
    group.finish();
}

criterion_group!(
    benches,
    bench_insert_tx_vs_non_tx,
    bench_batch_insert_pipeline
);
criterion_main!(benches);
