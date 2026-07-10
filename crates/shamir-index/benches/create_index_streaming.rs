//! §2.4 bench — `IndexManager::create_index` incremental-batch scaling.
//!
//! Before the §2.4 fix, `create_index` accumulated the ENTIRE table as
//! decoded `InnerValue`s into one `Vec` before building the index — O(table)
//! peak memory. After the fix, it processes each stream batch independently
//! (decode → index → flush → drop), bounding peak memory to O(batch).
//!
//! This bench measures wall-clock time at increasing table sizes (50k, 200k).
//! The fix is primarily a memory optimization (O(table) → O(batch)), so the
//! wall-clock signal is secondary — the key qualitative property is that
//! memory stays flat as table size grows (verifiable via the parameterized
//! scaling: if memory were O(table), the larger size would be measurably
//! slower due to allocation pressure + cache misses; with O(batch) the
//! per-record cost is constant).
//!
//! Run: `cargo bench -p shamir-index --bench create_index_streaming`
//! (uses the isolated bench target dir per the workspace convention).

use std::hint::black_box;
use std::sync::Arc;

use bench_scale_tool::Harness;
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

/// Populate a data_store with `n` records, each with a unique indexed field
/// value, then return a fresh `IndexManager` (no index yet) and the store.
async fn build_store_with_records(n: usize) -> (Arc<dyn Store>, IndexManager) {
    let data_store = Arc::new(InMemoryStore::new()) as Arc<dyn Store>;
    let info_store = Arc::new(InMemoryStore::new()) as Arc<dyn Store>;

    // Batched insert with unique string values.
    let batch_ts = RecordId::now_micros();
    for i in 0..n {
        let record_id = RecordId::from_ts_seq(batch_ts, i as u32);
        let mut map = new_map();
        map.insert(
            InternerKey::new(FIELD_ID),
            InnerValue::Str(format!("val_{i}")),
        );
        let value = InnerValue::Map(map);
        let bytes = value.to_bytes().unwrap();
        data_store
            .set(record_id.to_bytes().into(), bytes)
            .await
            .unwrap();
    }

    let manager = IndexManager::new(Arc::clone(&data_store), Arc::clone(&info_store))
        .await
        .unwrap();
    (data_store, manager)
}

fn rt() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
}

fn main() {
    let mut h = Harness::new("create_index_streaming", env!("CARGO_MANIFEST_DIR"));
    let runtime: &'static tokio::runtime::Runtime = Box::leak(Box::new(rt()));

    for &n_rows in &[50_000usize, 200_000] {
        // Setup: populate data_store ONCE (plan 1). The timed closure builds
        // the index (the §2.4 hot path).
        let (_data_store, manager) = runtime.block_on(build_store_with_records(n_rows));
        let manager_ref: &'static IndexManager = Box::leak(Box::new(manager));

        let id = format!("create_index/{n_rows}_rows");
        let name_counter = n_rows; // capture for unique index names
        h.bench(&id, move || {
            // Each iteration creates a NEW index on the same table (unique
            // name per call to avoid "already exists" no-op).
            let idx_name = name_counter as u64 + 200_000;
            let def = IndexDefinition::new(idx_name, vec![IndexInfoItem::new(vec![FIELD_ID])]);
            runtime.block_on(manager_ref.create_index(def)).unwrap();
            // Drop the index so the next iteration can re-create cleanly.
            runtime.block_on(manager_ref.drop_index(idx_name)).unwrap();
            black_box(());
        });
    }

    h.run();
}
