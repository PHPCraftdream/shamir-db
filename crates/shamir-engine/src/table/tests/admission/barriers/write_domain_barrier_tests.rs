//! Repository admission is retained by the compound write barrier.

use std::sync::Arc;

use crate::index::write_barrier_flags::REGULAR_INDEX_CREATE;
use super::super::domain::write_domain_shared::make_manager;

#[tokio::test]
async fn barrier_blocks_while_repo_permit_held() {
    let (domain, mgr) = make_manager("barrier-blocked").await;
    let permit = domain.admit().await.expect("admit");
    let mut barrier = Box::pin(mgr.begin_write_barrier(REGULAR_INDEX_CREATE));
    assert!(futures::poll!(barrier.as_mut()).is_pending());
    drop(permit);
    let (guard, _lock) = barrier.await.expect("barrier after release");
    drop(guard);
}

#[tokio::test]
async fn barrier_returns_without_holder() {
    let (_domain, mgr) = make_manager("barrier-free").await;
    let (guard, _lock) = mgr
        .begin_write_barrier(REGULAR_INDEX_CREATE)
        .await
        .expect("free barrier");
    drop(guard);
}

#[tokio::test]
async fn compound_guard_retains_admission_until_drop() {
    let (domain, mgr) = make_manager("barrier-retains").await;
    let (guard, _lock) = mgr
        .begin_write_barrier(REGULAR_INDEX_CREATE)
        .await
        .expect("barrier");
    let second_domain = Arc::clone(&domain);
    let mut second = Box::pin(second_domain.admit());
    assert!(futures::poll!(second.as_mut()).is_pending());
    drop(guard);
    second.await.expect("admission after compound guard drop");
}
