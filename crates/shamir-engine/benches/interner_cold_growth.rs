//! Cold-write / schema-growth bench for the interner persistence path.
//!
//! Pathology being measured: every distinct new field name triggers a
//! `persist()`. Old implementation rewrote the WHOLE dictionary blob
//! on every persist — total bytes written across N first-touches was
//! `1 + 2 + … + N = O(N²)`. New implementation appends a single
//! `(InternerKey, UserKey)` chunk per persist — total bytes is `O(N)`.
//!
//! We compare:
//! * `new_incremental` — current `InternerManager::persist()` path
//!   (one chunk per new key).
//! * `old_full_blob` — direct emulation of the old "rewrite the whole
//!   thing" persistence to give a wall-clock baseline. Uses the
//!   same `Interner::all_entries()` + bincode serialize + single
//!   `set()` write the legacy code did. NOT calling the manager —
//!   the manager no longer offers this path.
//!
//! Run:
//!   cargo bench -p shamir-engine --bench interner_cold_growth

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use std::pin::Pin;

use bytes::Bytes;
use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion, Throughput};
use futures::Stream;
use tokio::runtime::Runtime;

use shamir_engine::meta::MetaKey;
use shamir_engine::table::interner_manager::InternerManager;
use shamir_storage::error::DbError;
use shamir_storage::storage_in_memory::InMemoryStore;
use shamir_storage::types::{RecordKey, Store};
use shamir_types::codecs::basic::bincode;
use shamir_types::core::interner::{Interner, InternerKey, UserKey};

/// Byte-counting Store wrapper — totals the bytes written through
/// `set()` so we can prove the O(N²) → O(N) structural difference.
struct CountingStore {
    inner: Arc<dyn Store>,
    bytes_written: Arc<AtomicUsize>,
}

fn make_counting_store() -> (Arc<dyn Store>, Arc<AtomicUsize>) {
    let bytes = Arc::new(AtomicUsize::new(0));
    let s: Arc<dyn Store> = Arc::new(CountingStore {
        inner: Arc::new(InMemoryStore::new()),
        bytes_written: Arc::clone(&bytes),
    });
    (s, bytes)
}

#[async_trait::async_trait]
impl Store for CountingStore {
    async fn insert(
        &self,
        value: Bytes,
    ) -> shamir_storage::error::DbResult<shamir_storage::types::RecordKey> {
        self.bytes_written.fetch_add(value.len(), Ordering::Relaxed);
        self.inner.insert(value).await
    }
    async fn set(
        &self,
        key: shamir_storage::types::RecordKey,
        value: Bytes,
    ) -> shamir_storage::error::DbResult<bool> {
        self.bytes_written.fetch_add(value.len(), Ordering::Relaxed);
        self.inner.set(key, value).await
    }
    async fn get(
        &self,
        key: shamir_storage::types::RecordKey,
    ) -> shamir_storage::error::DbResult<Bytes> {
        self.inner.get(key).await
    }
    async fn remove(
        &self,
        key: shamir_storage::types::RecordKey,
    ) -> shamir_storage::error::DbResult<bool> {
        self.inner.remove(key).await
    }
    fn iter_stream(
        &self,
        batch_size: usize,
    ) -> Pin<Box<dyn Stream<Item = Result<Vec<(RecordKey, Bytes)>, DbError>> + Send>> {
        self.inner.iter_stream(batch_size)
    }
    fn scan_prefix_stream(
        &self,
        prefix: Bytes,
        batch_size: usize,
    ) -> Pin<Box<dyn Stream<Item = Result<Vec<(RecordKey, Bytes)>, DbError>> + Send>> {
        self.inner.scan_prefix_stream(prefix, batch_size)
    }
}

/// Simulate the OLD persistence path: serialize `all_entries()` and
/// write the WHOLE blob to `MetaKey::Internals` on every persist.
async fn old_full_blob_persist(
    store: &Arc<dyn Store>,
    interner: &Interner,
) -> shamir_storage::error::DbResult<()> {
    let entries = interner.all_entries();
    if entries.is_empty() {
        return Ok(());
    }
    let bytes = bincode::to_bytes(&entries).unwrap();
    store
        .set(MetaKey::Internals.as_record_id().to_bytes(), bytes)
        .await?;
    Ok(())
}

