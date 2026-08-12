//! `ReaderDrainGate` benchmark — audit `2026-08-09-new-wave-release-readonly-review-codex.md`
//! findings §2 (reader-vs-DROP exclusion gate cost).
//!
//! The gate uses `SeqCst` ordering on every hot read path: `enter()` does a
//! `SeqCst fetch_add` followed by a `SeqCst` load on `dropping`, and
//! `ReadGuard::drop` does a `SeqCst fetch_sub`. The review flagged this as
//! potentially expensive and suggested measuring it before any architectural
//! change (per-index granularity, weaker Acquire/Release ordering, `Notify`-based
//! wakeup). This benchmark provides those measurements.
//!
//! # Workloads
//!
//! - `enter_drop_baseline/1_thread` — baseline cost of `enter()`+drop with NO
//!   concurrent DROP. Single task looping `gate.enter()` → trivial black_box
//!   work → drop the guard. Measures wall-clock ns/op to see how the SeqCst
//!   RMWs perform.
//! - `enter_drop_baseline/8_threads` — same baseline with 8 concurrent tasks.
//! - `enter_drop_baseline/32_threads` — same baseline with 32 concurrent tasks.
//! - `enter_drop_baseline/64_threads` — same baseline with 64 concurrent tasks.
//! - `gate_absent_control/1_thread` — same workload with gate calls removed
//!   (true no-op control).
//! - `gate_absent_control/8_threads` — 8 threads, no gate.
//! - `gate_absent_control/32_threads` — 32 threads, no gate.
//! - `gate_absent_control/64_threads` — 64 threads, no gate.
//! - `concurrent_drop_drain` — with readers continuously calling `enter()`,
//!   triggers a `begin_drop()`+`wait_for_drain()` cycle.
//! - `index_manager_lookup` — measures the gate's cost through the REAL
//!   production call path (`IndexManager::lookup_by_index`).
//!
//! # Honest-reporting note
//!
//! The gate has always existed in this codebase, so there is no literal "before"
//! binary to revert to. The "gate absent" control provides the "before" signal —
//! "as if there were no gate". This mirrors `posting_cache_hit.rs`'s own precedent
//! for honest measurement when the historical baseline cannot be re-run.
//!
//! Run: `CARGO_TARGET_DIR=D:/dev/rust/.cargo-target-bench cargo bench -p shamir-index --bench reader_drain_gate`
//! (uses the isolated bench target dir per the workspace convention).

use std::hint::black_box;
use std::sync::Arc;

use bench_scale_tool::Harness;
use shamir_index::base_index::index_definition::IndexDefinition;
use shamir_index::base_index::index_info_item::IndexInfoItem;
use shamir_index::base_index::index_manager::IndexManager;
use shamir_index::reader_drain_gate::ReaderDrainGate;
use shamir_storage::storage_in_memory::InMemoryStore;
use shamir_storage::types::Store;
use shamir_types::core::interner::InternerKey;
use shamir_types::types::common::new_map;
use shamir_types::types::record_id::RecordId;
use shamir_types::types::value::InnerValue;

/// A single indexed field id used by the bench's index definition.
const FIELD_ID: u64 = 1;

/// Number of iterations per task in the baseline / gate_absent workloads.
const ITERATIONS_PER_TASK: usize = 100_000;

/// Number of records to populate for the IndexManager path benchmark.
const N_POSTINGS: usize = 10_000;

/// Builds an `IndexManager` over an in-memory store with a single-field
/// regular index on `FIELD_ID`, then populates `n_postings` records that
/// ALL share the same indexed value (`"active"`). This simulates the
/// review's low-cardinality example and mirrors `posting_cache_hit.rs`.
///
/// Returns the manager and the lookup values to query.
async fn build_manager_with_hot_posting(n_postings: usize) -> (IndexManager, Vec<InnerValue>) {
    let data_store = Arc::new(InMemoryStore::new()) as Arc<dyn Store>;
    let info_store = Arc::new(InMemoryStore::new()) as Arc<dyn Store>;
    let manager = IndexManager::new(Arc::clone(&data_store), Arc::clone(&info_store))
        .await
        .unwrap();

    // Single-field regular index on FIELD_ID.
    let index_def = IndexDefinition::new(1001, vec![IndexInfoItem::new(vec![FIELD_ID])]);
    manager.create_index(index_def).await.unwrap();

    // The hot indexed value — every record shares it (low cardinality).
    let hot_value = InnerValue::Str("active".to_string());
    let lookup_values = vec![hot_value.clone()];

    // Batched insert: one `now_micros()` per batch, ascending seq tails.
    let batch_ts = RecordId::now_micros();
    for i in 0..n_postings {
        let record_id = RecordId::from_ts_seq(batch_ts, i as u32);
        let mut map = new_map();
        map.insert(InternerKey::new(FIELD_ID), hot_value.clone());
        let value = InnerValue::Map(map);
        manager.on_record_created(&record_id, &value).await.unwrap();
    }

    (manager, lookup_values)
}

/// Multi-threaded runtime — REQUIRED for this bench's stated purpose.
/// `new_current_thread()` (the single-threaded runtime `posting_cache_hit.rs`
/// uses for its non-concurrent workload) would schedule every "concurrent"
/// task cooperatively on ONE OS thread, so the `enter()`/`drop()` SeqCst RMWs
/// would never actually contend across cores — the whole point of the
/// 1/8/32/64 "threads" workloads is to exercise genuine cross-core atomic
/// contention, which requires real OS threads under the tasks.
fn rt() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .unwrap()
}

