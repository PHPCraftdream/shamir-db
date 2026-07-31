//! F-78 (#905) bench — stream legacy regular-index build vs. materialize.
//!
//! Before F-78, `CREATE INDEX` on a legacy regular-hash index materialized the
//! ENTIRE table into a `Vec<(RecordId, InnerValue)>` (the
//! `collect_all_current_records` + `create_index_from_records` path) and then
//! built a SECOND full-table `Vec` of postings before one `set_many` — O(table)
//! peak heap held under the write barrier. After F-78, the path streams the
//! table in batches and writes postings per batch via
//! `create_index_from_stream` — O(batch) peak heap.
//!
//! This bench measures, per table size N (50k, 200k rows):
//! - **PEAK HEAP** (`shamir_bench_utils::peak_mem`, a `#[global_allocator]`
//!   wrapper tracking the max live-byte watermark) for OLD (materialize +
//!   `create_index_from_records`) vs NEW (`create_index_from_stream`). The
//!   OLD→NEW *delta* is the O(table) transient the fix eliminates; it grows
//!   ~linearly with N (compare the 50k vs 200k deltas), proving the
//!   O(table)→O(batch) change.
//! - **WALL TIME** (`bench_scale_tool::Harness`) for the same two paths.
//!
//! Both paths read the SAME shared `data_store` (populated once, untimed) and
//! each iteration builds into a FRESH `IndexManager` + `info_store` so postings
//! never accumulate across iterations.
//!
//! NOTE: enabling the `peak_mem` feature installs a `#[global_allocator]` that
//! adds one atomic op per alloc/free process-wide, so the Harness wall-time
//! cells carry that constant per-alloc overhead — but it is identical for OLD
//! and NEW, so the old-vs-new RATIO is unaffected (only the absolute ns/op is
//! inflated). The headline signal here is the peak-heap delta, which is
//! allocator-independent.
//!
//! Run:
//!   CARGO_TARGET_DIR=D:\dev\rust\.cargo-target-bench cargo bench -p shamir-index --bench create_index_streaming
//! First run / when counts drift, calibrate:
//!   ... -- --calibrate 2
//!
//! ## Measured results (F-78, Windows, release + peak_mem global allocator)
//!
//! Peak heap (peak_alloc max-live-byte watermark; raw includes the shared
//! `data_store` baseline — the OLD→NEW *delta* is the eliminated O(table)
//! transient):
//!
//! | N rows | OLD materialize | NEW stream   | delta eliminated | NEW/OLD |
//! |--------|-----------------|--------------|------------------|---------|
//! | 50_000 | 49.6 MiB        | 21.1 MiB     | −28.5 MiB        | 0.43×   |
//! | 200_000| 175.2 MiB       | 83.3 MiB     | −91.9 MiB        | 0.48×   |
//!
//! The eliminated transient grows 3.23× when N grows 4× (50k→200k) → ~linear,
//! confirming the O(table)→O(batch) change (sub-4× only because the index's own
//! O(n) postings, present in BOTH shapes, eat into the ratio).
//!
//! Wall time (Harness; both paths decode every row once, so decode-bound and
//! ~equal — the fix is a MEMORY optimization, not a speed one):
//!
//! | N rows | OLD materialize | NEW stream |
//! |--------|-----------------|------------|
//! | 50_000 | 361 ms/op       | 330 ms/op  |
//! | 200_000| 1932 ms/op      | 2054 ms/op |
//!
//! (Wall-time carries peak_alloc's per-alloc atomic overhead equally for both
//! paths, so the ratio is meaningful; absolute ns/op is inflated.)

use std::hint::black_box;
use std::sync::Arc;

