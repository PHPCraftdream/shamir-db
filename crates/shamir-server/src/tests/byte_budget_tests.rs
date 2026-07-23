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

// ---------------------------------------------------------------------------
// CR-B2 — `ByteBudgetGuard::shrink_to` / `grow_unchecked` (upfront-reserve
// support: reserve a pessimistic estimate, narrow to the actual size once
// known).
// ---------------------------------------------------------------------------

/// Shrinking a guard releases exactly the delta back to the budget and wakes
/// a parked waiter that can now fit.
#[tokio::test]
async fn shrink_to_releases_delta_and_wakes_waiter() {
    let budget = ByteBudget::new(Some(100));
    // Reserve a pessimistic 80-byte estimate.
    let mut guard = budget.acquire(80).await;
    assert_eq!(budget.used(), 80);

    // A waiter for 50 bytes cannot fit yet (80 + 50 > 100).
    let budget2 = budget.clone();
    let waiter = tokio::spawn(async move { budget2.acquire(50).await });
    tokio::task::yield_now().await;
    tokio::time::sleep(Duration::from_millis(20)).await;
    assert!(
        !waiter.is_finished(),
        "waiter must be blocked while the estimate still reserves 80 bytes"
    );

    // Shrink the guard down to the ACTUAL size (30 bytes) — frees 50 bytes,
    // which is exactly enough room for the waiter.
    guard.shrink_to(30);
    assert_eq!(
        budget.used(),
        30,
        "shrink must reduce budget.used() by the released delta"
    );

    let waiter_guard = tokio::time::timeout(Duration::from_millis(500), waiter)
        .await
        .expect("shrink must wake the parked waiter")
        .expect("waiter task must not panic");
    assert_eq!(budget.used(), 30 + 50);

    // Dropping the shrunk guard must release only the REMAINING (30) bytes,
    // not the original 80 — otherwise this double-releases the already
    // shrunk-away delta.
    drop(guard);
    assert_eq!(
        budget.used(),
        50,
        "drop after shrink must release only the remaining 30 bytes, not the original 80"
    );
    drop(waiter_guard);
    assert_eq!(budget.used(), 0);
}

/// A no-op shrink (`new_bytes >= self.bytes`) must not under- or
/// over-release: `Drop` still releases the full original amount.
#[tokio::test]
async fn shrink_to_with_new_size_gte_old_is_a_noop() {
    let budget = ByteBudget::new(Some(100));
    let mut guard = budget.acquire(40).await;
    assert_eq!(budget.used(), 40);

    // Equal size: no-op.
    guard.shrink_to(40);
    assert_eq!(
        budget.used(),
        40,
        "shrink_to(same size) must not release anything"
    );

    // Larger size via shrink_to must also be treated as a no-op (shrink-only
    // contract) — growth only happens through the explicit
    // `grow_unchecked` path.
    guard.shrink_to(1_000);
    assert_eq!(
        budget.used(),
        40,
        "shrink_to(new_bytes >= self.bytes) must be a no-op, never grow"
    );

    drop(guard);
    assert_eq!(
        budget.used(),
        0,
        "drop after a no-op shrink must release the original (unshrunk) amount"
    );
}

/// Shrinking an unbounded budget's guard (`inner == None`) is a safe no-op.
#[tokio::test]
async fn shrink_to_on_unbounded_guard_is_a_noop() {
    let budget = ByteBudget::unbounded();
    let mut guard = budget.acquire(1_000_000).await;
    assert_eq!(budget.used(), 0);
    guard.shrink_to(10);
    assert_eq!(budget.used(), 0);
    drop(guard);
    assert_eq!(budget.used(), 0);
}

/// `grow_unchecked` adds the shortfall unconditionally (no waiting, no cap
/// check) and bumps the guard's tracked size so `Drop` releases the correct
/// (grown) total — covers the "final serialized envelope is a few bytes
/// larger than the estimate" overshoot case.
#[tokio::test]
async fn grow_unchecked_adds_shortfall_and_drop_releases_grown_total() {
    let budget = ByteBudget::new(Some(100));
    let mut guard = budget.acquire(30).await;
    assert_eq!(budget.used(), 30);

    // Even past the cap — grow_unchecked never blocks or checks room.
    guard.grow_unchecked(90);
    assert_eq!(
        budget.used(),
        120,
        "grow_unchecked must add the shortfall unconditionally, even past the cap"
    );

    drop(guard);
    assert_eq!(
        budget.used(),
        0,
        "drop after grow_unchecked must release the FULL grown total (120), not the original 30"
    );
}

/// `grow_unchecked` on an unbounded guard is a safe no-op.
#[tokio::test]
async fn grow_unchecked_on_unbounded_guard_is_a_noop() {
    let budget = ByteBudget::unbounded();
    let mut guard = budget.acquire(10).await;
    guard.grow_unchecked(1_000);
    assert_eq!(budget.used(), 0);
    drop(guard);
    assert_eq!(budget.used(), 0);
}
