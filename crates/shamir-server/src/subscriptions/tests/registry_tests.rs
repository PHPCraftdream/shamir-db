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
