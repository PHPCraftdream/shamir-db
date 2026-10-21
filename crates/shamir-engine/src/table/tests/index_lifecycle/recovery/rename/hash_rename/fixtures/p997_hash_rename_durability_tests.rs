//! #997 — crash-recovery for compound regular/unique RENAME INDEX.
//!
//! Mirrors #988's `p03b_index2_drop_durability_tests.rs` and #962's
//! `p05b_sorted_rename_durability_tests.rs` for the base_index hash
//! (regular + unique) RENAME families. Both paths are a drop+rebuild (the
//! hash physical key embeds `name_interned` into h1/h2, so postings cannot
//! be rekeyed). The durable tombstone (`HashRenameTombstone`) carries the
//! resolved string names + paths so recovery can rebuild from nothing.
//!
//! Tests cover, per family:
//! - Every row of the crash-state matrix documented in
//!   `TableManager::recover_hash_renames`'s doc comment.
//! - Idempotence: a double restart after recovery is a no-op.
//! - The unique-duplicate-during-recovery hazard (should be impossible
//!   because recovery runs before any writer can access the table, but
//!   verified by test).
//! - A normal (no-crash) rename regression smoke test.
//! - A live rename crash simulation via the `rename_mid_pause_hook`.

use std::sync::Arc;

use shamir_storage::storage_in_memory::InMemoryStore;
use shamir_storage::types::Store;
use shamir_types::core::interner::{InternerKey, TouchInd};
use shamir_types::types::common::new_map_wc;
use shamir_types::types::value::InnerValue;

use crate::index::index_manager::HashRenameTombstone;
use crate::table::TableManager;
use shamir_types::types::record_id::RecordId;

// ============================================================================
// Helpers
// ============================================================================

pub(crate) fn make_stores() -> (Arc<dyn Store>, Arc<dyn Store>) {
    let data: Arc<dyn Store> = Arc::new(InMemoryStore::new());
    let info: Arc<dyn Store> = Arc::new(InMemoryStore::new());
    (data, info)
}

pub(crate) async fn key_id(tbl: &TableManager, name: &str) -> u64 {
    let interner = tbl.interner.get().await.unwrap();
    match interner.touch_ind(name).unwrap() {
        TouchInd::Exists(k) | TouchInd::New(k) => k.id(),
    }
}

pub(crate) fn record_with_str(key: u64, val: &str) -> InnerValue {
    let mut m = new_map_wc(1);
    m.insert(InternerKey::new(key), InnerValue::Str(val.into()));
    InnerValue::Map(m)
}

/// Seed a table with a regular index + data, persist the interner, then
/// drop the manager. Returns (data_store, info_store, old_name, new_name).
/// The index is NOT yet renamed — the caller seeds the crash state.
pub(crate) async fn seed_table_with_regular_index(
    index_name: &str,
    values: &[&str],
) -> (Arc<dyn Store>, Arc<dyn Store>) {
    let (data_store, info_store) = make_stores();
    let mgr = TableManager::create(
        "people".into(),
        Arc::clone(&data_store),
        Arc::clone(&info_store),
    )
    .await
    .unwrap();
    let email_field = key_id(&mgr, "email").await;
    for &val in values {
        mgr.insert(&record_with_str(email_field, val))
            .await
            .unwrap();
    }
    mgr.interner.persist().await.unwrap();
    mgr.create_index(index_name, &["email"]).await.unwrap();
    drop(mgr);
    (data_store, info_store)
}

/// Seed a table with a unique index + data, persist the interner, then
/// drop the manager. Returns (data_store, info_store).
pub(crate) async fn seed_table_with_unique_index(
    index_name: &str,
    values: &[&str],
) -> (Arc<dyn Store>, Arc<dyn Store>) {
    let (data_store, info_store) = make_stores();
    let mgr = TableManager::create(
        "people".into(),
        Arc::clone(&data_store),
        Arc::clone(&info_store),
    )
    .await
    .unwrap();
    let email_field = key_id(&mgr, "email").await;
    for &val in values {
        mgr.insert(&record_with_str(email_field, val))
            .await
            .unwrap();
    }
    mgr.interner.persist().await.unwrap();
    mgr.create_unique_index(index_name, &["email"])
        .await
        .unwrap();
    drop(mgr);
    (data_store, info_store)
}

/// Write a regular rename tombstone directly into info_store.
pub(crate) async fn seed_regular_rename_tombstone(info_store: &Arc<dyn Store>, entry: &HashRenameTombstone) {
    let key = RecordId::system("idx_ren").to_bytes();
    let bytes = bincode::serialize(&vec![entry.clone()]).unwrap();
    info_store.set(key.into(), bytes.into()).await.unwrap();
}

/// Write a unique rename tombstone directly into info_store.
pub(crate) async fn seed_unique_rename_tombstone(info_store: &Arc<dyn Store>, entry: &HashRenameTombstone) {
    let key = RecordId::system("uidx_ren").to_bytes();
    let bytes = bincode::serialize(&vec![entry.clone()]).unwrap();
    info_store.set(key.into(), bytes.into()).await.unwrap();
}

/// Read back the persisted regular rename tombstone.
pub(crate) async fn load_regular_rename_tombstone(info_store: &Arc<dyn Store>) -> Vec<HashRenameTombstone> {
    let key = RecordId::system("idx_ren").to_bytes();
    match info_store.get(key.into()).await {
        Ok(bytes) if bytes.is_empty() => Vec::new(),
        Ok(bytes) => bincode::deserialize(&bytes).unwrap(),
        Err(_) => Vec::new(),
    }
}

/// Read back the persisted unique rename tombstone.
pub(crate) async fn load_unique_rename_tombstone(info_store: &Arc<dyn Store>) -> Vec<HashRenameTombstone> {
    let key = RecordId::system("uidx_ren").to_bytes();
    match info_store.get(key.into()).await {
        Ok(bytes) if bytes.is_empty() => Vec::new(),
        Ok(bytes) => bincode::deserialize(&bytes).unwrap(),
        Err(_) => Vec::new(),
    }
}

pub(crate) fn rename_entry(old: &str, new: &str) -> HashRenameTombstone {
    HashRenameTombstone {
        old_name: old.into(),
        new_name: new.into(),
        paths: vec!["email".into()],
        op_id: None,
    }
}

// ============================================================================
// REGULAR: crash-state matrix
// ============================================================================
