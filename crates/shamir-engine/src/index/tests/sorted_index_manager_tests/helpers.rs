//! Shared test helpers for `SortedIndexManager` tests.

use std::sync::Arc;

use shamir_storage::storage_in_memory::InMemoryStore;
use shamir_storage::types::Store;
use shamir_types::core::interner::InternerKey;
use shamir_types::core::sort_codec;
use shamir_types::types::common::new_map;
use shamir_types::types::value::InnerValue;

use crate::index::sorted_index_manager::SortedIndexManager;

pub(super) async fn fresh_mgr() -> (Arc<dyn Store>, SortedIndexManager) {
    let info_store: Arc<dyn Store> = Arc::new(InMemoryStore::new());
    let mgr = SortedIndexManager::new(Arc::clone(&info_store))
        .await
        .unwrap();
    (info_store, mgr)
}

/// Build a Map record { field_key: Int(score) }.
pub(super) fn record_with_int(field_key: u64, score: i64) -> InnerValue {
    let mut m = new_map();
    m.insert(InternerKey::new(field_key), InnerValue::Int(score));
    InnerValue::Map(m)
}

pub(super) fn record_with_str(field_key: u64, s: &str) -> InnerValue {
    let mut m = new_map();
    m.insert(InternerKey::new(field_key), InnerValue::Str(s.to_string()));
    InnerValue::Map(m)
}

pub(super) fn enc_i64(v: i64) -> Vec<u8> {
    let mut b = Vec::new();
    sort_codec::encode_i64(&mut b, v);
    b
}

pub(super) fn enc_str(s: &str) -> Vec<u8> {
    let mut b = Vec::new();
    sort_codec::encode_str(&mut b, s);
    b
}
