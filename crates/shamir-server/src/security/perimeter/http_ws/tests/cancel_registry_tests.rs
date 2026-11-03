use crate::security::tokens::SessionCanceller;
use shamir_connect::server::token::TokenId;
use std::sync::{Arc, Barrier};
use tokio_util::sync::CancellationToken;

use super::super::cancel_registry::TokenWsCancelRegistry;

#[test]
fn registrations_share_revocation_and_last_drop_removes_entry() {
    let registry = Arc::new(TokenWsCancelRegistry::default());
    let id = TokenId([7; 16]);
    let first = registry.register(id);
    let second = registry.register(id);
    let first_token = first.token();
    let second_token = second.token();
    registry.cancel_token(&id);
    assert!(first_token.is_cancelled());
    assert!(second_token.is_cancelled());
    drop(first);
    assert!(second_token.is_cancelled());
    drop(second);
    let fresh = registry.register(id).token();
    assert!(!fresh.is_cancelled());
}

#[test]
fn register_then_recheck_observes_revoke_between_steps() {
    let registry = Arc::new(TokenWsCancelRegistry::default());
    let id = TokenId([4; 16]);
    let registration = registry.register(id);
    let connection_cancel = registration.token();
    registry.cancel_token(&id);
    assert!(connection_cancel.is_cancelled());
}

#[test]
fn concurrent_final_drop_and_register_preserves_the_new_registration() {
    let registry = Arc::new(TokenWsCancelRegistry::default());
    let id = TokenId([11; 16]);
    let old = registry.register(id);
    let barrier = Arc::new(Barrier::new(2));
    let thread_registry = Arc::clone(&registry);
    let thread_barrier = Arc::clone(&barrier);
    let join = std::thread::spawn(move || {
        thread_barrier.wait();
        thread_registry.register(id)
    });
    barrier.wait();
    drop(old);
    let new = join.join().expect("registration thread");
    assert_eq!(registry.entry_count(), 1);
    assert!(registry.contains_entry(&id));
    drop(new);
    assert_eq!(registry.entry_count(), 0);
    assert!(!registry.contains_entry(&id));
}

#[test]
fn dropping_without_revoke_removes_final_entry() {
    let registry = Arc::new(TokenWsCancelRegistry::default());
    let id = TokenId([9; 16]);
    let registration = registry.register(id);
    let old: CancellationToken = registration.token();
    drop(registration);
    assert!(!registry.contains_entry(&id));
    assert_eq!(registry.entry_count(), 0);
    assert!(!old.is_cancelled());
}
