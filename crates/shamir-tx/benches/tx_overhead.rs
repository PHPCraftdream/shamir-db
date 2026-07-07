//! Core overhead benchmarks for the tx layer.
//!
//! Measures four hot paths:
//!  - `mvcc_set_versioned_no_snapshots` — zero-overhead non-tx write
//!    (active_snapshots_empty() → main.set, no history).
//!  - `mvcc_get_at_fast_path` — version_cache hit → main.get.
//!  - `staging_store_set_get` — per-tx write buffer throughput.
//!  - `repo_tx_gate_assign_version` — atomic counter scaling.
//!
//! Migrated to the fixed-iteration harness (`bench_scale_tool`, `async`
//! feature). `mvcc_set_versioned_no_snapshots` and `staging_store` need a
//! fresh per-iteration key/state (writing the same key twice or reusing a
//! consumed `StagingStore` would change the workload's semantics), so both
//! use `bench_batched_async`. `mvcc_get_at_fast_path` reads a pre-populated,
//! shared store — plan 1 (`bench_async`). `repo_tx_gate_assign_version` is
//! sync CPU-only — plain `bench`.

use std::cell::Cell;
use std::hint::black_box;
use std::sync::Arc;

use bench_scale_tool::Harness;
use bytes::Bytes;
use shamir_storage::storage_in_memory::InMemoryStore;
use shamir_storage::types::Store;
use shamir_tx::{MvccStore, RepoTxGate, StagingStore};

fn main() {
    let mut h = Harness::new("tx_overhead", env!("CARGO_MANIFEST_DIR"));

    // --- mvcc_set_versioned_no_snapshots/zero_overhead_write ----------------
    {
        let history: Arc<dyn Store> = Arc::new(InMemoryStore::new());
        let gate = Arc::new(RepoTxGate::fresh());
        let mvcc = Arc::new(MvccStore::new(history, gate));
        let counter = Cell::new(0u64);

        h.bench_batched_async(
            "mvcc_set_versioned_no_snapshots/zero_overhead_write",
            move || {
                let n = counter.get();
                counter.set(n.wrapping_add(1));
                let mvcc = Arc::clone(&mvcc);
                async move {
                    let key = Bytes::copy_from_slice(&n.to_be_bytes());
                    let value = Bytes::from_static(b"v");
                    (mvcc, key, value)
                }
            },
            move |(mvcc, key, value)| async move {
                mvcc.set_versioned(key, value).await.unwrap();
            },
        );
    }

    // --- mvcc_get_at_fast_path/read_at_high_snapshot -------------------------
    {
        let history: Arc<dyn Store> = Arc::new(InMemoryStore::new());
        let gate = Arc::new(RepoTxGate::fresh());
        let mvcc = Arc::new(MvccStore::new(history, gate));

        // Pre-populate synchronously via a throwaway current-thread runtime —
        // this setup runs once, at registration time, not in the timed loop.
        let setup_rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        setup_rt.block_on(async {
            for i in 0..1000u32 {
                mvcc.set_versioned(
                    Bytes::copy_from_slice(&i.to_be_bytes()),
                    Bytes::from_static(b"v"),
                )
                .await
                .unwrap();
            }
        });

        let counter = Cell::new(0u32);
        h.bench_async("mvcc_get_at_fast_path/read_at_high_snapshot", move || {
            let n = counter.get();
            counter.set(n.wrapping_add(1));
            let key = (n % 1000).to_be_bytes();
            let mvcc = Arc::clone(&mvcc);
            async move {
                let _ = mvcc.get_at(&key, u64::MAX).await.unwrap();
            }
        });
    }

    // --- staging_store/set_then_get -----------------------------------------
    {
        let base: Arc<dyn Store> = Arc::new(InMemoryStore::new());
        let counter = Cell::new(0u64);

        h.bench_batched_async(
            "staging_store/set_then_get",
            move || {
                let n = counter.get();
                counter.set(n.wrapping_add(1));
                let base = Arc::clone(&base);
                async move {
                    let key: shamir_storage::types::RecordKey =
                        Bytes::copy_from_slice(&n.to_be_bytes());
                    (base, key)
                }
            },
            move |(base, key)| async move {
                let key2 = key.clone();
                let mut staging = StagingStore::new(base);
                staging.set(key, Bytes::from_static(b"v"));
                let _ = staging.get(key2).await.unwrap();
            },
        );
    }

    // --- repo_tx_gate/assign_next_version ------------------------------------
    {
        let gate = RepoTxGate::fresh();
        h.bench("repo_tx_gate/assign_next_version", move || {
            black_box(gate.assign_next_version());
        });
    }

    h.run();
}
