use std::sync::Arc;

use shamir_collections::TFxSet;
use shamir_connect::common::push_envelope::{PushEnvelope, PushKind};
use tokio::sync::{mpsc, oneshot};

use crate::client::subscription::{SubscriptionHandle, EARLY_BUFFER_CAP, EARLY_BUFFER_TOTAL_CAP};
use crate::client::transport::{HookPoint, Lifecycle};
use crate::error::CloseCause;

fn envelope(id: u64, seq: u64) -> PushEnvelope {
    PushEnvelope {
        push: PushKind::Event,
        sub: id,
        seq,
        data: Some(seq.to_le_bytes().to_vec()),
        gap_at: None,
    }
}

/// Register through the real path and wrap the result in a handle.
fn register(lifecycle: &Lifecycle, id: u64) -> SubscriptionHandle {
    let (tx, rx) = mpsc::channel(EARLY_BUFFER_CAP);
    let (term_tx, term_rx) = oneshot::channel();
    lifecycle
        .subs
        .register(id, tx.clone(), term_tx)
        .expect("open registration");
    SubscriptionHandle::new(id, rx, tx.downgrade(), lifecycle.subs.clone(), term_rx)
}

fn assert_no_residue(lifecycle: &Lifecycle) {
    assert_eq!(lifecycle.subs.early_total(), 0, "early budget leaked");
    assert_eq!(lifecycle.subs.debug_early_total(), 0, "early items leaked");
    assert!(lifecycle.subs.is_empty(), "registry entry leaked");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_routes_never_exceed_aggregate_cap_and_denied_keys_are_not_kept() {
    let state = Lifecycle::new();
    let gate = Arc::new(tokio::sync::Barrier::new(8));
    let mut tasks = Vec::new();
    for worker in 0..8usize {
        let state = state.clone();
        let gate = Arc::clone(&gate);
        tasks.push(tokio::spawn(async move {
            gate.wait().await;
            for item in 0..256usize {
                let id = (worker * 256 + item) as u64;
                state.subs.route(envelope(id, item as u64));
                assert!(state.subs.early_total() <= EARLY_BUFFER_TOTAL_CAP);
            }
        }));
    }
    for task in tasks {
        task.await.expect("router");
    }
    assert_eq!(state.subs.early_total(), EARLY_BUFFER_TOTAL_CAP);
    assert_eq!(state.subs.debug_early_total(), EARLY_BUFFER_TOTAL_CAP);
    let denied = 9_999;
    state.subs.route(envelope(denied, 0));
    assert!(!state.subs.contains(&denied));
    state.shutdown(CloseCause::PeerEof);
    assert_no_residue(&state);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn per_key_cap_is_enforced_and_early_flush_precedes_live_route() {
    let state = Lifecycle::new();
    let count = EARLY_BUFFER_CAP * 2 + 2;
    let gate = Arc::new(tokio::sync::Barrier::new(count));
    let mut routers = Vec::new();
    for seq in 0..count {
        let state = state.clone();
        let gate = Arc::clone(&gate);
        routers.push(tokio::spawn(async move {
            gate.wait().await;
            state.subs.route(envelope(70, seq as u64));
        }));
    }
    for task in routers {
        task.await.expect("same-key router");
    }
    assert_eq!(state.subs.early_total(), EARLY_BUFFER_CAP);
    assert_eq!(state.subs.debug_early_total(), EARLY_BUFFER_CAP);

    let mut handle = register(&state, 70);
    assert_eq!(state.subs.early_total(), 0);
    let mut observed = TFxSet::default();
    for _ in 0..EARLY_BUFFER_CAP {
        let seq = handle.next().await.expect("buffered envelope").seq;
        assert!(observed.insert(seq), "duplicate early envelope");
    }
    state.subs.route(envelope(70, 10_000));
    assert_eq!(
        handle.next().await.expect("later live envelope").seq,
        10_000
    );
    state.shutdown(CloseCause::PeerEof);
    assert_eq!(handle.termination(), Some(CloseCause::PeerEof));
}

#[tokio::test]
async fn stale_handle_drop_does_not_remove_re_registered_route() {
    let state = Lifecycle::new();
    let (old_tx, old_rx) = mpsc::channel(8);
    let (old_term_tx, old_term_rx) = oneshot::channel();
    state
        .subs
        .register(9, old_tx.clone(), old_term_tx)
        .expect("first registration");
    let old = SubscriptionHandle::new(
        9,
        old_rx,
        old_tx.downgrade(),
        state.subs.clone(),
        old_term_rx,
    );

    let (new_tx, mut new_rx) = mpsc::channel(8);
    let (new_term_tx, mut new_term_rx) = oneshot::channel();
    state
        .subs
        .register(9, new_tx.clone(), new_term_tx)
        .expect("replacement registration");
    drop(old);

    state.subs.route(envelope(9, 1));
    assert_eq!(new_rx.try_recv().expect("replacement route remains").seq, 1);
    state.shutdown(CloseCause::PeerEof);
    assert_eq!(new_term_rx.try_recv(), Ok(CloseCause::PeerEof));
}

#[tokio::test]
async fn registration_after_shutdown_yields_terminated_handle_without_residue() {
    let state = Lifecycle::new();
    state.shutdown(CloseCause::PeerEof);
    let (tx, rx) = mpsc::channel(8);
    let (term_tx, term_rx) = oneshot::channel();
    assert!(matches!(
        state.subs.register(12, tx.clone(), term_tx),
        Err(CloseCause::PeerEof)
    ));
    let owner = tx.downgrade();
    drop(tx);
    let mut handle = SubscriptionHandle::new(12, rx, owner, state.subs.clone(), term_rx);
    assert!(handle.next().await.is_none());
    assert_eq!(handle.termination(), Some(CloseCause::PeerEof));
    assert_no_residue(&state);
}

/// Shutdown lands after the router passed the closed check but before its
/// map transition: the drain sees an empty map, the late transition creates
/// an early buffer that only the post-operation cleanup can remove.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn route_shutdown_between_precheck_and_apply_leaves_no_residue() {
    let state = Lifecycle::new();
    let (hook, reached, release) = HookPoint::new();
    let registry = Arc::clone(&state.subs);
    let router = tokio::spawn(async move {
        registry
            .route_test(envelope(31, 1), Some(&hook), None)
            .await;
    });
    reached.await.expect("router reached the precheck hook");
    assert!(!state.subs.contains(&31), "apply must not have run yet");
    state.shutdown(CloseCause::PeerEof);
    release.send(()).expect("release router");
    router.await.expect("router task");
    assert_no_residue(&state);
}

/// Shutdown lands after the router's map transition but before its cleanup:
/// the drain itself must remove the entry and return its budget, and the
/// cleanup must not double-release.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn route_shutdown_between_apply_and_cleanup_leaves_no_residue() {
    let state = Lifecycle::new();
    let (hook, reached, release) = HookPoint::new();
    let registry = Arc::clone(&state.subs);
    let router = tokio::spawn(async move {
        registry
            .route_test(envelope(32, 1), None, Some(&hook))
            .await;
    });
    reached.await.expect("router reached the post-apply hook");
    assert!(state.subs.contains(&32), "apply must have run");
    assert_eq!(state.subs.early_total(), 1);
    state.shutdown(CloseCause::PeerEof);
    release.send(()).expect("release router");
    router.await.expect("router task");
    assert_no_residue(&state);
}

