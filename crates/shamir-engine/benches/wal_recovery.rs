//! Micro-bench for `WalManager::list_inflight()` — the recovery
//! scan that runs once on every table open. Cost determines
//! cold-start latency when a table was killed mid-batch with
//! many in-flight transactions.

use std::sync::Arc;

use criterion::{black_box, criterion_group, criterion_main, BenchmarkId, Criterion, Throughput};
use tokio::runtime::Runtime;

use shamir_storage::storage_in_memory::InMemoryStore;
use shamir_storage::types::Store;
use shamir_types::types::record_id::RecordId;
use shamir_wal::{WalManager, WalOp};

/// Seed `n` in-flight markers (begin without commit) and bench
/// `list_inflight` over them.
fn bench_list_inflight(c: &mut Criterion) {
    let rt = Runtime::new().unwrap();
    let mut group = c.benchmark_group("wal_list_inflight");

    for &n in &[100usize, 1_000, 10_000] {
        // Build the WAL with n pre-seeded markers ONCE per bench
        // point (outside the timed loop).
        let wal: Arc<WalManager> = rt.block_on(async {
            let store: Arc<dyn Store> = Arc::new(InMemoryStore::new());
            let mgr = WalManager::new(store);
            for _ in 0..n {
                let txn_id = mgr.fresh_txn_id();
                // Each entry carries 4 record ops — small but
                // non-empty payload, so bincode has real work to do.
                let ops: Vec<WalOp> = (0..4)
                    .map(|_| WalOp::RecordCreated {
                        record_id: RecordId::new(),
                    })
                    .collect();
                mgr.begin(txn_id, ops).await.unwrap();
            }
            Arc::new(mgr)
        });

        group.throughput(Throughput::Elements(n as u64));
        group.bench_with_input(BenchmarkId::from_parameter(n), &n, |b, _| {
            b.to_async(&rt).iter(|| {
                let wal = Arc::clone(&wal);
                async move {
                    let entries = wal.list_inflight().await.unwrap();
                    black_box(entries);
                }
            });
        });
    }
    group.finish();
}

criterion_group!(benches, bench_list_inflight);
criterion_main!(benches);
