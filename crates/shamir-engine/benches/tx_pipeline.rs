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
//! - `bench_commit_phase5c_indexed_fjall` — tx commit Phase 5c writing
//!   N postings to a sled-backed indexed table; exposes the
//!   batched-vs-per-key info_store apply cost.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion, Throughput};
use shamir_bench_utils as bu;
use shamir_engine::query::batch::{
    execute_batch, BatchOp, BatchRequest, QueryEntry, ResultEncoding, TableResolver,
};
use shamir_engine::repo::{BoxRepo, BoxRepoFactory, RepoInstance};
use shamir_engine::table::{TableConfig, TableManager};
use shamir_query_builder::query::Query;
use shamir_query_builder::write::doc;
use shamir_query_types::write::InsertOp;
use shamir_query_types::TableRef;
use shamir_storage::error::DbResult;
use shamir_storage::storage_in_memory::InMemoryRepo;
use shamir_types::access::Actor;
use shamir_types::mpack;
use shamir_types::types::common::new_map;
use shamir_types::types::value::{InnerValue, QueryValue};

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

    bu::tune_tiered(&mut group, 100, 5, 3, 90);
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
    bu::tune_tiered(&mut group, 100, 5, 3, 90);

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
                                    values: (0..n)
                                        .map(|i| mpack!({"i": @(QueryValue::from(i as i64))}))
                                        .collect(),
                                    records_idmsgpack: Vec::new(),
                                    select: None,
                                }),
                                return_result: true,
                                after: Vec::new(),
                            },
                        );
                        let request = BatchRequest {
                            id: QueryValue::Int(1),
                            name: None,
                            transactional,
                            isolation: None,
                            durability: None,
                            queries,
                            return_all: false,
                            return_only: None,
                            limits: Default::default(),
                            interner_epochs: Default::default(),
                            result_encoding: ResultEncoding::default(),
                        };
                        let _ =
                            execute_batch(&request, resolver, None, None, Actor::System, "bench")
                                .await
                                .unwrap();
                    }
                });
            });
        }
    }

    // fire-and-forget variant — same as above but return_result=false,
    // exercising the result_build skip fast-path added in the P8 cycle.
    for &n in &[1usize, 10, 100] {
        group.throughput(Throughput::Elements(n as u64));

        for transactional in [false, true] {
            let label = if transactional { "tx" } else { "non_tx" };
            group.bench_function(format!("{}/{}_no_result", label, n), |b| {
                b.to_async(&rt).iter(|| {
                    let resolver = &resolver;
                    async move {
                        let mut queries = new_map();
                        queries.insert(
                            "ins".to_string(),
                            QueryEntry {
                                op: BatchOp::Insert(InsertOp {
                                    insert_into: TableRef::new("bench_table"),
                                    values: (0..n)
                                        .map(|i| mpack!({"i": @(QueryValue::from(i as i64))}))
                                        .collect(),
                                    records_idmsgpack: Vec::new(),
                                    select: None,
                                }),
                                return_result: false,
                                after: Vec::new(),
                            },
                        );
                        let request = BatchRequest {
                            id: QueryValue::Int(1),
                            name: None,
                            transactional,
                            isolation: None,
                            durability: None,
                            queries,
                            return_all: false,
                            return_only: None,
                            limits: Default::default(),
                            interner_epochs: Default::default(),
                            result_encoding: ResultEncoding::default(),
                        };
                        let _ =
                            execute_batch(&request, resolver, None, None, Actor::System, "bench")
                                .await
                                .unwrap();
                    }
                });
            });
        }
    }

    // Indexed variant — exercises the per-row vs batched cost on
    // the heavier write path: 1 unique index (`uniq_email`) + 1
    // regular index (`by_city`).
    //
    // DDL (repo creation + index registration) is hoisted OUTSIDE the
    // timed section. Each Criterion iteration inserts into the SAME table
    // using disjoint keys (prefixed by `iter_counter`) so unique-index
    // constraint violations cannot occur across samples, and DDL cost is
    // amortised over the whole bench run rather than paid per sample.
    for &n in &[100usize, 1000] {
        group.throughput(Throughput::Elements(n as u64));

        for transactional in [false, true] {
            let label = if transactional { "tx" } else { "non_tx" };

            // --- Setup once, outside the timed loop ---
            let indexed_repo = make_repo();
            let tbl = rt.block_on(indexed_repo.get_table("bench_table")).unwrap();
            rt.block_on(tbl.create_unique_index("uniq_email", &["email"]))
                .unwrap();
            rt.block_on(tbl.create_index("by_city", &["city"])).unwrap();
            drop(tbl);
            let indexed_resolver = Resolver {
                repo: indexed_repo.clone(),
            };

            // Shared counter that survives across Criterion's warmup +
            // measurement calls to the bench closure — guarantees disjoint
            // email keys on EVERY iteration, including warmup.
            let iter_counter = Arc::new(AtomicU64::new(0));

            group.bench_function(format!("indexed/{}/{}", label, n), |b| {
                let iter_counter = Arc::clone(&iter_counter);
                b.iter_custom(|iters| {
                    let mut total = Duration::ZERO;
                    for _ in 0..iters {
                        let ic = iter_counter.fetch_add(1, Ordering::Relaxed);
                        let values: Vec<shamir_types::types::value::QueryValue> = (0..n)
                            .map(|i| {
                                mpack!({
                                    // Disjoint across iterations via iter_counter prefix.
                                    "email": @(QueryValue::from(format!("user_{ic}_{i}@example.com"))),
                                    "city": @(QueryValue::from(format!("c_{}", i % 8))),
                                    "score": @(QueryValue::from(i as i64)),
                                })
                            })
                            .collect();
                        let mut queries = new_map();
                        queries.insert(
                            "ins".to_string(),
                            QueryEntry {
                                op: BatchOp::Insert(InsertOp {
                                    insert_into: TableRef::new("bench_table"),
                                    values,
                                    records_idmsgpack: Vec::new(),
                                    select: None,
                                }),
                                return_result: false,
                                after: Vec::new(),
                            },
                        );
                        let request = BatchRequest {
                            id: QueryValue::Int(1),
                            name: None,
                            transactional,
                            isolation: None,
                            durability: None,
                            queries,
                            return_all: false,
                            return_only: None,
                            limits: Default::default(),
                            interner_epochs: Default::default(),
                            result_encoding: ResultEncoding::default(),
                        };

                        let start = Instant::now();
                        rt.block_on(execute_batch(
                            &request,
                            &indexed_resolver,
                            None,
                            None,
                            Actor::System,
                            "bench",
                        ))
                        .unwrap();
                        total += start.elapsed();
                    }
                    total
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
    bu::tune_tiered(&mut group, 100, 5, 3, 90);

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
    bu::tune_tiered(&mut group, 100, 5, 3, 90);

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

/// Phase 5c (`apply_index_ops_at_commit`) on a fjall-backed repo with a
/// table that has a non-unique regular index on `city` — exposes the
/// per-key vs batched info_store write cost. Each iter:
///   * provisions a fresh tempdir + fjall-backed RepoInstance,
///   * creates a `by_city` index on `city`,
///   * runs ONE transactional `BatchRequest` that inserts `n` rows.
///
/// At commit the tx pipeline drains `index_write_set` through
/// `apply_index_ops_at_commit` → `info_store.transact(...)`. On the
/// unbatched code path each `SetPosting` was one `Store::set`;
/// after batching it is one `Store::transact` → one fsync.
fn bench_commit_phase5c_indexed_fjall(c: &mut Criterion) {
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .unwrap();
    let mut group = c.benchmark_group("commit_tx/phase5c_indexed_fjall");
    bu::tune_tiered(&mut group, 10, 15, 3, 60);

    for &n in &[100usize, 1000usize] {
        group.throughput(Throughput::Elements(n as u64));
        group.bench_with_input(BenchmarkId::from_parameter(n), &n, |b, &n| {
            b.to_async(&rt).iter_custom(|iters| async move {
                let mut total = Duration::ZERO;
                for _ in 0..iters {
                    let tempdir = tempfile::TempDir::new().expect("tempdir");
                    // Raw fjall — no MemBuffer wrapper, so every commit
                    // sees real per-write cost.
                    let factory = BoxRepoFactory::fjall_raw(tempdir.path().to_path_buf());
                    let repo = RepoInstance::from_factory(
                        "bench".into(),
                        factory,
                        vec![TableConfig::new("indexed".to_string())],
                    )
                    .await
                    .unwrap();
                    let tbl = repo.get_table("indexed").await.unwrap();
                    tbl.create_index("by_city", &["city"]).await.unwrap();
                    drop(tbl);

                    let resolver = Resolver { repo: repo.clone() };
                    let mut queries = new_map();
                    let values: Vec<shamir_types::types::value::QueryValue> = (0..n)
                        .map(|i| {
                            mpack!({
                                "city": @(QueryValue::from(format!("c_{}", i % 8))),
                                "score": @(QueryValue::from(i as i64)),
                            })
                        })
                        .collect();
                    queries.insert(
                        "ins".to_string(),
                        QueryEntry {
                            op: BatchOp::Insert(InsertOp {
                                insert_into: TableRef::new("indexed"),
                                values,
                                records_idmsgpack: Vec::new(),
                                select: None,
                            }),
                            return_result: false,
                            after: Vec::new(),
                        },
                    );
                    let request = BatchRequest {
                        id: QueryValue::Int(1),
                        name: None,
                        transactional: true,
                        isolation: None,
                        durability: None,
                        queries,
                        return_all: false,
                        return_only: None,
                        limits: Default::default(),
                        interner_epochs: Default::default(),
                        result_encoding: ResultEncoding::default(),
                    };

                    let start = Instant::now();
                    let _ = execute_batch(&request, &resolver, None, None, Actor::System, "bench")
                        .await
                        .unwrap();
                    total += start.elapsed();

                    drop(repo);
                    drop(tempdir);
                }
                total
            });
        });
    }
    group.finish();
}

/// Client-visible commit latency on an INDEX-HEAVY tx, sync vs async-index
/// visibility. Async ON should return BEFORE Phase 5c (index posting writes
/// to `info_store`) lands — for N=100/1000 postings this is the dominant
/// cost on a sled-backed table, so the win is a multiple. Measured purely as
/// the time `commit_tx().await` takes — the background tail is NOT awaited
/// inside the timed window. Each iter creates a fresh repo and runs ONE
/// commit, so a previous iter's background tail can't contend with the next.
/// Two backends are measured: in-memory (CPU-bound 5c) and sled (sled
/// transact is heavier — exposes the largest sync→async delta).
fn bench_async_commit_index_heavy(c: &mut Criterion) {
    use shamir_engine::tx::commit_tx;
    use shamir_tx::{
        CommitVisibility, IndexWriteOp, IsolationLevel, StagingStore, TxContext, TxId,
    };
    use shamir_types::types::record_id::RecordId;

    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .unwrap();

    let mut group = c.benchmark_group("commit_tx/async_visibility_index_heavy");
    bu::tune_tiered(&mut group, 20, 8, 3, 60);

    for &n in &[100usize, 1000usize] {
        group.throughput(Throughput::Elements(n as u64));

        // In-memory backend — Phase 5c is per-key + (cheap) HashMap writes.
        for visibility in [CommitVisibility::Synchronous, CommitVisibility::AsyncIndex] {
            let label = match visibility {
                CommitVisibility::Synchronous => "sync",
                CommitVisibility::AsyncIndex => "async",
            };
            group.bench_with_input(
                BenchmarkId::new(format!("inmem_{}", label), n),
                &n,
                |b, &n| {
                    b.to_async(&rt).iter_custom(|iters| async move {
                        let mut total = Duration::ZERO;
                        for _ in 0..iters {
                            let repo = make_repo();
                            let tbl = repo.get_table("bench_table").await.unwrap();
                            let token = shamir_engine::table::table_token_for("bench_table");

                            let mut staging = StagingStore::new(Arc::clone(tbl.data_store()));
                            for i in 0..n {
                                let rid = RecordId::new();
                                let body = InnerValue::Str(format!("v{}", i)).to_bytes().unwrap();
                                staging.set(rid.to_bytes(), body);
                            }
                            let mut tx = TxContext::new(
                                TxId::new(7_900_000 + n as u64),
                                0,
                                0,
                                IsolationLevel::Snapshot,
                            );
                            tx.write_set.insert(token, staging);
                            for i in 0..n {
                                tx.index_write_set.push((
                                    token,
                                    IndexWriteOp::SetPosting {
                                        key: bytes::Bytes::from(format!("idx_k_{}", i)),
                                        value: bytes::Bytes::from(format!("idx_v_{}", i)),
                                    },
                                ));
                            }
                            tx.set_visibility(visibility);

                            let start = Instant::now();
                            let mut outcome = commit_tx(tx, &repo).await.unwrap();
                            total += start.elapsed();
                            // Drain the background tail OUTSIDE the timed
                            // window so subsequent iters aren't contending
                            // with this iter's pending work (no carry-over
                            // distortion across samples).
                            if let Some(bg) = outcome.take_background() {
                                let _ = bg.join().await;
                            }
                        }
                        total
                    });
                },
            );
        }

        // Fjall backend — Phase 5c does a real (batched) transact + fsync
        // per call; the absolute sync→async delta is largest here.
        for visibility in [CommitVisibility::Synchronous, CommitVisibility::AsyncIndex] {
            let label = match visibility {
                CommitVisibility::Synchronous => "sync",
                CommitVisibility::AsyncIndex => "async",
            };
            group.bench_with_input(
                BenchmarkId::new(format!("fjall_{}", label), n),
                &n,
                |b, &n| {
                    b.to_async(&rt).iter_custom(|iters| async move {
                        let mut total = Duration::ZERO;
                        for _ in 0..iters {
                            let tempdir = tempfile::TempDir::new().expect("tempdir");
                            let factory = BoxRepoFactory::fjall_raw(tempdir.path().to_path_buf());
                            let repo = RepoInstance::from_factory(
                                "bench".into(),
                                factory,
                                vec![TableConfig::new("bench_table".to_string())],
                            )
                            .await
                            .unwrap();
                            let tbl = repo.get_table("bench_table").await.unwrap();
                            let token = shamir_engine::table::table_token_for("bench_table");

                            let mut staging = StagingStore::new(Arc::clone(tbl.data_store()));
                            for i in 0..n {
                                let rid = RecordId::new();
                                let body = InnerValue::Str(format!("v{}", i)).to_bytes().unwrap();
                                staging.set(rid.to_bytes(), body);
                            }
                            let mut tx = TxContext::new(
                                TxId::new(7_900_500 + n as u64),
                                0,
                                0,
                                IsolationLevel::Snapshot,
                            );
                            tx.write_set.insert(token, staging);
                            for i in 0..n {
                                tx.index_write_set.push((
                                    token,
                                    IndexWriteOp::SetPosting {
                                        key: bytes::Bytes::from(format!("idx_k_{}", i)),
                                        value: bytes::Bytes::from(format!("idx_v_{}", i)),
                                    },
                                ));
                            }
                            tx.set_visibility(visibility);

                            let start = Instant::now();
                            let mut outcome = commit_tx(tx, &repo).await.unwrap();
                            total += start.elapsed();
                            // Drain the background tail OUTSIDE the timed
                            // window so subsequent iters aren't contending
                            // with this iter's pending work (no carry-over
                            // distortion across samples).
                            if let Some(bg) = outcome.take_background() {
                                let _ = bg.join().await;
                            }

                            drop(repo);
                            drop(tempdir);
                        }
                        total
                    });
                },
            );
        }
    }

    group.finish();
}

/// Read-scan bench: quantifies Stage 21 (QueryRecord elim per-row Map alloc)
/// and Stage 22 (matches_msgpack_bytes skips InnerValue decode on rejects).
///
/// 1000 rows are inserted ONCE before measurement. Three variants exercise
/// different filter selectivities so the Stage 22 skip-decode win is visible
/// on the filtered cases.
fn bench_read_scan(c: &mut Criterion) {
    let rt = rt();

    // ── one-time fixture setup ─────────────────────────────────────────────
    let repo = {
        let r = make_repo();
        // Pre-populate 1000 rows with varied fields for selective filtering.
        rt.block_on(async {
            let resolver = Resolver { repo: r.clone() };
            let mut queries = new_map();
            let values: Vec<shamir_types::types::value::QueryValue> = (0..1000usize)
                .map(|i| {
                    shamir_types::types::value::QueryValue::from(
                        doc()
                            .set("id", format!("k{i}"))
                            .set("score", i as i64)
                            .set("category", (i % 10) as i64)
                            .set("name", format!("name_{i}"))
                            .build(),
                    )
                })
                .collect();
            queries.insert(
                "ins".to_string(),
                QueryEntry {
                    op: BatchOp::Insert(InsertOp {
                        insert_into: TableRef::new("bench_table"),
                        values,
                        records_idmsgpack: Vec::new(),
                        select: None,
                    }),
                    return_result: false,
                    after: Vec::new(),
                },
            );
            let request = BatchRequest {
                id: QueryValue::Int(1),
                name: None,
                transactional: false,
                isolation: None,
                durability: None,
                queries,
                return_all: false,
                return_only: None,
                limits: Default::default(),
                interner_epochs: Default::default(),
                result_encoding: ResultEncoding::default(),
            };
            execute_batch(&request, &resolver, None, None, Actor::System, "bench")
                .await
                .unwrap();
        });
        r
    };
    let resolver = Resolver { repo: repo.clone() };

    let mut g = c.benchmark_group("read_scan");
    bu::tune_tiered(&mut g, 30, 5, 3, 90);

    // Variant 1: scan all rows — baseline; pure scan + projection cost.
    // Exercises Stage 21 (no per-row Map alloc on 3 scan paths).
    g.bench_function("scan_all_1000", |b| {
        let q = Query::from("bench_table");
        let req = {
            let mut queries = new_map();
            queries.insert(
                "r".to_string(),
                QueryEntry {
                    op: BatchOp::Read(q.into()),
                    return_result: true,
                    after: Vec::new(),
                },
            );
            BatchRequest {
                id: QueryValue::Int(2),
                name: None,
                transactional: false,
                isolation: None,
                durability: None,
                queries,
                return_all: true,
                return_only: None,
                limits: Default::default(),
                interner_epochs: Default::default(),
                result_encoding: ResultEncoding::default(),
            }
        };
        b.to_async(&rt).iter(|| {
            let resolver = &resolver;
            let req = &req;
            async move {
                execute_batch(req, resolver, None, None, Actor::System, "bench")
                    .await
                    .unwrap()
            }
        });
    });

    // Variant 2: selective filter 10% match (category == 5, 100/1000 rows).
    // Stage 22: bytes-level filter rejects 900 rows before InnerValue decode.
    g.bench_function("scan_filtered_10pct", |b| {
        let q = Query::from("bench_table").where_eq("category", 5i64);
        let req = {
            let mut queries = new_map();
            queries.insert(
                "r".to_string(),
                QueryEntry {
                    op: BatchOp::Read(q.into()),
                    return_result: true,
                    after: Vec::new(),
                },
            );
            BatchRequest {
                id: QueryValue::Int(3),
                name: None,
                transactional: false,
                isolation: None,
                durability: None,
                queries,
                return_all: true,
                return_only: None,
                limits: Default::default(),
                interner_epochs: Default::default(),
                result_encoding: ResultEncoding::default(),
            }
        };
        b.to_async(&rt).iter(|| {
            let resolver = &resolver;
            let req = &req;
            async move {
                execute_batch(req, resolver, None, None, Actor::System, "bench")
                    .await
                    .unwrap()
            }
        });
    });

    // Variant 3: selective filter 0.1% match (score == 42, 1/1000 rows).
    // Largest Stage 22 win: 999 rows skip InnerValue decode entirely.
    g.bench_function("scan_filtered_1pct", |b| {
        let q = Query::from("bench_table").where_eq("score", 42i64);
        let req = {
            let mut queries = new_map();
            queries.insert(
                "r".to_string(),
                QueryEntry {
                    op: BatchOp::Read(q.into()),
                    return_result: true,
                    after: Vec::new(),
                },
            );
            BatchRequest {
                id: QueryValue::Int(4),
                name: None,
                transactional: false,
                isolation: None,
                durability: None,
                queries,
                return_all: true,
                return_only: None,
                limits: Default::default(),
                interner_epochs: Default::default(),
                result_encoding: ResultEncoding::default(),
            }
        };
        b.to_async(&rt).iter(|| {
            let resolver = &resolver;
            let req = &req;
            async move {
                execute_batch(req, resolver, None, None, Actor::System, "bench")
                    .await
                    .unwrap()
            }
        });
    });

    g.finish();
}

criterion_group!(
    benches,
    bench_insert_tx_vs_non_tx,
    bench_batch_insert_pipeline,
    bench_commit_tx_phase_breakdown,
    bench_provider_overhead,
    bench_commit_phase5c_indexed_fjall,
    bench_async_commit_index_heavy,
    bench_read_scan,
);
criterion_main!(benches);
