//! Stage 4.D.6 / 4.H pipeline benchmarks.
//!
//! Measures:
//! - `bench_insert_tx_vs_non_tx` — single-record tx vs non-tx (D5).
//! - `bench_batch_insert_pipeline` — N-record execute_batch tx vs non-tx (D5).
//! - `bench_commit_tx_phase_breakdown` — scenario-isolated phase
//!   costs: baseline empty, Phase 2 SSI scaling, Phase 5 write
//!   scaling across 1 vs N tables.
//! - `bench_provider_overhead` — stub vs real VersionProvider for
//!   SSI read-set validation; delta = MvccStore lookup overhead.

use std::sync::Arc;

use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion, Throughput};
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

fn bench_commit_tx_phase_breakdown(c: &mut Criterion) {
    let mut group = c.benchmark_group("commit_tx/phases");
    let rt = rt();
    let repo = make_repo();
    rt.block_on(repo.get_table("bench_table")).unwrap();

    // Baseline: empty Tx (Phase 3 + 4 + 6 + 7 fixed overhead).
    group.bench_function("baseline_empty", |b| {
        b.to_async(&rt).iter(|| {
            let repo = repo.clone();
            async move {
                let (tx, _g) = repo
                    .begin_tx(shamir_tx::IsolationLevel::Snapshot)
                    .await
                    .unwrap();
                let _ = repo.commit_tx(tx).await.unwrap();
            }
        });
    });

    // Phase 2 scaling: Serializable with N read_set entries.
    // No-conflict provider → walks all entries successfully.
    for &n in &[10usize, 100, 1000] {
        group.throughput(Throughput::Elements(n as u64));
        group.bench_function(BenchmarkId::new("ssi_validate", n), |b| {
            b.to_async(&rt).iter(|| {
                let repo = repo.clone();
                async move {
                    let (mut tx, _g) = repo
                        .begin_tx(shamir_tx::IsolationLevel::Serializable)
                        .await
                        .unwrap();
                    let table_id = shamir_engine::table::table_token_for("bench_table");
                    for i in 0..n {
                        tx.record_read(table_id, bytes::Bytes::from(format!("k{i}")), 0);
                    }
                    let _ = repo.commit_tx(tx).await.unwrap();
                }
            });
        });
    }

    // Phase 5 scaling: write N keys into 1 table vs 5 tables.
    for table_count in [1usize, 5] {
        for i in 0..table_count {
            let name = format!("phase5_tbl_{i}");
            if !repo.has_table(&name) {
                repo.add_table(TableConfig::new(name.clone()));
                rt.block_on(repo.get_table(&name)).unwrap();
            }
        }

        let n = 100usize;
        group.throughput(Throughput::Elements((n * table_count) as u64));
        group.bench_function(
            BenchmarkId::new("write_100_keys", format!("{table_count}_tables")),
            |b| {
                b.to_async(&rt).iter(|| {
                    let repo = repo.clone();
                    async move {
                        let (mut tx, _g) = repo
                            .begin_tx(shamir_tx::IsolationLevel::Snapshot)
                            .await
                            .unwrap();
                        for i in 0..table_count {
                            let tbl = repo.get_table(&format!("phase5_tbl_{i}")).await.unwrap();
                            for _ in 0..n {
                                let _ = tbl
                                    .insert_tx(&InnerValue::Str("v".into()), Some(&mut tx))
                                    .await
                                    .unwrap();
                            }
                        }
                        let _ = repo.commit_tx(tx).await.unwrap();
                    }
                });
            },
        );
    }

    group.finish();
}

fn bench_provider_overhead(c: &mut Criterion) {
    use shamir_tx::VersionProvider;

    let mut group = c.benchmark_group("commit_tx/provider_overhead");
    let rt = rt();

    let real_repo = make_repo();
    rt.block_on(real_repo.get_table("bench_table")).unwrap();
    let stub_repo = make_repo();
    rt.block_on(stub_repo.get_table("bench_table")).unwrap();

    /// Always Some(0) — minimum-cost mock.
    struct StubAlwaysZero;
    impl VersionProvider for StubAlwaysZero {
        fn version_of(&self, _: u64, _: &bytes::Bytes) -> Option<u64> {
            Some(0)
        }
    }

    for &n in &[100usize, 1000] {
        group.throughput(Throughput::Elements(n as u64));

        group.bench_function(BenchmarkId::new("stub_provider", n), |b| {
            b.to_async(&rt).iter(|| {
                let repo = stub_repo.clone();
                async move {
                    let (mut tx, _g) = repo
                        .begin_tx(shamir_tx::IsolationLevel::Serializable)
                        .await
                        .unwrap();
                    tx.set_version_provider(Arc::new(StubAlwaysZero));
                    let table_id = shamir_engine::table::table_token_for("bench_table");
                    for i in 0..n {
                        tx.record_read(table_id, bytes::Bytes::from(format!("k{i}")), 0);
                    }
                    let _ = repo.commit_tx(tx).await.unwrap();
                }
            });
        });

        group.bench_function(BenchmarkId::new("real_provider", n), |b| {
            b.to_async(&rt).iter(|| {
                let repo = real_repo.clone();
                async move {
                    let (mut tx, _g) = repo
                        .begin_tx(shamir_tx::IsolationLevel::Serializable)
                        .await
                        .unwrap();
                    let table_id = shamir_engine::table::table_token_for("bench_table");
                    for i in 0..n {
                        tx.record_read(table_id, bytes::Bytes::from(format!("k{i}")), 0);
                    }
                    let _ = repo.commit_tx(tx).await.unwrap();
                }
            });
        });
    }

    group.finish();
}

criterion_group!(
    benches,
    bench_insert_tx_vs_non_tx,
    bench_batch_insert_pipeline,
    bench_commit_tx_phase_breakdown,
    bench_provider_overhead
);
criterion_main!(benches);
