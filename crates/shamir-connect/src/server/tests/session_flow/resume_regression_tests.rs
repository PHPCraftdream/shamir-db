//! Regression tests for the resume flow:
//! - RV-CONNECT-ECE3E157: counter persistence failure must be distinguishable
//!   from replay (distinct `Error` variant, not `AuthFailed`).

use crate::common::error::Error;
use crate::common::time::ns;
use crate::common::types::limits;
use crate::common::types::BindingMode;
use crate::server::resume::{
    issue_initial_ticket, new_user_state_map, process_resume, AdvanceOutcome, ConsumedCounterStore,
    InMemoryUserStateMap, ResumeConfig, ResumeRequest, ResumeUserState,
};
use crate::server::rotation::ServerIdentityState;
use crate::server::session::SessionStore;

const USER: [u8; 16] = [0x5au8; 16];

fn config() -> ResumeConfig {
    ResumeConfig::new([0xa1u8; 32], None, true, false)
}

fn lookup_map() -> InMemoryUserStateMap {
    let users = new_user_state_map();
    users.insert(
        USER,
        ResumeUserState {
            username: "u".into(),
            roles: vec![],
            superuser: false,
            replicator: false,
            tickets_invalid_before_ns: 0,
        },
    );
    users
}

fn request<'a>(ticket: &'a [u8], nonce: [u8; 32]) -> ResumeRequest<'a> {
    ResumeRequest {
        ticket_wire_bytes: ticket,
        client_nonce: nonce,
        binding_mode_now: BindingMode::TlsExporter,
        channel_binding_now: [0x77u8; 32],
    }
}

/// Counter stub that always reports a persistence failure (RV-CONNECT-ECE3E157
/// fault-injection seam): `try_advance` never accepts, mirroring a failing
/// durable backend (get/insert/persist error).
struct FailingCounterStore;

impl ConsumedCounterStore for FailingCounterStore {
    fn try_advance(
        &self,
        _user_id: &[u8; 16],
        _family_id: &[u8; limits::TICKET_FAMILY_ID_BYTES],
        _new_counter: u64,
    ) -> AdvanceOutcome {
        AdvanceOutcome::StorageFailed
    }
}

/// RV-CONNECT-ECE3E157: with a failing persistence path, `process_resume`
/// must return the DISTINCT `Error::StorageFailed` (server-side storage
/// error), not the replay rejection `Error::AuthFailed` — a persistence
/// outage must not be reported to the caller as a replayed ticket.
#[test]
fn rv_counters_ece3e157_persistence_failure_is_not_replay_rejection() {
    let cfg = config();
    let users = lookup_map();
    let store = SessionStore::new();
    let identity = ServerIdentityState::fresh();
    let now = ns::HOUR * 1_000;

    let (ticket, _) = issue_initial_ticket(
        &cfg.ticket_key,
        USER,
        "u".into(),
        BindingMode::TlsExporter.as_u8(),
        BindingMode::TlsExporter.as_u8(),
        [0x77u8; 32],
        0,
        now,
        ns::HOUR,
    )
    .unwrap();

    let req = request(&ticket, [0xabu8; 32]);
    let err = process_resume(
        &req,
        &cfg,
        &FailingCounterStore,
        &users,
        &store,
        &identity,
        24 * ns::HOUR,
        1_000,
        now,
    )
    .expect_err("storage failure must reject the resume");
    assert!(
        matches!(err, Error::StorageFailed),
        "persistence failure must map to StorageFailed, not AuthFailed (got {err:?})"
    );
    assert_eq!(
        store.len(),
        0,
        "no session may be created on storage failure"
    );
}
