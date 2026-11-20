//! Private tests for the R4b-1 in-flight request table
//! (`db_handler/repo_admin/activity/requests.rs`): normal
//! registration/deregistration, RAII cleanup on panic and on plain drop,
//! state-word encoding and mirror agreement.
//!
//! Placed under `tests/handlers/` (the `tests/` root is at the 7-entry
//! layout limit; `misc/` is full too). Only new, activity-scoped items are
//! touched — no visibility of any pre-existing item is widened.

use std::panic::AssertUnwindSafe;
use std::sync::atomic::Ordering;
use std::sync::Arc;

use shamir_connect::common::types::{BindingMode, TransportKind};
use shamir_connect::server::session::{Session, SessionPermissions};

use crate::engine::db_handler::activity::{ActivityRequests, RequestEntry};
use shamir_query_types::admin::activity::RequestExecId;
use shamir_tx::CancelState;

/// Mint ids for tests — the real minter is the handler's `AtomicU64`; the
/// table itself only requires uniqueness.
fn rid(n: u64) -> RequestExecId {
    RequestExecId(n)
}

/// A minimal session fixture: the entry constructor combines the session
/// metadata (handle, 16-byte user id, username) exactly like production.
fn fixture_session() -> Session {
    Session::new(
        [42u8; 16],
        "alice".to_string(),
        SessionPermissions::new(false, false, Vec::new()),
        TransportKind::Tcp,
        BindingMode::None,
        [0u8; 32],
        1_000,
    )
}

fn register(
    table: &Arc<ActivityRequests>,
    id: u64,
) -> crate::engine::db_handler::activity::InFlightRequest {
    table.register(RequestEntry::new(
        rid(id),
        &fixture_session(),
        "app".to_string(),
        "read",
        Some(9), // client rid
    ))
}

#[test]
fn register_makes_entry_observable_and_drop_removes_it() {
    let table = Arc::new(ActivityRequests::new());
    let guard = register(&table, 1);

    assert_eq!(table.in_flight(), 1, "mirror counts the live entry");
    let mut seen: Vec<RequestExecId> = Vec::new();
    table.for_each(|e| {
        assert_eq!(e.id, rid(1));
        // Un-stamped fixture session (`Session::new`, handle 0): the row
        // carries the documented `None` sentinel (no minted handle on
        // this test path).
        assert_eq!(e.session_handle, None);
        assert_eq!(e.user_id, [42u8; 16]);
        assert_eq!(e.username, "alice");
        assert_eq!(e.db, "app");
        assert_eq!(e.op_kind, "read");
        // Monotonic start stamp: set, positive, and in the past relative
        // to a fresh sample of the same clock.
        let started = e.started_at_ns.load(Ordering::Relaxed);
        assert!(started > 0);
        assert!(e.elapsed_ns() < u64::MAX);
        assert_eq!(e.client_rid, Some(9));
        assert_eq!(
            e.cancel.state(),
            CancelState::Running,
            "a freshly registered batch is Running"
        );
        seen.push(e.id);
    });
    assert_eq!(seen, vec![rid(1)]);

    drop(guard);
    assert_eq!(table.in_flight(), 0, "RAII drop removed the entry");
    table.for_each(|_| panic!("no entry may survive the guard drop"));
}

#[test]
fn panic_unwind_still_removes_the_entry() {
    let table = Arc::new(ActivityRequests::new());

    let result = std::panic::catch_unwind(AssertUnwindSafe(|| {
        let _guard = register(&table, 2);
        assert_eq!(table.in_flight(), 1);
        panic!("simulated request panic");
    }));
    assert!(result.is_err(), "the panic must propagate for this test");

    assert_eq!(
        table.in_flight(),
        0,
        "a panicking request must not leak an in-flight entry"
    );
    table.for_each(|_| panic!("table must be empty after unwind"));
}

#[test]
fn mirror_and_table_agree_across_many_lifecycles() {
    let table = Arc::new(ActivityRequests::new());
    let guards: Vec<_> = (0..16).map(|i| register(&table, i)).collect();
    assert_eq!(table.in_flight(), 16);
    let mut count = 0usize;
    table.for_each(|_| count += 1);
    assert_eq!(count, 16, "mirror and live-entry scan must agree");

    drop(guards);
    assert_eq!(table.in_flight(), 0);
    let mut count = 0usize;
    table.for_each(|_| count += 1);
    assert_eq!(count, 0);
}

#[test]
fn state_word_is_running_then_done_and_done_is_terminal() {
    let table = Arc::new(ActivityRequests::new());
    let guard = register(&table, 3);

    assert!(guard.finish());
    let handle = Arc::clone(guard.cancel_handle());
    assert_eq!(handle.state(), CancelState::Done);
    assert_eq!(handle.try_cancel(), Err(CancelState::Done));
    assert!(!handle.try_commit());

    drop(guard);
    table.for_each(|_| panic!("done entries are removed, never retained"));
}

#[test]
fn distinct_ids_are_distinct_entries() {
    let table = Arc::new(ActivityRequests::new());
    let a = register(&table, 10);
    let b = register(&table, 11);
    assert_ne!(a.id(), b.id(), "minted exec ids are process-unique");
    assert_eq!(table.in_flight(), 2);
    drop(a);
    assert_eq!(table.in_flight(), 1);
    drop(b);
    assert_eq!(table.in_flight(), 0);
}