use bench_scale_tool::Harness;
use futures::StreamExt;
use shamir_index::legacy::index_definition::IndexDefinition;
use shamir_index::legacy::index_info_item::IndexInfoItem;
use shamir_index::legacy::index_manager::IndexManager;
use shamir_storage::storage_in_memory::InMemoryStore;
use shamir_storage::types::Store;
use shamir_types::core::interner::InternerKey;
use shamir_types::types::common::new_map;
use shamir_types::types::record_id::RecordId;
use shamir_types::types::value::InnerValue;

const FIELD_ID: u64 = 1;
const NAME_PAYLOAD: u64 = 2;
const SCORE_PAYLOAD: u64 = 3;
/// Decode/stream batch size — mirrors `collect_all_current_records` (1000) and
/// `TableManager::create_index`'s `list_stream(1000)`.
const BATCH: usize = 1000;

/// Populate a fresh in-memory `data_store` with `n` records. Each record
/// carries the indexed field plus two payload fields, so the materialized
/// `Vec<(RecordId, InnerValue)>` (OLD path) holds a realistically-wide decoded
/// tree per row — the O(table) transient this task eliminates.
async fn populate_data_store(n: usize) -> Arc<dyn Store> {
    let data_store = Arc::new(InMemoryStore::new()) as Arc<dyn Store>;
    let base_ts = RecordId::now_micros();
    for i in 0..n {
        let rid = RecordId::from_ts_seq(base_ts, i as u32);
        let mut m = new_map();
        m.insert(
            InternerKey::new(FIELD_ID),
            InnerValue::Str(format!("val_{i}")),
        );
        m.insert(
            InternerKey::new(NAME_PAYLOAD),
            InnerValue::Str(format!("user_{i}")),
        );
        m.insert(InternerKey::new(SCORE_PAYLOAD), InnerValue::Int(i as i64));
        data_store
            .set(
                rid.to_bytes().into(),
                InnerValue::Map(m).to_bytes().unwrap(),
            )
            .await
            .unwrap();
    }
    data_store
}

fn index_def(name: u64) -> IndexDefinition {
    IndexDefinition::new(name, vec![IndexInfoItem::new(vec![FIELD_ID])])
}

/// OLD (pre-F-78) path: materialize the whole table into a
/// `Vec<(RecordId, InnerValue)>`, then `create_index_from_records` (one
/// `set_many` at the end). Mirrors `collect_all_current_records` +
/// `create_index_from_records`.
async fn old_build(data_store: &Arc<dyn Store>, name: u64) {
    let info_store = Arc::new(InMemoryStore::new()) as Arc<dyn Store>;
    let manager = IndexManager::new(Arc::clone(data_store), Arc::clone(&info_store))
        .await
        .unwrap();
    // Materialize the entire table (the O(table) allocation F-78 removes).
    let mut records: Vec<(RecordId, InnerValue)> = Vec::new();
    let mut stream = data_store.iter_stream(BATCH);
    while let Some(batch) = stream.next().await {
        for (k, v) in batch.unwrap() {
            let arr: [u8; 16] = k.as_ref().try_into().unwrap();
            records.push((RecordId(arr), InnerValue::from_bytes(v).unwrap()));
        }
    }
    manager
        .create_index_from_records(index_def(name), records)
        .await
        .unwrap();
}

/// NEW (F-78) path: stream the table in batches straight into
/// `create_index_from_stream` (per-batch `set_many`). No whole-table Vec.
async fn new_build(data_store: &Arc<dyn Store>, name: u64) {
    let info_store = Arc::new(InMemoryStore::new()) as Arc<dyn Store>;
    let manager = IndexManager::new(Arc::clone(data_store), Arc::clone(&info_store))
        .await
        .unwrap();
    let stream = data_store.iter_stream(BATCH).map(|batch| {
        batch.map(|rows| {
            rows.into_iter()
                .map(|(k, v)| {
                    let arr: [u8; 16] = k.as_ref().try_into().unwrap();
                    (RecordId(arr), InnerValue::from_bytes(v).unwrap())
                })
                .collect::<Vec<_>>()
        })
    });
    manager
        .create_index_from_stream(index_def(name), stream)
        .await
        .unwrap();
}

