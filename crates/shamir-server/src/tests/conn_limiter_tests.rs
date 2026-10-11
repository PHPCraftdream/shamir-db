use crate::conn_limiter::ConnLimiter;

#[test]
fn cap_zero_is_unlimited() {
    let l = ConnLimiter::new(0);
    let _g1 = l.try_acquire().expect("first acquire");
    let _g2 = l.try_acquire().expect("second acquire");
    let _g3 = l.try_acquire().expect("third acquire");
    // Active stays 0 because guards are no-ops.
    assert_eq!(l.active(), 0);
}

#[test]
fn cap_3_allows_3_then_rejects() {
    let l = ConnLimiter::new(3);
    let g1 = l.try_acquire().expect("1");
    let g2 = l.try_acquire().expect("2");
    let g3 = l.try_acquire().expect("3");
    assert_eq!(l.active(), 3);
    assert!(l.try_acquire().is_none(), "4th must be rejected");

    // Drop one — next acquire succeeds.
    drop(g1);
    assert_eq!(l.active(), 2);
    let _g4 = l.try_acquire().expect("after release");
    assert_eq!(l.active(), 3);
    let _ = (g2, g3);
}

#[test]
fn drop_releases_slot() {
    let l = ConnLimiter::new(1);
    {
        let _g = l.try_acquire().expect("first");
        assert_eq!(l.active(), 1);
        assert!(l.try_acquire().is_none(), "cap reached");
    }
    // Out of scope — slot freed.
    assert_eq!(l.active(), 0);
    let _g = l.try_acquire().expect("after drop");
}

#[test]
fn limiter_clones_share_state() {
    let l1 = ConnLimiter::new(2);
    let l2 = l1.clone();
    let _g1 = l1.try_acquire().expect("from l1");
    assert_eq!(l2.active(), 1, "l2 sees l1's acquire");
    let _g2 = l2.try_acquire().expect("from l2");
    assert!(l1.try_acquire().is_none(), "l1 sees l2's acquire");
}

// ---------------------------------------------------------------------------
// Live-connection tracking for graceful shutdown.
// ---------------------------------------------------------------------------

#[test]
fn live_counts_guards_even_when_the_cap_is_zero() {
    let l = ConnLimiter::new(0);
    assert_eq!(l.live(), 0);
    let g1 = l.try_acquire().expect("unlimited");
    let g2 = l.try_acquire().expect("unlimited");
    assert_eq!(l.live(), 2);
    assert_eq!(l.active(), 0, "the cap counter keeps its old semantics");
    drop(g1);
    assert_eq!(l.live(), 1);
    drop(g2);
    assert_eq!(l.live(), 0);
}

#[test]
fn live_ignores_refused_acquires() {
    let l = ConnLimiter::new(1);
    let _g = l.try_acquire().expect("first fits");
    assert!(l.try_acquire().is_none(), "second is over the cap");
    assert_eq!(l.live(), 1);
}

#[tokio::test]
async fn wait_idle_returns_at_once_without_connections() {
    let l = ConnLimiter::new(4);
    tokio::time::timeout(std::time::Duration::from_secs(5), l.wait_idle())
        .await
        .expect("idle limiter must not block");
}

#[tokio::test]
async fn wait_idle_resolves_only_after_the_last_guard_drops() {
    let l = ConnLimiter::new(0);
    let g1 = l.try_acquire().expect("unlimited");
    let g2 = l.try_acquire().expect("unlimited");
    let waiter = {
        let l = l.clone();
        tokio::spawn(async move { l.wait_idle().await })
    };
    tokio::task::yield_now().await;
    drop(g1);
    tokio::task::yield_now().await;
    assert!(!waiter.is_finished(), "one connection is still alive");
    drop(g2);
    tokio::time::timeout(std::time::Duration::from_secs(5), waiter)
        .await
        .expect("wait_idle must resolve after the last guard")
        .expect("waiter task");
}