fn main() {
    let mut h = Harness::new("reader_drain_gate", env!("CARGO_MANIFEST_DIR"));
    // Leak the runtime so each bench closure can capture a 'static reference.
    let runtime: &'static tokio::runtime::Runtime = Box::leak(Box::new(rt()));

    // ========================================================================
    // Workload 1: Baseline cost of `enter()` + drop with no concurrent DROP
    // ========================================================================
    for &num_threads in &[1usize, 8, 32, 64] {
        let gate: &'static ReaderDrainGate = Box::leak(Box::new(ReaderDrainGate::new()));
        let id = format!("enter_drop_baseline/{}_threads", num_threads);
        h.bench(&id, move || {
            // Each task does ITERATIONS_PER_TASK iterations of enter() -> black_box -> drop
            let mut tasks = Vec::with_capacity(num_threads);
            for _ in 0..num_threads {
                let gate = gate.clone();
                tasks.push(runtime.spawn(async move {
                    let mut local_sum = 0usize;
                    for _ in 0..ITERATIONS_PER_TASK {
                        let _guard = gate.enter();
                        // Simulate a trivial read (no-op, but prevent DCE).
                        local_sum = local_sum.wrapping_add(black_box(1));
                        // Guard drops here.
                    }
                    black_box(local_sum);
                }));
            }
            // Wait for all tasks to complete.
            for task in tasks {
                runtime.block_on(task).unwrap();
            }
        });
    }

    // ========================================================================
    // Workload 2: "Gate absent" control (same workload without gate calls)
    // ========================================================================
    for &num_threads in &[1usize, 8, 32, 64] {
        let id = format!("gate_absent_control/{}_threads", num_threads);
        h.bench(&id, move || {
            let mut tasks = Vec::with_capacity(num_threads);
            for _ in 0..num_threads {
                tasks.push(runtime.spawn(async move {
                    let mut local_sum = 0usize;
                    for _ in 0..ITERATIONS_PER_TASK {
                        // No gate.enter() call at all — true no-op baseline.
                        local_sum = local_sum.wrapping_add(black_box(1));
                    }
                    black_box(local_sum);
                }));
            }
            for task in tasks {
                runtime.block_on(task).unwrap();
            }
        });
    }

    // ========================================================================
    // Workload 3: Cost of concurrent DROP's drain window on sibling reads
    // ========================================================================
    //
    // This spawns a continuous stream of readers on one thread while another
    // thread triggers begin_drop() + wait_for_drain(). We measure:
    // - How many readers get None during the drain window.
    // - How long wait_for_drain() takes to complete.
    {
        let gate: &'static ReaderDrainGate = Box::leak(Box::new(ReaderDrainGate::new()));
        h.bench("concurrent_drop_drain", move || {
            // Spawn reader task that runs continuously.
            let gate_for_reader = gate.clone();
            let reader_task = runtime.spawn(async move {
                let mut none_count = 0;
                let mut total_reads = 0;
                let start = std::time::Instant::now();
                // Run for a fixed duration to allow the drop to happen.
                while start.elapsed() < std::time::Duration::from_secs(2) {
                    if gate_for_reader.enter().is_none() {
                        none_count += 1;
                    }
                    total_reads += 1;
                }
                (none_count, total_reads)
            });

            // Sleep a bit to let the reader ramp up.
            std::thread::sleep(std::time::Duration::from_millis(100));

            // Now trigger drop on the same gate (simulating "index A drop stalls
            // sibling index B reads" collateral effect).
            let _drain_guard = gate.begin_drop();
            runtime.block_on(_drain_guard.wait_for_drain());

            // Wait for the reader task to finish and collect stats.
            let (none_count, total_reads) = runtime.block_on(reader_task).unwrap();

            // Report as black_box to prevent DCE.
            black_box(none_count);
            black_box(total_reads);
        });
    }

    // ========================================================================
    // Workload 4: Real IndexManager path (not just raw atomic ops)
    // ========================================================================
    //
    // This measures the gate's cost through the production call path
    // (IndexManager::lookup_by_index), which internally uses the gate.
    // Uses a hot posting (low-cardinality index) to simulate a realistic query.
    {
        let (manager, lookup_values) = runtime.block_on(build_manager_with_hot_posting(N_POSTINGS));
        let manager_ref: &'static IndexManager = Box::leak(Box::new(manager));
        let lookup_values: &'static Vec<InnerValue> = Box::leak(Box::new(lookup_values));

        // Warm up: perform a lookup to prime the cache (cache-HIT path).
        let _ = runtime
            .block_on(manager_ref.lookup_by_index(1001, lookup_values))
            .unwrap()
            .unwrap();

        // Now benchmark the cache-HIT path (includes the gate's cost).
        h.bench("index_manager_lookup", move || {
            let ids: Arc<_> = runtime
                .block_on(manager_ref.lookup_by_index(1001, lookup_values))
                .unwrap()
                .unwrap();
            // Force materialization of the Arc result.
            black_box(ids.len());
        });
    }

    h.run();
}