fn rt() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
}

/// One-shot peak-heap measurement for one (path, n) cell. Peak is a max, not a
/// mean, so a single run suffices. Returns the peak live-byte watermark
/// observed DURING the build (the shared `data_store` baseline is live
/// throughout, so the raw figure includes it; the OLD-vs-NEW delta on identical
/// fixtures isolates the build's own transient).
fn measure_peak(label: &str, n: usize, data_store: &Arc<dyn Store>, new_path: bool) -> usize {
    let runtime = rt();
    shamir_bench_utils::peak_mem::reset();
    runtime.block_on(async {
        if new_path {
            new_build(data_store, n as u64 + 1).await;
        } else {
            old_build(data_store, n as u64).await;
        }
    });
    let peak = shamir_bench_utils::peak_mem::current_peak();
    eprintln!(
        "  F-78 peak-heap  n={n:<7} {label:<14} = {peak:>12} bytes ({:>8.1} MiB)",
        peak as f64 / (1u64 << 20) as f64,
    );
    peak
}

fn main() {
    // Force the peak_mem global allocator to be linked.
    shamir_bench_utils::peak_mem::setup();

    let sizes: &[usize] = &[50_000, 200_000];

    // ── PEAK-HEAP measurement (one-shot per cell) ──────────────────────────
    eprintln!("=== F-78 (#905) peak-heap: OLD materialize vs NEW stream ===");
    eprintln!("(raw figures include the shared data_store baseline; the OLD→NEW");
    eprintln!(" delta is the O(table) transient the fix eliminates)");
    let mut deltas: Vec<(usize, f64)> = Vec::new();
    for &n in sizes {
        // One shared data_store per n; both paths read it (read-only).
        let runtime = rt();
        let data_store = runtime.block_on(populate_data_store(n));
        drop(runtime);
        let peak_old = measure_peak("OLD_materialize", n, &data_store, false);
        let peak_new = measure_peak("NEW_stream", n, &data_store, true);
        let delta_mb = (peak_old as f64 - peak_new as f64) / (1u64 << 20) as f64;
        let reduction = peak_old as f64 / peak_new.max(1) as f64;
        eprintln!(
            "  → n={n}: delta={delta_mb:+.1} MiB eliminated, NEW peak is {reduction:.2}x of OLD"
        );
        deltas.push((n, delta_mb));
    }
    // Confirm the delta scales ~linearly with N (O(table) transient).
    if deltas.len() == 2 {
        let (n0, d0) = deltas[0];
        let (n1, d1) = deltas[1];
        let ratio = (d1 / d0).abs();
        let n_ratio = n1 as f64 / n0 as f64;
        eprintln!(
            "  → delta scales {ratio:.2}x when N grows {n_ratio:.0}x (linear ⇒ O(table) transient eliminated)"
        );
    }

    // ── WALL-TIME workloads (Harness owns iteration count) ─────────────────
    // Shared data_store per n (built once, untimed); each iteration builds into
    // a FRESH IndexManager + info_store so postings never accumulate.
    let mut h = Harness::new("create_index_streaming", env!("CARGO_MANIFEST_DIR"));
    for &n in sizes {
        let runtime = rt();
        let data_store_old = runtime.block_on(populate_data_store(n));
        let data_store_new = runtime.block_on(populate_data_store(n));
        drop(runtime);

        h.bench_async(&format!("f78/{n}/old_materialize"), move || {
            let data_store = Arc::clone(&data_store_old);
            async move {
                old_build(&data_store, n as u64 + 100_000).await;
                black_box(());
            }
        });
        h.bench_async(&format!("f78/{n}/new_stream"), move || {
            let data_store = Arc::clone(&data_store_new);
            async move {
                new_build(&data_store, n as u64 + 100_001).await;
                black_box(());
            }
        });
    }

    h.run();
}
