//! S5c-X — shared fixtures for the write-domain tests.
//!
//! A `TableManager` built on TWO fresh in-memory stores and ONE shared
//! per-repo `RepoWriteDomain` — the shape production uses
//! (`TableManager::create_with_domain`), so every DDL entry point in these
//! tests exercises the real admit-once gate.

use std::sync::Arc;

use shamir_storage::storage_in_memory::InMemoryStore;
use shamir_storage::types::Store;

use crate::repo::RepoWriteDomain;
use crate::table::TableManager;

/// Yield budget: a task blocked on admission NEVER finishes; a free
/// mutation finishes long before this many yields (proven by
/// `negative_control_mutations_complete_without_held_permit`).
pub const SPIN_BUDGET: usize = 500;

pub async fn spin(n: usize) {
    for _ in 0..n {
        tokio::task::yield_now().await;
    }
}

/// Wait (bounded) for `task`, then unwrap the JoinHandle.
pub async fn wait<T>(task: tokio::task::JoinHandle<T>) -> T {
    for _ in 0..1_000_000 {
        if task.is_finished() {
            break;
        }
        tokio::task::yield_now().await;
    }
    task.await.expect("test task panicked")
}

/// A manager sharing `domain`, over fresh in-memory data + info stores.
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
