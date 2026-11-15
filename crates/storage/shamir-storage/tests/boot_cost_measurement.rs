//! BOOT-COST measurement (docs/dev-artifacts/perf/boot-cost.md).
//!
//! Plain test, prints measurements to stdout so a CI log on any OS shows
//! the numbers. No production code is touched: every counter and seam used
//! here is already public (`FJALL_PERSIST_SYNC_ALL_COUNT`,
//! `reset_persist_sync_all_count`).
//!
//! Section numbers refer to that document. Ignored by default (it asserts
//! nothing and only costs CI time); to measure, run it with nextest's
//! `--run-ignored only` and the `boot_cost` filter.
#![cfg(feature = "fjall")]

use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Instant;

use shamir_storage::core::types::{Repo, Store};
use shamir_storage::storage_fjall::{
    reset_persist_sync_all_count, FjallRepo, FJALL_PERSIST_SYNC_ALL_COUNT,
};
use shamir_storage::storage_membuffer::{MemBufferConfig, MemBufferStore};

/// The 4 repo-level stores every repo opens eagerly at boot (§1.2).
const REPO_LEVEL_STORES: [&str; 4] = ["__tx__", "__changelog__", "__interner__", "__seed_schema__"];

/// 4 boot-read catalogue tables × 3 per-table stores (§1.2).
fn table_store_names() -> Vec<String> {
    let mut names = Vec::new();
    for table in ["databases", "repositories", "tables", "groups"] {
        for prefix in ["__data__", "__info__", "__history__"] {
            names.push(format!("{prefix}{table}"));
        }
    }
    names
}

fn persist_count() -> u64 {
    FJALL_PERSIST_SYNC_ALL_COUNT.load(Ordering::SeqCst)
}

#[tokio::test]
#[ignore = "measurement: prints timings, asserts nothing (boot-cost.md section 7)"]
async fn boot_cost_of_one_fjall_open() {
    let dir = tempfile::tempdir().expect("tempdir");
    reset_persist_sync_all_count();

    // §1.1: Database open (manifest read + journal recovery).
    let t = Instant::now();
    let repo = FjallRepo::new(dir.path()).expect("fjall db open");
    println!("BOOT-COST db_open_ms={}", t.elapsed().as_millis());

    // §1.2: 16 sequential keyspace opens (4 repo-level + 12 catalogue).
    let mut stores: Vec<Arc<dyn Store>> = Vec::new();
    let names: Vec<String> = REPO_LEVEL_STORES
        .iter()
        .map(|s| s.to_string())
        .chain(table_store_names())
        .collect();
    for name in names {
        let t = Instant::now();
        let store = repo.store_get(&name).await.expect("keyspace open");
        println!(
            "BOOT-COST keyspace_open_ms={} store={name}",
            t.elapsed().as_millis()
        );
        stores.push(store);
    }
    println!(
        "BOOT-COST persist_count_after_open={} (expected 0: quiet open issues no SyncAll, §1.3)",
        persist_count()
    );

    // §4: idle MemBuffer-wrapped store — ≥3 flusher ticks must not persist.
    let buffered = MemBufferStore::new(
        stores.swap_remove(0),
        MemBufferConfig {
            flush_interval_ms: 200,
            ..MemBufferConfig::default()
        },
    );
    tokio::time::sleep(std::time::Duration::from_millis(1600)).await;
    println!(
        "BOOT-COST idle_ticks_ge=7 persist_count_after_idle={} (expected 0, §4)",
        persist_count()
    );

    // §2: per-call SyncAll cost.
    let t = Instant::now();
    buffered.flush().await.expect("flush");
    println!(
        "BOOT-COST persist_sync_all_ms={} persist_count_after_flush={}",
        t.elapsed().as_millis(),
        persist_count()
    );

    // §2: close (Drop) wall time — lock release + journal shutdown + delete.
    drop(buffered);
    drop(repo);
    drop(stores);
    let t = Instant::now();
    drop(dir);
    println!("BOOT-COST close_delete_ms={}", t.elapsed().as_millis());
}
