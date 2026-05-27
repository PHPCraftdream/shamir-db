//! Core overhead benchmarks for the tx layer.
//!
//! Measures four hot paths:
//!  - `mvcc_set_versioned_no_snapshots` — zero-overhead non-tx write
//!    (active_snapshots_empty() → main.set, no history).
//!  - `mvcc_get_at_fast_path` — version_cache hit → main.get.
//!  - `staging_store_set_get` — per-tx write buffer throughput.
//!  - `repo_tx_gate_assign_version` — atomic counter scaling.

use std::sync::Arc;

use bytes::Bytes;
use criterion::{criterion_group, criterion_main, Criterion, Throughput};
use shamir_storage::storage_in_memory::InMemoryStore;
use shamir_storage::types::Store;
use shamir_tx::{MvccStore, RepoTxGate, StagingStore};

fn rt() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
}

fn bench_mvcc_set_versioned_no_snapshots(c: &mut Criterion) {
    let mut group = c.benchmark_group("mvcc_set_versioned_no_snapshots");
    let rt = rt();

    let main: Arc<dyn Store> = Arc::new(InMemoryStore::new());
    let history: Arc<dyn Store> = Arc::new(InMemoryStore::new());
    let gate = Arc::new(RepoTxGate::fresh());
    let mvcc = MvccStore::new(main, history, gate);

    group.throughput(Throughput::Elements(1));
    group.bench_function("zero_overhead_write", |b| {
        let mut counter = 0u64;
        b.to_async(&rt).iter(|| {
            let key = Bytes::copy_from_slice(&counter.to_be_bytes());
            let value = Bytes::from_static(b"v");
            counter = counter.wrapping_add(1);
            let mvcc = &mvcc;
            async move {
                mvcc.set_versioned(key, value).await.unwrap();
            }
        });
    });
    group.finish();
}

fn bench_mvcc_get_at_fast_path(c: &mut Criterion) {
    let mut group = c.benchmark_group("mvcc_get_at_fast_path");
    let rt = rt();

    let main: Arc<dyn Store> = Arc::new(InMemoryStore::new());
    let history: Arc<dyn Store> = Arc::new(InMemoryStore::new());
    let gate = Arc::new(RepoTxGate::fresh());
    let mvcc = MvccStore::new(main, history, gate);

    rt.block_on(async {
        for i in 0..1000u32 {
            mvcc.set_versioned(
                Bytes::copy_from_slice(&i.to_be_bytes()),
                Bytes::from_static(b"v"),
            )
            .await
            .unwrap();
        }
    });

    group.throughput(Throughput::Elements(1));
    group.bench_function("read_at_high_snapshot", |b| {
        let mut counter = 0u32;
        b.to_async(&rt).iter(|| {
            let key = (counter % 1000).to_be_bytes();
            counter = counter.wrapping_add(1);
            let mvcc = &mvcc;
            async move {
                let _ = mvcc.get_at(&key, u64::MAX).await.unwrap();
            }
        });
    });
    group.finish();
}

fn bench_staging_store(c: &mut Criterion) {
    let mut group = c.benchmark_group("staging_store");
    let rt = rt();
    let base: Arc<dyn Store> = Arc::new(InMemoryStore::new());
    let staging = StagingStore::new(base);

    group.throughput(Throughput::Elements(1));
    group.bench_function("set_then_get", |b| {
        let mut counter = 0u64;
        b.to_async(&rt).iter(|| {
            let key: shamir_storage::types::RecordKey =
                Bytes::copy_from_slice(&counter.to_be_bytes());
            counter = counter.wrapping_add(1);
            let staging = &staging;
            async move {
                staging.set(key.clone(), Bytes::from_static(b"v")).await;
                let _ = staging.get(key).await.unwrap();
            }
        });
    });
    group.finish();
}

fn bench_repo_tx_gate_assign_version(c: &mut Criterion) {
    let mut group = c.benchmark_group("repo_tx_gate");
    let gate = RepoTxGate::fresh();

    group.throughput(Throughput::Elements(1));
    group.bench_function("assign_next_version", |b| {
        b.iter(|| {
            let _ = gate.assign_next_version();
        });
    });
    group.finish();
}

criterion_group!(
    benches,
    bench_mvcc_set_versioned_no_snapshots,
    bench_mvcc_get_at_fast_path,
    bench_staging_store,
    bench_repo_tx_gate_assign_version,
);
criterion_main!(benches);
