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
//! Migrated to the fixed-iteration harness (`bench_scale_tool`): each
//! `new_incremental` / `old_full_blob` workload builds a fresh store +
//! interner/manager INSIDE the timed closure (the loop of N touch+persist
//! calls IS the thing under test — there is no separable "setup" phase to
//! hoist out), so these use `bench_batched_async` with a no-op setup and
//! the harness-owned shared runtime, mirroring the original `rt.block_on`
//! timing exactly. `bench_batched_async` (not `bench_async`) matters here:
//! calibration checks elapsed time after every single call for a `Batched`
//! workload, whereas `bench_async` batches 64 calls before its first
//! check — fine for cheap ops, but `old_full_blob` is `O(N^2)` by design
//! (the pathology under test), so 64 uncalibrated calls turned a 0.3s
//! calibration into several CPU-minutes.
//! The structural "bytes written" comparison (previously a Criterion
//! side-channel `eprintln!` piggy-backing a 1-iter noop bench) is now a
//! one-shot printout at registration time, before `h.run()` — it was never
//! really a timed workload. Its own `N` is capped well below the timed
//! workloads' largest `N` (see `STRUCTURAL_NS` below) — it runs
//! unconditionally on every invocation of this bench binary regardless of
//! `--scale`, so its `O(N^2)` old-path cost is NOT scaled down by the
//! harness and must stay small enough not to dominate a full sweep.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use std::pin::Pin;

use bench_scale_tool::Harness;
use bytes::Bytes;
use futures::Stream;
use tokio::runtime::Runtime;

use shamir_engine::meta::MetaKey;
use shamir_engine::table::interner_manager::InternerManager;
use shamir_storage::error::DbError;
use shamir_storage::storage_in_memory::InMemoryStore;
use shamir_storage::types::{RecordKey, Store};
use shamir_types::codecs::basic::bincode;
use shamir_types::core::interner::Interner;

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
        .set(MetaKey::Internals.as_record_id().to_bytes().into(), bytes)
        .await?;
    Ok(())
}

/// `N`s for the one-shot structural printout only — deliberately smaller
/// than the timed workloads' largest `N` (see the module docs above): this
/// runs unconditionally on every invocation, unscaled, so its `old_full_blob`
/// arm (`O(N^2)`) must stay cheap. 500 still shows a several-hundred-x
/// ratio without costing minutes.
const STRUCTURAL_NS: [usize; 2] = [250, 500];

/// One-shot structural measurement — print bytes written by each path
/// (NOT a timed workload, just a printout before the harness runs).
fn print_bytes_written_structural() {
    let rt = Runtime::new().unwrap();
    for &n in &STRUCTURAL_NS {
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
    }
}

fn main() {
    let mut h = Harness::new("interner_cold_growth", env!("CARGO_MANIFEST_DIR"));

    print_bytes_written_structural();

    // Same N cap rationale as `STRUCTURAL_NS`, tightened further per the
    // "every workload should cost the harness only a few ms per call"
    // normalization pass: `old_full_blob` is `O(N^2)` by design, so even
    // small N increases cost fast. 250/500 keeps the comparison meaningful
    // (still a visible ratio spread) while `old_full_blob_500` lands close
    // to, not many multiples of, the ~10ms per-call target — the
    // O(N^2) shape means it can't be driven arbitrarily low without
    // losing the demonstration entirely, so this is accepted as a known,
    // documented exception rather than force-fit under 10ms.
    for &n in &[250usize, 500] {
        // NEW path — append-only chunk per persist.
        //
        // `bench_batched_async` (not `bench_async`) on purpose: calibration
        // checks the elapsed time after every single iteration (batch=1),
        // whereas `bench_async` -> `Workload::Simple` batches 64 calls
        // before its first check. `old_full_blob` below costs
        // O(N^2) bytes-written per call by design (the pathology under
        // test) — 64 uncalibrated calls at that cost turned a 0.3s
        // calibration into several CPU-minutes. Setup is a no-op `()`;
        // the whole touch+persist loop is the timed routine, unchanged.
        h.bench_batched_async(
            &format!("interner_cold_growth/new_incremental_{n}"),
            || async {},
            move |()| async move {
                let (store, _bytes) = make_counting_store();
                let mgr = InternerManager::new(Arc::clone(&store));
                for i in 0..n {
                    let interner = mgr.get().await.unwrap();
                    let _ = interner.touch_ind(format!("field_{i}")).unwrap();
                    mgr.persist().await.unwrap();
                }
            },
        );

        // OLD path — full blob rewrite per persist. See the comment above:
        // `bench_batched_async` for the same batch=1 calibration reason.
        h.bench_batched_async(
            &format!("interner_cold_growth/old_full_blob_{n}"),
            || async {},
            move |()| async move {
                let (store, _bytes) = make_counting_store();
                let interner = Interner::new();
                for i in 0..n {
                    let _ = interner.touch_ind(format!("field_{i}")).unwrap();
                    old_full_blob_persist(&store, &interner).await.unwrap();
                }
            },
        );
    }

    // Direct in-memory `touch_ind` cold-growth bench.
    //
    // This bench measures the CAS-clone cost of growing the reverse spine
    // from 0 to N distinct keys. Op B (Arc<str> slots) changes this from
    // O(N²) byte copies to O(N) refcount bumps — each CAS-loop clone
    // previously deep-copied every `String` slot.
    //
    // Run alongside `interner_concurrent` to confirm read-path is unchanged.
    // N=100/200/300 keep each call near the ~10ms per-call target on the OLD
    // O(N²) path (measured ~26µs/touch, so N=300 lands around 8ms). #501: a
    // larger N=2000 case is added to make the O(N²)→O(N) shape visible at a
    // scale closer to the audit's "10k+ fields" scenario — on the fixed
    // (doubling-growth) path this is cheap (O(N) total), and even on the old
    // path the fixed-iteration harness self-calibrates iteration count, so a
    // heavier per-call cost just yields fewer samples rather than hanging.
    for &n in &[100usize, 200, 300, 2000] {
        h.bench(
            &format!("interner_touch_ind_cold_growth/touch_ind_{n}"),
            move || {
                let interner = Interner::new();
                for i in 0..n {
                    let _ = std::hint::black_box(interner.touch_ind(format!("field_{i}")));
                }
            },
        );
    }

    h.run();
}