/// Shutdown lands after registration passed the closed check but before it
/// inserted its route: the drain sees nothing, so the registration itself
/// must notice the close, remove its own entry and resolve its termination.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn registration_shutdown_before_insert_self_removes_and_terminates() {
    let state = Lifecycle::new();
    let (hook, reached, release) = HookPoint::new();
    let (tx, _rx) = mpsc::channel(8);
    let (term_tx, mut term_rx) = oneshot::channel();
    let registry = Arc::clone(&state.subs);
    let task = tokio::spawn(async move {
        registry
            .register_test(44, tx, term_tx, Some(&hook), None)
            .await
    });
    reached
        .await
        .expect("registration reached the pre-insert hook");
    assert!(!state.subs.contains(&44), "insert must not have run yet");
    state.shutdown(CloseCause::PeerEof);
    release.send(()).expect("release registration");
    assert!(matches!(
        task.await.expect("registration task"),
        Err(CloseCause::PeerEof)
    ));
    assert_eq!(term_rx.try_recv(), Ok(CloseCause::PeerEof));
    assert_no_residue(&state);
}

/// Shutdown lands after the registration inserted its route: the drain
/// resolves the termination channel, and the registration's own recheck must
/// find nothing left to remove and must not resolve a second time.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn registration_shutdown_after_insert_terminates_exactly_once() {
    let state = Lifecycle::new();
    let (hook, reached, release) = HookPoint::new();
    let (tx, _rx) = mpsc::channel(8);
    let (term_tx, mut term_rx) = oneshot::channel();
    let registry = Arc::clone(&state.subs);
    let task = tokio::spawn(async move {
        registry
            .register_test(45, tx, term_tx, None, Some(&hook))
            .await
    });
    reached
        .await
        .expect("registration reached the post-insert hook");
    assert!(state.subs.contains(&45), "insert must have run");
    state.shutdown(CloseCause::PeerEof);
    release.send(()).expect("release registration");
    assert!(matches!(
        task.await.expect("registration task"),
        Err(CloseCause::PeerEof)
    ));
    assert_eq!(term_rx.try_recv(), Ok(CloseCause::PeerEof));
    assert!(term_rx.try_recv().is_err(), "terminal resolved twice");
    assert_no_residue(&state);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn route_registration_shutdown_stress_terminates_current_handles_once() {
    let state = Lifecycle::new();
    let start = Arc::new(tokio::sync::Barrier::new(4));

    let registrants = {
        let state = state.clone();
        let gate = Arc::clone(&start);
        tokio::spawn(async move {
            gate.wait().await;
            let mut handles = Vec::new();
            for id in 0..8u64 {
                let (tx, rx) = mpsc::channel(256);
                let (term_tx, term_rx) = oneshot::channel();
                let _ = state.subs.register(id, tx.clone(), term_tx);
                handles.push((
                    id,
                    SubscriptionHandle::new(id, rx, tx.downgrade(), state.subs.clone(), term_rx),
                ));
            }
            handles
        })
    };
    let routers = {
        let state = state.clone();
        let gate = Arc::clone(&start);
        tokio::spawn(async move {
            gate.wait().await;
            for seq in 0..48u64 {
                for id in 0..8u64 {
                    state.subs.route(envelope(id, seq));
                }
            }
        })
    };
    let shutdown = {
        let state = state.clone();
        let gate = Arc::clone(&start);
        tokio::spawn(async move {
            gate.wait().await;
            state.shutdown(CloseCause::PeerEof);
        })
    };
    start.wait().await;
    let handles = registrants.await.expect("registrants");
    routers.await.expect("routers");
    shutdown.await.expect("shutdown");

    let mut seen = TFxSet::default();
    for (id, mut handle) in handles {
        let mut last: Option<u64> = None;
        while let Some(env) = handle.next().await {
            assert_eq!(env.sub, id);
            assert!(seen.insert((id, env.seq)), "duplicate envelope");
            if let Some(previous) = last {
                assert!(env.seq >= previous, "per-id order regressed");
            }
            last = Some(env.seq);
        }
        assert_eq!(handle.termination(), Some(CloseCause::PeerEof));
    }
    assert_no_residue(&state);
}
