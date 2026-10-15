//! S5c-X — `begin_write_barrier` is now a COMPOUND acquisition: repository
//! admission FIRST, then the per-table raise→drain→lock sequence, with the
//! repo permit retained inside the returned [`WriteBarrierGuard`] (released
//! only when the guard drops).
//!
//! Fails on pre-S5c-X code: `begin_write_barrier` returned a bare tuple,
//! never consulted the repo domain, and never retained a permit — every
//! "blocked" assertion below would trip immediately.

use std::sync::Arc;

use crate::index::write_barrier_flags::REGULAR_INDEX_CREATE;
use crate::table::tests::write_receipt::write_domain_shared::{
    make_manager, spin, wait, SPIN_BUDGET,
};

/// With another task holding repo admission, `begin_write_barrier` blocks
/// BEFORE touching `ddl_admission` (repo permit first in the lock order).
#[tokio::test]
async fn barrier_blocks_while_repo_permit_held() {
    let (domain, mgr) = make_manager("barrier-blocked").await;
    let permit = domain.admit().await.expect("admit");
    let m = mgr.clone();
    let task = tokio::spawn(async move { m.begin_write_barrier(REGULAR_INDEX_CREATE).await });
    spin(SPIN_BUDGET).await;
    assert!(
        !task.is_finished(),
        "begin_write_barrier completed while repo admission was held"
    );
    drop(permit);
    let res = wait(task).await;
    assert!(
        res.is_ok(),
        "barrier must succeed after the permit is released"
    );
}

/// Negative control: with no holder, the barrier returns within the spin
/// budget (the blocked-* assertions mean something).
#[tokio::test]
async fn barrier_returns_without_holder() {
    let (_domain, mgr) = make_manager("barrier-free").await;
    let m = mgr.clone();
    let task = tokio::spawn(async move { m.begin_write_barrier(REGULAR_INDEX_CREATE).await });
    spin(SPIN_BUDGET).await;
    assert!(
        task.is_finished(),
        "free barrier did not finish inside the spin budget"
    );
    let (guard, _lock) = task.await.unwrap().unwrap();
    drop(guard);
}

/// The returned compound guard RETAINS repo admission: a second `admit()`
/// stays pending while the guard lives and completes when it drops.
#[tokio::test]
async fn compound_guard_retains_admission_until_drop() {
    let (domain, mgr) = make_manager("barrier-retains").await;
    let (guard, _lock) = mgr
        .begin_write_barrier(REGULAR_INDEX_CREATE)
        .await
        .expect("barrier");
    let d2 = Arc::clone(&domain);
    let task = tokio::spawn(async move { d2.admit().await });
    spin(SPIN_BUDGET).await;
    assert!(
        !task.is_finished(),
        "second admit() completed while the barrier guard still holds the permit"
    );
    drop(guard);
    let res = wait(task).await;
    assert!(res.is_ok(), "admit must succeed once the guard drops");
    // Keep `_lock` alive to the end — drop order mirrors DDL call sites
    // (lock released before the guard clears the bit and releases repo
    // admission).
    let _ = &mgr;
}
