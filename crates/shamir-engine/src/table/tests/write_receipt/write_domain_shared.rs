//! S5c-X — shared fixtures and read-only snapshots for write-domain tests.

use std::sync::Arc;

use futures::StreamExt;
use shamir_storage::storage_in_memory::InMemoryStore;
use shamir_storage::types::{RecordKey, Store};

use crate::repo::RepoWriteDomain;
use crate::table::TableManager;

pub async fn store_snapshot(store: &dyn Store) -> Vec<(RecordKey, bytes::Bytes)> {
    let mut rows = Vec::new();
    let mut stream = store.iter_stream(256);
    while let Some(batch) = stream.next().await {
        rows.extend(batch.expect("store snapshot batch"));
    }
    rows
}

pub async fn make_manager(name: &str) -> (Arc<RepoWriteDomain>, TableManager) {
    let domain = RepoWriteDomain::new(name);
    let data: Arc<dyn Store> = Arc::new(InMemoryStore::new());
    let info: Arc<dyn Store> = Arc::new(InMemoryStore::new());
    let mgr = TableManager::create_with_domain(
        name.to_string(),
        Arc::clone(&data),
        Arc::clone(&info),
        Arc::clone(&domain),
    )
    .await
    .expect("table manager with shared domain");
    (domain, mgr)
}

pub async fn make_manager_on(domain: &Arc<RepoWriteDomain>, name: &str) -> TableManager {
    let data: Arc<dyn Store> = Arc::new(InMemoryStore::new());
    let info: Arc<dyn Store> = Arc::new(InMemoryStore::new());
    TableManager::create_with_domain(
        name.to_string(),
        Arc::clone(&data),
        Arc::clone(&info),
        Arc::clone(domain),
    )
    .await
    .expect("table manager on caller domain")
}
