//! Bench for #1108: O(N²) scaling in `released_unique_keys_in_tx` when
//! `tx.index_write_set` genuinely GROWS across rows in the same tx.
//!
//! `p1099_touched_probe.rs` covers `update_tx_bytes`'s OTHER O(N) walk
//! (`touched_records_in_tx`, fixed by #1099) but its workload leaves the
//! unique column's VALUE unchanged across rows, so `tx.index_write_set`
//! stays empty for the unique family the whole run — `released_unique_keys_in_tx`
//! never sees more than an empty suffix there and its own cost is invisible.
//!
//! THIS bench changes the unique-indexed `email` field's value on EVERY row
//! of a single transaction — the natural worst case for
//! `released_unique_keys_in_tx`: each row's `plan_record_updated_unique`
//! emits a `RemovePosting` (old email) + `SetPosting` (new email), so
//! `tx.index_write_set` grows by (at least) 2 unique-family entries per row.
//! Drives `TableManager::update_tx` directly (the same per-row call
//! `released_unique_keys_in_tx` sees inside `update_tx_bytes` on the wire
//! UPDATE path — see that function's own `has_unique_indexes()`-gated call
//! site) so each row's new value can be a genuinely distinct string, which
//! the batch query-builder's single shared `set` map cannot express.
//!
//! Before #1108's fix: `released_unique_keys_in_tx` fully re-walked
//! `tx.index_write_set` from scratch on every row call, so the total work
//! across N rows was `Theta(N^2)`.
//!
//! After #1108's fix: an incremental `ReleasedUniqueCache` (in `TxContext`)
//! folds in only the NEW suffix of `tx.index_write_set` since the last call,
//! so the total work across N rows is `Theta(N)` — each row's call costs
//! O(1) amortized (a handful of new ops), not O(row_index).

use std::sync::Arc;

use bench_scale_tool::Harness;
use shamir_engine::repo::{BoxRepo, RepoInstance};
use shamir_engine::table::TableConfig;
use shamir_storage::storage_in_memory::InMemoryRepo;
use shamir_tx::IsolationLevel;
use shamir_types::core::interner::{InternerKey, TouchInd};
use shamir_types::types::common::new_map_wc;
use shamir_types::types::record_id::RecordId;
use shamir_types::types::value::InnerValue;

fn record_with_two_str(key1: u64, val1: &str, key2: u64, val2: &str) -> InnerValue {
    let mut m = new_map_wc(2);
    m.insert(InternerKey::new(key1), InnerValue::Str(val1.into()));
    m.insert(InternerKey::new(key2), InnerValue::Str(val2.into()));
    InnerValue::Map(m)
}

fn main() {
    let mut h = Harness::new("p1108_released_unique_growth", env!("CARGO_MANIFEST_DIR"));

    // Benchmark: ONE transaction, N sequential `update_tx` calls on a table
    // WITH a unique index, where EVERY row's `email` value changes to a
    // brand-new value — so tx.index_write_set grows by a RemovePosting +
    // SetPosting pair (unique family) on every row.
    //
    // Before fix: O(N²) scaling because `released_unique_keys_in_tx`
    // re-walks the whole (growing) `tx.index_write_set` from scratch on
    // every row.
    // After fix: near-linear scaling — the incremental cache folds in only
    // the new suffix per call.
    for &n in &[400usize, 800usize, 1600usize] {
        h.bench_batched_async(
            &format!("tx_update_unique_value_change/n_{n}"),
            move || async move {
                // FRESH instance per iteration (no shared state across bench iterations)
                let repo = Arc::new(InMemoryRepo::new());
                let instance =
                    RepoInstance::new("bench".into(), BoxRepo::InMemory(repo), Vec::new());
                instance.add_table(TableConfig::new("users".to_string()));

                let tbl = instance.get_table("users").await.unwrap();
                tbl.create_unique_index("uniq_email", &["email"])
                    .await
                    .unwrap();

                let interner = tbl.interner().get().await.unwrap();
                let email_field = match interner.touch_ind("email").unwrap() {
                    TouchInd::Exists(k) | TouchInd::New(k) => k.id(),
                };
                let name_field = match interner.touch_ind("name").unwrap() {
                    TouchInd::Exists(k) | TouchInd::New(k) => k.id(),
                };

                // Insert N rows (setup, not timed) — each with its own
                // unique starting email.
                let mut ids: Vec<RecordId> = Vec::with_capacity(n);
                for i in 0..n {
                    let rid = tbl
                        .insert(&record_with_two_str(
                            email_field,
                            &format!("user_{i}@example.com"),
                            name_field,
                            &format!("name_{i}"),
                        ))
                        .await
                        .unwrap();
                    ids.push(rid);
                }

                (instance, tbl, ids, email_field, name_field)
            },
            move |(instance, tbl, ids, email_field, name_field)| async move {
                // ONE transaction, N sequential `update_tx` calls (timed) —
                // every row's email is rewritten to a FRESH unique value, so
                // tx.index_write_set grows by 2 unique-family ops per row.
                let (mut tx, _guard) = instance.begin_tx(IsolationLevel::Snapshot).await.unwrap();
                for (i, rid) in ids.iter().enumerate() {
                    let new_val = record_with_two_str(
                        email_field,
                        &format!("user_{i}_v2@example.com"),
                        name_field,
                        &format!("name_{i}"),
                    );
                    tbl.update_tx(*rid, &new_val, Some(&mut tx)).await.unwrap();
                }
                let _ = instance.commit_tx(tx).await.unwrap();
            },
        );
    }

    h.run();
}
