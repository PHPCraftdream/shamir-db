use crate::subscriptions::SubscriptionRegistry;

#[test]
fn registry_insert_and_remove() {
    let reg = SubscriptionRegistry::new();
    let id = reg.next_id();
    assert_eq!(id, 1);
    let id2 = reg.next_id();
    assert_eq!(id2, 2);
}

#[test]
fn registry_count() {
    let reg = SubscriptionRegistry::new();
    assert_eq!(reg.count(), 0);
}

/// Regression for the `@fl`-review-found race in task #527's self-exit
/// slot-release fix: on a multi-thread runtime a fast-exiting bridge task's
/// RAII guard can call `remove(id)` BEFORE the handler gets around to
/// attaching the real `JoinHandle` (`attach_handle` runs strictly after
/// `tokio::spawn` returns). Forces that EXACT worst-case ordering
/// deterministically (no scheduling luck needed) and asserts no leak and no
/// resurrected entry.
#[tokio::test]
async fn reserve_pending_survives_remove_before_attach_handle_race() {
    let reg = SubscriptionRegistry::with_cap(4);
    reg.try_reserve().unwrap();
    let id = reg.next_id();

    // 1. Reserve the placeholder BEFORE "spawning" (mirrors
    //    subscribe_handler.rs's ordering).
    reg.reserve_pending(id);
    assert_eq!(reg.count(), 1, "pending slot counts as active");

    // 2. Simulate the bridge task's guard firing (self-exit) BEFORE the
    //    handler calls attach_handle — the worst-case race.
    assert!(
        reg.remove(id),
        "reserve_pending must have inserted a REAL entry, not merely bumped \
         the counter — otherwise remove() finds nothing and this assertion \
         fails exactly like the leak this test guards against"
    );
    assert_eq!(reg.count(), 0, "slot released — no leak from the race");

    // 3. attach_handle arrives late (the task is already gone). Must be a
    //    safe no-op: no panic, no resurrected entry, no double-decrement.
    let handle = tokio::spawn(async {});
    reg.attach_handle(id, handle);
    assert_eq!(
        reg.count(),
        0,
        "a late attach_handle on an already-removed id must not resurrect \
         the slot or change the counter"
    );
}