fn bench_cold_growth(c: &mut Criterion) {
    let rt = Runtime::new().unwrap();
    let mut group = c.benchmark_group("interner_cold_growth");

    for &n in &[1000usize, 5000] {
        group.throughput(Throughput::Elements(n as u64));

        // NEW path — append-only chunk per persist.
        group.bench_with_input(BenchmarkId::new("new_incremental", n), &n, |b, &n| {
            b.iter_custom(|iters| {
                let mut total = std::time::Duration::ZERO;
                for _ in 0..iters {
                    let (store, _bytes) = make_counting_store();
                    let mgr = InternerManager::new(Arc::clone(&store));
                    let elapsed = rt.block_on(async {
                        let start = std::time::Instant::now();
                        for i in 0..n {
                            let interner = mgr.get().await.unwrap();
                            let _ = interner.touch_ind(format!("field_{i}")).unwrap();
                            mgr.persist().await.unwrap();
                        }
                        start.elapsed()
                    });
                    total += elapsed;
                }
                total
            });
        });

        // OLD path — full blob rewrite per persist.
        group.bench_with_input(BenchmarkId::new("old_full_blob", n), &n, |b, &n| {
            b.iter_custom(|iters| {
                let mut total = std::time::Duration::ZERO;
                for _ in 0..iters {
                    let (store, _bytes) = make_counting_store();
                    let interner = Interner::new();
                    let elapsed = rt.block_on(async {
                        let start = std::time::Instant::now();
                        for i in 0..n {
                            let _ = interner.touch_ind(format!("field_{i}")).unwrap();
                            old_full_blob_persist(&store, &interner).await.unwrap();
                        }
                        start.elapsed()
                    });
                    total += elapsed;
                }
                total
            });
        });
    }

    group.finish();
}

/// One-shot structural measurement — print bytes written by each path
/// (NOT a Criterion bench, just executed once per `criterion_main`
/// invocation via a 1-iter benchmark so the numbers show up).
fn bench_bytes_written_structural(c: &mut Criterion) {
    let rt = Runtime::new().unwrap();
    let mut group = c.benchmark_group("interner_cold_growth_bytes");
    // Tight measurement window — we want the bytes-written totals,
    // not a tight timing distribution.
    group.sample_size(10);

    for &n in &[1000usize, 5000] {
        // Measure NEW total bytes — single run.
        let (store_new, bytes_new) = make_counting_store();
        let mgr = InternerManager::new(Arc::clone(&store_new));
        rt.block_on(async {
            for i in 0..n {
                let interner = mgr.get().await.unwrap();
                let _ = interner.touch_ind(format!("field_{i}")).unwrap();
                mgr.persist().await.unwrap();
            }
        });
        let new_total = bytes_new.load(Ordering::Relaxed);

        // Measure OLD total bytes — single run.
        let (store_old, bytes_old) = make_counting_store();
        let interner = Interner::new();
        rt.block_on(async {
            for i in 0..n {
                let _ = interner.touch_ind(format!("field_{i}")).unwrap();
                old_full_blob_persist(&store_old, &interner).await.unwrap();
            }
        });
        let old_total = bytes_old.load(Ordering::Relaxed);

        eprintln!(
            "  [bytes_written] N={n}  new={new_total}  old={old_total}  \
             ratio_old/new={:.1}x",
            old_total as f64 / new_total.max(1) as f64
        );

        // Touch the values so the compiler can't elide.
        let _ = (new_total, old_total);
        // Register a trivially-cheap bench point so Criterion runs the
        // group (otherwise it skips an empty group).
        group.bench_function(BenchmarkId::new("noop", n), |b| {
            b.iter(|| criterion::black_box(InternerKey::new(n as u64)));
        });
        let _ = UserKey::from_str("x");
    }

    group.finish();
}

criterion_group!(benches, bench_cold_growth, bench_bytes_written_structural);
criterion_main!(benches);
