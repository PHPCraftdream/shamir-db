//! Unit tests for [`crate::byte_budget::ByteBudget`] — RI-15.

use std::sync::Arc;
use std::time::Duration;

use tokio::sync::Barrier;

use crate::byte_budget::ByteBudget;

/// Acquiring under budget succeeds immediately (no waiting).
#[tokio::test]
async fn acquire_under_budget_succeeds_immediately() {
    let budget = ByteBudget::new(Some(100));
    let fut = budget.acquire(40);
    // Must resolve without needing a second poll driven by a notify —
    // wrap in a short timeout so a bug that makes this block forever
    // fails fast instead of hanging the suite.
    let guard = tokio::time::timeout(Duration::from_millis(200), fut)
        .await
        .expect("acquire under budget must not block");
    assert_eq!(budget.used(), 40);
    drop(guard);
    assert_eq!(budget.used(), 0, "drop must release the reservation");
}

/// Unbounded budget (`cap = None`) never blocks and never accounts bytes.
#[tokio::test]
async fn unbounded_budget_never_blocks_and_tracks_nothing() {
    let budget = ByteBudget::unbounded();
    let g1 = tokio::time::timeout(Duration::from_millis(200), budget.acquire(usize::MAX / 2))
        .await
        .expect("unbounded acquire must not block");
    let g2 = tokio::time::timeout(Duration::from_millis(200), budget.acquire(usize::MAX / 2))
        .await
        .expect("unbounded acquire must not block even while another guard is held");
    assert_eq!(budget.used(), 0, "unbounded budget never accounts bytes");
    drop(g1);
    drop(g2);
}

/// Acquiring over budget blocks until a release frees enough bytes.
#[tokio::test]
async fn acquire_over_budget_blocks_until_release() {
    let budget = ByteBudget::new(Some(100));
    let g1 = budget.acquire(80).await;
    assert_eq!(budget.used(), 80);

    // A second acquire for 50 bytes cannot fit (80 + 50 > 100) — must block.
    let budget2 = budget.clone();
    let waiter = tokio::spawn(async move { budget2.acquire(50).await });

    // Give the waiter task a chance to actually start polling and park.
    tokio::task::yield_now().await;
    tokio::time::sleep(Duration::from_millis(20)).await;
    assert!(
        !waiter.is_finished(),
        "acquire(50) must still be blocked while 80/100 bytes are held"
    );

    // Release the first guard — frees 80 bytes, room for the waiter's 50.
    drop(g1);

    let g2 = tokio::time::timeout(Duration::from_millis(500), waiter)
        .await
        .expect("waiter must wake up after release")
        .expect("waiter task must not panic");
    assert_eq!(
        budget.used(),
        50,
        "only the waiter's 50 bytes remain reserved"
    );
    drop(g2);
    assert_eq!(budget.used(), 0);
}

/// Multiple waiters: each one makes progress (at-least-one-progress
/// guarantee — see `byte_budget.rs` module docs on fairness). This test
/// documents the guarantee this implementation actually gives: every
/// waiter eventually acquires as bytes are released one at a time, none
/// starve forever. Each waiter task drops its own guard immediately after
/// acquiring, so releases cascade until all three have made progress.
#[tokio::test]
async fn multiple_waiters_all_eventually_acquire() {
    let budget = ByteBudget::new(Some(10));
    let g0 = budget.acquire(10).await;
    assert_eq!(budget.used(), 10);

    let barrier = Arc::new(Barrier::new(4));
    let mut waiters = Vec::new();
    for _ in 0..3 {
        let b = budget.clone();
        let bar = Arc::clone(&barrier);
        waiters.push(tokio::spawn(async move {
            bar.wait().await;
            let guard = b.acquire(10).await;
            // Acquired — immediately release so the next waiter can proceed.
            drop(guard);
        }));
    }
    barrier.wait().await;
    // Let all three waiters actually park on the exhausted budget.
    tokio::time::sleep(Duration::from_millis(20)).await;
    for w in &waiters {
        assert!(
            !w.is_finished(),
            "waiter must be parked while budget is full"
        );
    }

    // Release the initial guard — this must cascade: waiter 1 acquires and
    // immediately drops, waking waiter 2, and so on, until all three
    // waiter tasks have completed.
    drop(g0);

    for (i, w) in waiters.into_iter().enumerate() {
        tokio::time::timeout(Duration::from_millis(500), w)
            .await
            .unwrap_or_else(|_| panic!("waiter {i} must eventually acquire and complete"))
            .expect("waiter task must not panic");
    }
    assert_eq!(
        budget.used(),
        0,
        "every waiter released its guard; budget must be fully drained"
    );
}
